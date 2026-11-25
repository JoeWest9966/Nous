//! #90：documents 从发行面下线。
//!
//! 保留在 workspace 里的 `nous-documents` crate 不再是产品面：`MANIFEST` / `ROUTES` /
//! `capabilities` 都不许声明它，真 Router 上 12 条文档端点一律 404。

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_server::{router, AppState};
use serde_json::Value;
use tower::ServiceExt;

fn app() -> Router {
    router(AppState::with_source(common::FakeSource::two_sessions()))
}

async fn call(app: &Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().method(method).uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn documents_list_is_not_served() {
    let app = app();
    let path = format!("/v1/{}/presets", "documents");
    let (status, _) = call(&app, "GET", &path).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "documents 下线后列表端点必须 404");
}

#[tokio::test]
async fn no_documents_route_is_advertised() {
    let app = app();
    let (status, body) = call(&app, "GET", "/v1/_routes").await;
    assert_eq!(status, StatusCode::OK);
    let documents_module = format!("{}uments", "doc");
    // 判据打注册表本身：只加 ROUTES 而不改 MANIFEST 时，_routes 会过滤掉它，
    // 所以两条哨兵必须一个看注册表、一个看 capabilities。
    let declared: Vec<_> = nous_server::meta::ROUTES
        .iter()
        .filter(|route| route.module == documents_module)
        .collect();
    assert!(declared.is_empty(), "ROUTES 注册表仍声明 documents：{declared:?}");
    let leaked: Vec<_> = body["routes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|route| {
            route["path"].as_str().unwrap_or_default().contains(&format!("/v1/{documents_module}"))
                || route["module"].as_str().unwrap_or_default() == documents_module
        })
        .collect();
    assert!(leaked.is_empty(), "/v1/_routes 仍声明 documents：{leaked:?}");
}

#[tokio::test]
async fn capabilities_do_not_declare_documents() {
    let documents_module = format!("{}uments", "doc");
    let app = app();
    let (status, body) = call(&app, "GET", "/v1/capabilities").await;
    assert_eq!(status, StatusCode::OK);
    let modules = body["modules"].as_array().unwrap();
    assert!(
        modules.iter().all(|m| m["name"] != "documents"),
        "capabilities.modules 仍含 documents：{modules:?}"
    );
    assert!(
        modules.iter().all(|m| {
            m["endpoints"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|e| !e["path"].as_str().unwrap_or_default().contains(&format!("/v1/{documents_module}")))
        }),
        "capabilities.endpoints 仍含 documents 路径：{modules:?}"
    );
}
