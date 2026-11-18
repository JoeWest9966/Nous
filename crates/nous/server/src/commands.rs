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
use nous_durable::session::commands::{
    abort, fork, scan_orphans, submit_message, CommandError, ForkRequest, SubmitRequest,
};
use nous_durable::session::{CommitObserver, ObserverRegistry, Session, SessionError};
use nous_durable::Storage;
use serde_json::Value;

use crate::error::{Result, ServerError};
use crate::records::{CreateSessionRequest, CreatedSession};

/// 受理结果（`POST /messages` 的 202 体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitResult {
    pub submission_id: i64,
    pub status: String,
}

/// fork 结果（`POST /fork` 的 201 体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkResult {
    pub id: i64,
    pub at: i64,
}

#[async_trait]
pub trait CommandSource: Send + Sync {
    /// 建会话：经 `Session::create_conversation`，和别的写共享一把锁并广播帧。
    async fn create(&self, request: CreateSessionRequest) -> Result<CreatedSession>;

    async fn submit(
        &self,
        session: i64,
        content: Value,
        request_id: Option<String>,
    ) -> Result<SubmitResult>;

    async fn abort(&self, session: i64) -> Result<()>;

    async fn fork(&self, session: i64, at: Option<i64>, name: Option<String>) -> Result<ForkResult>;

    /// 组装根要的那个 `Arc<Session>`（起轮次用它，取消 token 才同源）。
    ///
    /// 只读装配返回 `None`：那套装配没有写者，也就不该能起轮次。
    fn session(&self) -> Option<Arc<Session>>;

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
}

impl DurableCommands {
    /// 打开**本库自己**的库（上游 v2 库在写模式下会明确失败，见 `storage/mod.rs`）。
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        let path = path.as_ref();
        let registry = ObserverRegistry::new();
        let storage = Storage::open(path)
            .map_err(|error| ServerError::Internal(format!("open {}: {error}", path.display())))?;
        Ok(Arc::new(Self {
            session: Arc::new(Session::with_observers(storage, registry.clone())),
            registry,
        }))
    }

    /// 挂观察者。必须在开始接请求之前调用；挂上之后的每一次提交都会走到它。
    pub fn attach(&self, observer: Arc<dyn CommitObserver>) {
        self.registry.attach(observer);
    }
}

#[async_trait]
impl CommandSource for DurableCommands {
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
        )
        .await
        .map_err(command_error)?;
        Ok(SubmitResult {
            submission_id: outcome.submission_id,
            status: outcome.status,
        })
    }

    async fn abort(&self, session: i64) -> Result<()> {
        abort(&self.session, session).await.map_err(command_error)?;
        Ok(())
    }

    fn session(&self) -> Option<Arc<Session>> {
        Some(Arc::clone(&self.session))
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
}

/// 只读装配：写请求一律 409。
pub struct ReadOnlyCommands;

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

    fn session(&self) -> Option<Arc<Session>> {
        None
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
