//! #70：真 provider 装配下也要起轮次；模型解析一条明路；`waitMs` / `whenBusy` 的语义。

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_ai::Provider;
use nous_durable::session::{ObserverRegistry, Session};
use nous_server::{router, AppState, DurableCommands, ImportedSource};
use serde_json::{json, Value};
use tower::ServiceExt;

const ANSWER: &str = "ANSWER-70-UNIQUE-4c1e";

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-70-{tag}-{}.sqlite",
        std::process::id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(app: &axum::Router, uri: &str) -> Value {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap())
        .unwrap()
}

/// 组装一个「指向脚本 provider」的 runtime；`kind` 决定走哪条分支。
fn fixture(tag: &str, kind: &'static str) -> (i64, axum::Router, String, String) {
    let db = temp_db(tag);
    let session = Arc::new(Session::open(&db).unwrap());
    let provider = nous_ai::providers::faux::FauxProvider::new({
        let script = nous_ai::providers::faux::FauxScript::new();
        script.append_responses(vec![nous_ai::providers::faux::FauxStep::factory(|_| {
            nous_ai::providers::faux::faux_assistant_message(
                ANSWER,
                nous_ai::StopReason::Stop,
            )
        })]);
        script
    });
    let provider_id = provider.id().to_string();
    let model_id = provider.models()[0].id.clone();
    let tools: Arc<BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>>> = Arc::new(BTreeMap::new());
    let runtime = nous_server::runtime::assemble_with_provider_kind(
        provider,
        tools,
        session.clone(),
        kind,
    );
    let registry = ObserverRegistry::new();
    let commands = DurableCommands::with_session(session.clone(), registry);
    let state = AppState::durable_with_commands(
        ImportedSource::open(&db).unwrap(),
        commands,
    )
    .with_runtime(Arc::new(runtime));
    let id = {
        let created = futures::executor::block_on(session.create_conversation(json!({"name": "t"})))
            .unwrap();
        created["id"].as_i64().unwrap()
    };
    (id, router(state), provider_id, model_id)
}

#[tokio::test]
async fn real_kind_assembly_still_runs_a_turn() {
    // 判据 1：`provider_kind != "faux"` 时也必须起轮次（修之前这里永远只有 user entry）。
    let (id, app, provider, model) = fixture("real-kind", "real");
    let (status, body) = post(
        &app,
        &format!("/v1/sessions/{id}/messages"),
        json!({"content": "hi", "requestId": "r1", "model": {"provider": provider, "modelId": model}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    // 注意：`submit` 会先落一条 assistant **占位** entry，所以「有 pi.assistant」不算跑起来了。
    // 真正的证据是正文里出现 provider 的解释文本（只有轮次真跑了才会写进去）。
    let mut seen = false;
    for _ in 0..200 {
        let entries = get(&app, &format!("/v1/sessions/{id}/entries")).await;
        seen = entries["items"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["kind"] == json!("pi.assistant")
                    && item["text"].as_str().is_some_and(|text| text.contains(ANSWER))
            })
        });
        if seen {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(seen, "真装配下轮次没跑起来：assistant entry 里没有 provider 的正文");
}

#[tokio::test]
async fn no_model_anywhere_is_503_not_202() {
    // 判据 2：请求体、会话、编目默认都没有模型 → 503（不是 202 让客户端白等）。
    let (id, app, _provider, _model) = fixture("no-model", "real");
    let (status, body) = post(
        &app,
        &format!("/v1/sessions/{id}/messages"),
        json!({"content": "hi", "requestId": "r1"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body["error"].is_string(), "错误体要 {{error}}: {body}");
}

#[tokio::test]
async fn wait_ms_returns_the_answer_entry() {
    // 判据 3：`waitMs > 0` 拿到答案回 200 + entry；等待接在轮次完成信号上。
    let (id, app, provider, model) = fixture("wait-ms", "faux");
    let (status, body) = post(
        &app,
        &format!("/v1/sessions/{id}/messages"),
        json!({"content": "hi", "requestId": "r1", "waitMs": 5000,
               "model": {"provider": provider, "modelId": model}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], json!("done"));
    assert_eq!(body["entry"]["kind"], json!("pi.assistant"), "{body}");
    assert!(
        body["entry"]["text"].as_str().is_some_and(|text| text.contains(ANSWER)),
        "200 回包里的 entry 必须带 provider 的正文：{body}"
    );
    assert_eq!(body["submissionId"].as_i64().is_some(), true);
}

#[tokio::test]
async fn wait_ms_zero_is_immediate_202() {
    let (id, app, provider, model) = fixture("wait-zero", "faux");
    let (status, body) = post(
        &app,
        &format!("/v1/sessions/{id}/messages"),
        json!({"content": "hi", "requestId": "r1", "waitMs": 0,
               "model": {"provider": provider, "modelId": model}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn when_busy_only_accepts_two_values() {
    let (id, app, provider, model) = fixture("when-busy", "faux");
    let (status, body) = post(
        &app,
        &format!("/v1/sessions/{id}/messages"),
        json!({"content": "hi", "requestId": "r1", "whenBusy": "whatever",
               "model": {"provider": provider, "modelId": model}}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // 合法值照样受理。
    let (status, body) = post(
        &app,
        &format!("/v1/sessions/{id}/messages"),
        json!({"content": "hi", "requestId": "r2", "whenBusy": "interrupt",
               "model": {"provider": provider, "modelId": model}}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}
