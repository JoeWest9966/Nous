//! #82 combos 动作面判据：`fire`（试跑）/ `launch`（手动起一条）经真 HTTP + 真库。
//!
//! 接缝全在公共面：真 Router、真 durable sqlite、真 triggers store、`GET /v1/sessions`
//! 与 `GET /v1/origins`。判定用的时钟是注入的假时钟 —— 判据不随真实时间抖动。
//! 血缘从磁盘读回（丢掉句柄重开），不让内存状态替库作答。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_durable::harness::clock::{Clock, ManualClock};
use nous_durable::session::{ObserverRegistry, Session};
use nous_durable::Storage;
use trigger_core::{TemplateLookup, TriggerRuntime};
use nous_server::{router, AppState, DurableCommands, ImportedSource};
use nous_store::{NewTriggerDefinition as NewTrigger, Store as TriggerStore};
use serde_json::{json, Value};
use tower::ServiceExt;

fn dir(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-82-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_preset(root: &std::path::Path, section: &str, name: &str, body: &Value) {
    let dir = root.join(section).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("preset.json"), serde_json::to_vec_pretty(body).unwrap()).unwrap();
}

fn ultimate(trigger: &str) -> Value {
    json!({
        "name": "nightly",
        "summary": "每晚",
        "description": "",
        "category": ["默认"],
        "spec": {
            "v": 1, "trigger": trigger, "params": { "at": "22:00" },
            "session": "morning", "enabled": true,
        },
    })
}

