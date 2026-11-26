//! #58 的判据 1/2/4/5/6：观测读口（HTTP + CLI 同一函数）、counts 从库算、
//! 认证、404。判据 3 的纯函数半边在 `nous-store`（`tests/fires.rs`），这里的
//! `scheduler.lagMs` 部分用 `record_tick` + 注入时钟钉住。
//!
//! 装配复用 `trigger_fire.rs` 的六步点火 fixture：counts / lastFire 的素材直接
//! 来自真点火，不手拼账本行。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::actor::{Actor, ActorKind};
use nous_durable::session::{ObserverRegistry, Session};
use nous_durable::Storage;
use trigger_core::{FireOutcome, TemplateLookup, TriggerRuntime};
use nous_server::{router, AppState, DurableCommands, ImportedSource};
use nous_store::{FireState, NewTriggerDefinition as NewTrigger, Store as TriggerStore};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "nous-58-{tag}-{}-{}",
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

fn definition(code: &str, template: &str) -> NewTrigger<'static> {
    NewTrigger {
        code: Box::leak(code.to_string().into_boxed_str()),
        name: "夜报",
        spec: spec(None),
        template_ref: Box::leak(format!(r#"{{"kind":"preset","ref":"{template}"}}"#).into_boxed_str()),
        fires_allowed_by: Box::leak(r#"[{"kind":"system","id":"cron"},{"kind":"system","id":"local"}]"#.to_string().into_boxed_str()),
        target: Box::leak(r#"{"kind":"new"}"#.to_string().into_boxed_str()),
        policy: Box::leak("serial".to_string().into_boxed_str()),
    }
}

struct Fixture {
    runtime: TriggerRuntime,
    state: Arc<AppState>,
    trigger_db: std::path::PathBuf,
    durable_db: std::path::PathBuf,
}

fn fixture(tag: &str) -> Fixture {
    let (durable_db, trigger_db) = temp(tag);
    let registry = ObserverRegistry::new();
    let durable = Arc::new(Session::with_observers(Storage::open(&durable_db).unwrap(), registry.clone()));
    let commands = DurableCommands::with_session(durable.clone(), registry.clone());
    let state = AppState::durable_with_commands(ImportedSource::open(&durable_db).unwrap(), commands);
    std::env::set_var("NOUS_DB", &durable_db);
    let store = TriggerStore::open(&trigger_db).unwrap();
    let templates: Vec<(String, String)> = vec![("nightly".to_string(), "hello {{code}}".to_string())];
    let lookup: TemplateLookup = Arc::new(move |name: &str| {
        templates.iter().find(|(key, _)| key == name).map(|(_, body)| body.clone())
    });
    // faux driver：点火成功的判据要 user + assistant 两条 entry（#51 的确定性轮次）。
    let provider = nous_ai::providers::faux::FauxProvider::new({
        let script = nous_ai::providers::faux::FauxScript::new();
        script.append_responses(vec![nous_ai::providers::faux::FauxStep::factory(|_| {
            nous_ai::providers::faux::faux_assistant_message("hello from faux", nous_ai::StopReason::Stop)
        })]);
        script
    });
    let tools: std::collections::BTreeMap<String, Arc<dyn nous_ai::tool::AgentTool>> = Default::default();
    let assembled = nous_server::runtime::assemble_with_provider(provider, Arc::new(tools), durable.clone());
    let runtime = TriggerRuntime::new(store, durable.clone(), lookup, Some(assembled.driver.clone()), None, durable_db.clone()).unwrap();
    Fixture { runtime, state, trigger_db, durable_db }
}

async fn create(fixture: &Fixture, input: NewTrigger<'static>) {
    let trigger = {
        let store = fixture.runtime.store.lock().await;
        let t = nous_store::Trigger::new(input, 1_000, Some(&store)).unwrap();
        let json = t.definition_json();
        let code = t.code().to_string();
        store.update_definition(&code, &json).unwrap();
        code
    };
    drop(trigger);
    fixture.runtime.reload_definitions().await;
}

fn actor() -> Actor {
    Actor { kind: ActorKind::System, id: "local".to_string() }
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().method("GET").uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

// ── 判据 1 + 2：counts 与库逐项相等；lastFire 反映账本状态 ─────────────────

#[tokio::test]
async fn counts_match_the_ledger_and_last_fire_reflects_the_state() {
    let fixture = fixture("counts");
    create(&fixture, definition("nightly_digest", "nightly")).await;

    // 一次成功点火：fires 落 done，durable 里一轮 user+assistant。
    let outcome = fixture.runtime.fire("nightly_digest", &actor(), 7_000).await.unwrap();
    assert!(matches!(outcome, FireOutcome::Fired { .. }), "{outcome:?}");

    // 一条 orphaned（#56 的 reclaim 产物；这里直接按同一套 store API 制造）。
    {
        let store = fixture.runtime.store.lock().await;
        store.record_fire("nightly_digest", 8_000, None).unwrap();
        store.mark_orphaned("nightly_digest", 8_000).unwrap();
    }

    let app = router(fixture.state.clone().with_triggers(clone_runtime(&fixture)));
    let (status, body) = get_json(&app, "/v1/triggers").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["triggers"].as_array().unwrap();
    assert_eq!(items.len(), 1, "只有 nightly_digest 一条：{items:?}");
    let item = &items[0];
    assert_eq!(item["code"], json!("nightly_digest"));

    // 判据 1：counts 与 `SELECT state, count(*) FROM fires` 逐项相等。
    let store = fixture.runtime.store.lock().await;
    let mut stmt = store
        .conn()
        .prepare("SELECT state, count(*) FROM fires WHERE trigger_code = 'nightly_digest' GROUP BY state")
        .unwrap();
    let from_db: std::collections::BTreeMap<String, i64> = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .unwrap()
        .collect::<std::result::Result<std::collections::BTreeMap<String, i64>, _>>()
        .unwrap();
    drop(stmt);
    drop(store);
    let counts = &item["counts"];
    let db = |key: &str| from_db.get(key).copied().unwrap_or(0);
    assert_eq!(counts["done"], json!(db("done")), "done 与库相等");
    assert_eq!(counts["orphaned"], json!(db("orphaned")), "orphaned 与库相等");
    assert_eq!(counts["skipped"], json!(db("skipped")), "skipped 与库相等");
    assert_eq!(counts["pending"], json!(db("pending")), "pending 与库相等");
    assert_eq!(counts["done"], json!(1));
    // 判据 2：orphaned 进了 counts（把 orphaned 从 GROUP BY 过滤掉这里就红）。
    assert_eq!(counts["orphaned"], json!(1));

    // 判据 2：lastFire 是最近一次**有结论**的点火 —— 8000 的 orphaned 比 7000 的 done 新，
    // 所以如实报 orphaned（最新一次点火失败了，读口不许替账本说“上次是 done”）。
    assert_eq!(item["lastFire"]["state"], json!("orphaned"));
    assert_eq!(item["lastFire"]["at"], json!(8_000));
    // 调度健康也在顶层（循环未落地时诚实 null，不是伪造一拍心跳）。
    assert_eq!(body["scheduler"]["lastTickAt"], Value::Null);
    assert_eq!(body["scheduler"]["tickMs"], json!(nous_store::SCHEDULER_TICK_MS));
    assert_eq!(body["scheduler"]["lagMs"], json!(0));
}

/// 同一个 store 另开一份 TriggerRuntime（HTTP 装配用；定义从库里重载）。
fn clone_runtime(fixture: &Fixture) -> TriggerRuntime {
    let store = TriggerStore::open(&fixture.trigger_db).unwrap();
    let registry = ObserverRegistry::new();
    let durable = Arc::new(Session::with_observers(
        Storage::open(&fixture.durable_db).unwrap(),
        registry,
    ));
    TriggerRuntime::new(
        store,
        durable,
        Arc::new(|_: &str| Some("hello {{code}}".to_string())),
        None,
        None,
        fixture.durable_db.clone(),
    )
    .unwrap()
}

// ── 判据 4（前半）：同一条读口两次装配必须逐字节相同 ──────────────────────

/// **这条不跑 CLI**（真进程那条在 `cli_actions::triggers_cli_prints_the_same_json_as_the_endpoint_reader`
/// 和 `verify.sh` 3d）—— 它守的是「HTTP 侧与直接调用同一个 `observation_report` 不会分叉」：
/// 比如 HTTP handler 自己又拼了一份 JSON，这里就红。别把它当成「CLI 逐字节」的判据。
#[tokio::test]
async fn http_handler_and_reader_function_serialize_the_same_bytes() {
    let fixture = fixture("cli-bytes");
    create(&fixture, definition("nightly_digest", "nightly")).await;
    fixture.runtime.fire("nightly_digest", &actor(), 7_000).await.unwrap();
    {
        let store = fixture.runtime.store.lock().await;
        store.record_fire("nightly_digest", 8_000, None).unwrap();
        store.mark_skipped("nightly_digest", 8_000).unwrap();
    }

    let runtime = clone_runtime(&fixture);
    let app = router(fixture.state.clone().with_triggers(runtime.clone()));
    let (_, http_body) = get_json(&app, "/v1/triggers").await;

    // CLI 走同一个函数：真 store（同一份库文件）+ 同一份定义快照 + 同一个心跳（无）。
    // 心跳与时钟都是 0/None：两边去掉运行时刻的漂移，剩下的字节必须完全相同 ——
    // 包括 scheduler 段（判据 4 比的是整个 body，不是只有 triggers 数组）。
    let store = TriggerStore::open(&fixture.trigger_db).unwrap();
    let definitions = runtime.definitions();
    let cli_body = trigger_core::observation_report(&store, &definitions, None, 0).unwrap();
    let mut http_fixed = http_body.clone();
    http_fixed["scheduler"]["lastTickAt"] = Value::Null;
    http_fixed["scheduler"]["lagMs"] = json!(0);
    assert_eq!(
        serde_json::to_string(&http_fixed).unwrap(),
        serde_json::to_string(&cli_body).unwrap(),
        "HTTP 与 CLI 必须逐字节相同（#58 判据 4）"
    );
}

// ── 判据 5：认证 + /v1/_routes 里能看到这两条 ───────────────────────────────

#[tokio::test]
async fn fires_endpoint_is_404_for_an_unknown_trigger() {
    let fixture = fixture("unknown");
    create(&fixture, definition("nightly_digest", "nightly")).await;
    let app = router(fixture.state.clone().with_triggers(clone_runtime(&fixture)));
    let (status, body) = get_json(&app, "/v1/triggers/no_such_trigger/fires").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不许空 200（#58 判据 6）");
    assert!(body["error"].as_str().unwrap().contains("no_such_trigger"), "{body}");
}

#[tokio::test]
async fn fires_endpoint_returns_recent_ledger_rows_desc() {
    let fixture = fixture("history");
    create(&fixture, definition("nightly_digest", "nightly")).await;
    for at in [5_000, 6_000, 7_000] {
        fixture.runtime.fire("nightly_digest", &actor(), at).await.unwrap();
    }
    let app = router(fixture.state.clone().with_triggers(clone_runtime(&fixture)));
    let (status, body) = get_json(&app, "/v1/triggers/nightly_digest/fires?limit=2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], json!("nightly_digest"));
    let fires = body["fires"].as_array().unwrap();
    assert_eq!(fires.len(), 2, "limit 生效");
    let ats: Vec<i64> = fires.iter().map(|f| f["fireAt"].as_i64().unwrap()).collect();
    assert_eq!(ats, vec![7_000, 6_000], "时间倒序");
    assert_eq!(fires[0]["state"], json!("done"));
    assert_eq!(fires[0]["requestId"], json!("trigger:nightly_digest:7000"));
}

#[tokio::test]
async fn observation_endpoints_pass_bearer_auth_and_show_up_in_routes() {
    let fixture = fixture("auth");
    create(&fixture, definition("nightly_digest", "nightly")).await;
    let state = fixture.state.clone().with_triggers(clone_runtime(&fixture));

    // _routes 来自唯一注册表：两条观测端点必须在（判据 5 的前半）。
    let app = router(state.clone());
    let (status, routes) = get_json(&app, "/v1/_routes").await;
    assert_eq!(status, StatusCode::OK);
    let paths: Vec<&str> = routes["routes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|route| route["path"].as_str())
        .collect();
    assert!(paths.contains(&"/v1/triggers"), "routes 缺 GET /v1/triggers：{paths:?}");
    // 注册表只认 `:code` 写法（`to_registry_path`），插件声明与静态表同一套。
    assert!(
        paths.contains(&"/v1/triggers/:code/fires"),
        "routes 缺 fires 端点：{paths:?}"
    );

    // 带 token：两条都 200（判据 5 的后半；401 分支由 auth_endpoints 的通用判据管）。
    let token = "observation-token-0123456789abcdef";
    let app = router(state).layer(axum::middleware::from_fn(
        move |req, next| nous_server::auth::require_bearer(token.to_string(), req, next),
    ));
    for uri in ["/v1/triggers", "/v1/triggers/nightly_digest/fires"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{uri} 带 token 应 200");
        let without = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(without.status(), StatusCode::UNAUTHORIZED, "{uri} 无 token 应 401");
    }
}

// ── review 2：HTTP 观测只读库里的 tick，内存里有什么都不算 ─────────────────

/// 真哨兵：HTTP `/v1/triggers` 的 `lastTickAt` 必须来自 durable 的 tick 行；
/// 哪怕内存快照里有值、库里没有，也必须是 null（把内存兜底加回来这条会红）。
#[tokio::test]
async fn http_last_tick_at_comes_only_from_the_durable_ledger() {
    let fixture = fixture("http-last-tick");
    let runtime = clone_runtime(&fixture);
    // 内存里故意记一拍，库里一行 tick 都没有 —— 旧实现会把它兜出来。
    runtime.record_tick(5_000);
    let app = router(fixture.state.clone().with_triggers(runtime.clone()));
    let (_, body) = get_json(&app, "/v1/triggers").await;
    assert_eq!(body["scheduler"]["lastTickAt"], Value::Null,
        "内存里的心跳不许冒充库里的 tick：{body}");
    assert_eq!(body["scheduler"]["lagMs"], json!(0));

    // 现在把一拍真写进 durable 的 sleeping tick 行，HTTP 必须报库里的 wakeAt。
    let session = nous_durable::session::Session::open(&fixture.durable_db).unwrap();
    nous_dispatcher::arm_next_tick(&session, &(Arc::new(nous_durable::harness::clock::ManualClock::new(1_760_000_000_000))
        as Arc<dyn nous_durable::harness::clock::Clock>), 1, 1_000).await.unwrap();
    let expected = runtime.load_last_tick_at().expect("库里已有 tick 行");
    let (_, body) = get_json(&app, "/v1/triggers").await;
    assert_eq!(body["scheduler"]["lastTickAt"], json!(expected),
        "HTTP 必须报库里 tick 记录的 wakeAt：{body}");
}

/// 纯函数的 lag 语义独立钉住：不经过内存快照，也不靠 HTTP 的时区/时钟漂移。
#[test]
fn lag_reports_stall_only_after_a_tick_is_recorded() {
    let store_lag = nous_store::scheduler_lag;
    assert_eq!(store_lag(20_000, None, 5_000), 0);
    assert_eq!(store_lag(20_000, Some(5_000), 5_000), 10_000);
    assert_eq!(store_lag(600_000, Some(5_000), 5_000), 590_000);
}

// 一条没落进账本的 pending 不算 lastFire（读口不许替账本撒谎）。
#[tokio::test]
async fn pending_fire_is_never_reported_as_last_fire() {
    let fixture = fixture("pending");
    create(&fixture, definition("nightly_digest", "nightly")).await;
    fixture.runtime.fire("nightly_digest", &actor(), 7_000).await.unwrap();
    {
        let store = fixture.runtime.store.lock().await;
        store.record_fire("nightly_digest", 8_000, None).unwrap(); // 留在 pending
        assert_eq!(store.fire("nightly_digest", 8_000).unwrap().unwrap().state, FireState::Pending);
    }
    let app = router(fixture.state.clone().with_triggers(clone_runtime(&fixture)));
    let (_, body) = get_json(&app, "/v1/triggers").await;
    let item = &body["triggers"].as_array().unwrap()[0];
    assert_eq!(item["lastFire"]["at"], json!(7_000), "lastFire 是 done 的 7000，不是 pending 的 8000");
    assert_eq!(item["counts"]["pending"], json!(1), "pending 照样进 counts");
}
