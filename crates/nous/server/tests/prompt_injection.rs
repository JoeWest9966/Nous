//! #72 判据 4/5：会话 template 起轮时注入 pi.system；缺模板记诊断但轮次照跑。

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::session::Session;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const UNIQUE_BODY: &str = "TEMPLATE-X-UNIQUE-7f3a9c";

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("nous-inject-{tag}-{}.sqlite", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn call(app: &axum::Router, uri: &str) -> Value {
    let response = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "实测 {:?}", response.status());
    serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method("POST").uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn fixture(tag: &str, template: Option<&str>) -> (i64, axum::Router, std::path::PathBuf) {
    let db = temp_db(tag);
    let root = std::env::temp_dir().join(format!("nous-inject-prompts-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("X")).unwrap();
    fs::write(root.join("X/PROMPT.md"), UNIQUE_BODY).unwrap();
    fs::write(root.join("X/nous.json"), json!({"description":"X","category":[]}).to_string()).unwrap();
    std::env::set_var("NOUS_PROMPTS", &root);

    let session = Arc::new(Session::open(&db).unwrap());
    let config = json!({"name": "", "template": template});
    let created = session.create_conversation(config).await.unwrap();
    let id = created["id"].as_i64().unwrap();

    let provider = nous_ai::providers::faux::FauxProvider::new(
        nous_ai::providers::faux::FauxScript::new().with_text("ok"),
    );
    let tools: Arc<BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>>> = Arc::new(BTreeMap::new());
    let runtime = nous_server::runtime::assemble_with_provider(provider, tools, session.clone());
    let state = AppState::durable_with_commands(
        nous_server::ImportedSource::open(&db).unwrap(),
        nous_server::DurableCommands::with_session(session, nous_durable::session::ObserverRegistry::new()),
    ).with_runtime(Arc::new(runtime));
    (id, router(state), root.join("X/PROMPT.md"))
}

#[tokio::test]
async fn the_session_template_body_is_replayed_in_the_system_prompt() {
    let (id, app, _prompt) = fixture("with-template", Some("X")).await;
    let result = call(&app, &format!("/v1/sessions/{id}/entries?system=1")).await;
    // 起轮前只有会话创建，还没有 system entry —— 发一条消息触发注入。
    assert!(result["items"].as_array().is_some());
    let (status, submitted) = post(&app, &format!("/v1/sessions/{id}/messages"), json!({"content":"hi","requestId":"r1"})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{submitted}");
    // 等轮次落库：assistant entry 出现即注入已发生。
    for _ in 0..100 {
        let body = call(&app, &format!("/v1/sessions/{id}/entries?system=1")).await;
        if body["items"].as_array().is_some_and(|items| items.iter().any(|i| i["kind"] == json!("pi.assistant"))) {
            let systems: Vec<_> = body["items"].as_array().unwrap().iter()
                .filter(|i| i["kind"] == json!("pi.system")).collect();
            let hit = systems.iter().any(|entry| {
                entry["model"][0]["sections"]["prompts"].as_str()
                    .is_some_and(|text| text.contains(UNIQUE_BODY))
            });
            assert!(hit, "pi.system 的 sections.prompts 要含模板正文：{body}");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("轮次没落库");
}

#[tokio::test]
async fn a_missing_template_does_not_fail_the_turn_and_records_a_diagnostic() {
    let (id, app, _prompt) = fixture("missing-template", Some("ghost")).await;
    let (status, submitted) = post(&app, &format!("/v1/sessions/{id}/messages"), json!({"content":"hi","requestId":"r2"})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{submitted}");
    for _ in 0..100 {
        let body = call(&app, &format!("/v1/sessions/{id}/entries?system=1")).await;
        if body["items"].as_array().is_some_and(|items| items.iter().any(|i| i["kind"] == json!("pi.assistant"))) {
            let text = serde_json::to_string(&body).unwrap();
            assert!(text.contains("template: missing"), "缺模板要留可查诊断：{text}");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("缺模板时轮次仍应 done");
}
