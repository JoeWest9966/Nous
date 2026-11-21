//! 触发器点火路径（#57）：许可 → 记账 → 串行策略 → 会话解析 → 提交 → done。
//!
//! `nous-store` 只提供触发器的定义与账本，不认识 `Session`；`nous-server` 把 durable 的
//! `Session` 和 `Actor` 接到同一个 fire 函数上。会话写入全部经 `nous_durable::session::commands`
//! 与同一个 `Session`（#42 的坑：不开第二条写通道）。

use std::sync::Arc;

use axum::extract::{Path as AxumPath, State};
use axum::Json as AxumJson;
use nous_durable::actor::{Actor, ActorKind};
use nous_durable::session::commands::{record_origin, submit_message, OriginRecord, SubmitRequest};
use nous_durable::session::Session;
use nous_store::{
    FireActor, TriggerError, scheduler_lag, FireCounts, FireState, Inserted, SCHEDULER_TICK_MS,
    Store as TriggerStore, Trigger as TriggerDefinition,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Result, ServerError};

/// 点火的六个结果（对应票面六步，全部按短名给调用方）。
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
        return Err(ServerError::NotFound(format!("trigger {code} not found")));
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
            value.get("id").and_then(Value::as_i64).ok_or(ServerError::Internal("create_conversation 没回 id".to_string()))?
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
    .map_err(|error| ServerError::Internal(format!("submit_message: {error}")))?;
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
    .map_err(|error| ServerError::Internal(format!("record_origin: {error}")))?;
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
fn durable_storage(path: &std::path::Path) -> std::result::Result<nous_durable::Storage, ServerError> {
    nous_durable::Storage::open_read_only(path)
        .map_err(|error| ServerError::Internal(format!("read durable: {error}")))
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
        return Err(ServerError::NotFound(format!("trigger {code} not found")));
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

fn internal<E: std::fmt::Display>(error: E) -> ServerError {
    ServerError::Internal(error.to_string())
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

/// #73：`PATCH /v1/triggers/{code}` 的请求体。逐键覆盖；未在协议里的键按
/// `wire.md` 原样忽略（`code` 是写死的例外，由 PUT 的显式字段拒绝）。
#[derive(Debug, Deserialize)]
pub struct PatchBody {
    /// `code` 是不可变 slug：PATCH 里出现就 400（不是静默忽略），与 PUT 同一规矩。
    #[serde(default)]
    code: Option<Value>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    schedule: Option<Value>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    enabled: Option<bool>,
    /// #73 的 ETag：当前 `version`。整数是票面写法；字符串 / `W/"N"` 也认，
    /// 方便和既有 PATCH 的 If-Match 客户端共用。
    #[serde(default, deserialize_with = "deserialize_version")]
    version: Option<i64>,
}

fn deserialize_version<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<i64>, D::Error> {
    /// 请求里的 `version` 字段是可选的；缺省时 `Option` 自己会进来，这里只接有值的那种。
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawVersion {
        Number(i64),
        Text(String),
    }
    let value = match Option::<RawVersion>::deserialize(d)? {
        Some(RawVersion::Number(number)) => Value::Number(number.into()),
        Some(RawVersion::Text(text)) => Value::String(text),
        None => return Ok(None),
    };
    let text = match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text,
        Value::Null => return Ok(None),
        other => return Err(serde::de::Error::custom(format!("version 不是数字：{other}"))),
    };
    let trimmed = text.trim().trim_start_matches("W/").trim_matches('"');
    trimmed
        .parse::<i64>()
        .map(Some)
        .map_err(|_| serde::de::Error::custom("version 不是数字"))
}

fn parse_version(raw: &str) -> Result<i64> {
    let trimmed = raw.trim().trim_start_matches("W/").trim_matches('"');
    trimmed
        .parse::<i64>()
        .map_err(|_| ServerError::BadRequest(format!("version 不是数字：{raw}")))
}

/// `If-Match` 头也必须和 body 的 `version` 一致：两个地方都不许各说各话。
fn header_version(headers: &axum::http::HeaderMap) -> Result<Option<String>> {
    Ok(headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string))
}

