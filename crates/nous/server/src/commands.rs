//! 写命令的后端。端点只认这个 trait：路由不直接碰 `Session`。
//!
//! 生产实现 [`DurableCommands`] 把 `nous_durable::session::commands` 包了一层，并把
//! `CommitObserver` 挂到同一个 `ObserverRegistry` 上 —— 提交落库后帧立刻进 `Hub`，
//! 端点不用自己「发一条 message_start」。
//!
//! 只读装配（[`ReadOnlyCommands`]）返回 409：`/v1/sessions` 与 SSE 不需要写后端，
//! 判据也不该为了读一条会话去建一个库。写请求打到只读装配上是**明确拒绝**，不是 500。

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use nous_durable::actor::Actor;
use nous_durable::session::commands::{
    abort, fork, mark_read, rename_session, scan_orphans, set_pin, submit_message, unread_map,
    CommandError, ForkRequest, PinOutcome, PinRequest, SubmitRequest,
};
use nous_durable::session::{CommitObserver, ObserverRegistry, Session, SessionError};
use nous_durable::Storage;
use serde_json::{json, Value};

use crate::error::{Result, ServerError};
use crate::records::{CreateSessionRequest, CreatedSession};

/// 受理结果（`POST /messages` 的 202 体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitResult {
    pub submission_id: i64,
    pub status: String,
    /// durable 的 `submit_message` 同批分配的 id：user entry、submission。
    /// 轮次需要的辅助 id 由这两个推出来（submit 同批铸两个 id，下一个从 entry_id+2 起）。
    pub entry_id: i64,
    pub assistant_entry_id: i64,
    pub live_id: i64,
}

/// fork 结果（`POST /fork` 的 201 体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkResult {
    pub id: i64,
    pub at: i64,
}

/// `POST /v1/read/:id` 的回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    pub id: i64,
    pub read_at: i64,
    pub unread: u64,
}

/// `POST /v1/sessions/:id/rename` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameResult {
    pub id: i64,
    pub name: String,
}

/// `POST /v1/sessions/:id/pin` 的回执。
pub type PinResult = PinOutcome;

#[async_trait]
pub trait CommandSource: Send + Sync {
    /// 真写后端的同一条 `Session`（只读投影用）；只读装配没有，`None`。
    fn session(&self) -> Option<Arc<Session>> { None }

    /// 建会话：经 `Session::create_conversation`，和别的写共享一把锁并广播帧。
    async fn create(&self, request: CreateSessionRequest) -> Result<CreatedSession>;

    async fn submit(
        &self,
        session: i64,
        content: Value,
        request_id: Option<String>,
    ) -> Result<SubmitResult>;

    async fn abort(&self, session: i64) -> Result<()>;

    /// #70：会话有没有未收敛的轮次（`whenBusy=interrupt` 用）。
    async fn busy(&self, session: i64) -> Result<bool> { let _ = session; Ok(false) }

    /// #70：轮次失败时把 submission 收敛到 `unanswered`，不许停在 queued。
    async fn fail_submission(
        &self,
        session: i64,
        submission: i64,
        reason: &str,
    ) -> Result<()> {
        let _ = (session, submission, reason);
        Ok(())
    }

    async fn fork(&self, session: i64, at: Option<i64>, name: Option<String>) -> Result<ForkResult>;

    /// 全部会话的未读状态（平铺 map），只读。
    async fn unread(&self) -> Result<Value>;

    /// 标已读：走 `Session` 写者并广播。
    async fn read(&self, session: i64) -> Result<ReadResult>;

    /// 改名：走 `Session` 写者并广播。
    async fn rename(&self, session: i64, name: String) -> Result<RenameResult>;

    /// 置顶/取消置顶：走 `Session` 写者并广播。
    async fn pin(&self, session: i64, pinned: bool, request_id: String) -> Result<PinResult>;

