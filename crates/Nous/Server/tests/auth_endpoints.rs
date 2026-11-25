//! #50 判据 1/2：Bearer 中间件在真 Router 上全量覆盖 `/v1/*`。
//!
//! 负向哨兵：把 `require_bearer` 这层从 app 上摘掉，前两条测试立刻变红
//! （401 变 200 / SSE 无鉴权也能进）。

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_server::auth::require_bearer;
use nous_server::{router, AppState};
use tower::ServiceExt;

const TOKEN: &str = "test-token-abcdefghijklmnopqrstuvwxyz";

fn protect(state: std::sync::Arc<AppState>) -> axum::Router {
    let token = TOKEN.to_string();
    router(state).layer(axum::middleware::from_fn(move |req, next| {
        require_bearer(token.clone(), req, next)
    }))
}

fn app() -> axum::Router {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("auth.db");
    std::mem::forget(dir); // 测试进程短命：别让 TempDir 在连接还活着时把库删了
    protect(AppState::durable(db).unwrap())
}

fn fake_app() -> axum::Router {
    protect(AppState::with_source(common::FakeSource::two_sessions()))
}

async fn status(app: &axum::Router, uri: &str, auth: Option<&str>) -> StatusCode {
    let mut builder = Request::builder().uri(uri);
    if let Some(value) = auth {
        builder = builder.header("authorization", value);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn missing_wrong_and_empty_bearer_are_all_401_with_www_authenticate() {
    let app = app();
    for auth in [
        None,
        Some("Bearer wrong-token"),
        Some("Bearer "),
        Some("Basic whatever"),
    ] {
        let mut builder = Request::builder().uri("/v1/sessions");
        if let Some(value) = auth {
            builder = builder.header("authorization", value);
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "auth={auth:?}");
        assert_eq!(
            response.headers()["www-authenticate"],
            "Bearer",
            "auth={auth:?}"
        );
    }
}

#[tokio::test]
async fn every_v1_route_is_protected_including_capabilities_and_sse() {
    let app = app();
    // 建一条会话，这样 messages / events 走到真处理逻辑（而不是 404）。
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/sessions")
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"auth-test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(created.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = created["id"].as_i64().expect("created.id 是数字");

    let paths = vec![
        format!("/v1/sessions/{id}/messages"),
        format!("/v1/sessions/{id}/events"),
        "/v1/capabilities".to_string(),
        "/v1/_routes".to_string(),
    ];
    for path in paths {
        assert_eq!(
            status(&app, &path, None).await,
            StatusCode::UNAUTHORIZED,
            "{path} 没有鉴权就放进去了"
        );
    }

    // 对 token：三条点名的 200（SSE 只验握手状态码，字节背压另测）。
    assert_eq!(
        status(&app, "/v1/sessions", Some(&format!("Bearer {TOKEN}"))).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, "/v1/capabilities", Some(&format!("Bearer {TOKEN}"))).await,
        StatusCode::OK,
        "capabilities 也必须在 token 后面"
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/sessions/{id}/messages"))
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"content":"hello"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    // #70：无模型来源时 503 是新契约；这里只验「token 挡不住这条路由」，
    // 所以 202/503 都算过，401/403 才算红。
    assert!(
        matches!(response.status(), StatusCode::ACCEPTED | StatusCode::SERVICE_UNAVAILABLE),
        "带 token 的 messages 不该被鉴权挡掉：{}",
        response.status()
    );

    // SSE 用假源（真库空快照没有 live 文档会 500，那是 #47 前的行为，不是本票要改的）。
    let response = fake_app()
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/1/events")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/event-stream",
        "SSE 头被中间件改了"
    );
    // 同一条 SSE 路径，没 token 时必须 401。
    assert_eq!(
        status(&fake_app(), "/v1/sessions/1/events", None).await,
        StatusCode::UNAUTHORIZED
    );
}
