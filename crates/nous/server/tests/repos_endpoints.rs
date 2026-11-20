//! #75 接缝 B：`/v1/repos/:repo/*` 的统一 HTTP 形状（统一一套，四套私库共用）。
//!
//! 票面端点只覆盖发布/归档/读视图；文件 CRUD 是 `nous_repo` 的公共 API（tests in
//! crates/nous/repo/tests/files.rs），这里不另造 `/files` 路由。

mod common;

use std::path::Path;
fn git_bin() -> String { String::from("git") }
use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(git_bin()).args(args).current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0").env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fixture(kind: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(kind);
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-qm", "init"]);
    (tmp, root)
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri)
        .header("content-type", "application/json")
        .body(match body { Some(v) => Body::from(serde_json::to_vec(&v).unwrap()), None => Body::empty() })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn app_with(kind: &str, root: &Path) -> axum::Router {
    let state = AppState::with_source(common::FakeSource::two_sessions())
        .with_repos_for_test(kind, root);
    router(state)
}

#[tokio::test]
async fn status_and_pending_use_the_unified_repo_route() {
    let (_tmp, root) = fixture("skills");
    let app = app_with("skills", &root);
    let (status, body) = call(&app, "GET", "/v1/repos/skills/status", None).await;
    assert_eq!(status, StatusCode::OK, "status 200：{body}");
    assert_eq!(body["root"], json!(root.to_string_lossy().into_owned()));
    assert!(body["head"].as_str().is_some_and(|h| h.len() == 40));
    assert_eq!(body["dirty"], json!(false));

    std::fs::create_dir_all(root.join("unit")).unwrap();
    std::fs::write(root.join("unit/a.md"), "a").unwrap();
    let (_, pending) = call(&app, "GET", "/v1/repos/skills/pending", None).await;
    let units = pending["units"].as_array().expect("units 数组");
    assert!(units.iter().any(|u| u["name"] == "unit"), "pending 非空：{pending}");

    let (_, diff) = call(&app, "GET", "/v1/repos/skills/pending/diff?path=unit/a.md", None).await;
    assert_eq!(diff["after"], json!("a"), "单文件 diff 给当前正文：{diff}");
}

#[tokio::test]
async fn commit_endpoint_publishes_only_requested_paths() {
    let (_tmp, root) = fixture("prompts");
    let app = app_with("prompts", &root);
    std::fs::create_dir_all(root.join("unit")).unwrap();
    std::fs::write(root.join("unit/nous.json"), r#"{"name":"unit","description":"d","prompt":"p"}"#).unwrap();
    std::fs::write(root.join("UNRELATED.md"), "搭车\n").unwrap();

    let (status, result) = call(&app, "POST", "/v1/repos/prompts/commit", Some(json!({
        "paths": ["unit/nous.json"], "message": "发布 unit", "requestId": "req-1",
    }))).await;
    assert_eq!(status, StatusCode::OK, "commit 200：{result}");
    assert!(result["pushed"].is_boolean(), "pushed 三态：{result}");
    let commit = result["commit"].as_str().unwrap();
    let names = git(&root, &["show", "--pretty=format:", "--name-only", commit]);
    assert_eq!(names, "unit/nous.json", "提交里不许有搭车文件");
    assert!(result["refs"].as_array().is_some());
}

#[tokio::test]
async fn gate_failure_is_400_and_never_commits() {
    let (_tmp, root) = fixture("skills");
    let app = app_with("skills", &root);
    std::fs::create_dir_all(root.join("bad")).unwrap();
    std::fs::write(root.join("bad/SKILL.md"), "# no frontmatter\n").unwrap();
    let before = git(&root, &["rev-parse", "HEAD"]);
    let (status, body) = call(&app, "POST", "/v1/repos/skills/commit", Some(json!({
        "paths": ["bad/SKILL.md"], "message": "bad",
    }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "内容闸 400：{body}");
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), before, "闸失败不许 commit");
}

#[tokio::test]
async fn history_and_pending_commit_are_readable_over_http() {
    let (_tmp, root) = fixture("skills");
    let app = app_with("skills", &root);
    std::fs::create_dir_all(root.join("unit")).unwrap();
    std::fs::write(root.join("unit/SKILL.md"), "---\nname: unit\ndescription: d\n---\nbody\n").unwrap();
    let (_, committed) = call(&app, "POST", "/v1/repos/skills/commit", Some(json!({
        "paths": ["unit/SKILL.md"], "message": "发布 unit",
    }))).await;
    let hash = committed["commit"].as_str().unwrap();
    let (_, detail) = call(&app, "GET", &format!("/v1/repos/skills/pending/commit?hash={hash}"), None).await;
    assert_eq!(detail["hash"], json!(hash));
    assert_eq!(detail["files"][0]["path"], json!("unit/SKILL.md"));
    let (_, history) = call(&app, "GET", "/v1/repos/skills/history?name=unit&limit=10", None).await;
    assert!(history["items"].as_array().is_some_and(|a| a.iter().any(|i| i["hash"] == json!(hash))));
    let (_, diff) = call(&app, "GET", &format!("/v1/repos/skills/history/diff?name=unit&hash={hash}&path=SKILL.md"), None).await;
    assert!(diff["after"].as_str().is_some_and(|s| s.contains("name: unit")), "{diff}");
}

#[tokio::test]
async fn archive_restore_purge_are_one_unified_route_family() {
    let (_tmp, root) = fixture("prompts");
    let app = app_with("prompts", &root);
    std::fs::create_dir_all(root.join("unit")).unwrap();
    std::fs::write(root.join("unit/nous.json"), r#"{"name":"unit","description":"d","prompt":"p"}"#).unwrap();
    git(&root, &["add", "unit/nous.json"]);
    git(&root, &["commit", "-qm", "seed"]);

    for (action, exists) in [("archive", false), ("restore", true), ("archive", false), ("purge", false)] {
        let (status, body) = call(&app, "POST", &format!("/v1/repos/prompts/{action}"),
            Some(json!({"name": "unit", "requestId": "r-1"}))).await;
        assert_eq!(status, StatusCode::OK, "{action} 200：{body}");
        assert!(body["commit"].as_str().is_some());
        assert_eq!(root.join("unit").exists(), exists, "{action} 后的单元位置");
    }
}

#[tokio::test]
async fn invalid_repo_name_is_400() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, _) = call(&app, "GET", "/v1/repos/unknown/status", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
