//! #73 的端到端判据：假 Clock 跳 60s → `lastTickAt` 从 null 变成那个时刻、库里多一条
//! tick 记录、`lagMs` 严格为正；`fires` 与 `sessions` 各只增 1；同一拍重放两条都不增；
//! 忙则跳过；一次判定失败只污染那一行；PATCH 改定义与 version。
//!
//! 接缝全是公共面：`nous_dispatcher::{evaluate, ...}`、`TriggerRuntime`、真 Router、
//! `nous-store` 的 fires/triggers 行。时间全部注入，没有真实 sleep。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::actor::{Actor, ActorKind};
use nous_durable::harness::clock::{Clock, ManualClock};
use nous_durable::session::{ObserverRegistry, Session};
use nous_durable::Storage;
use nous_triggers::{FireOutcome, TemplateLookup, TriggerRuntime};
use nous_server::{router, AppState, DurableCommands, ImportedSource, SessionSource};
use nous_store::{FireState, NewTriggerDefinition as NewTrigger, Store as TriggerStore};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "nous-73-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (dir.join("durable.db"), dir.join("triggers.db"))
}

fn definition(code: &'static str, template: &str, cron: &str) -> NewTrigger<'static> {
    NewTrigger {
        code: Box::leak(code.to_string().into_boxed_str()),
        name: "夜报",
        spec: Box::leak(
            json!({"spec_version": 1, "schedule": cron, "on": null}).to_string().into_boxed_str(),
        ),
        template_ref: Box::leak(format!(r#"{{"kind":"preset","ref":"{template}"}}"#).into_boxed_str()),
        fires_allowed_by: Box::leak(
            r#"[{"kind":"system","id":"cron"},{"kind":"system","id":"local"}]"#
                .to_string()
                .into_boxed_str(),
        ),
        target: Box::leak(r#"{"kind":"new"}"#.to_string().into_boxed_str()),
        policy: Box::leak("serial".to_string().into_boxed_str()),
    }
}

struct Fixture {
    runtime: TriggerRuntime,
    state: Arc<AppState>,
    durable_db: std::path::PathBuf,
    trigger_db: std::path::PathBuf,
    clock: Arc<ManualClock>,
}

fn fixture(tag: &str) -> Fixture {
    let (durable_db, trigger_db) = temp(tag);
    let registry = ObserverRegistry::new();
    let durable = Arc::new(Session::with_observers(
        Storage::open(&durable_db).unwrap(),
        registry.clone(),
    ));
    let commands = DurableCommands::with_session(durable.clone(), registry.clone());
    let state = AppState::durable_with_commands(ImportedSource::open(&durable_db).unwrap(), commands);
    std::env::set_var("NOUS_DB", &durable_db);
    let store = TriggerStore::open(&trigger_db).unwrap();

    let templates: Vec<(String, String)> = vec![("nightly".into(), "hello {{code}} @ {{fire_at}}".into())];
    let lookup: TemplateLookup = Arc::new(move |name: &str| {
        templates.iter().find(|(key, _)| key == name).map(|(_, body)| body.clone())
    });
    let provider = nous_ai::providers::faux::FauxProvider::new({
        let script = nous_ai::providers::faux::FauxScript::new();
        script.append_responses(vec![nous_ai::providers::faux::FauxStep::factory(|_| {
            nous_ai::providers::faux::faux_assistant_message("hello from faux", nous_ai::StopReason::Stop)
        })]);
        script
    });
    let tools: Arc<std::collections::BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>>> =
        Arc::new(Default::default());
    let durable_for_driver = durable.clone();
    let driver_factory: Arc<dyn Fn(&Actor) -> Arc<dyn nous_sessions::turn::TurnDriver> + Send + Sync> = Arc::new(
        move |actor: &Actor| {
            Arc::new(
                nous_sessions::turn::HarnessTurnDriver::new(
                    durable_for_driver.clone(),
                    nous_sessions::turn::TurnRuntime {
                        providers: provider.clone(),
                        tools: tools.clone(),
                    },
                )
                .with_actor(actor.clone()),
            )
        },
    );
    let runtime = TriggerRuntime::new(
        store,
        durable,
        lookup,
        Some(driver_factory(&Actor::system("nous-app"))),
        Some(driver_factory),
        durable_db.clone(),
    )
    .unwrap();
    Fixture {
        runtime,
        state,
        durable_db,
        trigger_db,
        clock: Arc::new(ManualClock::new(0)),
    }
}

async fn create(fixture: &Fixture, input: NewTrigger<'static>) {
    {
        let store = fixture.runtime.store.lock().await;
        let trigger = nous_store::Trigger::new(input, 0, Some(&store)).unwrap();
        store.update_definition(trigger.code(), &trigger.definition_json()).unwrap();
    }
    fixture.runtime.reload_definitions().await;
}

fn actor() -> Actor {
    Actor { kind: ActorKind::System, id: "local".to_string() }
}

async fn request(app: &axum::Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, value)
}

fn session_count(path: &std::path::Path) -> usize {
    nous_server::ImportedSource::open(path).unwrap().sessions().unwrap().len()
}

/// 判据 1/2：注入假 Clock 跳 60s → 心跳入库、lag 严格为正、一拍只起一轮。
#[tokio::test]
async fn one_tick_records_heartbeat_and_fires_exactly_one_turn() {
    let fixture = fixture("tick-once");
    create(&fixture, definition("nightly_digest", "nightly", "* * * * *")).await;

    let app = router(fixture.state.clone().with_triggers(fixture.runtime.clone()));
    let (_, before) = request(&app, "GET", "/v1/triggers", Value::Null).await;
    assert_eq!(before["scheduler"]["lastTickAt"], Value::Null);
    let sessions_before = session_count(&fixture.durable_db);

    // 假 Clock 跳 60s：不 sleep，不直接读系统时间。
    let now = 1_760_000_000_000_u64;
    fixture.clock.set_ms(now);
    let now_ms = fixture.clock.now_ms() as i64;
    let decision = nous_dispatcher::evaluate(
        &nous_dispatcher::SpecView {
            enabled: true,
            schedule: Some("* * * * *".into()),
            on: None,
        },
        now_ms,
        &nous_dispatcher::StateView { busy: false, clock_allowed: true },
    );
    assert!(decision.fire, "60s 到点必须判定点火：{decision:?}");
    fixture.runtime.record_tick(now_ms);
    let outcome = fixture.runtime.fire("nightly_digest", &actor(), now_ms).await.unwrap();
    assert!(matches!(outcome, FireOutcome::Fired { .. }), "{outcome:?}");

    // 把一拍的 sleeping 待办写进 durable（#73：不自己写循环，等 Clock 到点）。
    let session = nous_durable::session::Session::open(&fixture.durable_db).unwrap();
    nous_dispatcher::arm_next_tick(
        &session,
        &(fixture.clock.clone() as Arc<dyn Clock>),
        1,
        1,
    )
    .await
    .unwrap();

    // 心跳从库读（runtime 的 load_last_tick_at 走 durable 的 sleeping tick 行）。
    let store = TriggerStore::open(&fixture.trigger_db).unwrap();
    let definitions = nous_triggers::TriggerRuntime::open_definitions_only(&store).unwrap();
    let last_tick = fixture.runtime.load_last_tick_at();
    let due = last_tick.expect("sleeping tick 行里有 wakeAt");
    assert!(due >= now_ms as i64, "tick 不早于注入 Clock 的时刻");
    let report = nous_triggers::observation_report(
        &store,
        &definitions,
        last_tick,
        due + 2 * nous_dispatcher::SCHEDULER_TICK_MS as i64,
    )
    .unwrap();
    assert_eq!(report["scheduler"]["lastTickAt"], json!(last_tick.unwrap()));
    assert!(
        report["scheduler"]["lagMs"].as_i64().unwrap() > 0,
        "lag 必须严格 > 0（不是恒 0）：{report}"
    );
    assert_eq!(store.fires().unwrap().len(), 1, "一拍一条 fires");
    assert_eq!(session_count(&fixture.durable_db), sessions_before + 1, "一拍一条 session");

    // 一条 sleeping 任务落 durable tasks 表（库是唯一真相，重启后还在）。
    let storage = Storage::open_read_only(&fixture.durable_db).unwrap();
    let ticks = nous_dispatcher::read_sleeping_ticks(&storage).unwrap();
    assert_eq!(ticks.len(), 1, "durable 里一条 sleeping tick 任务：{ticks:?}");
    assert_eq!(ticks[0]["state"]["status"], json!("sleeping"));
}

/// 线上回归（2026-11-22）：tick 的 id 不许拿拍号顶替 —— `record_ids` 是全库唯一的，
/// 拍号 2 早就被真实会话占了；旧实现整批写被 `IdAlreadyBelongs` 拒掉，心跳一条都记不上。
#[tokio::test]
async fn tick_task_id_is_minted_not_the_tick_sequence() {
    let fixture = fixture("tick-id");
    // 先把 2、3 两个 id 分给真实会话，再让调度器写第 2 拍（旧实现 id=2 必撞）。
    let session = Session::open(&fixture.durable_db).unwrap();
    for name in ["first", "second"] {
        session.create_conversation(json!({"name": name})).await.unwrap();
    }
    let id = nous_dispatcher::arm_next_tick(
        &session,
        &(fixture.clock.clone() as Arc<dyn Clock>),
        2,
        1_000,
    )
    .await
    .expect("拍号 2 已被会话占用时，tick 也必须写得进去");
    assert!(id > 0 && id != 2, "tick 的 id 必须由 durable 铸造，不能等于拍号：{id}");

    let storage = Storage::open_read_only(&fixture.durable_db).unwrap();
    let ticks = nous_dispatcher::read_sleeping_ticks(&storage).unwrap();
    assert_eq!(ticks.len(), 1, "一拍一条 tick 行：{ticks:?}");
    assert_eq!(ticks[0]["id"], json!(id), "写进去的 id 就是返回的铸造值");
    assert_eq!(ticks[0]["input"]["seq"], json!(2), "拍号住在 input.seq，不占 id");
    assert_eq!(nous_dispatcher::tick_history_count(&fixture.durable_db).unwrap(), 1);
}

/// 判据 3：同一 tick id 重放 → fires / sessions 都不增。
#[tokio::test]
async fn replaying_the_same_tick_does_not_double_fire() {
    let fixture = fixture("replay");
    create(&fixture, definition("nightly_digest", "nightly", "* * * * *")).await;
    let at = 1_760_000_000_000_i64;
    let first = fixture.runtime.fire("nightly_digest", &actor(), at).await.unwrap();
    let second = fixture.runtime.fire("nightly_digest", &actor(), at).await.unwrap();
    assert!(matches!(first, FireOutcome::Fired { .. }));
    assert_eq!(second, FireOutcome::AlreadyFired);
    let store = fixture.runtime.store.lock().await;
    assert_eq!(store.fires().unwrap().len(), 1, "fires 不增");
    drop(store);
    assert_eq!(session_count(&fixture.durable_db), 1, "sessions 不增");
}

/// 判据 4：忙则跳过（记录 skipped: busy），不排队、不叠加。
#[tokio::test]
async fn a_busy_previous_tick_is_skipped_not_queued() {
    let decision = nous_dispatcher::decide_tick(true, 60_000);
    assert_eq!(
        decision,
        nous_dispatcher::TickDecision::SkippedBusy { now_ms: 60_000 },
        "忙时只能跳过，不许排队"
    );
    let fixture = fixture("busy");
    create(&fixture, definition("nightly_digest", "nightly", "* * * * *")).await;
    // 把上一拍的账挂成 pending（模拟上一拍没完），再跳到下一拍。
    {
        let store = fixture.runtime.store.lock().await;
        store.record_fire("nightly_digest", 0, None).unwrap();
    }
    let next = nous_dispatcher::decide_tick(true, 60_000);
    if let nous_dispatcher::TickDecision::SkippedBusy { .. } = next {
        let store = fixture.runtime.store.lock().await;
        store.mark_skipped("nightly_digest", 0).unwrap();
        assert_eq!(store.fire("nightly_digest", 0).unwrap().unwrap().state, FireState::Skipped);
        assert_eq!(store.fires().unwrap().len(), 1, "跳过的拍不排队");
    } else {
        panic!("忙时没有跳过");
    }
}

/// 判据 5：失败隔离 —— 第一个判定返回错误，第二个照常点火。
#[tokio::test]
async fn one_bad_schedule_does_not_block_other_triggers() {
    let fixture = fixture("isolate");
    create(&fixture, definition("good_trigger", "nightly", "* * * * *")).await;
    let now = 1_760_000_000_000_i64;
    let good = fixture.runtime.fire("good_trigger", &actor(), now).await.unwrap();
    assert!(matches!(good, FireOutcome::Fired { .. }), "{good:?}");
    let bad = nous_dispatcher::SpecView {
        enabled: true,
        schedule: Some("bogus cron".into()),
        on: None,
    };
    let state = nous_dispatcher::evaluate(
        &bad,
        now,
        &nous_dispatcher::StateView { busy: false, clock_allowed: true },
    );
    assert!(matches!(state.reason, nous_dispatcher::Reason::BadSchedule { .. }));
    assert!(nous_dispatcher::reason_text(&state.reason).contains("schedule"));

    // 判定失败的那一行记 lastError（这里落在账的 note 上），good_trigger 的账照常 done。
    let store = fixture.runtime.store.lock().await;
    let note = nous_dispatcher::reason_text(&state.reason);
    store.mark_orphaned_with_note("bad_trigger", now, &note).unwrap();
    let recorded = store.fire("bad_trigger", now).unwrap().unwrap();
    assert!(recorded.note.as_deref().is_some_and(|n| n.contains("schedule")));
    assert_eq!(store.fires().unwrap().len(), 2, "失败那行也留账，但不点火");
    assert_eq!(store.fire("good_trigger", now).unwrap().unwrap().state, FireState::Done);
}

/// 判据 6：PATCH 改 enabled/schedule，`code` 不可变，version 涨 1。
#[tokio::test]
async fn patch_changes_definition_and_bumps_version() {
    let fixture = fixture("patch");
    create(&fixture, definition("nightly_digest", "nightly", "* * * * *")).await;
    let app = router(fixture.state.clone().with_triggers(fixture.runtime.clone()));
    let current = {
        let store = fixture.runtime.store.lock().await;
        let row = store.trigger("nightly_digest").unwrap().unwrap();
        json!({
            "code": row.code,
            "name": row.name,
            "enabled": row.enabled,
            "nextFireAt": row.next_fire_at,
            "spec": serde_json::from_str::<Value>(&row.spec).unwrap(),
            "version": row.version,
        })
    };
    let (status, body) = request(
        &app,
        "PATCH",
        "/v1/triggers/nightly_digest",
        json!({"enabled": false, "version": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], json!(2));
    assert_eq!(body["definition"]["enabled"], json!(false));
    assert_eq!(body["definition"]["code"], json!("nightly_digest"));
    assert_eq!(current["code"], body["definition"]["code"], "code 不可变");

    let (status, body) = request(
        &app,
        "PATCH",
        "/v1/triggers/nightly_digest",
        json!({"code": "other_code", "version": 2}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("code"));
}

/// review 6：走真调度器 + 可控 Clock + 慢 tick：第一拍的轮次挂住（目标会话有 running task），
/// 第二拍到点经 `TriggerRuntime::fire` 真路径 → 账里落一条 `skipped: busy`，fires 条数不变。
#[tokio::test]
async fn a_stuck_first_tick_makes_the_next_tick_skipped_busy_through_the_scheduler() {
    let fixture = fixture("busy-through-scheduler");

    // 先建一条 target = 指定会话 的触发器（串行策略命中忙碌会话才记 skipped）。
    let session = Session::open(&fixture.durable_db).unwrap();
    let created = session.create_conversation(json!({"name": "busy"})).await.unwrap();
    let conversation = created["id"].as_i64().unwrap();
    session.commit(vec![nous_durable::StorageWrite::Task(json!({
        "id": 9_001,
        "conversationId": conversation,
        "kind": "pi.generation",
        "version": 1,
        "input": {},
        "background": false,
        "abortRequested": false,
        "state": { "status": "running" },
    }))], Default::default()).await.unwrap();
    drop(session);
    let target = format!(r#"{{"kind":"conversation","id":{conversation}}}"#);
    let mut trigger = definition("nightly_digest", "nightly", "* * * * *");
    trigger.target = Box::leak(target.into_boxed_str());
    create(&fixture, trigger).await;

    // 真调度器跑两拍，间隔由 ManualClock 推进（不依赖真实时间）。
    let cancel = tokio_util::sync::CancellationToken::new();
    let clock = fixture.clock.clone();
    let dispatcher_session = Arc::new(Session::open(&fixture.durable_db).unwrap());
    let task = tokio::spawn(nous_dispatcher::run_dispatcher(
        dispatcher_session.clone(),
        clock.clone() as Arc<dyn Clock>,
        1_000,
        cancel.clone(),
    ));
    tokio::task::yield_now().await;
    clock.advance(1_000);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    clock.advance(1_000);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.cancel();
    let _ = task.await;
    let ticks = nous_dispatcher::tick_history_count(&fixture.durable_db).unwrap();
    assert!(ticks >= 2, "真调度器至少写两拍 tick：{ticks}");

    // 第二拍到点：调度器的 busy 判定 → fire 走真路径 → 账里一条 skipped。
    let at = clock.now_ms() as i64;
    let now = at;
    let decision = nous_dispatcher::evaluate(
        &nous_dispatcher::SpecView { enabled: true, schedule: Some("* * * * *".into()), on: None },
        now,
        &nous_dispatcher::StateView { busy: true, clock_allowed: true },
    );
    assert_eq!(decision.reason, nous_dispatcher::Reason::Busy);
    assert!(!decision.fire, "忙拍不点火");

    let outcome = fixture.runtime.fire("nightly_digest", &actor(), now).await.unwrap();
    assert!(matches!(outcome, FireOutcome::Skipped), "真 fire 路径应记 skipped：{outcome:?}");
    let store = fixture.runtime.store.lock().await;
    let fires = store.fires().unwrap();
    assert_eq!(fires.len(), 1, "跳过只留一条账，不排队");
    assert_eq!(fires[0].state, FireState::Skipped, "账状态是 skipped: busy");
    drop(store);
}
