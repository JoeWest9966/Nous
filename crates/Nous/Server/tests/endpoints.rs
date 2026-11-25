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
        "pinned",
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

#[tokio::test]
async fn entries_are_newest_first_and_follow_an_opaque_cursor() {
    use std::collections::HashMap;

    let mut entries = HashMap::new();
    entries.insert(
        1,
        vec![
            json!({"id": 11, "kind": "pi.assistant", "text": "c", "parent": null}),
            json!({"id": 10, "kind": "pi.user", "text": "b", "parent": null}),
            json!({"id": 9, "kind": "pi.user", "text": "a", "parent": null}),
        ],
    );
    let source = Arc::new(FakeSource {
        sessions: vec![common::item(1, "first")],
        entries,
        live: Default::default(),
    });
    let app = router(AppState::with_source(source));

    let page = |uri: &str| {
        let app = app.clone();
        let uri = uri.to_string();
        async move {
            let response = app
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        }
    };

    let first = page("/v1/sessions/1/entries?limit=2").await;
    assert_eq!(first["items"][0]["id"], json!(11));
    assert_eq!(first["items"][1]["id"], json!(10));
    let next = first["next"].as_str().expect("还有下一页").to_string();
    // 不透明：只许原样回传，不许是明文 `after=10` 或裸 id。
    assert!(!next.contains('='), "游标里不该有键值对，实测 {next}");
    assert_ne!(next, "10", "游标不是裸 id");

    let second = page(&format!("/v1/sessions/1/entries?limit=2&cursor={next}")).await;
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["id"], json!(9));
    assert_eq!(second["next"], Value::Null, "最后一页 next 是 null");
}

#[tokio::test]
async fn entries_are_capped_at_a_thousand_without_an_error() {
    use std::collections::HashMap;

    let items: Vec<Value> = (1..=1_200)
        .rev()
        .map(|id| json!({"id": id, "kind": "pi.user", "text": "", "parent": null}))
        .collect();
    let mut entries = HashMap::new();
    entries.insert(1, items);
    let source = Arc::new(FakeSource {
        sessions: vec![common::item(1, "first")],
        entries,
        live: Default::default(),
    });
    let app = router(AppState::with_source(source));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/1/entries?limit=5000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "超上限是截断，不是报错");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1_000);
}

#[tokio::test]
async fn a_broken_cursor_or_limit_is_400_not_500() {
    let app = router(AppState::with_source(FakeSource::two_sessions()));
    for uri in [
        "/v1/sessions/1/entries?cursor=not-a-cursor",
        "/v1/sessions/1/entries?cursor=a10",
        "/v1/sessions/1/entries?limit=0",
        "/v1/sessions/1/entries?limit=many",
        "/v1/sessions/not-a-number/entries",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["error"].is_string(), "{uri} 的错误形状");
    }
}

/// 内部错误也是 `{ error: string }` + 500，不是纯文本、不是 200 里带 error。
struct BrokenSource;

impl nous_server::SessionSource for BrokenSource {
    fn sessions(&self) -> nous_server::Result<Vec<nous_durable::storage::sessions::SessionItem>> {
        Err(nous_server::ServerError::Internal("boom".to_string()))
    }
    fn entries(&self, _: i64) -> nous_server::Result<Vec<Value>> {
        Err(nous_server::ServerError::Internal("boom".to_string()))
    }
    fn live_message(&self, _: i64) -> nous_server::Result<Option<Value>> {
        Err(nous_server::ServerError::Internal("boom".to_string()))
    }
}

#[tokio::test]
async fn internal_errors_are_500_with_the_error_shape() {
    let app = router(AppState::with_source(Arc::new(BrokenSource)));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], json!("boom"));
}

#[tokio::test]
async fn system_switch_adds_system_entries_and_replays_the_effective_prompt() {
    let source = common::FakeSource::two_sessions();
    // FakeSource 是 Arc，重新造一份带 system entry 的源（id 1）。
    let mut entries = source.entries.clone();
    entries.entry(1).or_default().insert(0, json!({
        "id": 10,
        "kind": "pi.system",
        "text": "you are helpful",
        "parent": null,
        "model": [{"role": "system", "timestamp": 1, "content": "you are helpful"}]
    }));
    entries.entry(1).or_default().insert(1, json!({
        "id": 12,
        "kind": "pi.system",
        "text": "new persona",
        "parent": null,
        "model": [{"role": "system", "timestamp": 2, "content": "new persona"}]
    }));
    let source = Arc::new(common::FakeSource {
        sessions: source.sessions.clone(),
        entries,
        live: source.live.clone(),
    });
    let app = router(AppState::with_source(source));
    let response = app.clone().oneshot(
        Request::builder().uri("/v1/sessions/1/entries").body(Body::empty()).unwrap()
    ).await.unwrap();
    let default: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(default.get("system").is_none(), "不传 system=1 时不能出现 system 字段：{default}");
    let default_items = default["items"].as_array().unwrap();
    assert!(!default_items.iter().any(|item| item["kind"] == json!("pi.system")), "默认要过滤 pi.system");

    let response = app.clone().oneshot(
        Request::builder().uri("/v1/sessions/1/entries?system=1").body(Body::empty()).unwrap()
    ).await.unwrap();
    let system: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    let items = system["items"].as_array().unwrap();
    assert!(items.len() > default_items.len(), "system=1 的 items 应包括 pi.system：{system}");
    assert!(items.iter().any(|item| item["kind"] == json!("pi.system") && item["delta"].is_string()), "system entry 要带 delta：{system}");
    let replay = system["system"].as_str().unwrap_or_default();
    assert!(replay.contains("new persona") && !replay.contains("you are helpful"), "system 是回放后生效的提示词：{system}");
}