/// #73：`PATCH /v1/triggers/{code}`。改 `schedule` / `params` / `enabled`，`code` 不可变。
pub async fn patch_trigger(
    State(state): State<Arc<crate::app::AppState>>,
    AxumPath(code): AxumPath<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<AxumJson<Value>> {
    let runtime = state.triggers().ok_or_else(|| {
        ServerError::Conflict("triggers not configured".to_string())
    })?;
    let body: PatchBody = serde_json::from_slice(&body)
        .map_err(|error| ServerError::BadRequest(format!("patch body: {error}")))?;
    if body.code.is_some() {
        return Err(ServerError::BadRequest("code 不可变（PATCH 里不许带 code）".to_string()));
    }
    let expected = body.version.ok_or_else(|| {
        ServerError::BadRequest("version is required".to_string())
    })?;
    if let Some(header) = header_version(&headers)? {
        if parse_version(&header)? != expected {
            return Err(ServerError::PreconditionFailed("If-Match 与 version 不一致".to_string()));
        }
    }
    if body.schedule.is_none() && body.params.is_none() && body.enabled.is_none() {
        return Err(ServerError::BadRequest("patch body 至少要改一个键".to_string()));
    }
    // requestId 可选：重放回第一次回执，版本不涨。
    if let Some(request_id) = body.request_id.as_deref().filter(|id| !id.is_empty()) {
        let store = runtime.store.lock().await;
        if let Some(receipt) = store.patch_receipt(&code, request_id).map_err(internal)? {
            return Ok(AxumJson(serde_json::from_str(&receipt).map_err(internal)?));
        }
    }
    let response = {
        let store = runtime.store.lock().await;
        let row = store.trigger(&code).map_err(internal)?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(|| ServerError::NotFound(format!("trigger {code} not found")))?;
        let current = nous_store::Trigger::from_row(&row, &row.spec).map_err(internal)?;
        let schedule = match body.schedule {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(cron)) => Some(Some(cron)),
            Some(other) => return Err(ServerError::BadRequest(format!("schedule 必须是字符串或 null：{other}"))),
        };
        let patched = current
            .patch(schedule, body.params, body.enabled)
            .map_err(|error| ServerError::BadRequest(error.slug().to_string()))?;
        // 回执正文在事务之前算好：definition 与库里改后的行逐字段一致，
        // version 就是 expected + 1（乐观锁保证只有这一笔能写进去）。
        let mut after = definition_json(&row);
        after["enabled"] = json!(patched.enabled());
        let patch_json = patched.definition_json();
        if let Ok(spec) = serde_json::from_str::<Value>(&patch_json) {
            after["spec"] = spec["spec"].clone();
            after["nextFireAt"] = spec["nextFireAt"].clone();
        }
        after["version"] = json!(expected + 1);
        let response = json!({
            "code": code,
            "enabled": after["enabled"].clone(),
            "name": after["name"].clone(),
            "spec": after["spec"].clone(),
            "version": expected + 1,
            "definition": after,
        });
        // review 10：变更 + 回执同一次提交。回执写失败 → 定义不涨 version、库不变。
        let version = if let Some(request_id) = body.request_id.as_deref().filter(|id| !id.is_empty()) {
            store.patch_with_receipt(&code, &patch_json, patched.enabled(), expected,
                request_id, &response.to_string()).map_err(internal)?
        } else {
            store.patch_trigger(&code, &patch_json, patched.enabled(), expected).map_err(internal)?
        };
        version.ok_or_else(|| ServerError::PreconditionFailed(format!("trigger {code} version 不匹配")))?;
        response
    };
    runtime.reload_definitions().await;
    Ok(AxumJson(response))
}

fn definition_json(row: &nous_store::triggers::Trigger) -> Value {
    json!({
        "code": row.code,
        "name": row.name,
        "enabled": row.enabled,
        "nextFireAt": row.next_fire_at,
        "version": row.version,
        "spec": serde_json::from_str::<Value>(&row.spec).unwrap_or(Value::Null),
    })
}

