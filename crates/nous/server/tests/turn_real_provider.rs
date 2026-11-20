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
use nous_durable::session::{CommitOptions, Session};
use nous_durable::StorageWrite;
use nous_server::runtime::AssembledRuntime;
use nous_server::{router, AppState};
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
    let mut conversation = json!({ "id": 1 });
    if let Some(model) = model {
        conversation["model"] = json!(model);
    }
    session
        .commit(vec![StorageWrite::Conversation(conversation)], CommitOptions::default())
        .await
        .unwrap();
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
    let state = state.with_runtime(runtime(provider, session));
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
    model: Option<&str>,
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

#[tokio::test]
async fn a_model_in_the_request_body_wins_over_session_and_catalog() {
    let path = temp_db("model-body");
    seed(&path, Some("session/session-model")).await;
    let app = app_for_model_resolution(&path, None, Some(catalog_with_default("catalog", "default-model")));
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
    let app = app_for_model_resolution(&path, None, Some(catalog_with_default("catalog", "default-model")));
    let used = answer_model(&app, &path, json!({"content": "ping"})).await;
    assert_eq!(used, "session-model");
}

#[tokio::test]
async fn the_catalog_default_is_used_when_nobody_specified_a_model() {
    let path = temp_db("model-catalog");
    seed(&path, None).await;
    let app = app_for_model_resolution(&path, None, Some(catalog_with_default("catalog", "default-model")));
    let used = answer_model(&app, &path, json!({"content": "ping"})).await;
    assert_eq!(used, "default-model");
}

#[tokio::test]
async fn with_no_model_anywhere_the_request_is_503_not_202() {
    let path = temp_db("model-none");
    seed(&path, None).await;
    let app = app_for_model_resolution(&path, None, Some(nous_device::ModelsResponse::default()));
    let (status, body) = call(&app, "POST", "/v1/sessions/1/messages", Some(json!({"content": "ping"}))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "全都没有模型必须 503：{body}");
    assert_eq!(body["error"], json!("没有可用模型"));
    assert!(read_rows(&path, "submissions").is_empty(), "503 之前不许写 submission");
    assert!(read_rows(&path, "entries").is_empty(), "503 之前不许写 user entry");
}
