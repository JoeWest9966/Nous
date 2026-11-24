//! 元素库（`/v1/elements*`）的 HTTP 形状：真 Router + 真 git 仓（临时目录）。
//!
//! Desk 的页面按这几条接线：全量 → 单条读 → 存（带 base / requestId）→ 新建 → 删 → 补推；
//! 判据打在公共接缝上，不调内部函数。

mod common;

use std::path::Path;
use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 一个已提交的元素库：一份组（引用了在的与不在的包）、一份包、一份坏 JSON。
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Nous-Elements");
    std::fs::create_dir_all(root.join("groups")).unwrap();
    std::fs::create_dir_all(root.join("packages")).unwrap();
    std::fs::write(
        root.join("groups/frontend.json"),
        "{\n  \"id\": \"frontend\",\n  \"name\": \"前端\",\n  \"packages\": [\"react-rules\", \"gone\"]\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("packages/react-rules.json"),
        "{\n  \"id\": \"react-rules\",\n  \"name\": \"React 规范\",\n  \"kind\": \"prompt\",\n  \"body\": \"1. Hooks 只能放顶层。\",\n  \"tags\": [\"react\"]\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("packages/broken.json"), "{ 这不是 JSON").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);
    (tmp, root)
}

fn app_with(root: &Path) -> Router {
    let state =
        AppState::with_source(common::FakeSource::two_sessions()).with_repos_for_test("elements", root);
    router(state)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(value) => Body::from(serde_json::to_vec(&value).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn commits(root: &Path) -> i32 {
    git(root, &["rev-list", "--count", "HEAD"]).parse().unwrap()
}

#[tokio::test]
async fn state_lists_both_kinds_flags_broken_and_resolves_references() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(&app, "GET", "/v1/elements", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind"], "elements");
    assert_eq!(body["repo"]["present"], true);
    assert_eq!(body["repo"]["branch"], "main");

    let groups = body["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["kind"], "groups");
    assert_eq!(groups[0]["id"], "frontend");
    assert_eq!(groups[0]["state"], "ok");
    assert!(groups[0]["etag"].as_str().unwrap().len() == 64, "etag 是 sha256 hex");

    let packages = body["packages"].as_array().unwrap();
    assert_eq!(packages.len(), 2, "坏的那份也在列表里，只是标红");
    let broken = packages.iter().find(|item| item["id"] == "broken").unwrap();
    assert_eq!(broken["state"], "problem");
    assert_eq!(broken["fields"], Value::Null);
    assert!(broken["detail"].as_str().unwrap().contains("JSON"));
    let ok = packages.iter().find(|item| item["id"] == "react-rules").unwrap();
    assert_eq!(ok["state"], "ok");
    assert_eq!(ok["fields"]["kind"], "prompt");

    assert_eq!(body["resolved"]["groups"]["frontend"]["react-rules"]["state"], "ok");
    assert_eq!(body["resolved"]["groups"]["frontend"]["gone"]["state"], "missing");
    assert_eq!(body["resolved"]["missing"][0], "frontend/gone");
}

#[tokio::test]
async fn get_reads_one_element_with_the_full_fields() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(&app, "GET", "/v1/elements/packages/react-rules", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind"], "packages");
    assert_eq!(body["id"], "react-rules");
    assert_eq!(body["fields"]["tags"][0], "react");
    assert!(body["etag"].is_string());

    let (status, _) = call(&app, "GET", "/v1/elements/packages/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_kind_is_400_not_a_guess() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(&app, "GET", "/v1/elements/skills", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("groups"));
}

#[tokio::test]
async fn put_writes_commits_and_reports_the_receipt() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let before = commits(&root);
    let fields = json!({
        "id": "backend",
        "name": "后端",
        "packages": ["react-rules"],
        "unknown": { "keep": true }
    });
    let (status, body) = call(
        &app,
        "PUT",
        "/v1/elements/groups/backend",
        Some(json!({ "fields": fields, "requestId": "req-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind"], "groups");
    assert_eq!(body["id"], "backend");
    assert_eq!(body["created"], true);
    assert_eq!(body["replay"], false);
    assert!(body["commit"].as_str().unwrap().len() >= 7);
    assert_eq!(body["pushed"], Value::Null, "没有远端 = null，不是 false");
    assert_eq!(commits(&root), before + 1);

    let text = std::fs::read_to_string(root.join("groups/backend.json")).unwrap();
    assert_eq!(text, "{\n  \"id\": \"backend\",\n  \"name\": \"后端\",\n  \"packages\": [\n    \"react-rules\"\n  ],\n  \"unknown\": {\n    \"keep\": true\n  }\n}\n");
    let log = git(&root, &["log", "-1", "--format=%s%n%b"]);
    assert!(log.contains("feat(group): backend"));
    assert!(log.contains("Request-Id: req-1"));
}

#[tokio::test]
async fn replaying_the_same_request_id_does_not_write_or_commit_again() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let fields = json!({ "id": "backend", "name": "后端" });
    let (_, first) = call(
        &app,
        "PUT",
        "/v1/elements/groups/backend",
        Some(json!({ "fields": fields, "requestId": "req-2" })),
    )
    .await;
    assert!(first["commit"].is_string());
    let after_first = commits(&root);

    let (status, replay) = call(
        &app,
        "PUT",
        "/v1/elements/groups/backend",
        Some(json!({ "fields": fields, "requestId": "req-2" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["replay"], true);
    assert_eq!(replay["commit"], Value::Null);
    assert_eq!(commits(&root), after_first, "重放不许再提交一条");
}

#[tokio::test]
async fn stale_base_is_a_conflict() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(
        &app,
        "PUT",
        "/v1/elements/packages/react-rules",
        Some(json!({ "fields": { "id": "react-rules", "name": "改" }, "base": "deadbeef" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"].as_str().unwrap().contains("If-Match"));
}

#[tokio::test]
async fn create_takes_the_id_from_the_body_and_returns_201() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(
        &app,
        "POST",
        "/v1/elements/packages",
        Some(json!({ "fields": { "id": "hooks-rules", "name": "Hooks 规范", "body": "只放顶层" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["id"], "hooks-rules");
    assert_eq!(body["created"], true);
    assert!(root.join("packages/hooks-rules.json").is_file());

    let (status, body) = call(
        &app,
        "POST",
        "/v1/elements/packages",
        Some(json!({ "fields": { "name": "缺 id" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("id"));
}

#[tokio::test]
async fn delete_commits_and_then_reads_404() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let before = commits(&root);
    let (status, body) = call(&app, "DELETE", "/v1/elements/packages/react-rules", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["commit"].is_string());
    assert_eq!(commits(&root), before + 1);
    assert!(!root.join("packages/react-rules.json").exists());
    assert!(git(&root, &["log", "-1", "--format=%s"]).contains("chore(package): rm react-rules"));

    let (status, _) = call(&app, "GET", "/v1/elements/packages/react-rules", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, "DELETE", "/v1/elements/packages/react-rules", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "删两次只算一次");
}

#[tokio::test]
async fn unassembled_repo_is_an_honest_empty_state() {
    let (tmp, _root) = fixture();
    // 只装配 skills → elements 缺席（不是「路径在但空」）。
    let state = AppState::with_source(common::FakeSource::two_sessions())
        .with_repos_for_test("skills", tmp.path());
    let app = router(state);
    let (status, body) = call(&app, "GET", "/v1/elements", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["repo"]["present"], false);
    assert!(body["repo"]["missing"].as_str().unwrap().contains("NOUS_ELEMENTS"));
    assert_eq!(body["groups"], json!([]));

    let (status, _) = call(&app, "DELETE", "/v1/elements/packages/x", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "写端点不假装能写");
}

#[tokio::test]
async fn capabilities_declare_the_elements_module_with_its_endpoints() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(&app, "GET", "/v1/capabilities", None).await;
    assert_eq!(status, StatusCode::OK);
    let module = body["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|module| module["name"] == "elements")
        .expect("capabilities 里必须有 elements");
    assert_eq!(module["enabled"], true);
    assert_eq!(module["layer"], "app");
    assert_eq!(module["crate"], "`nous-elements`");
    let endpoints = module["endpoints"].as_array().unwrap();
    assert!(endpoints.iter().any(|line| line == "PUT /v1/elements/{kind}/{id}"));
    assert!(endpoints.iter().any(|line| line == "POST /v1/elements/push"));
}

#[tokio::test]
async fn a_locked_element_refuses_writes_and_deletes_with_423() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);

    // 上锁。
    let (status, body) = call(
        &app,
        "POST",
        "/v1/elements/groups/frontend/lock",
        Some(json!({ "locked": true, "requestId": "lock-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locked"], true);
    assert_eq!(body["kind"], "groups");
    assert_eq!(body["changed"], true);
    assert!(body["commit"].as_str().unwrap().len() >= 7);
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "chore(group): lock frontend");

    // 存：423，且盘上那份没被改。
    let before = std::fs::read_to_string(root.join("groups/frontend.json")).unwrap();
    let (status, body) = call(
        &app,
        "PUT",
        "/v1/elements/groups/frontend",
        Some(json!({ "fields": { "id": "frontend", "name": "改名", "packages": [] } })),
    )
    .await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert!(body["error"].as_str().unwrap().contains("已锁定"), "{body}");
    assert!(body["error"].as_str().unwrap().contains("/lock"), "要说清楚下一步怎么解");
    assert_eq!(
        std::fs::read_to_string(root.join("groups/frontend.json")).unwrap(),
        before
    );

    // 删：423。
    let (status, body) = call(&app, "DELETE", "/v1/elements/groups/frontend", None).await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(
        call(&app, "GET", "/v1/elements/groups/frontend", None).await.0,
        StatusCode::OK,
        "删不掉 = 还在"
    );

    // 已经锁着再锁一次：changed=false，不造提交。
    let commits_before = commits(&root);
    let (status, body) = call(
        &app,
        "POST",
        "/v1/elements/groups/frontend/lock",
        Some(json!({ "locked": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], false);
    assert_eq!(commits(&root), commits_before);

    // 解锁 → 能删。
    let (status, body) = call(
        &app,
        "POST",
        "/v1/elements/groups/frontend/lock",
        Some(json!({ "locked": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locked"], false);
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "chore(group): unlock frontend");
    let (status, _) = call(&app, "DELETE", "/v1/elements/groups/frontend", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_lock_endpoint_needs_locked_and_404s_on_unknown_ids() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(&app, "POST", "/v1/elements/groups/frontend/lock", Some(json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("locked"));

    let (status, _) = call(
        &app,
        "POST",
        "/v1/elements/groups/nope/lock",
        Some(json!({ "locked": true })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不存在的元素不许凭空锁出一个来");
}

/// 锁和旧 `base` 一起撞上来：**423 先出**（`If-Match` 排后面）。
///
/// 客户端看到 409 会以为「拉一遍再来就行」，可锁着的东西拉多少遍也写不进去 ——
/// 所以 423 必须是那个先说出口的号。
#[tokio::test]
async fn a_lock_outranks_a_stale_if_match() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(
        &app,
        "POST",
        "/v1/elements/groups/frontend/lock",
        Some(json!({ "locked": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = commits(&root);

    let stale = "0".repeat(64);
    let (status, body) = call(
        &app,
        "PUT",
        &format!("/v1/elements/groups/frontend?base={stale}"),
        Some(json!({ "fields": { "id": "frontend", "name": "改名" } })),
    )
    .await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert!(body["error"].as_str().unwrap().contains("/lock"), "{body}");

    let (status, body) =
        call(&app, "DELETE", &format!("/v1/elements/groups/frontend?base={stale}"), None).await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(commits(&root), before, "两条被拒的请求没造提交");
}

/// 解锁走 `/lock`，它不被锁拦，但**照样认 `If-Match`**：拿旧指纹 409，拿对指纹 200。
#[tokio::test]
async fn unlocking_honours_if_match_but_is_not_blocked_by_the_lock() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    call(&app, "POST", "/v1/elements/groups/frontend/lock", Some(json!({ "locked": true }))).await;
    let current = call(&app, "GET", "/v1/elements/groups/frontend", None).await.1["etag"]
        .as_str()
        .unwrap()
        .to_string();

    let stale = "0".repeat(64);
    let (status, body) = call(
        &app,
        "POST",
        "/v1/elements/groups/frontend/lock",
        Some(json!({ "locked": false, "base": stale })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        call(&app, "GET", "/v1/elements/groups/frontend", None).await.1["fields"]["locked"],
        true,
        "409 之后还是锁着"
    );

    let (status, body) = call(
        &app,
        "POST",
        "/v1/elements/groups/frontend/lock",
        Some(json!({ "locked": false, "base": current })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["locked"], false);
    assert_eq!(body["changed"], true);
}