/// 进程级点火装配：store + 同一个 Session + 定义快照 + 模板读取 + faux 轮次驱动器。
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
        .map_err(|error| ServerError::Internal(format!("read durable: {error}")))?;
    let _record = storage
        .submissions(conversation)
        .map_err(internal)?
        .into_iter()
        .find(|item| item.get("id").and_then(Value::as_i64) == Some(submission))
        .ok_or_else(|| ServerError::Internal("submission not found".to_string()))?;
    // `submit_message` 同批铸 entry_id、submission_id 两个 id；assistant/live 从 entry_id+2 起
    // （与 `DurableCommands::submit` 的派生同一条规矩，别从 submission id 反推 —— 中间可能夹别的写）。
    Ok(nous_sessions::turn::TurnRequest {
        conversation,
        submission,
        entry_id: entry_id + 2,
        live: entry_id + 3,
        model: nous_ai::Model::new(
            crate::runtime::FAUX_MODEL_ID,
            "Faux Echo",
            nous_ai::Api::Faux,
            "faux",
            "http://localhost:0",
        ),
        context: nous_ai::Context::new(Vec::new()),
        attempt: 1,
    })
}

// ── #80：trigger-catalog 管理面 ─────────────────────────────────────────────
//
// 都在同一个 `triggers` 面、同一张 `triggers` 表上；不新增 crate、不开第二条写通道。
// 写入前过发布闸（五段 cron + on slug），坏定义 400 且库里一行都不写。

/// POST /v1/triggers 的 body。`code` 是身份，只在建的时候出现在 body 里；
/// 未知键按 `wire.md` 忽略（传输层的未知字段不是本地配置）。
#[derive(Debug, Deserialize)]
pub struct CreateBody {
    code: String,
    #[serde(default)]
    name: Option<String>,
    spec: Value,
    #[serde(rename = "templateRef", default)]
    template_ref: Option<Value>,
    #[serde(rename = "firesAllowedBy", default)]
    fires_allowed_by: Option<Value>,
    #[serde(default)]
    target: Option<Value>,
    #[serde(default)]
    policy: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SpecBody {
    #[serde(rename = "spec_version", alias = "specVersion", default)]
    spec_version: Option<u32>,
    #[serde(default)]
    schedule: Option<Value>,
    #[serde(default)]
    on: Option<Value>,
    #[serde(default)]
    params: Option<Value>,
    /// #80：session 是落点简写（null=新会话），与显式 target 互斥。
    #[serde(rename = "session", default)]
    session: Option<Value>,
}

fn bad(message: impl std::fmt::Display) -> ServerError {
    ServerError::BadRequest(message.to_string())
}

/// 与 `routes.rs` / `presets.rs` 同一处取时（app 层允许直接时钟；kernel 不许）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

fn value_as_opt_string(value: Option<&Value>, field: &str) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(other) => Err(bad(format!("{field} 必须是字符串或 null：{other}"))),
    }
}

/// 发布闸：schedule 五段 cron 或 null；on 是封闭 kind 的 slug 或 null。
/// 返回规范化的 (schedule, on)，**任何坏值都不落库**。
fn validate_rules(schedule: Option<&Value>, on: Option<&Value>) -> Result<(Option<String>, Option<String>)> {
    let schedule = value_as_opt_string(schedule, "schedule")?;
    let on = value_as_opt_string(on, "on")?;
    if let Some(cron) = schedule.as_deref() {
        nous_store::parse_five_field_cron(cron).map_err(|error| bad(format!("schedule 无效（{}）：{cron}", error.slug())))?;
    }
    nous_store::validate_on(on.as_deref()).map_err(|error| bad(format!("on 无效（{}）", error.slug())))?;
    Ok((schedule, on))
}

fn validate_params(params: Option<&Value>) -> Result<Option<Value>> {
    let Some(params) = params else { return Ok(None) };
    if params.is_null() {
        return Ok(None);
    }
    let object = params.as_object().ok_or_else(|| bad("params 必须是对象"))?;
    if !object.values().all(|value| value.is_string() || value.is_number() || value.is_boolean() || value.is_null()) {
        return Err(bad("params 的值只能是字符串 / 数字 / 布尔 / null"));
    }
    Ok(Some(params.clone()))
}

fn spec_json(spec_version: u32, schedule: Option<String>, on: Option<String>, params: Option<Value>) -> String {
    let mut spec = serde_json::json!({
        "spec_version": spec_version,
        "schedule": schedule,
        "on": on,
    });
    if let Some(params) = params {
        spec["params"] = params;
    }
    spec.to_string()
}

