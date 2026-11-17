//! 端点判据：`GET /v1/sessions` 的形状 + SSE 的第一帧。
//!
//! 打的是真 Router（`tower::ServiceExt::oneshot`），不起监听端口：没有端口冲突、没有
//! 防火墙，判据只测「请求进来 → 响应出去」这一面。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{first_sse_data, FakeSource};
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn get_sessions_returns_the_wire_shape_newest_first() {
    let app = router(AppState::with_source(FakeSource::two_sessions()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let items = body["items"].as_array().expect("{ items: [...] }");
    assert_eq!(items.len(), 2);
    // 新的在前：假源给的就是 2, 1；投影层不再重排。
    assert_eq!(items[0]["id"], json!(2));
    assert_eq!(items[0]["name"], json!("second"));

    // `wire.md` 的字段名是 camelCase；蛇形键出现就是契约破了。
    for key in [
        "id",
        "name",
        "parent",
        "owner",
        "model",
        "thinkingLevel",
        "thinkingLevels",
        "skills",
        "template",
        "readOnly",
        "busy",
        "updatedAt",
        "last",
        "archived",
    ] {
        assert!(items[0].get(key).is_some(), "缺字段 {key}");
    }
    assert!(items[0].get("thinking_level").is_none(), "出现了蛇形键");
    // 空值是 `null` / `[]` / `""`，不是省略。
    assert_eq!(items[0]["thinkingLevels"], json!([]));
    assert_eq!(items[0]["readOnly"], json!(false));
    assert_eq!(items[0]["parent"], Value::Null);
}

#[tokio::test]
async fn an_unknown_session_is_404_with_the_error_shape() {
    let app = router(AppState::with_source(FakeSource::two_sessions()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/99/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["error"].is_string(), "错误形状必须是 {{ error: string }}");
}

#[tokio::test]
async fn the_event_stream_opens_with_a_snapshot_frame() {
    let app = router(AppState::with_source(FakeSource::two_sessions()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let data = first_sse_data(response.into_body()).await;
    let frame: Value = serde_json::from_str(&data).expect("首帧是 JSON");
    assert_eq!(frame["type"], json!("snapshot"), "重连第一帧必须是 snapshot");
    assert_eq!(frame["session"]["id"], json!(1));
    // `entries[].model[].timestamp` 是 SSE 客户端唯一的时间源，必须原样在。
    assert_eq!(frame["entries"][0]["model"][0]["timestamp"], json!(1_700_000_000_000_i64));
    // 有正在生成的消息就续上。
    assert_eq!(
        frame["generation"]["message"]["content"][0]["text"],
        json!("streaming")
    );
}

#[tokio::test]
async fn a_session_without_a_live_message_has_no_generation_key() {
    let source = Arc::new(FakeSource {
        sessions: vec![common::item(3, "")],
        entries: Default::default(),
        live: Default::default(),
    });
    let app = router(AppState::with_source(source));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/3/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let data = first_sse_data(response.into_body()).await;
    let frame: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(frame["type"], json!("snapshot"));
    assert_eq!(frame["entries"], json!([]));
    assert!(frame.get("generation").is_none(), "没有生成中就不吐 generation");
}

/// 从开着的 SSE 流里读下一帧（2 秒超时）。
async fn next_frame(stream: &mut axum::body::BodyDataStream) -> Value {
    use futures::StreamExt;
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("2 秒内该到一帧")
        .expect("流不该先关")
        .expect("块读得出来");
    let text = String::from_utf8(chunk.to_vec()).unwrap();
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("data 行");
    serde_json::from_str(data).expect("帧是 JSON")
}

#[tokio::test]
async fn a_published_event_reaches_the_open_stream_in_order() {
    use nous_wire::{MessageChange, RemoteEvent};
    use serde_json::json;

    let state = AppState::with_source(FakeSource::two_sessions());
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();

    assert_eq!(next_frame(&mut stream).await["type"], json!("snapshot"));

    // 工具启动：状态转换帧，不参与合帧，立刻出来。
    state.publish_event(
        1,
        RemoteEvent::ToolExecutionStart {
            tool_call_id: "call-1".into(),
            tool_name: "bash".into(),
            args: json!({}),
        },
    );
    let frame = next_frame(&mut stream).await;
    assert_eq!(frame["type"], json!("tool_execution_start"));
    assert_eq!(frame["toolName"], json!("bash"));

    // 增量：一个窗口之后出来。
    state.publish_event(
        1,
        RemoteEvent::MessageUpdate {
            changes: vec![MessageChange::TextDelta {
                content_index: 0,
                delta: "hi".into(),
            }],
            message_id: None,
            message: None,
        },
    );
    let frame = next_frame(&mut stream).await;
    assert_eq!(frame["type"], json!("message_update"));
    assert_eq!(frame["changes"][0]["type"], json!("text_delta"));
    assert_eq!(frame["changes"][0]["delta"], json!("hi"));
}
