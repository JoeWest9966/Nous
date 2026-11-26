//! 触发器点火与观测的**领域层**（#57 / #58 / #73 / #80）。
//!
//! 搬自 `server/src/triggers.rs`，逻辑逐行未改：许可 → 记账 → 串行策略 → 会话解析 →
//! 提交 → done 六步仍是唯一写通道，观测清单仍是唯一的读口（HTTP 与 CLI 共用同一份
//! `serde_json` 装配，不许另写一份 SQL）。
//!
//! 这一层不认识 axum，只认 [`HttpRefusal`]（语义拒绝 + 状态码）——
//! handler 在 [`crate::catalog`] / [`crate::projects`]，宿主能力在 [`crate::routes`]。

use std::sync::Arc;

use nous_durable::actor::{Actor, ActorKind};
use nous_durable::session::commands::{record_origin, submit_message, OriginRecord, SubmitRequest};
use nous_durable::session::Session;
use nous_routes::HttpRefusal;
use nous_store::{
    scheduler_lag, FireActor, FireCounts, FireState, Inserted, Store as TriggerStore,
    Trigger as TriggerDefinition, TriggerError, SCHEDULER_TICK_MS,
};
use serde_json::{json, Value};

use crate::refusal::{internal, not_found};

/// 本模块的失败都是「拒绝 + 状态码」：语义在 `nous_routes::HttpRefusal`，不另造一套枚举。
type Result<T> = std::result::Result<T, HttpRefusal>;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FireOutcome {
    /// ① 没许可：账本一行也没写。
    Denied { slug: String },
    /// ② 同刻重复点火：不是错误；账本与 durable 都没动。
    AlreadyFired,
    /// ③ 上一轮还在跑：账本记 `skipped`，没有新 entry。
    Skipped,
    /// ④–⑥ 成功：账本 `done`，durable 里两个 entry（user + assistant）。
    Fired {
        session: i64,
        submission: i64,
        /// `submit_message` 同批铸的 user entry id（assistant 从它 +2 起）。
        entry: i64,
    },
    /// 模板缺失：账本 `orphaned` + note，没有 entry。
    TemplateMissingOrphaned,
}

/// 把 durable 的 `Actor` 投到 store 的许可三值（同一语义，不另造第四种 actor）。
fn map_actor(actor: &Actor) -> FireActor {
    match actor.kind {
        ActorKind::Human => FireActor::Human(actor.id.clone()),
        ActorKind::Agent => FireActor::Agent(actor.id.clone()),
        ActorKind::System => FireActor::System(actor.id.clone()),
    }
}

