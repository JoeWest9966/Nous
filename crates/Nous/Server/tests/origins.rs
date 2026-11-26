//! #74：会话血缘反查（`GET /v1/origins`）落库判据。
//!
//! 接缝：真 Router + 真 sqlite + 真触发器点火。重启用「关掉所有句柄，重新 AppState::durable」
//! 模拟，不让内存状态替磁盘作答。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::actor::Actor;
use nous_durable::session::{ObserverRegistry, Session};
use nous_durable::Storage;
use nous_triggers::{TemplateLookup, TriggerRuntime};
use nous_server::{router, AppState, DurableCommands, ImportedSource};
use nous_store::{NewTriggerDefinition as NewTrigger, Store as TriggerStore};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "nous-74-origins-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::create_dir_all(&dir);
    (dir.join("durable.db"), dir.join("triggers.db"))
}

fn definition(code: &'static str, template: &'static str) -> NewTrigger<'static> {
    NewTrigger {
        code: Box::leak(code.to_string().into_boxed_str()),
        name: "夜报",
        spec: Box::leak(json!({"spec_version": 1, "schedule": null, "on": null}).to_string().into_boxed_str()),
        template_ref: Box::leak(format!(r#"{{"kind":"preset","ref":"{template}"}}"#).into_boxed_str()),
        fires_allowed_by: Box::leak(r#"[{"kind":"system","id":"local"}]"#.to_string().into_boxed_str()),
        target: Box::leak(r#"{"kind":"new"}"#.to_string().into_boxed_str()),
        policy: Box::leak("serial".to_string().into_boxed_str()),
    }
}

async fn call(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app.clone().oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn fired_sessions_have_a_persisted_origin_and_manual_sessions_have_none() {
    let (durable_db, trigger_db) = temp("fire");
    let registry = ObserverRegistry::new();
    let durable = Arc::new(Session::with_observers(Storage::open(&durable_db).unwrap(), registry.clone()));
    let commands = DurableCommands::with_session(durable.clone(), registry.clone());
    let state = AppState::durable_with_commands(ImportedSource::open(&durable_db).unwrap(), commands);
    let app = router(state.clone());

    let trigger = {
        let store = TriggerStore::open(&trigger_db).unwrap();
        nous_store::Trigger::new(definition("nightly_digest", "ultimate-a"), 1_000, Some(&store)).unwrap()
    };
    let store = TriggerStore::open(&trigger_db).unwrap();
    store.update_definition(trigger.code(), &trigger.definition_json()).unwrap();
    let lookup: TemplateLookup = Arc::new(|name: &str| Some(format!("prompt for {name}")));
    let runtime = TriggerRuntime::new(store, durable.clone(), lookup, None, None, durable_db.clone()).unwrap();
    runtime.reload_definitions().await;
    let fired = runtime.fire("nightly_digest", &Actor::system("local"), 7_000).await.unwrap();
    let nous_triggers::FireOutcome::Fired { session, .. } = fired else { panic!("点火成功：{fired:?}") };

    // 手动建的会话没有 origins 条目（forced:true 只在手动 launch 时写，本票不写）。
    let manual = state.commands().create(serde_json::from_value(json!({"name": "manual"})).unwrap()).await.unwrap().id;

    let (status, body) = call(&app, "/v1/origins?trigger=nightly_digest").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items 是数组");
    let origin = items.iter().find(|item| item["id"] == json!(session)).unwrap_or_else(|| panic!("点火会话的 origin 缺失：{body}"));
    assert_eq!(origin["trigger"], json!("nightly_digest"));
    assert_eq!(origin["ultimate"], json!("ultimate-a"));
    assert_eq!(origin["at"], json!(7_000));
    assert_eq!(origin["forced"], json!(false));
    assert!(!items.iter().any(|item| item["id"] == json!(manual)), "手动会话不该有 origins 条目");

    let (_, all) = call(&app, "/v1/origins").await;
    assert_eq!(all["items"].as_array().unwrap().len(), 1);
    let (_, other) = call(&app, "/v1/origins?trigger=not_a_trigger").await;
    assert!(other["items"].as_array().unwrap().is_empty());

    // 负向哨兵：全部句柄丢掉再打开，只能从磁盘拿回血缘。
    drop(runtime);
    drop(durable);
    drop(state);
    drop(app);
    let reopened = AppState::durable(&durable_db).unwrap();
    let app = router(reopened);
    let (status, body) = call(&app, "/v1/origins?trigger=nightly_digest").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("重启后 items 是数组");
    assert!(items.iter().any(|item| item["id"] == json!(session) && item["forced"] == json!(false)),
        "重启后必须能从库里查到血缘：{body}");

    let _ = std::fs::remove_dir_all(durable_db.parent().unwrap());
}
