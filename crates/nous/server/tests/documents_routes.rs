//! #48 第三片：`/v1/_routes` 自省与 documents 端点的字段形状。
//!
//! 接缝：真 Router（`ServiceExt::oneshot`）。"列出的路由数 == 实际注册数"由同一个
//! registry 构造 router + 测试逐条打请求验证，测试里不手抄两份路由清单。

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use nous_server::meta;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

mod common;

fn app() -> axum::Router {
    router(AppState::with_source(common::FakeSource::two_sessions()))
}

async fn call(app: &axum::Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::from(
            body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default(),
        )).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

#[tokio::test]
async fn routes_introspection_matches_what_the_router_registers() {
    let app = app();
    let (status, body) = call(&app, Method::GET, "/v1/_routes", None).await;
    assert_eq!(status, StatusCode::OK);
    // 调试视图与 capabilities 共用 `meta.rs` 的唯一注册表（不手抄两份）。
    assert_eq!(body, meta::routes_view(), "_routes 必须来自 meta 的唯一注册表");
    let routes = body["routes"].as_array().expect("routes 是数组");
    let modules = body["modules"].as_array().expect("modules 是数组");
    assert!(modules.iter().any(|m| m["name"] == "documents" && m["enabled"] == true));
    let documents = routes
        .iter()
        .filter(|r| r["module"] == "documents")
        .count();
    assert_eq!(
        documents,
        nous_documents::routes::DOCUMENT_ROUTES.len(),
        "注册表里的 documents 端点数 == 泛型 handler 的注册源条数"
    );
    // 逐条比对模板原文：不是手抄第二份列表，而是断言注册源里的每一条都在唯一注册表里。
    for doc_route in nous_documents::routes::DOCUMENT_ROUTES {
        assert!(
            meta::ROUTES.iter().any(|r| r.method == doc_route.method && r.path == doc_route.path),
            "documents 注册源 {} {} 不在 meta 的唯一注册表里",
            doc_route.method, doc_route.path
        );
    }
    // 每一条声明的路由都能在真 router 上走到 handler（未知 kind 的 404 也算走到 —— 不是 405/500）。
    for route in meta::ROUTES {
        if !meta::route_enabled(route.module) || route.path.ends_with("/events") {
            // SSE 是长连接，`to_bytes` 会永远等 —— 它的注册由 wire_shapes 另测。
            continue;
        }
        let method: Method = route.method.parse().unwrap();
        // 把模板参数换成具体值；一行测试自己的参数替换，不复制路径清单。
        let uri = route.path
            .replace(":id", "2")
            .replace("{kind}", "presets")
            .replace("{section}", "Session-Template")
            .replace("{name}", "missing-name");
        let probe_body = if method == Method::GET { None } else { Some(json!({})) };
        let (status, _) = call(&app, method.clone(), &uri, probe_body).await;
        assert!(
            status != StatusCode::METHOD_NOT_ALLOWED && status != StatusCode::INTERNAL_SERVER_ERROR,
            "{method} {uri} 没有在 router 上注册：{status}"
        );
    }
}

#[tokio::test]
async fn validate_response_shape_is_snapshotted() {
    let app = app();
    let (status, body) = call(
        &app,
        Method::GET,
        "/v1/documents/presets/Session-Template/missing-name/validate",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for key in ["ok", "errors", "warnings", "resolved"] {
        assert!(body.get(key).is_some(), "缺 {key}: {body}");
    }
    assert!(body["errors"].is_array());
    assert!(body["warnings"].is_array());
    assert!(body["resolved"].is_object());
    assert_eq!(body["ok"], json!(false));

    let expected = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/snapshots/documents_validation.json"),
    )
    .expect("快照文件存在");
    assert_eq!(
        serde_json::to_string_pretty(&body).unwrap() + "\n",
        expected,
        "validate 形状快照漂移"
    );
}