/// 调用方给的模板文本源。`DocumentReader` 的只读接口不方便跨 crate 对象安全传递，
/// 这里用闭包把它包起来：函数只认「按 ref 取文本」。
pub type TemplateLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// 固定六步的点火函数。**不做调度、不读时钟**（`now_ms` 由调用方给）。
///
/// `store` 用 `Arc<Mutex<..>>` 包着：每个步骤一个同步的 `with_store` 闭包，绝不把
/// `&Store`（rusqlite 的 Connection 不是 Sync）留到 `.await` 的另一边。
pub async fn fire(
    store: &Arc<tokio::sync::Mutex<TriggerStore>>,
    durable: &Session,
    lookup: &TemplateLookup,
    definitions: &std::sync::RwLock<Vec<TriggerDefinition>>,
    durable_db: &std::path::Path,
    code: &str,
    fired_by: &Actor,
    now_ms: i64,
) -> Result<FireOutcome> {
    let trigger = {
        let definition = definitions.read().unwrap_or_else(|p| p.into_inner());
        definition.iter().find(|definition| definition.code() == code).cloned()
    };
    let Some(trigger) = trigger else {
        return Err(not_found(format!("trigger {code} not found")));
    };
    // ① 许可（纯值判断，不碰库）
    if !trigger.permits(&map_actor(fired_by)) {
        return Ok(FireOutcome::Denied {
            slug: format!("fire-denied: {code} by {}", fired_by.id),
        });
    }
    // ② 记账（唯一键去重）
    let inserted = {
        let store = store.lock().await;
        store.record_fire(code, now_ms, None).map_err(internal)?
    };
    if inserted == Inserted::AlreadyThere {
        return Ok(FireOutcome::AlreadyFired);
    }
    // ③ 串行策略：该会话有非终态 task 就 skipped。
    if trigger.policy() == nous_store::Policy::Serial {
        if let nous_store::Target::Conversation(conversation) = trigger.target() {
            if task_busy(durable_db, *conversation)? {
                let store = store.lock().await;
                store.mark_skipped(code, now_ms).map_err(internal)?;
                return Ok(FireOutcome::Skipped);
            }
        }
    }
    // ④ 目标会话
    let conversation = match trigger.target() {
        nous_store::Target::Conversation(id) => *id,
        nous_store::Target::New => {
            let value = durable
                .create_conversation(json!({ "name": format!("trigger:{code}") }))
                .await
                .map_err(internal)?;
            value.get("id").and_then(Value::as_i64).ok_or(internal("create_conversation 没回 id".to_string()))?
        }
    };
    // 模板渲染：只替换 {{code}} / {{fire_at}} 两个占位符。
    let ref_name = trigger.template_ref().ref_name().to_string();
    let Some(text) = lookup(&ref_name) else {
        let store = store.lock().await;
        store.mark_orphaned_with_note(code, now_ms, "template-missing").map_err(internal)?;
        return Ok(FireOutcome::TemplateMissingOrphaned);
    };
    let prompt = text.replace("{{code}}", code).replace("{{fire_at}}", &now_ms.to_string());
    // ⑤ 提交
    let trigger_actor = Actor::system(format!("trigger:{code}"));
    let outcome = submit_message(
        durable,
        SubmitRequest {
            conversation,
            content: Value::String(prompt),
            request_id: Some(nous_store::request_id(code, now_ms)),
        },
        &trigger_actor,
    )
    .await
    .map_err(|error| internal(format!("submit_message: {error}")))?;
    // ⑥ 血缘：与目标会话同在一个 durable 库里（`qrd3wh.sessions.origins`），不建表。
    // 模板 ref 就是「终极模板」的名字；`forced` 只给手动 launch，这里恒 false。
    record_origin(
        durable,
        OriginRecord {
            id: conversation,
            ultimate: ref_name.clone(),
            trigger: code.to_string(),
            at: now_ms,
            forced: false,
        },
    )
    .await
    .map_err(|error| internal(format!("record_origin: {error}")))?;
    // ⑦ 落账 done（又是同步片段）
    {
        let store = store.lock().await;
        store.mark_done(code, now_ms).map_err(internal)?;
    }
    Ok(FireOutcome::Fired {
        session: conversation,
        submission: outcome.submission_id,
        entry: outcome.entry_id,
    })
}

/// 该会话里有没有非终态 task（只读，走 durable 的只读句柄）。
fn task_busy(path: &std::path::Path, conversation: i64) -> Result<bool> {
    let storage = durable_storage(path)?;
    Ok(storage.tasks(conversation).map_err(internal)?.iter().any(|task| {
        task.get("state")
            .and_then(|state| state.get("status"))
            .and_then(Value::as_str)
            != Some("terminal")
    }))
}

/// 只读窥看 durable 的 task 状态：不需要第二个写者，调用点在 fire 的③里同步用完就放。
fn durable_storage(path: &std::path::Path) -> std::result::Result<nous_durable::Storage, HttpRefusal> {
    nous_durable::Storage::open_read_only(path)
        .map_err(|error| internal(format!("read durable: {error}")))
}

/// 触发器观测的**唯一读口**（#58）：HTTP 端点与 CLI `--triggers` 都走这一个函数、
/// 同一份 `serde_json` 序列化 —— 不许另写一份 SQL 或第二套 JSON 装配（#58 判据 4）。
///
/// 故意是**同步**的：HTTP 侧锁住 `Mutex<Store>` 后把 `&Store` 递进来，CLI 侧直接递
/// 自己开的那个 `Store` —— 两边拿着同一个函数、同一份序列化，字节只能相同。
/// `counts` 每次从 `fires` 表现算（`GROUP BY state`），没有内存计数器；
/// `lagMs` 由调用方注入时钟，这个函数自身零时钟。
pub fn observation_report(
    store: &TriggerStore,
    definitions: &[TriggerDefinition],
    last_tick_at: Option<i64>,
    now_ms: i64,
) -> Result<Value> {
    let mut items = Vec::with_capacity(definitions.len());
    for trigger in definitions {
        let counts = store.fire_counts(trigger.code()).map_err(internal)?;
        let last_fire = store.last_resolved_fire(trigger.code()).map_err(internal)?;
        items.push(json!({
            "code": trigger.code(),
            "name": trigger.name(),
            "enabled": trigger.enabled(),
            "category": row_category(store, trigger.code()),
            "nextFireAt": trigger.next_fire_at(),
            "lastFire": last_fire.as_ref().map(|fire| json!({
                "at": fire.fire_at,
                "state": fire.state.as_str(),
            })),
            "counts": counts_json(&counts),
        }));
    }
    Ok(json!({
        "triggers": items,
        "root": Value::Null,
        "scheduler": {
            "lastTickAt": last_tick_at,
            "lagMs": scheduler_lag(now_ms, last_tick_at, SCHEDULER_TICK_MS),
            "tickMs": SCHEDULER_TICK_MS,
        },
    }))
}

