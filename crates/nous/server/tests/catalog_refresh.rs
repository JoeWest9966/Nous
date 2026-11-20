//! #79 判据：远端编目刷新 —— 假服务 → `POST /v1/catalog/refresh` → `/v1/models` / `/v1/device`。
//!
//! 接缝（公开面，不碰内部函数）：
//!   - `nous_device::CatalogEnvironment`（远端地址 / token 快照 + 本地缓存路径）；
//!   - 真 Router：`POST /v1/catalog/refresh` 与 `GET /v1/models` / `GET /v1/device`；
//!   - 假编目服务用真 TCP listener（`std::net::TcpListener` + 线程），断言它收到的
//!     `Authorization: Bearer <token>` —— 不靠看实现源码。

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use nous_device::CatalogEnvironment;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const FAKE_TOKEN: &str = "catalog-secret-79";

/// 一次请求：方法 / 路径 / authorization 头 / 是否带 body。
#[derive(Debug)]
struct Seen {
    authorization: Option<String>,
    #[allow(dead_code)]
    path: String,
}

/// 起一个一次性假编目服务：回 `status` + `body`，把看到的请求头回给主线程。
fn fake_catalog(status: u16, body: String) -> (String, mpsc::Receiver<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 4096];
        let size = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..size]).to_string();
        let head = request.lines().next().unwrap_or_default().to_string();
        let authorization = request
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_string());
        tx.send(Seen { authorization, path: head }).unwrap();
        let reason = if status == 200 { "OK" } else { "Internal Server Error" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(), body
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });
    (base, rx)
}

fn three_models() -> String {
    json!({
        "default": { "provider": "remote", "modelId": "remote/one" },
        "items": [
            { "provider": "remote", "modelId": "remote/one", "name": "One",
              "thinkingLevels": ["off"], "contextWindow": null, "maxTokens": null },
            { "provider": "remote", "modelId": "remote/two", "name": "Two",
              "thinkingLevels": ["off", "high"], "contextWindow": 1000, "maxTokens": 100 },
            { "provider": "remote", "modelId": "remote/three", "name": "Three",
              "thinkingLevels": ["off"], "contextWindow": null, "maxTokens": null }
        ]
    })
    .to_string()
}

fn catalog_file(dir: &tempfile::TempDir) -> PathBuf {
    let path = dir.path().join("models.json");
    std::fs::write(
        &path,
        json!({
            "default": null,
            "items": [
                { "provider": "old", "modelId": "old/only", "name": "Old",
                  "thinkingLevels": ["off"], "contextWindow": null, "maxTokens": null }
            ]
        })
        .to_string(),
    )
    .unwrap();
    path
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

fn app_with(env: CatalogEnvironment) -> Router {
    router(
        AppState::with_source(common::FakeSource::two_sessions()).with_catalog_env(env),
    )
}

fn env_with(base: &str, dir: &tempfile::TempDir, path: &PathBuf) -> CatalogEnvironment {
    CatalogEnvironment {
        inline: None,
        file: Some(path.clone()),
        file_path: path.clone(),
        remote_base: Some(base.to_string()),
        remote_token: Some(FAKE_TOKEN.to_string()),
        cache_path: Some(dir.path().join("catalog-cache.json")),
    }
}

#[tokio::test]
async fn refresh_lists_three_remote_models_and_reports_fetched_at() {
    let dir = tempfile::tempdir().unwrap();
    let path = catalog_file(&dir);
    let (base, seen) = fake_catalog(200, three_models());
    let app = app_with(env_with(&base, &dir, &path));

    let (status, body) = call(&app, Method::POST, "/v1/catalog/refresh", None).await;
    assert_eq!(status, StatusCode::OK, "refresh 应 200：{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 3, "远端三模型逐条回");
    assert!(
        body["fetchedAt"].as_str().is_some_and(|s| !s.is_empty()),
        "fetchedAt 必须是非空时间：{body}"
    );

    let request = seen.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        request.authorization.as_deref(),
        Some(format!("Bearer {FAKE_TOKEN}").as_str()),
        "刷新必须带 Bearer token，且测试内可见"
    );

    // 同一份 Router 再读：远端编目已写进本地缓存文件，模型表不再手填。
    let (_, models) = call(&app, Method::GET, "/v1/models", None).await;
    assert_eq!(models["items"].as_array().unwrap().len(), 3, "GET /v1/models 应是远端三条");
    assert_eq!(models["default"]["modelId"], json!("remote/one"));
    let (_, device) = call(&app, Method::GET, "/v1/device", None).await;
    assert!(
        device["catalog"]["fetchedAt"].as_str().is_some_and(|s| !s.is_empty()),
        "/v1/device 的 catalog 必须有真实 fetchedAt：{device}"
    );
}