fn target_json(body_session: Option<&Value>, target: Option<&Value>) -> Result<String> {
    if body_session.is_some() && target.is_some() {
        return Err(bad("session 与 target 不能同时出现"));
    }
    if let Some(session) = body_session {
        return Ok(match session {
            Value::Null => json!({ "kind": "new" }).to_string(),
            Value::Number(number) => number.as_i64()
                .map(|id| json!({ "kind": "conversation", "id": id }).to_string())
                .ok_or_else(|| bad("session 必须是整数或 null"))?,
            other => return Err(bad(format!("session 必须是整数或 null：{other}"))),
        });
    }
    if let Some(target) = target {
        let kind = target.get("kind").and_then(Value::as_str).ok_or_else(|| bad("target.kind 必须"))?;
        return match kind {
            "new" => Ok(json!({ "kind": "new" }).to_string()),
            "conversation" => {
                let id = target.get("id").and_then(Value::as_i64).ok_or_else(|| bad("target.id 必须是整数"))?;
                Ok(json!({ "kind": "conversation", "id": id }).to_string())
            }
            other => Err(bad(format!("未知 target.kind：{other}"))),
        };
    }
    Ok(json!({ "kind": "new" }).to_string())
}

fn definition_row(store: &nous_store::Store, code: &str) -> Result<nous_store::triggers::Trigger> {
    store.trigger(code).map_err(internal)?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(|| ServerError::NotFound(format!("trigger {code} not found")))
}

fn catalog_definition_json(row: &nous_store::triggers::Trigger) -> Value {
    definition_json(row)
}

/// POST /v1/triggers：新建。返回 201 + 生效定义。
pub async fn create_trigger(
    State(state): State<Arc<crate::app::AppState>>,
    body: axum::body::Bytes,
) -> Result<(axum::http::StatusCode, AxumJson<Value>)> {
    let runtime = state.triggers().ok_or_else(|| ServerError::Conflict("triggers not configured".to_string()))?;
    let body: CreateBody = serde_json::from_slice(&body).map_err(|error| bad(format!("create body: {error}")))?;
    let spec: SpecBody = serde_json::from_value(body.spec).map_err(|error| bad(format!("spec: {error}")))?;
    let (schedule, on) = validate_rules(spec.schedule.as_ref(), spec.on.as_ref())?;
    let params = validate_params(spec.params.as_ref())?;
    let spec_version = spec.spec_version.unwrap_or(1);
    let template_ref = body.template_ref
        .map(|value| value.to_string())
        .unwrap_or_else(|| json!({ "kind": "preset", "ref": "default" }).to_string());
    let fires_allowed_by = body.fires_allowed_by
        .map(|value| value.to_string())
        .unwrap_or_else(|| json!([{ "kind": "system", "id": "cron" }]).to_string());
    let target = target_json(spec.session.as_ref(), body.target.as_ref())?;
    let policy = body.policy.unwrap_or_else(|| "serial".to_string());
    let name = body.name.unwrap_or_else(|| body.code.clone());
    let spec_raw = spec_json(spec_version, schedule.clone(), on.clone(), params.clone());

    let (definition, version) = {
        let store = runtime.store.lock().await;
        let input = nous_store::NewTriggerDefinition {
            code: &body.code,
            name: &name,
            spec: &spec_raw,
            template_ref: &template_ref,
            fires_allowed_by: &fires_allowed_by,
            target: &target,
            policy: &policy,
        };
        // 发布闸：Trigger::new 校验九条准则 + cron；code 冲突（含墓碑）跃到 409。
        let trigger = nous_store::Trigger::new(input, now_ms(), Some(&store))
            .map_err(|error| definition_error(&error))?;
        let category = body.category.as_deref();
        if let Some(name) = category {
            store.set_trigger_category(&body.code, Some(name)).map_err(internal)?;
        }
        let row = store.trigger(&body.code).map_err(internal)?.ok_or_else(|| internal("建完读不回触发器"))?;
        (catalog_definition_json(&row), row.version)
    };
    runtime.reload_definitions().await;
    let response = json!({
        "code": body.code,
        "name": name,
        "category": body.category,
        "spec": serde_json::from_str::<Value>(&spec_raw).unwrap_or(Value::Null),
        "version": version,
        "enabled": true,
        "definition": definition,
    });
    Ok((axum::http::StatusCode::CREATED, AxumJson(response)))
}