/// 一个触发器最近的账（#58 的 `GET /v1/triggers/:code/fires`），时间倒序。
/// 与 [`observation_report`] 同一条接缝：`&Store` 进，JSON 出。
pub fn fire_history(
    store: &TriggerStore,
    definitions: &[TriggerDefinition],
    code: &str,
    limit: i64,
) -> Result<Value> {
    if !definitions.iter().any(|trigger| trigger.code() == code) {
        return Err(not_found(format!("trigger {code} not found")));
    }
    let rows = store.recent_fires(code, limit).map_err(internal)?;
    Ok(json!({
        "code": code,
        "fires": rows.iter().map(|fire| fire_row_json(fire)).collect::<Vec<_>>(),
    }))
}

fn fire_row_json(fire: &nous_store::Fire) -> Value {
    json!({
        "fireAt": fire.fire_at,
        "state": fire.state.as_str(),
        "requestId": fire.request_id,
        "note": fire.note,
    })
}

/// `FireCounts` → JSON。四个键恒在：没账就是 0，读者不用猜缺键的意思。
/// **不是从内存攒的**，入参就是刚从库里 `GROUP BY` 回来的那份。
fn counts_json(counts: &FireCounts) -> Value {
    json!({
        "done": counts.done,
        "skipped": counts.skipped,
        "orphaned": counts.orphaned,
        "pending": counts.pending,
    })
}

/// #80：观测列表带 category（触发器行上的挂值，读口在这里补，不改 store 的读法）。
fn row_category(store: &nous_store::Store, code: &str) -> Value {
    store.trigger(code).ok().flatten().and_then(|row| row.category).map(Value::String).unwrap_or(Value::Null)
}

/// 校验一个触发器定义能否被本进程点火（供 HTTP 层统一映射 slug）。
pub fn definition_error_slug(error: &TriggerError) -> &'static str {
    error.slug()
}

/// 账本状态读回（测试/HTTP 列表用）。
pub async fn fire_state(store: &tokio::sync::Mutex<TriggerStore>, code: &str, now_ms: i64) -> Result<Option<FireState>> {
    Ok(store.lock().await.fire(code, now_ms).map_err(internal)?.map(|fire| fire.state))
}

/// 一行定义 → 对外 JSON（`/v1/triggers` 与 CLI 观测读口共用同一个形状）。
pub(crate) fn definition_json(row: &nous_store::triggers::Trigger) -> Value {
    json!({
        "code": row.code,
        "name": row.name,
        "enabled": row.enabled,
        "nextFireAt": row.next_fire_at,
        "version": row.version,
        "spec": serde_json::from_str::<Value>(&row.spec).unwrap_or(Value::Null),
    })
}


#[derive(Clone)]
pub struct TriggerRuntime {
    pub store: Arc<tokio::sync::Mutex<TriggerStore>>,
    pub durable: Arc<Session>,
    pub lookup: TemplateLookup,
    definitions: Arc<std::sync::RwLock<Vec<TriggerDefinition>>>,
    driver: Option<Arc<dyn nous_sessions::turn::TurnDriver>>,
    // 按指定 actor 现造一条 driver（faux 下让 assistant entry 也带 trigger:<code>）。
    driver_with_actor: Option<Arc<dyn Fn(&Actor) -> Arc<dyn nous_sessions::turn::TurnDriver> + Send + Sync>>,
    // durable 库路径：submission 反查与模板读取都指同一个库，不读环境。
    durable_db: std::path::PathBuf,
}

impl TriggerRuntime {
    pub fn new(
        store: TriggerStore,
        durable: Arc<Session>,
        lookup: TemplateLookup,
        driver: Option<Arc<dyn nous_sessions::turn::TurnDriver>>,
    // 按指定 actor 现造一条 driver（faux 下让 assistant entry 也带 trigger:<code>）。
    driver_with_actor: Option<Arc<dyn Fn(&Actor) -> Arc<dyn nous_sessions::turn::TurnDriver> + Send + Sync>>,
    // durable 库路径：submission 反查与模板读取都指同一个库，不读环境。
    durable_db: std::path::PathBuf,
    ) -> Result<Self> {
        let definitions = Self::load_definitions(&store)?;
        Ok(Self {
            store: Arc::new(tokio::sync::Mutex::new(store)),
            durable,
            lookup,
            definitions: Arc::new(std::sync::RwLock::new(definitions)),
            driver,
            driver_with_actor,
            durable_db,
        })
    }

