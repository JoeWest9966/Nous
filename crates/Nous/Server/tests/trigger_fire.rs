//! #57 的判据 2/3/4/5/6/8：真 `Store` + 真 `Session` + faux provider + 真 Router。
//!
//! 接缝全部是公开面：HTTP 路由、`TriggerRuntime::fire`、store 的 fires/entries 行。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::actor::{Actor, ActorKind};
use nous_durable::session::{ObserverRegistry, Session};
use nous_durable::{EntryQuery, Storage};
use trigger_core::{FireOutcome, TemplateLookup, TriggerRuntime};
use nous_server::{router, AppState, DurableCommands, ImportedSource};
use nous_store::{FireState, NewTriggerDefinition as NewTrigger, Store as TriggerStore};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "nous-57-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (dir.join("durable.db"), dir.join("triggers.db"))
}

fn spec(cron: Option<&str>) -> &'static str {
    Box::leak(serde_json::json!({ "spec_version": 1, "schedule": cron, "on": null }).to_string().into_boxed_str())
}

fn definition(code: &str, template: &str, allow: &str, target: &str, policy: &str, cron: Option<&str>) -> NewTrigger<'static> {
    NewTrigger {
        code: Box::leak(code.to_string().into_boxed_str()),
        name: "夜报",
        spec: spec(cron),
        template_ref: Box::leak(format!(r#"{{"kind":"preset","ref":"{template}"}}"#).into_boxed_str()),
        fires_allowed_by: Box::leak(allow.to_string().into_boxed_str()),
        target: Box::leak(target.to_string().into_boxed_str()),
        policy: Box::leak(policy.to_string().into_boxed_str()),
    }
}

const PRESET_ALLOW: &str = r#"[{"kind":"system","id":"cron"},{"kind":"system","id":"local"}]"#;
const CRON_ONLY: &str = r#"[{"kind":"system","id":"cron"}]"#;

struct Fixture {
    runtime: TriggerRuntime,
    durable: Arc<Session>,
    state: Arc<AppState>,
    trigger_db: std::path::PathBuf,
    durable_db: std::path::PathBuf,
}

fn fixture(tag: &str, templates: &[(&str, &str)]) -> Fixture {
    let (durable_db, trigger_db) = temp(tag);
    let registry = ObserverRegistry::new();
    let durable = Arc::new(Session::with_observers(Storage::open(&durable_db).unwrap(), registry.clone()));
    let commands = DurableCommands::with_session(durable.clone(), registry.clone());
    let state = AppState::durable_with_commands(ImportedSource::open(&durable_db).unwrap(), commands);
    std::env::set_var("NOUS_DB", &durable_db);
    let store = TriggerStore::open(&trigger_db).unwrap();
    let templates: Vec<(String, String)> = templates.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let lookup: TemplateLookup = Arc::new(move |name: &str| {
        templates.iter().find(|(key, _)| key == name).map(|(_, body)| body.clone())
    });
    // faux driver：判据 5 要 user + assistant 两条 entry（#51 的确定性轮次）。
    let provider = nous_ai::providers::faux::FauxProvider::new({
        let script = nous_ai::providers::faux::FauxScript::new();
        script.append_responses(vec![nous_ai::providers::faux::FauxStep::factory(|_| {
            nous_ai::providers::faux::faux_assistant_message("hello from faux", nous_ai::StopReason::Stop)
        })]);
        script
    });
    let tools: std::collections::BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>> = Default::default();
    let assembled = nous_server::runtime::assemble_with_provider(provider, Arc::new(tools), durable.clone());
    let provider_for_actor = {
        let provider = nous_ai::providers::faux::FauxProvider::new({
            let script = nous_ai::providers::faux::FauxScript::new();
            script.append_responses(vec![nous_ai::providers::faux::FauxStep::factory(|_| {
                nous_ai::providers::faux::faux_assistant_message("hello from faux", nous_ai::StopReason::Stop)
            })]);
            script
        });
        let tools: Arc<std::collections::BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>>> = Arc::new(Default::default());
        let d = durable.clone();
        Arc::new(move |actor: &Actor| -> Arc<dyn nous_sessions::turn::TurnDriver> {
            Arc::new(nous_sessions::turn::HarnessTurnDriver::new(
                d.clone(),
                nous_sessions::turn::TurnRuntime { providers: provider.clone(), tools: tools.clone() },
            ).with_actor(actor.clone()))
        })
    };
    let runtime = TriggerRuntime::new(store, durable.clone(), lookup, Some(assembled.driver.clone()), Some(provider_for_actor), durable_db.clone()).unwrap();
    Fixture { runtime, durable, state, trigger_db, durable_db }
}

async fn create(fixture: &Fixture, input: NewTrigger<'static>) {
    let trigger = {
        let store = fixture.runtime.store.lock().await;
        let t = nous_store::Trigger::new(input, 1_000, Some(&store)).unwrap();
        t
    };
    let json = trigger.definition_json();
    let code = trigger.code().to_string();
    let store = fixture.runtime.store.lock().await;
    store.update_definition(&code, &json).unwrap();
    drop(store);
    fixture.runtime.reload_definitions().await;
}

fn entries(fixture: &Fixture, session: i64) -> Vec<Value> {
    let storage = Storage::open_read_only(&fixture.durable_db).unwrap();
    storage
        .scan_entries(&EntryQuery { conversation_id: session, min_entry_id: None, max_entry_id: None, order: None }, 100, None)
        .unwrap()
        .unwrap()
        .items
}

fn actor(kind: ActorKind, id: &str) -> Actor {
    Actor { kind, id: id.to_string() }
}

#[tokio::test]
async fn fire_twice_at_the_same_moment_writes_one_ledger_row_and_two_entries() {
    let fixture = fixture("idem", &[("nightly", "hello {{code}} @ {{fire_at}}")]);
    create(&fixture, definition("nightly_digest", "nightly", PRESET_ALLOW, r#"{"kind":"new"}"#, "serial", None)).await;
    let first = fixture.runtime.fire("nightly_digest", &actor(ActorKind::System, "local"), 7_000).await.unwrap();
    assert!(matches!(first, FireOutcome::Fired { .. }), "{first:?}");
    let second = fixture.runtime.fire("nightly_digest", &actor(ActorKind::System, "local"), 7_000).await.unwrap();
    assert_eq!(second, FireOutcome::AlreadyFired, "同一 (code, fire_at) 第二次不是错误");

    let store = fixture.runtime.store.lock().await;
    assert_eq!(store.fires().unwrap().len(), 1, "fires 行数不增");
    assert_eq!(store.fire("nightly_digest", 7_000).unwrap().unwrap().state, FireState::Done);
    drop(store);
    let FireOutcome::Fired { session, .. } = first else { unreachable!() };
    assert_eq!(entries(&fixture, session).len(), 2, "只有一轮 user+assistant，没有第二条 entry");
    let actors: Vec<Value> = entries(&fixture, session).iter().map(|e| e["actor"].clone()).collect();
    assert!(actors.iter().all(|a| a["id"] == json!("trigger:nightly_digest")), "两条 entry 的 actor.id 都是 trigger:<code>：{actors:?}");
}

#[tokio::test]
async fn fire_denied_writes_no_ledger_row() {
    let fixture = fixture("denied", &[("nightly", "hello")]);
    create(&fixture, definition("nightly_digest", "nightly", CRON_ONLY, r#"{"kind":"new"}"#, "serial", None)).await;
    let outcome = fixture.runtime.fire("nightly_digest", &actor(ActorKind::Agent, "ext:x"), 8_000).await.unwrap();
    assert_eq!(outcome, FireOutcome::Denied { slug: "fire-denied: nightly_digest by ext:x".into() });
    assert!(fixture.runtime.store.lock().await.fires().unwrap().is_empty(), "没许可不许记账");
}

#[tokio::test]
async fn missing_template_is_orphaned_with_note_and_no_entry() {
    let fixture = fixture("no-template", &[]);
    create(&fixture, definition("nightly_digest", "missing-preset", PRESET_ALLOW, r#"{"kind":"new"}"#, "serial", None)).await;
    let outcome = fixture.runtime.fire("nightly_digest", &actor(ActorKind::System, "local"), 9_000).await.unwrap();
    assert_eq!(outcome, FireOutcome::TemplateMissingOrphaned);
    let store = fixture.runtime.store.lock().await;
    let fire = store.fire("nightly_digest", 9_000).unwrap().unwrap();
    assert_eq!(fire.state, FireState::Orphaned);
    assert_eq!(fire.note.as_deref(), Some("template-missing"));
}

#[tokio::test]
async fn http_fire_returns_200_and_writes_ledger_and_two_entries() {
    let fixture = fixture("http", &[("nightly", "hello {{code}}")]);
    create(&fixture, definition("nightly_digest", "nightly", PRESET_ALLOW, r#"{"kind":"new"}"#, "serial", None)).await;
    // 环境变量给 fire 的 durable 只读句柄指到这个临时库。
    std::env::set_var("NOUS_DB", &fixture.durable_db);
    // 定义是建完后写进 store 的，重新装载一次快照（和进程启动同构）。
    let runtime = TriggerRuntime::new(
        nous_store::Store::open(&fixture.trigger_db).unwrap(),
        fixture.durable.clone(),
        Arc::new(|_: &str| Some("hello {{code}}".to_string())),
        Some({
            let provider = nous_ai::providers::faux::FauxProvider::new({
                let script = nous_ai::providers::faux::FauxScript::new();
                script.append_responses(vec![nous_ai::providers::faux::FauxStep::factory(|_| {
                    nous_ai::providers::faux::faux_assistant_message("hello from faux", nous_ai::StopReason::Stop)
                })]);
                script
            });
            let tools: std::collections::BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>> = Default::default();
            nous_server::runtime::assemble_with_provider(provider, Arc::new(tools), fixture.durable.clone()).driver.clone()
        }),
        None,
        fixture.durable_db.clone(),
    ).unwrap();
    let state = fixture.state.clone().with_triggers(runtime);
    let app = router(state);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/triggers/nightly_digest/fire")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "POST fire 200");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["status"], json!("fired"), "{body}");
    let fires = fixture.runtime.store.lock().await.fires().unwrap();
    assert_eq!(fires.len(), 1, "fires 多一行");
    let session = body["session"].as_i64().unwrap();
    assert_eq!(entries(&fixture, session).len(), 2, "entries 多两条");
}


#[tokio::test]
async fn serial_policy_skips_when_the_target_has_a_live_task() {
    let fixture = fixture("serial", &[("nightly", "hello")]);
    // 先在 durable 里造一个会话 1 + 一条非终态 task。
    fixture
        .durable
        .commit(
            vec![
                nous_durable::StorageWrite::Conversation(json!({ "id": 1 })),
                nous_durable::StorageWrite::Task(json!({
                    "id": 10, "conversationId": 1, "kind": "pi.generation",
                    "version": 1, "input": {}, "background": false,
                    "abortRequested": false, "state": { "status": "running" },
                })),
            ],
            nous_durable::session::CommitOptions::default(),
        )
        .await
        .unwrap();
    create(
        &fixture,
        definition(
            "serial_digest",
            "nightly",
            PRESET_ALLOW,
            r#"{"kind":"conversation","id":1}"#,
            "serial",
            None,
        ),
    )
    .await;
    let before = entries(&fixture, 1).len();
    let outcome = fixture
        .runtime
        .fire("serial_digest", &actor(ActorKind::System, "local"), 11_000)
        .await
        .unwrap();
    assert_eq!(outcome, FireOutcome::Skipped, "serial 下非终态 task => skipped");
    let store = fixture.runtime.store.lock().await;
    assert_eq!(
        store.fire("serial_digest", 11_000).unwrap().unwrap().state,
        FireState::Skipped,
        "fires 记 skipped"
    );
    drop(store);
    assert_eq!(entries(&fixture, 1).len(), before, "durable entries 不增");
}
