//! 模型入口（`GET /v1/models/elements`）：真 Router + 真 git 元素库。
//!
//! 判据打在公共接缝上：组的约定、包的展开、覆盖包的合并、缺什么报什么、明文密钥不回显。

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

/// 元素库：一个模型入口组 + 一个供应商包 + 一个覆盖包 + 一个不认识约定的组。
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Nous-Elements");
    std::fs::create_dir_all(root.join("groups")).unwrap();
    std::fs::create_dir_all(root.join("packages")).unwrap();
    std::fs::write(
        root.join("groups/nous-models.json"),
        json!({
            "id": "nous-models",
            "name": "Nous 模型入口",
            "packages": ["provider-wb2api", "qwen-vl-max", "provider-gone"],
            "default": "wb2api/cn:fast",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("groups/frontend.json"),
        json!({ "id": "frontend", "name": "前端", "packages": [] }).to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("packages/provider-wb2api.json"),
        json!({
            "id": "provider-wb2api",
            "name": "WB2API 网关",
            "kind": "provider",
            "provider": "wb2api",
            "baseUrl": "http://127.0.0.1:7864/v1",
            "api": "openai-completions",
            "apiKey": "$WB2API_API_KEY",
            "models": [
                { "id": "cn:fast", "name": "Fast", "contextWindow": 300000, "maxTokens": 48000 },
                { "id": "cn:vl", "name": "VL", "contextWindow": 32000 },
            ],
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("packages/qwen-vl-max.json"),
        json!({
            "id": "qwen-vl-max",
            "name": "VL 覆盖",
            "kind": "model",
            "provider": "wb2api",
            "modelId": "cn:vl",
            "contextWindow": 131072,
            "input": ["text", "image"],
        })
        .to_string(),
    )
    .unwrap();
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

async fn call(app: &Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn group_projects_into_catalog_with_override_and_diagnostics() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(&app, "/v1/models/elements").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind"], "models");
    assert_eq!(body["state"], "ok", "{body}");
    assert_eq!(body["group"], "nous-models");
    assert_eq!(body["repo"]["present"], true);

    // 两条模型，provider 是供应商键（不是包 id）。
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["provider"], "wb2api");
    assert_eq!(items[0]["modelId"], "cn:fast");
    assert_eq!(items[0]["thinkingLevels"], json!(["off"]));

    // 覆盖包把 contextWindow 压上去了（逐键合并，其余字段留着）。
    let vl = items.iter().find(|item| item["modelId"] == "cn:vl").unwrap();
    assert_eq!(vl["contextWindow"], 131072);
    assert_eq!(vl["name"], "VL");

    // default 走供应商键。
    assert_eq!(body["default"], json!({ "provider": "wb2api", "modelId": "cn:fast" }));

    // 包的引用键只回形状。
    assert_eq!(body["providers"][0]["key"], "env");
    assert_eq!(body["providers"][0]["baseUrl"], "http://127.0.0.1:7864/v1");

    // 缺的包如实点名，但不影响其余编目。
    assert_eq!(body["missing"], json!(["nous-models/provider-gone"]));
    assert_eq!(body["state"], "ok");
}

#[tokio::test]
async fn main_models_endpoint_serves_the_elements_group_first() {
    let (_tmp, root) = fixture();
    let app = app_with(&root);
    let (status, body) = call(&app, "/v1/models").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    // 编目来自元素组：两条模型、default 用供应商键、来源标 elements。
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{body}");
    assert_eq!(items[0]["provider"], "wb2api");
    assert_eq!(items[0]["modelId"], "cn:fast");
    assert_eq!(body["default"], json!({ "provider": "wb2api", "modelId": "cn:fast" }));
    assert_eq!(body["catalog"]["state"], "ok");
    assert_eq!(body["catalog"]["source"], "elements");
}

#[tokio::test]
async fn main_models_endpoint_falls_back_to_file_without_the_group() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Nous-Elements");
    std::fs::create_dir_all(root.join("groups")).unwrap();
    std::fs::create_dir_all(root.join("packages")).unwrap();
    // 组不存在：编目应回落到 file 来源（这里没给文件 → missing），不编数据。
    let state = AppState::with_source(common::FakeSource::two_sessions())
        .with_repos_for_test("elements", &root);
    let app = router(state);
    let (status, body) = call(&app, "/v1/models").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"], json!([]));
    assert_eq!(body["catalog"]["state"], "missing");
}

#[tokio::test]
async fn plaintext_key_is_redacted_to_a_shape() {
    let (_tmp, root) = fixture();
    std::fs::write(
        root.join("packages/provider-wb2api.json"),
        json!({
            "id": "provider-wb2api",
            "name": "WB2API 网关",
            "kind": "provider",
            "models": [{ "id": "cn:fast", "name": "Fast" }],
            "apiKey": "sk-live-0123456789abcdef",
        })
        .to_string(),
    )
    .unwrap();
    let app = app_with(&root);
    let (status, body) = call(&app, "/v1/models/elements").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["providers"][0]["key"], "plaintext");
    assert!(!body.to_string().contains("sk-live"), "明文不许回显：{body}");
}

#[tokio::test]
async fn missing_group_is_an_honest_empty_catalog_not_404() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Nous-Elements");
    std::fs::create_dir_all(root.join("groups")).unwrap();
    std::fs::create_dir_all(root.join("packages")).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@e"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init", "--allow-empty"]);

    let app = app_with(&root);
    let (status, body) = call(&app, "/v1/models/elements").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "missing");
    assert_eq!(body["items"], json!([]));
    assert_eq!(body["missing"], json!(["nous-models"]));
    assert_eq!(body["repo"]["present"], true);
}

#[tokio::test]
async fn unassembled_elements_repo_is_missing_not_a_crash() {
    let state = AppState::with_source(common::FakeSource::two_sessions());
    let app = router(state);
    let (status, body) = call(&app, "/v1/models/elements").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "missing");
    assert_eq!(body["repo"]["present"], false);
    assert_eq!(body["missing"], json!(["NOUS_ELEMENTS"]));
}
