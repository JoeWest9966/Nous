//! #83 判据：选中的技能正文进 `pi.system` 的 `sections.prompts`。
//!
//! 接缝：真 Router（HTTP 建会话 / 起轮 / 读 entries）+ 临时私库里的 SKILL.md。
//! 判据打的是字节级事实：两条技能各自独一无二的正文逐字出现在回放段里。

use std::collections::BTreeMap;
use std::fs;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::session::Session;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const BODY_A: &str = "SKILL-ALPHA-SENTINEL-3f91c";
const BODY_B: &str = "SKILL-BETA-SENTINEL-82d4e";

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("nous-skill-inject-{tag}-{}-{:?}.sqlite", std::process::id(), std::thread::current().id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(value) => Body::from(serde_json::to_vec(&value).unwrap()),
            None => Body::empty(),
        }).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// 两条技能 + 一份 faux 装配的真 Router。技能的正文是独立字符串，重复时用名字数不出来。
async fn fixture(tag: &str, skills: &[&str]) -> (axum::Router, std::path::PathBuf, i64) {
    let db = temp_db(tag);
    let root = std::env::temp_dir().join(format!("nous-skill-inject-root-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = fs::remove_dir_all(&root);
    for (dir, body) in [("alpha", BODY_A), ("beta", BODY_B)] {
        fs::create_dir_all(root.join(dir)).unwrap();
        fs::write(root.join(dir).join("SKILL.md"), format!("---\nname: {dir}\ndescription: {dir}\n---\n\n{body}\n")).unwrap();
    }
    std::env::set_var("NOUS_SKILLS", &root);
    // 不挂 template：本票只验技能；#72 的模板注入已另有判据。
    std::env::set_var("NOUS_PROMPTS", root.join("no-prompts"));

    let session = Arc::new(Session::open(&db).unwrap());
    let provider = nous_ai::providers::faux::FauxProvider::new(
        nous_ai::providers::faux::FauxScript::new().with_text("ok"),
    );
    let tools: Arc<BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>>> = Arc::new(BTreeMap::new());
    let runtime = nous_server::runtime::assemble_with_provider(provider, tools, session.clone());
    let state = AppState::durable_with_commands(
        nous_server::ImportedSource::open(&db).unwrap(),
        nous_server::DurableCommands::with_session(session, nous_durable::session::ObserverRegistry::new()),
    ).with_runtime(Arc::new(runtime));
    let app = router(state);
    let (status, created) = call(&app, "POST", "/v1/sessions", Some(json!({"name": "", "skills": skills, "template": null}))).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (app, root, created["id"].as_i64().unwrap())
}

/// 起一轮，等到 assistant entry 落库后读回放段（最后一条生效的 system 的 sections.prompts）。
async fn turn_prompts(app: &axum::Router, id: i64, request_id: &str) -> String {
    let (status, submitted) = call(app, "POST", &format!("/v1/sessions/{id}/messages"), Some(json!({"content": "hi", "requestId": request_id}))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{submitted}");
    for _ in 0..100 {
        let (status, body) = call(app, "GET", &format!("/v1/sessions/{id}/entries?system=1"), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if body["items"].as_array().is_some_and(|items| items.iter().any(|item| item["kind"] == json!("pi.assistant"))) {
            // #72 同款接缝：pi.system entry 的 model[0].sections.prompts。
            // 最后一条 system entry 就是本轮生效的那份（顺序稳定、覆盖式）。
            return body["items"].as_array().unwrap().iter()
                .rev()
                .find(|item| item["kind"] == json!("pi.system"))
                .and_then(|item| item["model"][0]["sections"]["prompts"].as_str())
                .unwrap_or_default()
                .to_string();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("轮次没落库");
}

#[tokio::test]
async fn selected_skill_bodies_are_replayed_in_the_system_prompt() {
    let (app, _root, id) = fixture("two", &["alpha", "beta"]).await;
    let prompts = turn_prompts(&app, id, "r1").await;
    assert!(prompts.contains(BODY_A), "判据 1a：系统提示词要含技能 alpha 独有正文，实测：{prompts}");
    assert!(prompts.contains(BODY_B), "判据 1b：系统提示词要含技能 beta 独有正文，实测：{prompts}");
}

#[tokio::test]
async fn the_same_selection_replays_byte_identical_sections_across_turns() {
    // 判据 2 + 哨兵：选择顺序故意打乱（beta 在前），按目录遍历会得到相反顺序。
    let (app, _root, id) = fixture("order", &["beta", "alpha"]).await;
    let first = turn_prompts(&app, id, "r2a").await;
    let second = turn_prompts(&app, id, "r2b").await;
    assert_eq!(first, second, "判据 2a：同一选择两轮逐字相同");
    let beta = first.find(BODY_B).expect("beta 正文应存在");
    let alpha = first.find(BODY_A).expect("alpha 正文应存在");
    assert!(beta < alpha, "判据 2b：顺序 = 选中顺序 beta → alpha，实测 {first}");
}

#[tokio::test]
async fn a_missing_skill_does_not_fail_the_turn_and_records_a_diagnostic() {
    let (app, root, id) = fixture("missing", &["alpha", "ghost"]).await;
    let prompts = turn_prompts(&app, id, "r3").await;
    assert!(prompts.contains(BODY_A), "还在的技能正文仍要注入：{prompts}");
    assert!(prompts.contains("skill: missing (ghost)"), "判据 3：缺技能要有诊断，实测：{prompts}");
    let _ = root;
}

#[tokio::test]
async fn no_selected_skills_leaves_no_skill_block_in_the_system_prompt() {
    let (app, _root, id) = fixture("none", &[]).await;
    let prompts = turn_prompts(&app, id, "r4").await;
    assert!(!prompts.contains(BODY_A) && !prompts.contains(BODY_B), "判据 4：未选技能不该残留技能块：{prompts}");
    assert!(!prompts.contains("skill:"), "判据 4：未选技能也不该有技能诊断：{prompts}");
}
