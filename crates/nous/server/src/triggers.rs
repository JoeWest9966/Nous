//! 触发器点火路径（#57）：许可 → 记账 → 串行策略 → 会话解析 → 提交 → done。
//!
//! `nous-store` 只提供触发器的定义与账本，不认识 `Session`；`nous-server` 把 durable 的
//! `Session` 和 `Actor` 接到同一个 fire 函数上。会话写入全部经 `nous_durable::session::commands`
//! 与同一个 `Session`（#42 的坑：不开第二条写通道）。

use std::sync::Arc;

use nous_durable::actor::{Actor, ActorKind};
use nous_durable::session::commands::{submit_message, SubmitRequest};
use nous_durable::session::Session;
use nous_store::{
    FireActor, FireState, Inserted, Store as TriggerStore, Trigger as TriggerDefinition, TriggerError,
};
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
    // ⑥ 落账 done（又是同步片段）
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

fn internal<E: std::fmt::Display>(error: E) -> ServerError {
    ServerError::Internal(error.to_string())
}

/// 校验一个触发器定义能否被本进程点火（供 HTTP 层统一映射 slug）。
pub fn definition_error_slug(error: &TriggerError) -> &'static str {
    error.slug()
}

/// 账本状态读回（测试/HTTP 列表用）。
pub async fn fire_state(store: &tokio::sync::Mutex<TriggerStore>, code: &str, now_ms: i64) -> Result<Option<FireState>> {
    Ok(store.lock().await.fire(code, now_ms).map_err(internal)?.map(|fire| fire.state))
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
        Ok(Self { store: Arc::new(tokio::sync::Mutex::new(store)), durable, lookup, definitions: Arc::new(std::sync::RwLock::new(definitions)), driver, driver_with_actor, durable_db })
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