    /// CLI 观测读口用的轻装配（#58）：只从 store 装载定义快照，不接 Session、不接 driver。
    /// HTTP 端点与 `nous-app --triggers` 走同一个 [`observation_report`]，接缝只有这一条。
    /// **故意返回快照而非自构造的 runtime**：CLI 不需要那份多余的 store/Session 句柄。
    pub fn open_definitions_only(
        store: &TriggerStore,
    ) -> Result<Vec<TriggerDefinition>> {
        Self::load_definitions(store)
    }

    pub async fn reload_definitions(&self) {
        let store = self.store.lock().await;
        if let Ok(definitions) = Self::load_definitions(&store) {
            *self.definitions.write().unwrap_or_else(|p| p.into_inner()) = definitions;
        }
    }

    fn load_definitions(store: &TriggerStore) -> Result<Vec<TriggerDefinition>> {
        let mut definitions = Vec::new();
        for row in store.triggers().map_err(internal)? {
            if row.deleted_at.is_some() {
                continue;
            }
            match TriggerDefinition::from_row(&row, &row.spec) {
                Ok(definition) => definitions.push(definition),
                Err(error) => eprintln!("nous-server: 触发器 {} 定义无效（{}），跳过；raw = {}", row.code, error.slug(), row.spec),
            }
        }
        Ok(definitions)
    }

    pub fn definitions(&self) -> std::sync::RwLockReadGuard<'_, Vec<TriggerDefinition>> {
        self.definitions.read().unwrap_or_else(|p| p.into_inner())
    }

    /// 仅供判据模拟「内存里有旧心跳」：实现已删掉内存快照，这是个 no-op。
    /// 真心跳只存在于 durable 的 tick 记录里（`load_last_tick_at`）。
    pub fn record_tick(&self, _now_ms: i64) {}

    /// #73：从 durable 库读回最近一拍的时刻（`lastTickAt` 的真相来源）。
    pub fn load_last_tick_at(&self) -> Option<i64> {
        let storage = durable_storage(&self.durable_db).ok()?;
        let ticks = nous_dispatcher::read_sleeping_ticks(&storage).ok()?;
        ticks.last().and_then(|record| record["state"]["wakeAt"].as_i64())
    }

    /// 经统一六步点火；成功时若有 faux 装配就顺手跑一轮 assistant。
    pub async fn fire(&self, code: &str, fired_by: &Actor, now_ms: i64) -> Result<FireOutcome> {
        // 每一步单独取/放锁：rusqlite 的 Connection 不是 Sync，MutexGuard 不许跨 await。
        let outcome = fire(&self.store, &self.durable, &self.lookup, &self.definitions, &self.durable_db, code, fired_by, now_ms).await?;
        if let FireOutcome::Fired { session, submission, entry } = &outcome {
            let submitted = submission_to_turn(&self.durable_db, *session, *submission, *entry).await?;
            let trigger_actor = Actor::system(format!("trigger:{code}"));
            let driver = self
                .driver_with_actor
                .as_ref()
                .map(|factory| factory(&trigger_actor))
                .or_else(|| self.driver.clone());
            if let Some(driver) = driver {
                if let Err(error) = driver.run(submitted).await {
                    eprintln!("nous-server: trigger fire 的 faux 轮次失败：{error}");
                }
            }
        }
        Ok(outcome)
    }
}

/// 刚 submit 的 user entry → 一轮 faux 需要的 ids。
async fn submission_to_turn(
    path: &std::path::Path,
    conversation: i64,
    submission: i64,
    entry_id: i64,
) -> Result<nous_sessions::turn::TurnRequest> {
    let storage = nous_durable::Storage::open_read_only(path)
        .map_err(|error| internal(format!("read durable: {error}")))?;
    let _record = storage
        .submissions(conversation)
        .map_err(internal)?
        .into_iter()
        .find(|item| item.get("id").and_then(Value::as_i64) == Some(submission))
        .ok_or_else(|| internal("submission not found".to_string()))?;
    // `submit_message` 同批铸 entry_id、submission_id 两个 id；assistant/live 从 entry_id+2 起
    // （与 `DurableCommands::submit` 的派生同一条规矩，别从 submission id 反推 —— 中间可能夹别的写）。
    Ok(nous_sessions::turn::TurnRequest {
        conversation,
        submission,
        entry_id: entry_id + 2,
        live: entry_id + 3,
        model: nous_ai::Model::new(
            nous_ai::FAUX_MODEL_ID,
            "Faux Echo",
            nous_ai::Api::Faux,
            "faux",
            "http://localhost:0",
        ),
        context: nous_ai::Context::new(Vec::new()),
        attempt: 1,
    })
}
