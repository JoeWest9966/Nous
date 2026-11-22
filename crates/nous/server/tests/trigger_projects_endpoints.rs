//! #95：触发器工程面（n8qkkj 的宿主）—— 一个触发器 = 盘上一个 cargo 项目。
//!
//! 与 `/v1/triggers` 的定义面分开测：这里只认目录、文件、etag、git、归档和发布。

mod common;

use std::path::Path;
use std::process::Command;

use axum::body::Body;
use axum::http::{Request, StatusCode};
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

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("triggers");
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

fn app_with(root: &Path) -> axum::Router {
    let state = AppState::with_source(common::FakeSource::two_sessions())
        .with_repos_for_test("triggers", root);
    router(state)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
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

fn manifest(name: &str) -> String {
    serde_json::to_string_pretty(&json!({
        "name": name,
        "version": "0.1.0",
        "summary": "测试用触发器",
        "category": ["日常"],
        "binary": name,
        "params": [
            { "name": "at", "type": "epoch", "label": "每天几点", "required": true },
            { "name": "tone", "type": "select", "label": "语气", "options": ["简洁", "详细"] }
        ]
    }))
    .unwrap()
}

fn project(root: &Path, name: &str) {
    std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    std::fs::write(root.join(name).join("nous.json"), manifest(name) + "\n").unwrap();
    std::fs::write(
        root.join(name).join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .unwrap();
    std::fs::write(
        root.join(name).join("src/main.rs"),
        "fn main() { println!(\"{{\\\"fire\\\":false}}\"); }\n",
    )
    .unwrap();
}

#[tokio::test]
async fn empty_repo_still_reports_repo_block() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(&app, "GET", "/v1/trigger-projects", None).await;
    assert_eq!(status, StatusCode::OK, "空仓也要 200：{body}");
    assert_eq!(body["triggers"], json!([]));
    assert_eq!(body["archived"], json!([]));
    assert_eq!(body["repo"]["branch"], json!("main"));
    assert!(body["repo"]["head"].as_str().is_some_and(|h| h.len() == 7));
}

#[tokio::test]
async fn create_writes_a_compilable_skeleton_without_committing() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/create",
        Some(json!({
            "name": "morning-review",
            "summary": "每天按昨天的日志起一条复盘",
            "category": ["日常", "日常/复盘"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create 200：{body}");
    for file in ["nous.json", "Cargo.toml", "src/main.rs", "README.md"] {
        assert!(root.join("morning-review").join(file).is_file(), "缺 {file}");
    }
    // 建完不提交：HEAD 里没有它，清单里 tracked=false。
    let head_files = git(&root, &["show", "--pretty=format:", "--name-only", "HEAD"]);
    assert!(!head_files.contains("morning-review"), "create 不许提交：{head_files}");
    let (_, list) = call(&app, "GET", "/v1/trigger-projects", None).await;
    let item = list["triggers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "morning-review")
        .expect("清单里有新建的这一条");
    assert_eq!(item["tracked"], json!(false));
    assert!(item["error"].is_null(), "骨架必须过内容闸：{item}");
    assert_eq!(item["params"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn create_rejects_bad_names_and_existing_dirs() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    for bad in ["Morning", "a", "../x", "has space"] {
        let (status, body) = call(
            &app,
            "POST",
            "/v1/trigger-projects/create",
            Some(json!({ "name": bad })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "坏名字必须 400：{bad} {body}");
    }
    project(&root, "taken");
    let (status, _) = call(
        &app,
        "POST",
        "/v1/trigger-projects/create",
        Some(json!({ "name": "taken" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn file_roundtrip_uses_etag_and_409_on_stale_base() {
    let (_tmp, root) = fixture();
    project(&root, "daily-digest");
    let app = app_with(&root);

    let (status, files) = call(
        &app,
        "GET",
        "/v1/trigger-projects/files?name=daily-digest",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{files}");
    let paths: Vec<&str> = files["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["nous.json", "src/main.rs", "Cargo.toml"], "{files}");

    let (_, file) = call(
        &app,
        "GET",
        "/v1/trigger-projects/file?name=daily-digest&path=nous.json",
        None,
    )
    .await;
    let etag = file["etag"].as_str().unwrap().to_string();
    assert!(file["text"].as_str().unwrap().contains("daily-digest"));

    let (status, saved) = call(
        &app,
        "PUT",
        "/v1/trigger-projects/file",
        Some(json!({
            "name": "daily-digest",
            "path": "nous.json",
            "text": file["text"].as_str().unwrap().replace("测试用触发器", "测试用触发器 v2"),
            "base": etag,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "第一次保存成功：{saved}");

    // 旧 etag 再存一次 → 409，且盘上内容不变（没有静默覆盖）。
    let (status, body) = call(
        &app,
        "PUT",
        "/v1/trigger-projects/file",
        Some(json!({
            "name": "daily-digest",
            "path": "nous.json",
            "text": "{\"name\":\"daily-digest\"}",
            "base": etag,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "旧 etag 必须 409：{body}");
    let disk = std::fs::read_to_string(root.join("daily-digest/nous.json")).unwrap();
    assert!(disk.contains("测试用触发器"), "409 不许落盘：{disk}");
}

#[tokio::test]
async fn categories_add_rename_remove_rewrite_manifests() {
    let (_tmp, root) = fixture();
    project(&root, "daily-digest");
    let app = app_with(&root);

    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/categories",
        Some(json!({ "op": "add", "name": "日常/摘录" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["categories"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "日常/摘录"));

    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/categories",
        Some(json!({ "op": "rename", "name": "日常", "to": "每天" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let manifest_text = std::fs::read_to_string(root.join("daily-digest/nous.json")).unwrap();
    assert!(manifest_text.contains("每天"), "改名要重写 nous.json：{manifest_text}");

    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/categories",
        Some(json!({ "op": "remove", "name": "每天" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let manifest_text = std::fs::read_to_string(root.join("daily-digest/nous.json")).unwrap();
    let manifest_json: Value = serde_json::from_str(&manifest_text).unwrap();
    assert_eq!(manifest_json["category"], json!([]), "删除连子类一起摘：{manifest_text}");
    let (_, list) = call(&app, "GET", "/v1/trigger-projects", None).await;
    assert_eq!(list["triggers"][0]["category"], json!([]));
}

#[tokio::test]
async fn untracked_project_makes_the_repo_dirty() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (_, clean) = call(&app, "GET", "/v1/trigger-projects", None).await;
    assert_eq!(clean["repo"]["dirty"], false);
    project(&root, "fresh-one");
    let (_, dirty) = call(&app, "GET", "/v1/trigger-projects", None).await;
    assert_eq!(dirty["repo"]["dirty"], true, "未跟踪的新项目也要算脏：{dirty}");
    assert!(
        dirty["repo"]["dirtyCount"].as_u64().unwrap_or(0) >= 1,
        "脏路径数要跟着算：{dirty}"
    );
}

#[tokio::test]
async fn categories_come_from_manifests_when_file_is_missing() {
    let (_tmp, root) = fixture();
    project(&root, "daily-digest");
    let manifest_path = root.join("daily-digest/nous.json");
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    value["category"] = json!(["日常", "日常/摘录"]);
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    // 旧仓没有 categories.json；类别只写在 nous.json 里。
    assert!(!root.join("categories.json").exists());
    let app = app_with(&root);
    let (status, body) = call(&app, "GET", "/v1/trigger-projects/categories", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let categories: Vec<&str> = body["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert!(categories.contains(&"日常"), "在用类别要进词表：{body}");
    assert!(categories.contains(&"日常/摘录"), "前缀祖先要进词表：{body}");
    let (_, list) = call(&app, "GET", "/v1/trigger-projects", None).await;
    assert!(list["categories"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "日常"));
}

#[tokio::test]
async fn archive_restore_purge_round_trip() {
    let (_tmp, root) = fixture();
    project(&root, "daily-digest");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "add daily-digest"]);
    let app = app_with(&root);

    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/archive",
        Some(json!({ "name": "daily-digest" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "archive 200：{body}");
    assert!(root.join(".archive/daily-digest").is_dir());
    let (_, list) = call(&app, "GET", "/v1/trigger-projects", None).await;
    assert_eq!(list["triggers"], json!([]), "归档后清单不返回：{list}");
    assert_eq!(list["archived"][0]["name"], json!("daily-digest"));

    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/restore",
        Some(json!({ "name": "daily-digest" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "restore 200：{body}");
    assert!(root.join("daily-digest").is_dir());
    assert!(!root.join(".archive/daily-digest").exists());

    // 没归档就 purge → 404；归档之后才能真删。
    let (status, _) = call(
        &app,
        "POST",
        "/v1/trigger-projects/purge",
        Some(json!({ "name": "daily-digest" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    call(
        &app,
        "POST",
        "/v1/trigger-projects/archive",
        Some(json!({ "name": "daily-digest" })),
    )
    .await;
    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/purge",
        Some(json!({ "name": "daily-digest" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "purge 200：{body}");
    assert!(!root.join(".archive/daily-digest").exists());
}

#[tokio::test]
async fn broken_manifest_is_a_card_error_not_a_500() {
    let (_tmp, root) = fixture();
    project(&root, "bad-trigger");
    std::fs::write(
        root.join("bad-trigger/nous.json"),
        r#"{"name":"bad-trigger","version":"0.1.0","summary":"x","params":[{"name":"a","type":"percent"}]}"#,
    )
    .unwrap();
    let app = app_with(&root);
    let (status, list) = call(&app, "GET", "/v1/trigger-projects", None).await;
    assert_eq!(status, StatusCode::OK, "坏清单也要能列：{list}");
    let item = &list["triggers"][0];
    assert_eq!(item["name"], json!("bad-trigger"));
    assert!(item["error"].as_str().unwrap().contains("percent"), "{item}");
}

#[tokio::test]
async fn publish_runs_cargo_generates_index_and_commits() {
    let (_tmp, root) = fixture();
    project(&root, "tiny-trigger");
    // 项目里有一个没被 .gitignore 的 target/ 垃圾文件：发布只按显式文件清单 add。
    std::fs::create_dir_all(root.join("tiny-trigger/target")).unwrap();
    std::fs::write(root.join("tiny-trigger/target/junk"), "junk").unwrap();
    let app = app_with(&root);

    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/publish",
        Some(json!({ "name": "tiny-trigger" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "publish 200：{body}");
    assert_eq!(body["committed"], json!(true));
    assert_eq!(body["pushed"], json!(null), "fixture 没有远端：{body}");
    assert!(root.join("index.json").is_file(), "发布要生成 index.json");
    let names = git(&root, &["show", "--pretty=format:", "--name-only", "HEAD"]);
    assert!(names.contains("tiny-trigger/nous.json"), "{names}");
    assert!(names.contains("index.json"), "{names}");
    assert!(!names.contains("target/"), "target 不许进提交：{names}");
    assert!(!names.contains("junk"), "target 里的文件不许进提交：{names}");

    // 再发布一次：没有改动 → commit 失败按 stage=commit 回 422，不假装成功。
    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/publish",
        Some(json!({ "name": "tiny-trigger" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["stage"], json!("commit"));
}

#[tokio::test]
async fn publish_reports_the_failing_stage() {
    let (_tmp, root) = fixture();
    std::fs::create_dir_all(root.join("broken-build/src")).unwrap();
    std::fs::write(root.join("broken-build/nous.json"), manifest("broken-build")).unwrap();
    std::fs::write(
        root.join("broken-build/Cargo.toml"),
        "[package]\nname = \"broken-build\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("broken-build/src/main.rs"),
        "fn main() { let _ = missing_symbol; }\n",
    )
    .unwrap();
    let app = app_with(&root);
    let (status, body) = call(
        &app,
        "POST",
        "/v1/trigger-projects/publish",
        Some(json!({ "name": "broken-build" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["stage"], json!("check"), "编译失败要说是 check：{body}");
    let head_files = git(&root, &["show", "--pretty=format:", "--name-only", "HEAD"]);
    assert!(!head_files.contains("broken-build"), "失败不许提交：{head_files}");
}