    /// 启动时补写孤儿终态（整机崩/重启留下的）。**只在组装根里调一次**，不是端点。
    ///
    /// 放在这条 trait 上是为了让**同一个 `Session` 写者**做这件事：另开一个 `Session`
    /// 等于第二条写通道（`#38` 收尾，2026-11-18）。
    async fn reclaim_orphans(&self) -> Result<usize>;
}

/// 生产写后端：一条 `Session` + 它的观察者名单。
pub struct DurableCommands {
    session: Arc<Session>,
    registry: Arc<ObserverRegistry>,
    /// 写 entry / submission 时显式传下去的 actor；将来认证在这里换成 principal。
    actor: Actor,
}

impl DurableCommands {
    /// 用一条现成的 `Session` 装配写命令；与 sessions 的轮次共享同一个取消 token。
    pub fn with_session(session: Arc<Session>, registry: Arc<ObserverRegistry>) -> Arc<Self> {
        Arc::new(Self { session, registry, actor: Actor::system("nous-app") })
    }

    /// 换掉写命令用的 actor（认证落地时由 principal 派生；现在只有显式调用）。
    pub fn with_actor(&self, actor: Actor) -> Arc<Self> {
        Arc::new(Self {
            session: self.session.clone(),
            registry: self.registry.clone(),
            actor,
        })
    }

    /// 打开**本库自己**的库（上游 v2 库在写模式下会明确失败，见 `storage/mod.rs`）。
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        let path = path.as_ref();
        let registry = ObserverRegistry::new();
        let storage = Storage::open(path)
            .map_err(|error| ServerError::Internal(format!("open {}: {error}", path.display())))?;
        Ok(Arc::new(Self {
            session: Arc::new(Session::with_observers(storage, registry.clone())),
            registry,
            actor: Actor::system("nous-app"),
        }))
    }

    /// 同一条 `Session` 的只读句柄，给 #48 的 documents 写后端复用（不另开写者）。
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// 挂观察者。必须在开始接请求之前调用；挂上之后的每一次提交都会走到它。
    pub fn attach(&self, observer: Arc<dyn CommitObserver>) {
        self.registry.attach(observer);
    }
}

#[async_trait]
impl CommandSource for DurableCommands {
    fn session(&self) -> Option<Arc<Session>> {
        Some(self.session.clone())
    }

    async fn create(&self, request: CreateSessionRequest) -> Result<CreatedSession> {
        let config = request.into_config();
        let value = self.session.create_conversation(config).await.map_err(|error| match error {
            SessionError::Closed => ServerError::Conflict("session is closed".to_string()),
            other => ServerError::Internal(other.to_string()),
        })?;
        serde_json::from_value(value).map_err(|error| ServerError::Internal(error.to_string()))
    }

    async fn submit(
        &self,
        session: i64,
        content: Value,
        request_id: Option<String>,
    ) -> Result<SubmitResult> {
        let outcome = submit_message(
            &self.session,
            SubmitRequest {
                conversation: session,
                content,
                request_id,
            },
            &self.actor,
        )
        .await
        .map_err(command_error)?;
        Ok(SubmitResult {
            submission_id: outcome.submission_id,
            status: outcome.status,
            entry_id: outcome.entry_id,
            // submit 同批铸了 entry_id、submission_id 两个 id；下一个可用 id 从 entry_id+2 起。
            assistant_entry_id: outcome.entry_id + 2,
            live_id: outcome.entry_id + 3,
        })
    }

    async fn abort(&self, session: i64) -> Result<()> {
        abort(&self.session, session).await.map_err(command_error)?;
        Ok(())
    }

    async fn busy(&self, session: i64) -> Result<bool> {
        nous_durable::session::commands::has_pending_submission(&self.session, session)
            .await
            .map_err(command_error)
    }

    async fn fail_submission(
        &self,
        _session: i64,
        submission: i64,
        reason: &str,
    ) -> Result<()> {
        nous_durable::session::commands::fail_submission(
            &self.session,
            submission,
            reason,
            None,
        )
        .await
        .map_err(command_error)
    }

