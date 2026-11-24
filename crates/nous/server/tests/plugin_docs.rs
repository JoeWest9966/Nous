//! `GET /v1/plugins/{name}/readme`：插件人话 README 的只读口。
//!
//! 三条判据都在「真 Router」上打（`tower::ServiceExt::oneshot`）：白名单、未装文档的
//! 404 措辞、以及读盘缓存（改文件后不用重启就能读到新正文）。
//!
//! `NOUS_DOCS_DIR` 是进程级环境变量，同文件内的测试用一把锁串行，避免互相污染。

mod common;

use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::FakeSource;
use nous_server::{router, AppState};
use serde_json::Value;
use tower::ServiceExt;

static ENV_LOCK: Mutex<()> = Mutex::new(());

async fn get(uri: &str) -> (StatusCode, Value) {
    let app = router(AppState::with_source(FakeSource::two_sessions()));
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn readme_serves_the_doc_from_the_published_directory() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("env.md"),
        "# env\n\n执行环境的契约层。\n",
    )
    .unwrap();
    std::env::set_var("NOUS_DOCS_DIR", dir.path());

    let (status, body) = get("/v1/plugins/env/readme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["module"], "env");
    assert_eq!(body["markdown"], "# env\n\n执行环境的契约层。\n");
    assert_eq!(body["bytes"], "# env\n\n执行环境的契约层。\n".len());

    // 缓存键是 mtime + 长度：改长一点一定失效，不用重启引擎。
    std::fs::write(
        dir.path().join("env.md"),
        "# env\n\n执行环境的契约层，改过。\n",
    )
    .unwrap();
    let (status, body) = get("/v1/plugins/env/readme").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["markdown"], "# env\n\n执行环境的契约层，改过。\n");

    std::env::remove_var("NOUS_DOCS_DIR");
}

#[tokio::test]
async fn readme_rejects_names_outside_the_manifest() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("NOUS_DOCS_DIR", dir.path());

    let (status, body) = get("/v1/plugins/..%2F..%2Fetc/readme").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown_module");

    let (status, body) = get("/v1/plugins/not-a-plugin/readme").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "unknown_module");

    std::env::remove_var("NOUS_DOCS_DIR");
}

#[tokio::test]
async fn readme_says_so_when_the_doc_was_not_published() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("NOUS_DOCS_DIR", dir.path());

    let (status, body) = get("/v1/plugins/sessions/readme").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "readme_not_installed");
    assert!(body["hint"].as_str().unwrap().contains("--docs-only"));

    std::env::remove_var("NOUS_DOCS_DIR");
}
