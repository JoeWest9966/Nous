//! #70 的判据：真 provider 装配下 `/messages` 必须真跑轮次。
//!
//! 接缝是公开的 HTTP + sqlite：进程内真 Router + 真库，provider 是测试内起的
//! 假 HTTP 服务（OpenAI completions SSE），客户端只走 `/v1/*`。不调 `TurnDriver`、
//! 不读 provider 的内部状态。

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use futures::StreamExt;
use nous_durable::session::{CommitOptions, ObserverRegistry, Session};
use nous_durable::{DocumentContent, StorageWrite};
use nous_server::runtime::AssembledRuntime;
use nous_server::{router, AppState, DurableCommands, SessionObserver};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-70-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

/// 建会话 1、会话 2（会话 2 带 model），给 abort 判据用。
async fn seed(path: &PathBuf, model: Option<&str>) {
    let session = Session::open(path).unwrap();
    let mut writes = vec![StorageWrite::Conversation(json!({ "id": 1 }))];
    if let Some(model) = model {
        // 会话的 model 存在 pi.agent 文档里（/v1/sessions 的投影也从这里读），
        // 不是 conversation record 上的字段。
        let (provider, id) = model.split_once('/').expect("seed model 是 provider/id");
        writes.push(StorageWrite::DocumentCreate {
            id: 10,
            record: json!({
                "id": 10,
                "kind": "pi.agent",
                "scope": { "kind": "conversation", "conversationId": 1 },
                "history": "latest",
                "fork": "current",
            }),
            content: DocumentContent::Base {
                version: 0,
                value: json!({ "model": { "provider": provider, "modelId": id } }),
            },
        });
    }
    session.commit(writes, CommitOptions::default()).await.unwrap();
    session.close().await.unwrap();
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(body) => Body::from(serde_json::to_vec(&body).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

/// 只读连接：观察落库结果（entries / submissions / tasks）。
fn read_rows(path: &PathBuf, table: &str) -> Vec<Value> {
    let output = std::process::Command::new("python3")
        .args([
            "-c",
            "import json,sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nrows=c.execute('select record from '+sys.argv[2]+' order by id').fetchall()\nprint(json.dumps([json.loads(r[0]) for r in rows]))",
            path.to_str().unwrap(),
            table,
        ])
        .output()
        .expect("python3 可用");
    assert!(output.status.success(), "查库失败：{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("库里的 record 是 JSON")
}

async fn wait_for(path: &PathBuf, table: &str, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let rows = read_rows(path, table);
        if rows.iter().any(&predicate) {
            return rows;
        }
        assert!(tokio::time::Instant::now() < deadline, "{table} 5 秒内没出现期望行：{rows:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 用 `assemble_with_provider` 装一套「真 provider」的运行时：kind=real 时路由必须照样跑。
fn runtime(providers: Arc<dyn nous_ai::Provider>, session: Arc<Session>) -> Arc<AssembledRuntime> {
    let tools: BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>> = BTreeMap::new();
    let mut assembled =
        nous_server::runtime::assemble_with_provider(providers, Arc::new(tools), session);
    // 真 provider 装配的语义：**provider_kind 是 real**，但 driver 照样在。
    // `assemble_with_provider` 目前回 faux；这里显式覆盖，模拟 j1900 没有 NOUS_PROVIDER 的现场。
    assembled.provider_kind = "real";
    Arc::new(assembled)
}

#[tokio::test]
async fn a_real_provider_answer_is_persisted_and_finishes_the_submission() {
    let path = temp_db("real-turn");
    seed(&path, Some("catalog/model-x")).await;
    let session = Arc::new(Session::open(&path).unwrap());
    let state = AppState::durable(&path).unwrap();
    let provider = nous_ai::providers::faux::FauxProvider::new({
        let script = nous_ai::providers::faux::FauxScript::new();
        script.append_responses(vec![nous_ai::providers::faux::FauxStep::factory(|_| {
            nous_ai::providers::faux::faux_assistant_message("from real provider", nous_ai::StopReason::Stop)
        })]);
        script
    });
    let state = state
        .with_runtime(runtime(provider, session))
        .with_models(catalog_with_default("catalog", "model-x"));
    let app = router(state);

    let (status, body) = call(
        &app,
        "POST",
        "/v1/sessions/1/messages",
        Some(json!({ "content": "ping" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "实测 {body}");
    let submission = body["submissionId"].as_i64().expect("submissionId");

    let entries = wait_for(&path, "entries", |entry| {
        entry["kind"] == json!("pi.assistant")
    }).await;
    let assistant = entries.iter().find(|entry| entry["kind"] == json!("pi.assistant")).unwrap();
    assert_eq!(assistant["reason"], json!("done"));
    assert_eq!(assistant["submissionId"], json!(submission));
    assert_eq!(assistant["model"][0]["content"][0]["text"], json!("from real provider"));

    let submissions = read_rows(&path, "submissions");
    let record = submissions.iter().find(|row| row["id"] == json!(submission)).unwrap();
    assert_eq!(record["status"], json!("done"), "submission 必须推进到 done：{record}");
}

/// 假 provider 的回复文本固定；模型 id 由 FauxProvider 自动写进 assistant entry 的
/// `model[0].model`，用它断言「这一轮用的是谁」。
fn echo_model_provider() -> Arc<dyn nous_ai::Provider> {
    nous_ai::providers::faux::FauxProvider::new({
        let script = nous_ai::providers::faux::FauxScript::new();
        script.append_responses(vec![nous_ai::providers::faux::FauxStep::text("pong")]);
        script
    })
}

/// 建一套没有模型来源的运行时（真 provider），返回 app。
fn app_for_model_resolution(
    path: &PathBuf,
    catalog: Option<nous_device::ModelsResponse>,
) -> Router {
    let session = Arc::new(Session::open(path).unwrap());
    let mut assembled = nous_server::runtime::assemble_with_provider(
        echo_model_provider(),
        Arc::new(BTreeMap::new()),
        session,
    );
    assembled.provider_kind = "real";
    let state = AppState::durable(path).unwrap().with_runtime(Arc::new(assembled));
    let state = match catalog {
        Some(catalog) => state.with_models(catalog),
        None => state,
    };
    router(state)
}

fn catalog_with_default(provider: &str, id: &str) -> nous_device::ModelsResponse {
    nous_device::ModelsResponse {
        default: Some(nous_device::ModelRef {
            provider: provider.into(),
            model_id: id.into(),
        }),
        items: vec![nous_device::ModelItem {
            provider: provider.into(),
            model_id: id.into(),
            name: id.into(),
            thinking_levels: vec![],
            context_window: None,
            max_tokens: None,
        }],
        catalog: nous_device::CatalogDiagnostics {
            state: "ok",
            source: Some("file"),
            path: PathBuf::from("/tmp/models.json"),
        },
    }
}

async fn answer_model(app: &Router, path: &PathBuf, body: Value) -> String {
    let (status, response) = call(app, "POST", "/v1/sessions/1/messages", Some(body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "实测 {response}");
    let entries = wait_for(path, "entries", |entry| entry["kind"] == json!("pi.assistant")).await;
    let assistant = entries.iter().find(|entry| entry["kind"] == json!("pi.assistant")).unwrap();
    assistant["model"][0]["model"].as_str().expect("assistant 带 model id").to_string()
}

fn app_with_provider(path: &PathBuf, provider: Arc<dyn nous_ai::Provider>, default_id: &str) -> Router {
    let session = Arc::new(Session::open(path).unwrap());
    let mut assembled = nous_server::runtime::assemble_with_provider(
        provider,
        Arc::new(BTreeMap::new()),
        session,
    );
    assembled.provider_kind = "real";
    let state = AppState::durable(path)
        .unwrap()
        .with_runtime(Arc::new(assembled))
        .with_models(catalog_with_default("catalog", default_id));
    router(state)
}

#[tokio::test]
async fn a_model_in_the_request_body_wins_over_session_and_catalog() {
    let path = temp_db("model-body");
    seed(&path, Some("session/session-model")).await;
    let app = app_for_model_resolution(&path, Some(catalog_with_default("catalog", "default-model")));
    let used = answer_model(
        &app,
        &path,
        json!({"content": "ping", "model": {"provider": "body", "modelId": "body-model"}}),
    )
    .await;
    assert_eq!(used, "body-model");
}

#[tokio::test]
async fn a_session_model_is_used_when_the_request_has_none() {
    let path = temp_db("model-session");
    seed(&path, Some("session/session-model")).await;
    let app = app_for_model_resolution(&path, Some(catalog_with_default("catalog", "default-model")));
    let used = answer_model(&app, &path, json!({"content": "ping"})).await;
    assert_eq!(used, "session-model");
}

#[tokio::test]
async fn the_catalog_default_is_used_when_nobody_specified_a_model() {
    let path = temp_db("model-catalog");
    seed(&path, None).await;
    let app = app_for_model_resolution(&path, Some(catalog_with_default("catalog", "default-model")));
    let used = answer_model(&app, &path, json!({"content": "ping"})).await;
    assert_eq!(used, "default-model");
}

#[tokio::test]
async fn with_no_model_anywhere_the_request_is_503_not_202() {
    let path = temp_db("model-none");
    seed(&path, None).await;
    let app = app_for_model_resolution(&path, Some(nous_device::ModelsResponse::default()));
    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping"}))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "全都没有模型必须 503：{body}");
    assert_eq!(body["error"], json!("没有可用模型"));
    assert!(read_rows(&path, "submissions").is_empty(), "503 之前不许写 submission");
    assert!(read_rows(&path, "entries").is_empty(), "503 之前不许写 user entry");
}

/// 慢 provider：每 5 秒吐一段文本，给 abort 留窗口。
fn slow_provider() -> Arc<dyn nous_ai::Provider> {
    nous_ai::providers::faux::FauxProvider::new(
        nous_ai::providers::faux::FauxScript::new()
            .with_text("a slow answer that should be interrupted")
            .with_tokens_per_second(0.2),
    )
}

#[tokio::test]
async fn abort_settles_the_submission_with_user_cancel_and_an_aborted_entry() {
    let path = temp_db("abort");
    seed(&path, Some("catalog/slow-model")).await;
    let session = Arc::new(Session::open(&path).unwrap());
    let mut assembled = nous_server::runtime::assemble_with_provider(
        slow_provider(),
        Arc::new(BTreeMap::new()),
        session,
    );
    assembled.provider_kind = "real";
    let state = AppState::durable(&path)
        .unwrap()
        .with_runtime(Arc::new(assembled))
        .with_models(catalog_with_default("catalog", "slow-model"));
    let app = router(state);

    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "实测 {body}");
    let submission = body["submissionId"].as_i64().unwrap();

    let (status, aborted) = call(&app, "POST", "/v1/sessions/1/abort", None).await;
    assert_eq!(status, StatusCode::OK, "abort 应该成功：{aborted}");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let submissions = read_rows(&path, "submissions");
        let record = submissions.iter().find(|row| row["id"] == json!(submission));
        if record.is_some_and(|row| row["status"] == json!("unanswered")) {
            let record = record.unwrap();
            assert_eq!(record["reason"], json!("user_cancel"), "abort 归因：{record}");
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "abort 后 submission 没收敛：{submissions:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let entries = read_rows(&path, "entries");
    let terminal = entries
        .iter()
        .find(|entry| entry["kind"] == json!("pi.assistant") && entry["reason"] == json!("user_cancel"))
        .expect("abort 应落一条 user_cancel 的 assistant entry");
    assert_eq!(terminal["model"][0]["stopReason"], json!("aborted"));
}

#[tokio::test]
async fn a_real_turn_emits_message_start_message_end_and_entry_appended_on_sse() {
    let path = temp_db("sse-frames");
    seed(&path, Some("catalog/sse-model")).await;
    // 生产里 driver 与 commands 共用同一个 Session（唯一写者 + 同一个 ObserverRegistry）。
    let registry = ObserverRegistry::new();
    let session = Arc::new(Session::with_observers(
        nous_durable::Storage::open(&path).unwrap(),
        registry.clone(),
    ));
    let commands = DurableCommands::with_session(session.clone(), registry.clone());
    let mut assembled = nous_server::runtime::assemble_with_provider(
        echo_model_provider(),
        Arc::new(BTreeMap::new()),
        session,
    );
    assembled.provider_kind = "real";
    let state = AppState::durable_with_commands(
        nous_server::ImportedSource::open(&path).unwrap(),
        commands.clone(),
    );
    commands.attach(SessionObserver::new(state.clone()));
    let state = state
        .with_runtime(Arc::new(assembled))
        .with_models(catalog_with_default("catalog", "sse-model"));
    let app = router(state);

    // 先订阅再发消息：snapshot 必须先到，之后的三帧都在同一条连接上。
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();

    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("snapshot 2 秒内到")
        .expect("流不该先关")
        .expect("块可读");
    let text = String::from_utf8(first.to_vec()).unwrap();
    assert!(text.contains("\"snapshot\""), "第一帧必须是 snapshot：{text}");

    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "实测 {body}");

    let mut types = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while types.iter().filter(|t| *t == "message_end").count() < 1 && tokio::time::Instant::now() < deadline {
        let chunk = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .expect("帧 3 秒内到")
            .expect("流不该先关")
            .expect("块可读");
        for line in String::from_utf8(chunk.to_vec()).unwrap().lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(frame) = serde_json::from_str::<Value>(data) {
                    if let Some(kind) = frame["type"].as_str() {
                        types.push(kind.to_string());
                    }
                }
            }
        }
    }
    for expected in ["message_start", "message_end", "entry_appended"] {
        assert!(types.iter().any(|seen| seen == expected), "缺 {expected}：{types:?}");
    }
}

/// waitMs=0 与负数的判据：0 立即 202；负数/非数字 400。
#[tokio::test]
async fn wait_ms_zero_accepts_immediately_and_invalid_wait_ms_is_400() {
    let path = temp_db("wait-zero");
    seed(&path, Some("catalog/wait-model")).await;
    let app = app_for_model_resolution(&path, Some(catalog_with_default("catalog", "wait-model")));
    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping", "waitMs": 0}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "waitMs=0 必须 202：{body}");
    assert_eq!(body["status"], json!("queued"));

    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping", "waitMs": -1}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "负数 waitMs 必须 400：{body}");
    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping", "waitMs": "soon"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非数字 waitMs 必须 400：{body}");
}

#[tokio::test]
async fn wait_ms_waits_for_the_answer_and_times_out_when_it_is_slow() {
    let path = temp_db("wait-timeout");
    seed(&path, Some("catalog/wait-model")).await;
    let (status, body) = call(&app_with_provider(&path, slow_provider(), "wait-model"), "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping", "waitMs": 50}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "慢轮次超时必须 202：{body}");
    assert_eq!(body["status"], json!("running"), "超时回 running：{body}");

    let path2 = temp_db("wait-answer");
    seed(&path2, Some("catalog/wait-model")).await;
    let (status, body) = call(&app_with_provider(&path2, echo_model_provider(), "wait-model"), "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping", "waitMs": 5000}))).await;
    assert_eq!(status, StatusCode::OK, "轮次完成后必须 200 + 答案：{body}");
    assert_eq!(body["kind"], json!("pi.assistant"), "200 体带答案 entry：{body}");
    assert_eq!(body["model"][0]["content"][0]["text"], json!("pong"));
}

#[tokio::test]
async fn when_busy_interrupt_stops_the_old_turn_before_accepting_a_new_one() {
    let path = temp_db("busy-interrupt");
    seed(&path, Some("catalog/slow-model")).await;
    let app = app_with_provider(&path, slow_provider(), "slow-model");

    let (status, first) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "first"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "实测 {first}");
    let first_submission = first["submissionId"].as_i64().unwrap();

    // 旧轮次还在跑：interrupt 必须先停它。
    let (status, second) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "second", "whenBusy": "interrupt"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "实测 {second}");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let submissions = read_rows(&path, "submissions");
        if let Some(old) = submissions.iter().find(|row| row["id"] == json!(first_submission)) {
            if old["status"] == json!("unanswered") {
                assert_eq!(old["reason"], json!("user_cancel"));
                break;
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "旧轮次没被中断：{submissions:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_unknown_when_busy_value_is_400() {
    let path = temp_db("busy-invalid");
    seed(&path, Some("catalog/model")).await;
    let app = app_for_model_resolution(&path, Some(catalog_with_default("catalog", "model")));
    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping", "whenBusy": "explode"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法 whenBusy 必须 400：{body}");
}

/// 空脚本 provider（= unconfigured/offline 的真实行为）：不许静默受理后停在 queued。
#[tokio::test]
async fn a_provider_with_no_answer_leaves_the_submission_unanswered_not_queued() {
    let path = temp_db("empty-provider");
    seed(&path, Some("catalog/empty-model")).await;
    let provider = nous_ai::providers::faux::FauxProvider::new(nous_ai::providers::faux::FauxScript::new());
    let app = app_with_provider(&path, provider, "empty-model");

    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping"}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "实测 {body}");
    let submission = body["submissionId"].as_i64().unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let rows = read_rows(&path, "submissions");
        if let Some(record) = rows.iter().find(|row| row["id"] == json!(submission)) {
            assert_ne!(record["status"], json!("queued"), "受理后不许停在 queued：{record}");
            if record["status"] == json!("unanswered") {
                assert!(record["reason"].is_string(), "unanswered 必须有 reason：{record}");
                break;
            }
        }
        assert!(tokio::time::Instant::now() < deadline, "空 provider 没收敛 submission：{rows:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
