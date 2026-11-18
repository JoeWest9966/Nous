//! **端到端判据**（`#44` 的 AC 第 3/4 条）：请求从 HTTP 进来，库里真的多一条 assistant entry。
//!
//! 这条判据存在的理由：上一版交付的轮次单测全绿，但 `/messages` 里只有一行
//! `let _driver = runtime.driver.clone();` —— 一行让 `git grep` 有命中、却没起任何轮次的代码。
//! **判据绿 / `cargo tree` 含依赖 / `git grep` 有命中，都不能替代"库里多了一条"**。
//! 见 `docs/pitfalls.md` 2026-11-18 那条。
//!
//! 装配方式：真 `AppState::durable`（真 SQLite、真 `Session`、真 SSE hub）+ 注入一个
//! faux 轮次工厂。于是这条测试同时覆盖了「provider → 工具 → 唯一写者 → 一条提交」和
//! 「提交 → 观察者 → SSE」两段，**不联网**。
mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::session::Session;
use nous_server::runtime::{TurnFactory, TurnHandle};
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

/// faux 轮次工厂：把「真 provider 解析」这一步换掉，其余（Session、提交、SSE）都是真的。
struct FauxTurns {
    session: Arc<Session>,
}

impl TurnFactory for FauxTurns {
    fn turn_for(&self, _model: &str) -> nous_server::Result<TurnHandle> {
        let script = nous_ai::providers::faux::FauxScript::new().with_text("answer from the turn");
        Ok(TurnHandle::faux(
            Arc::clone(&self.session),
            Arc::new(nous_tools::ToolMap::new()),
            script,
        ))
    }
}

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-server-e2e-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn post(app: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn a_message_through_the_http_endpoint_commits_an_assistant_entry() {
    let path = temp_db("turn");
    let state = AppState::durable(&path).expect("durable state");
    let session = state.commands().session().expect("写者");
    let state = state.with_turn_factory(Arc::new(FauxTurns {
        session: Arc::clone(&session),
    }));
    let app = router(Arc::clone(&state));

    // 建会话：模型必须能被 `/messages` 读出来（它在会话配置里，不在请求里）。
    let (status, created) = post(
        app.clone(),
        "/v1/sessions",
        json!({
            "name": "e2e",
            "model": { "provider": "faux", "modelId": "faux-model" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "建会话: {created}");
    let conversation = created["id"].as_i64().expect("id");

    // 先订阅 SSE，再发消息 —— 帧必须真的出去（AC：`message_start` + `entry_appended`）。
    let stream_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&format!("/v1/sessions/{conversation}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream_response.status(), StatusCode::OK, "SSE 订阅");
    let mut frames = stream_response.into_body().into_data_stream();

    let (status, accepted) = post(
        app.clone(),
        &format!("/v1/sessions/{conversation}/messages"),
        json!({ "content": "hello from the endpoint", "requestId": "req-e2e" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "受理: {accepted}");
    let submission = accepted["submissionId"].as_i64().expect("submissionId");

    // 轮次是 spawn 出去的：轮询到库里有终态 entry 为止（不 sleep 定长，不猜）。
    let mut assistants = Vec::new();
    for _ in 0..200 {
        assistants = entries_of_kind(&path, conversation, "pi.assistant");
        if !assistants.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        assistants.len(),
        1,
        "一轮一条 assistant entry（HTTP 进来的这一条），实际 {assistants:?}"
    );
    assert_eq!(assistants[0]["reason"], json!("done"), "终态是 done");
    assert_eq!(
        assistants[0]["submissionId"],
        json!(submission),
        "终态 entry 上的 submissionId 必须与受理时返回的一致"
    );

    // SSE：把已经发出的帧读干（订阅在 POST 之前建立，所以不会漏）。
    use futures::StreamExt;
    let mut seen = String::new();
    for _ in 0..64 {
        let next = tokio::time::timeout(std::time::Duration::from_millis(500), frames.next()).await;
        match next {
            Ok(Some(Ok(chunk))) => {
                seen.push_str(&String::from_utf8_lossy(&chunk));
                if seen.contains("message_end") && seen.contains("entry_appended") {
                    break;
                }
            }
            Ok(Some(Err(error))) => panic!("SSE 读失败: {error}"),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    assert!(
        seen.contains("message_start"),
        "SSE 里要有 message_start（用户输入），实收：{seen}"
    );
    assert!(
        seen.contains("entry_appended"),
        "SSE 里要有 entry_appended，实收：{seen}"
    );
    assert!(
        seen.contains("message_end"),
        "SSE 里要有 message_end（终态），实收：{seen}"
    );

    // 用户那条输入也在库里（受理时写的，不是伪造的）。
    let users = entries_of_kind(&path, conversation, "pi.user");
    assert_eq!(users.len(), 1, "受理写了一条 pi.user");
    assert_eq!(users[0]["submissionId"], Value::Null, "input/queued 阶段没有 submissionId 之外的形状承诺");
}

fn entries_of_kind(path: &std::path::Path, conversation: i64, kind: &str) -> Vec<Value> {
    let storage = nous_durable::Storage::open_read_only(path).expect("只读打开");
    let page = storage
        .scan_entries(
            &nous_durable::EntryQuery {
                conversation_id: conversation,
                min_entry_id: None,
                max_entry_id: None,
                order: None,
            },
            100,
            None,
        )
        .expect("扫 entry")
        .expect("会话存在");
    page.items
        .into_iter()
        .filter(|entry| entry["kind"] == json!(kind))
        .collect()
}
