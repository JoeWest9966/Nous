//! #60 判据 9：`/v1/openapi.json` 的 presets 路径集合 == `wire.md` presets 段。
//!
//! 两边**不同源**：一边是 HTTP 响应（从 axum 路由注册表生成），一边是读仓库根的
//! `wire.md` 表格文本。集合比对，不比顺序；少一条 / 多一条都红。

use std::collections::BTreeSet;
use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_server::{router, AppState};
use serde_json::Value;
use tower::ServiceExt;

mod common;

fn wire_presets_paths() -> BTreeSet<String> {
    let wire = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../wire.md");
    let text = std::fs::read_to_string(&wire)
        .unwrap_or_else(|error| panic!("wire.md 读不到：{} —— {}", wire.display(), error));
    let mut paths = BTreeSet::new();
    for line in text.lines() {
        if !line.starts_with('|') { continue; }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.is_empty() { continue; }
        // 表格第一列是 "`GET /v1/presets/:section`"：方法和路径写在同一格。
        let combined = cells[0].trim_matches('`').trim();
        let mut parts = combined.splitn(2, char::is_whitespace);
        let method = parts.next().unwrap_or("").trim();
        let path = parts.next().unwrap_or("").trim();
        if !matches!(method, "GET" | "PUT" | "POST" | "PATCH" | "DELETE") { continue; }
        if !path.starts_with("/v1/presets") { continue; }
        // wire 用 `:param`，OpenAPI 用 `{param}`：集合比对前归一化参数风格。
        let normalized = path.split('/').map(|seg| {
            if let Some(name) = seg.strip_prefix(':') { format!("{{{name}}}") } else { seg.to_string() }
        }).collect::<Vec<_>>().join("/");
        paths.insert(format!("{method} {normalized}"));
    }
    assert!(!paths.is_empty(), "wire.md 的 presets 表一条都没解析到 —— 判据在空转");
    paths
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let response = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn openapi_presets_paths_match_the_contract_exactly() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get(&app, "/v1/openapi.json").await;
    assert_eq!(status, StatusCode::OK, "OpenAPI 端点应 200：{body}");

    let mut generated = BTreeSet::new();
    let paths = body["paths"].as_object().unwrap_or_else(|| panic!("OpenAPI 缺 paths：{body}"));
    for (path, item) in paths {
        for method in ["get", "put", "post", "patch", "delete"] {
            if item.get(method).is_some() {
                generated.insert(format!("{} {path}", method.to_uppercase()));
            }
        }
    }
    assert!(!generated.is_empty(), "生成物是空的 —— 判据在空转");
    let expected = wire_presets_paths();
    assert_eq!(generated, expected, "OpenAPI 与 wire.md presets 段漂移（左边=生成物，右边=契约表）");
}

#[tokio::test]
async fn every_generated_preset_path_is_really_registrered_on_the_router() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    // 反向哨兵：注册表写了但 handler 没挂时，真 Router 会回 405（不是 404 —— 404 可能是资源不存在）。
    let (_, body) = get(&app, "/v1/openapi.json").await;
    for (path, item) in body["paths"].as_object().unwrap() {
        for method in ["get", "put", "post", "patch", "delete"] {
            if item.get(method).is_none() { continue; }
            let uri = path.replace("{section}", "Session-Template").replace("{name}", "not-there")
                .replace("{n}", "1");
            let request = Request::builder().method(axum::http::Method::from_bytes(method.to_uppercase().as_bytes()).unwrap()).uri(uri.clone())
                .body(Body::empty()).unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert!(
                response.status() != StatusCode::METHOD_NOT_ALLOWED,
                "生成物声明的 {method} {uri} 没挂在真 Router 上（405）"
            );
        }
    }
}
