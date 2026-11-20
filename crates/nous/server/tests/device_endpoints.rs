//! #65 判据 3/4/5：真 Router 上的模型编目与默认模型写回。
//!
//! 接缝：`router(state)` + HTTP（真 JSON body / status）。测试用临时目录做编目文件，
//! 用 `AppState::with_catalog_env` 注入环境快照（不依赖全局进程环境变量）。

mod common;

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use nous_device::CatalogEnvironment;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn catalog_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(
        &path,
        json!({
            "default": null,
            "items": [
                { "provider": "file", "modelId": "file/one", "name": "One",
                  "thinkingLevels": ["off"], "contextWindow": null, "maxTokens": null },
                { "provider": "file", "modelId": "file/two", "name": "Two",
                  "thinkingLevels": ["off", "max"], "contextWindow": 200000, "maxTokens": 8192 }
            ]
        })
        .to_string(),
    )
    .unwrap();
    Fixture { _dir: dir, path }
}

fn app_with(fixture: &Fixture, inline: Option<String>) -> Router {
    let env = CatalogEnvironment {
        inline,
        file: Some(fixture.path.clone()),
        file_path: fixture.path.clone(),
    };
    router(
        AppState::with_source(common::FakeSource::two_sessions())
            .with_catalog_env(env),
    )
}

async fn call(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(body.map(|value| Body::from(value.to_string())).unwrap_or_default())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn inline_models_are_listed_item_by_item_with_ok_catalog() {
    let fixture = catalog_fixture();
    let inline = json!({
        "default": { "provider": "inline", "modelId": "inline/a" },
        "items": [
            { "provider": "inline", "modelId": "inline/a", "name": "A",
              "thinkingLevels": ["off"], "contextWindow": null, "maxTokens": null },
            { "provider": "inline", "modelId": "inline/b", "name": "B",
              "thinkingLevels": ["off", "high"], "contextWindow": 1000, "maxTokens": 100 }
        ]
    })
    .to_string();
    let app = app_with(&fixture, Some(inline));
    let (status, body) = call(&app, Method::GET, "/v1/models", None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["modelId"], "inline/a");
    assert_eq!(items[1]["modelId"], "inline/b");
    assert_eq!(body["catalog"]["state"], json!("ok"));
    assert_eq!(body["catalog"]["source"], json!("env"));
}

#[tokio::test]
async fn absent_catalog_reports_missing_state_and_default_path() {
    let dir = tempfile::tempdir().unwrap();
    let env = CatalogEnvironment {
        inline: None,
        file: None,
        file_path: PathBuf::from("/var/lib/nous/models.json"),
    };
    let app = router(AppState::with_source(common::FakeSource::two_sessions()).with_catalog_env(env));
    let (status, body) = call(&app, Method::GET, "/v1/models", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));
    assert_eq!(body["catalog"]["state"], json!("missing"));
    assert!(body["catalog"]["source"].is_null());
    assert_eq!(body["catalog"]["path"], json!("/var/lib/nous/models.json"));
}

#[tokio::test]
async fn put_default_accepts_a_catalogued_model_and_it_survives_a_reload() {
    let fixture = catalog_fixture();
    let app = app_with(&fixture, None);
    let (status, body) = call(
        &app,
        Method::PUT,
        "/v1/default",
        Some(json!({ "provider": "file", "modelId": "file/two" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "写存在的模型应 200：{body}");
    assert_eq!(body["wrote"], json!(true));
    assert_eq!(body["default"]["modelId"], json!("file/two"));

    // 重启 = 新的 AppState + 新的 Router，同一份文件。
    let restarted = app_with(&fixture, None);
    let (status, after) = call(&restarted, Method::GET, "/v1/models", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["default"]["modelId"], json!("file/two"));
}

#[tokio::test]
async fn put_default_refuses_a_model_outside_the_catalog() {
    let fixture = catalog_fixture();
    let app = app_with(&fixture, None);
    let (status, body) = call(
        &app,
        Method::PUT,
        "/v1/default",
        Some(json!({ "provider": "ghost", "modelId": "ghost/nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "不存在的模型必须 400：{body}");
    assert!(body["error"].is_string());
}