fn definition_error(error: &nous_store::TriggerError) -> ServerError {
    match error {
        nous_store::TriggerError::CodeTaken | nous_store::TriggerError::CodeTombstoned => {
            ServerError::Conflict(error.slug().to_string())
        }
        _ => ServerError::BadRequest(error.slug().to_string()),
    }
}

/// PUT /v1/triggers/{code} 的 body：逐键覆盖，未知键忽略，`code` 必拒。
#[derive(Debug, Deserialize)]
struct PutBody {
    #[serde(default)]
    code: Option<Value>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    schedule: Option<Value>,
    #[serde(default)]
    on: Option<Value>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    session: Option<Value>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    version: Option<i64>,
}

/// PUT /v1/triggers/{code}：整份改定义，version 是 ETag。
pub async fn put_trigger(
    State(state): State<Arc<crate::app::AppState>>,
    AxumPath(code): AxumPath<String>,
    body: axum::body::Bytes,
) -> Result<AxumJson<Value>> {
    let runtime = state.triggers().ok_or_else(|| ServerError::Conflict("triggers not configured".to_string()))?;
    let body: PutBody = serde_json::from_slice(&body).map_err(|error| bad(format!("put body: {error}")))?;
    if body.code.is_some() {
        return Err(bad("code 不可改（路径里的 code 才是身份）"));
    }
    if body.schedule.is_none() && body.on.is_none() && body.params.is_none()
        && body.session.is_none() && body.enabled.is_none() && body.name.is_none() {
        return Err(bad("put body 至少要改一个键"));
    }
    // 先把规则过闸：坏 schedule / on / params 一律 400，不进事务、不写半份定义。
    let (new_schedule, new_on) = validate_rules(body.schedule.as_ref(), body.on.as_ref())?;
    let new_params = validate_params(body.params.as_ref())?;

    let (definition, version) = {
        let store = runtime.store.lock().await;
        let row = definition_row(&store, &code)?;
        let current = nous_store::Trigger::from_row(&row, &row.spec).map_err(internal)?;
        let expected = body.version.unwrap_or(row.version);
        if expected != row.version {
            return Err(ServerError::PreconditionFailed(format!("trigger {code} version 不匹配")));
        }
        let spec = current.spec();
        let schedule = match &body.schedule {
            None => spec.schedule.clone(),
            Some(_) => new_schedule.clone(),
        };
        let on = match &body.on {
            None => spec.on.clone(),
            Some(_) => new_on.clone(),
        };
        let params = match &body.params {
            None => spec.params.clone(),
            Some(_) => new_params.clone(),
        };
        let spec_raw = spec_json(spec.spec_version, schedule, on, params);
        let target = if body.session.is_some() {
            target_json(body.session.as_ref(), None)?
        } else {
            // 保留旧 target：从库里的定义 JSON 读。
            let old: Value = serde_json::from_str(&row.spec).map_err(internal)?;
            old.get("target").map(|value| value.to_string()).unwrap_or_else(|| json!({ "kind": "new" }).to_string())
        };
        // 用定义层构造新对象（会再验一次 cron），再写回；code 原样不动。
        let old: Value = serde_json::from_str(&row.spec).map_err(internal)?;
        let rebuilt = nous_store::Trigger::new(
            nous_store::NewTriggerDefinition {
                code: &code,
                name: body.name.as_deref().unwrap_or(&row.name),
                spec: &spec_raw,
                template_ref: &old["template_ref"].to_string(),
                fires_allowed_by: &old["fires_allowed_by"].to_string(),
                target: &target,
                policy: old["policy"].as_str().unwrap_or("serial"),
            },
            now_ms(),
            None,
        ).map_err(|error| definition_error(&error))?;
        let enabled = body.enabled.unwrap_or(row.enabled);
        let version = store.put_definition(&code, &rebuilt.definition_json(), enabled, expected)
            .map_err(internal)?
            .ok_or_else(|| ServerError::PreconditionFailed(format!("trigger {code} version 不匹配")))?;
        if let Some(name) = body.name.as_deref() {
            store.rename_trigger(&code, name).map_err(internal)?;
        }
        let after = store.trigger(&code).map_err(internal)?.ok_or_else(|| internal("put 后读不回触发器"))?;
        (catalog_definition_json(&after), version)
    };
    runtime.reload_definitions().await;
    Ok(AxumJson(json!({
        "code": code,
        "version": version,
        "definition": definition,
    })))
}

