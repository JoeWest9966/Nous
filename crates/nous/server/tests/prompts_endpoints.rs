//! #72 判据 1/2：GET /v1/prompts 真读盘，settings.default 不静默变 null。

mod common;

use std::fs;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("nous-prompts-http-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

async fn get_prompts(root: &std::path::Path) -> Value {
    std::env::set_var("NOUS_PROMPTS", root);
    let app = nous_server::router(nous_server::AppState::with_source(common::FakeSource::two_sessions()));
    let response = app.oneshot(Request::builder().uri("/v1/prompts").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn prompts_lists_real_directories_and_names_an_unconfigured_default() {
    let root = temp_root("list");
    fs::create_dir_all(root.join("alpha")).unwrap();
    fs::write(root.join("alpha/PROMPT.md"), "alpha body").unwrap();
    fs::write(root.join("alpha/nous.json"), json!({"description": "A", "category": ["编写"]}).to_string()).unwrap();
    fs::create_dir_all(root.join("beta")).unwrap();
    fs::write(root.join("settings.json"), json!({"default": "alpha"}).to_string()).unwrap();

    let body = get_prompts(&root).await;
    let items = body["prompts"].as_array().expect("prompts 数组");
    assert_eq!(items.len(), 2, "两个目录两条：{body}");
    let alpha = items.iter().find(|i| i["name"] == json!("alpha")).unwrap();
    assert_eq!(alpha["description"], json!("A"));
    assert_eq!(alpha["problem"], Value::Null);
    let beta = items.iter().find(|i| i["name"] == json!("beta")).unwrap();
    assert!(beta["problem"].as_str().unwrap_or_default().contains("PROMPT.md"), "缺文件说清：{body}");
    assert_eq!(body["default"], json!("alpha"));

    fs::write(root.join("settings.json"), json!({"default": "ghost"}).to_string()).unwrap();
    let body = get_prompts(&root).await;
    assert_eq!(body["default"], json!("ghost"), "错配置不许变 null");
    let problem = body["problem"].as_str().unwrap_or_default();
    assert!(problem.contains("不存在"), "不存在要点名：{problem}");
}