fn definition(code: &'static str, template: &'static str, cron: &str) -> NewTrigger<'static> {
    NewTrigger {
        code: Box::leak(code.to_string().into_boxed_str()),
        name: "夜报",
        spec: Box::leak(json!({"spec_version": 1, "schedule": cron, "on": null}).to_string().into_boxed_str()),
        template_ref: Box::leak(format!(r#"{{"kind":"preset","ref":"{template}"}}"#).into_boxed_str()),
        fires_allowed_by: Box::leak(r#"[{"kind":"system","id":"local"}]"#.to_string().into_boxed_str()),
        target: Box::leak(r#"{"kind":"new"}"#.to_string().into_boxed_str()),
        policy: Box::leak("serial".to_string().into_boxed_str()),
    }
}

/// 注入的假时钟给判定与血缘共用；`clock_allowed` 由装配带进来（不是 evaluate 自己查库）。
struct Fixture {
    app: Router,
    clock: Arc<ManualClock>,
    durable_db: std::path::PathBuf,
    _dir: std::path::PathBuf,
}

fn fixture(tag: &str, cron: &str) -> Fixture {
    let root = dir(tag);
    let durable_db = root.join("durable.db");
    write_preset(&root, "Ultimate-Template", "nightly", &ultimate("nightly_digest"));
    write_preset(&root, "Session-Template", "morning", &json!({
        "name": "morning", "summary": "早报", "description": "", "category": ["默认"],
    }));
    let triggers_dir = root.join("triggers");
    std::fs::create_dir_all(triggers_dir.join("nightly_digest")).unwrap();

    let registry = ObserverRegistry::new();
    let durable = Arc::new(Session::with_observers(Storage::open(&durable_db).unwrap(), registry.clone()));
    let commands = DurableCommands::with_session(durable.clone(), registry.clone());
    let state = AppState::durable_with_commands(ImportedSource::open(&durable_db).unwrap(), commands)
        .with_combos_roots(root.clone(), triggers_dir);

    let store = TriggerStore::open(root.join("triggers.db")).unwrap();
    let trigger = nous_store::Trigger::new(definition("nightly_digest", "ultimate-a", cron), 1_000, Some(&store)).unwrap();
    store.update_definition(trigger.code(), &trigger.definition_json()).unwrap();
    let lookup: TemplateLookup = Arc::new(|name: &str| Some(format!("prompt for {name}")));
    let runtime = TriggerRuntime::new(store, durable, lookup, None, None, durable_db.clone()).unwrap();
    let clock = Arc::new(ManualClock::new(1_760_000_000_000));
    let state = state.with_triggers(runtime).with_combos_action_clock(clock.clone(), true);
    Fixture { app: router(state), clock, durable_db, _dir: root }
}

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app.clone().oneshot(
        Request::builder().method("POST").uri(uri).header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap(),
    ).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let response = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn session_ids(body: &Value) -> Vec<i64> {
    body["items"].as_array().unwrap().iter().map(|item| item["id"].as_i64().unwrap()).collect()
}

/// 判据 1：判定不 fire → 200 `{fired:false, reason}`，会话条数不变。
#[tokio::test]
async fn fire_respects_the_trigger_decision_and_creates_nothing_when_it_says_no() {
    // 2033-05-18 16:00 UTC 是周三；把 cron 钉在另一个小时，判定必不命中。
    let fixture = fixture("no-fire", "0 3 * * *");
    let (_, before) = get(&fixture.app, "/v1/sessions").await;
    let sessions_before = session_ids(&before);

    let (status, body) = post(&fixture.app, "/v1/combos/Ultimate-Template/nightly/fire",
        json!({"requestId": "nofire-1"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["fired"], json!(false), "不 fire：{body}");
    assert!(body["reason"].is_string(), "reason 是人话：{body}");
    assert!(body["sessionId"].is_null(), "不建会话就不该有 sessionId：{body}");
    let (_, after) = get(&fixture.app, "/v1/sessions").await;
    assert_eq!(session_ids(&after), sessions_before, "sessions 条数不变");
    let (_, origins) = get(&fixture.app, "/v1/origins").await;
    assert!(origins["items"].as_array().unwrap().is_empty(), "不 fire 不写血缘");
}

/// 判据 2 + 4：判定 fire → 起会话、血缘 forced:false；requestId 重放回同一条。
#[tokio::test]
async fn fired_combo_launches_once_with_a_non_forced_origin_and_replays_by_request_id() {
    let fixture = fixture("fire", "* * * * *");
    let at = fixture.clock.now_ms() as i64;
    let (_, before) = get(&fixture.app, "/v1/sessions").await;
    let sessions_before = session_ids(&before);

    let body = json!({"requestId": "fire-1"});
    let (status, first) = post(&fixture.app, "/v1/combos/Ultimate-Template/nightly/fire", body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["fired"], json!(true), "判定到点必须 fire：{first}");
    let session = first["sessionId"].as_i64().unwrap_or_else(|| panic!("fire 回包缺 sessionId：{first}"));
    let (_, after) = get(&fixture.app, "/v1/sessions").await;
    let ids = session_ids(&after);
    assert_eq!(ids.len(), sessions_before.len() + 1, "只多一条会话：{ids:?}");
    assert!(ids.contains(&session), "新会话在 GET /v1/sessions 里：{ids:?}");
    let (_, origins) = get(&fixture.app, "/v1/origins").await;
    let item = origins["items"].as_array().unwrap().iter()
        .find(|item| item["id"] == json!(session)).unwrap_or_else(|| panic!("血缘缺失：{origins}"));
    assert_eq!(item["ultimate"], json!("nightly"));
    assert_eq!(item["trigger"], json!("nightly_digest"));
    assert_eq!(item["forced"], json!(false));
    assert_eq!(item["at"], json!(at));

    // 重放：同一 requestId → 同一回执、会话不涨、血缘不涨。
    let (status, replay) = post(&fixture.app, "/v1/combos/Ultimate-Template/nightly/fire", body).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay, first, "幂等重放回同一份回执");
    let (_, after_replay) = get(&fixture.app, "/v1/sessions").await;
    assert_eq!(session_ids(&after_replay).len(), ids.len(), "重放会话不涨");
    let (_, origins_replay) = get(&fixture.app, "/v1/origins").await;
    assert_eq!(origins_replay["items"].as_array().unwrap().len(), 1, "重放血缘不涨");
}

/// 判据 3 + 4：`launch` 跳过判定、forced:true；重放同 sessionId。
#[tokio::test]
async fn launch_bypasses_the_trigger_and_writes_a_forced_origin() {
    let fixture = fixture("launch", "0 3 * * *"); // 不命中的 cron：launch 仍要起会话
    let (_, before) = get(&fixture.app, "/v1/sessions").await;
    let sessions_before = session_ids(&before);
    let at = fixture.clock.now_ms() as i64;

    let body = json!({"name": "nightly", "requestId": "launch-1"});
    let (status, first) = post(&fixture.app, "/v1/presets/Ultimate-Template/launch", body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let session = first["sessionId"].as_i64().unwrap_or_else(|| panic!("launch 回包缺 sessionId：{first}"));
    assert_eq!(first["forced"], json!(true));
    let (_, after) = get(&fixture.app, "/v1/sessions").await;
    assert_eq!(session_ids(&after).len(), sessions_before.len() + 1, "跳过判定照起会话");
    assert!(session_ids(&after).contains(&session));
    let (_, origins) = get(&fixture.app, "/v1/origins").await;
    let item = origins["items"].as_array().unwrap().iter()
        .find(|item| item["id"] == json!(session)).unwrap_or_else(|| panic!("血缘缺失：{origins}"));
    assert_eq!(item["ultimate"], json!("nightly"));
    assert_eq!(item["trigger"], json!("nightly_digest"));
    assert_eq!(item["forced"], json!(true), "与 fire 可区分");
    assert_eq!(item["at"], json!(at));

    let (status, replay) = post(&fixture.app, "/v1/presets/Ultimate-Template/launch", body).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay, first, "launch 重放回第一次回执");
    let (_, after_replay) = get(&fixture.app, "/v1/sessions").await;
    assert_eq!(session_ids(&after_replay).len(), sessions_before.len() + 1, "重放会话不涨");
}

/// 血缘必须真落库：丢掉所有句柄重开，仍查得到 forced 两种值。
#[tokio::test]
async fn origins_survive_dropping_every_handle() {
    let fixture = fixture("persist", "* * * * *");
    let (_, fired) = post(&fixture.app, "/v1/combos/Ultimate-Template/nightly/fire", json!({"requestId": "persist-fire"})).await;
    let fired_id = fired["sessionId"].as_i64().unwrap();
    let (_, launched) = post(&fixture.app, "/v1/presets/Ultimate-Template/launch", json!({"name":"nightly","requestId":"persist-launch"})).await;
    let launched_id = launched["sessionId"].as_i64().unwrap();
    drop(fixture.app);
    drop(fixture.clock);

    let state = AppState::durable(&fixture.durable_db).unwrap();
    let app = router(state);
    let (_, origins) = get(&app, "/v1/origins").await;
    let items = origins["items"].as_array().unwrap();
    assert!(items.iter().any(|item| item["id"] == json!(fired_id) && item["forced"] == json!(false)), "{origins}");
    assert!(items.iter().any(|item| item["id"] == json!(launched_id) && item["forced"] == json!(true)), "{origins}");
}