/// POST /v1/triggers/{code}/archive：归档墓碑；账不动。
pub async fn archive_trigger(
    State(state): State<Arc<crate::app::AppState>>,
    AxumPath(code): AxumPath<String>,
) -> Result<AxumJson<Value>> {
    let runtime = state.triggers().ok_or_else(|| ServerError::Conflict("triggers not configured".to_string()))?;
    let now = now_ms();
    let ok = {
        let store = runtime.store.lock().await;
        store.archive_trigger(&code, now).map_err(internal)?
    };
    if !ok {
        return Err(ServerError::NotFound(format!("trigger {code} not found 或已归档")));
    }
    runtime.reload_definitions().await;
    Ok(AxumJson(json!({ "code": code, "archivedAt": now })))
}

/// POST /v1/triggers/{code}/restore：抹掉归档墓碑。
pub async fn restore_trigger(
    State(state): State<Arc<crate::app::AppState>>,
    AxumPath(code): AxumPath<String>,
) -> Result<AxumJson<Value>> {
    let runtime = state.triggers().ok_or_else(|| ServerError::Conflict("triggers not configured".to_string()))?;
    let now = now_ms();
    let ok = {
        let store = runtime.store.lock().await;
        store.restore_trigger(&code, now).map_err(internal)?
    };
    if !ok {
        return Err(ServerError::NotFound(format!("trigger {code} 不是归档态")));
    }
    runtime.reload_definitions().await;
    Ok(AxumJson(json!({ "code": code, "restoredAt": now })))
}

/// POST /v1/triggers/{code}/purge：先归档才能真删，账一并删。
pub async fn purge_trigger(
    State(state): State<Arc<crate::app::AppState>>,
    AxumPath(code): AxumPath<String>,
) -> Result<AxumJson<Value>> {
    let runtime = state.triggers().ok_or_else(|| ServerError::Conflict("triggers not configured".to_string()))?;
    let now = now_ms();
    let fires = {
        let store = runtime.store.lock().await;
        store.purge_trigger(&code, now).map_err(internal)?
    };
    let Some(fires) = fires else {
        return Err(ServerError::Conflict(format!("trigger {code} 必须先归档才能真删（或已真删）")));
    };
    runtime.reload_definitions().await;
    Ok(AxumJson(json!({ "code": code, "purgedAt": now, "fires": fires })))
}

#[derive(Debug, Deserialize)]
struct CategoryBody {
    code: String,
    category: Option<String>,
}

/// POST /v1/triggers/category：给一个触发器挂单值类别；null 清除。
pub async fn set_category(
    State(state): State<Arc<crate::app::AppState>>,
    body: axum::body::Bytes,
) -> Result<AxumJson<Value>> {
    let runtime = state.triggers().ok_or_else(|| ServerError::Conflict("triggers not configured".to_string()))?;
    let body: CategoryBody = serde_json::from_slice(&body).map_err(|error| bad(format!("category body: {error}")))?;
    let changed = {
        let store = runtime.store.lock().await;
        store.set_trigger_category(&body.code, body.category.as_deref()).map_err(internal)?
    };
    if changed.is_none() {
        return Err(ServerError::NotFound(format!("trigger {} not found", body.code)));
    }
    Ok(AxumJson(json!({ "code": body.code, "category": body.category })))
}

/// GET /v1/triggers/categories：类别词表（词表 ∪ 在用）。
pub async fn categories(
    State(state): State<Arc<crate::app::AppState>>,
) -> Result<AxumJson<Value>> {
    let runtime = state.triggers().ok_or_else(|| ServerError::Conflict("triggers not configured".to_string()))?;
    let names = runtime.store.lock().await.trigger_categories().map_err(internal)?;
    Ok(AxumJson(json!({ "categories": names })))
}