    async fn reclaim_orphans(&self) -> Result<usize> {
        scan_orphans(&self.session).await.map_err(command_error)
    }

    async fn fork(&self, session: i64, at: Option<i64>, name: Option<String>) -> Result<ForkResult> {
        let outcome = fork(
            &self.session,
            ForkRequest {
                conversation: session,
                at,
                name,
            },
        )
        .await
        .map_err(command_error)?;
        Ok(ForkResult {
            id: outcome.id,
            at: outcome.at,
        })
    }

    async fn unread(&self) -> Result<Value> {
        unread_map(&self.session).await.map_err(command_error)
    }

    async fn read(&self, session: i64) -> Result<ReadResult> {
        let outcome = mark_read(&self.session, session)
            .await
            .map_err(command_error)?;
        Ok(ReadResult {
            id: outcome.id,
            read_at: outcome.read_at,
            unread: outcome.unread,
        })
    }

    async fn rename(&self, session: i64, name: String) -> Result<RenameResult> {
        let outcome = rename_session(&self.session, session, &name)
            .await
            .map_err(command_error)?;
        Ok(RenameResult {
            id: outcome.id,
            name: outcome.name,
        })
    }

    async fn pin(&self, session: i64, pinned: bool, request_id: String) -> Result<PinResult> {
        set_pin(
            &self.session,
            PinRequest {
                conversation: session,
                pinned,
                request_id,
            },
        )
        .await
        .map_err(command_error)
    }
}

/// 只读装配：写请求一律 409；读投影（unread）从注入的只读数据源出处，形状与写装配一致。
pub struct ReadOnlyCommands {
    source: Arc<dyn crate::source::SessionSource>,
}

impl ReadOnlyCommands {
    pub fn new(source: Arc<dyn crate::source::SessionSource>) -> Arc<Self> {
        Arc::new(Self { source })
    }
}

#[async_trait]
impl CommandSource for ReadOnlyCommands {
    async fn create(&self, _: CreateSessionRequest) -> Result<CreatedSession> {
        Err(read_only())
    }

    async fn submit(&self, _: i64, _: Value, _: Option<String>) -> Result<SubmitResult> {
        Err(read_only())
    }

    async fn abort(&self, _: i64) -> Result<()> {
        Err(read_only())
    }

    async fn fork(&self, _: i64, _: Option<i64>, _: Option<String>) -> Result<ForkResult> {
        Err(read_only())
    }

    async fn unread(&self) -> Result<Value> {
        let mut map = serde_json::Map::new();
        for session in self.source.sessions()? {
            map.insert(session.id.to_string(), json!({ "unread": 0, "readAt": Value::Null }));
        }
        Ok(Value::Object(map))
    }

    async fn read(&self, _: i64) -> Result<ReadResult> {
        Err(read_only())
    }

    async fn rename(&self, _: i64, _: String) -> Result<RenameResult> {
        Err(read_only())
    }

    async fn pin(&self, _: i64, _: bool, _: String) -> Result<PinResult> {
        Err(read_only())
    }

    /// 只读装配没有写者，也就没有孤儿要补：明确返回 0，不报错。
    async fn reclaim_orphans(&self) -> Result<usize> {
        Ok(0)
    }
}

fn read_only() -> ServerError {
    ServerError::Conflict("this server was started read-only".to_string())
}

/// `CommandError` → HTTP 语义。**409 不许写成 400**：状态冲突是「现在不行」，
/// 参数错误才是「这么写不行」。
fn command_error(error: CommandError) -> ServerError {
    match error {
        CommandError::BadRequest(message) => ServerError::BadRequest(message),
        CommandError::NotFound(message) => ServerError::NotFound(message),
        CommandError::Conflict(message) => ServerError::Conflict(message),
        CommandError::Session(SessionError::Closed) => {
            ServerError::Conflict("session is closed".to_string())
        }
        CommandError::Session(other) => ServerError::Internal(other.to_string()),
    }
}
