//! 会话语义层。上游 `packages/durable/src/session/**`，只借语义、不借结构。
//!
//! 这一层回答一个问题：**一次提交把什么变成了什么** —— 会话的可见转录、活动上下文、
//! 提交（submission）的生命周期，以及提交后谁该知道。它不碰传输（`nous-server`，#22）、
//! 不碰模型调用（`harness`，#14）、不碰工具（`nous-tools`）。
//!
//! ## 与上游的结构差别（有意为之）
//!
//! - 上游把事务回调（`Tx`）交给调用方，回调里再读写；本库的事务封在
//!   [`Storage::commit`](crate::Storage) 里，调用方给的是**一批写**（`12b` 的
//!   [`StorageWrite`](crate::StorageWrite)）。`Tx` 不进 Rust 的公开面。
//! - 上游 `session/` 有 2,105 行的 `session.ts` + 1,300 行的 `transaction.ts` + 缓存的
//!   `cached-storage.ts`。缓存是性能件（#18），本层先不搬；语义对得上再谈缓存。
//!
//! ## 现在有什么
//!
//! - [`entries`] —— 六个 entry 判别键（`pi.user` … `pi.compaction`）；
//! - [`publish`] —— 提交发布面：`CommitPublication` / `CommitChange` 与观察者名单；
//! - [`Session`] —— 五个方法的公开面（`commit` / `close` / `closed` / `snapshot` / `get_entry`）。
//!
//! ## 公开面只有五个方法，这是判据不是愿望
//!
//! `tests/session_face.rs` 扫这个文件的源码，断言 `pub async fn` 恰好是那五个、`pub fn` 恰好是
//! 两个构造器。多一个就红 —— 因为这张表是 `#22` 的输入，让它漂就等于让端点层猜。
//!
//! `observe_doc` / `watch_doc` / `observe_commits` / `subscribe_close` **不在**这里：订阅机制属
//! `#19`。本模块只交发布侧的**回调契约**（[`CommitObserver`]），挂在构造器入参上 ——
//! 谁要观察提交，自己造 [`ObserverRegistry`] 并留着那个 `Arc`。

pub mod entries;
pub mod publish;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use nous_chord::context::Context;
use nous_chord::watch::WatchEnd;

use crate::storage::commit::{StoredBatch, StoredChange, StoredDocumentChange};
use crate::storage::writes::Refusal;
use crate::types::EntryRecord;
use crate::{DocumentPoint, Id, Seq, Storage, StorageError, StorageWrite, StoredDocument};

pub use entries::EntryKind;
pub use publish::{
    CommitChange, CommitObservation, CommitObserver, CommitPublication, DocumentCommitChange,
    ObserverRegistry,
};

/// 会话怎么收的场。上游 `SessionEnd`。
///
/// `failed` 的 `error` 上游是 `unknown`；这里落成可读文本 —— 它不过线上（`closed()` 是进程内的
/// settle 点），所以不假装 `Error` 有对象形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SessionEnd {
    Closed,
    Failed { error: String },
}

/// 这次提交什么时候落库。上游 `CommitPersistence`。
///
/// **本票只支持 [`Immediate`](CommitPersistence::Immediate)**。`deferred` 不是「还没做」，是
/// **明确拒绝**（[`SessionError::DeferredUnsupported`]）：上游的延后提交只对「丢了也无所谓」的
/// 流式进度成立（崩溃就丢），静默按 immediate 存会让调用方以为自己拿到了延后语义 —— 那正是
/// 崩溃丢数据的那类差别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CommitPersistence {
    #[default]
    Immediate,
    Deferred {
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "withinMs")]
        within_ms: Option<u64>,
    },
}

/// 一次提交的选项。上游 `CommitOptions`。默认 [`CommitPersistence::Immediate`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CommitOptions {
    #[serde(default)]
    pub persistence: CommitPersistence,
}

/// 一次提交的结果。`stored` 是落库的批数，`refused` 是这一段被拒的原因，
/// `publication` 只在真落库时才有（拒绝不回滚，但这一段没写就没有可发布的事实）。
#[derive(Debug, Clone, PartialEq)]
pub struct CommitOutcome {
    pub stored: usize,
    pub refused: Option<Refusal>,
    pub publication: Option<CommitPublication>,
}

/// 会话层的失败。**不是**拒绝（拒绝是 [`CommitOutcome::refused`]）：能到这里的都是「这次调用
/// 没法继续」，而且存储层的失败会把会话自己关掉（上游同一语义）。
#[derive(Debug)]
pub enum SessionError {
    /// 会话已经关了（`close()` 之后，或在它排队期间关的）。不 panic、不挂住，就是这条错误。
    Closed,
    /// `deferred` 落库本库还不支持。
    DeferredUnsupported,
    /// 存储层失败。**存储失败会把会话关掉**（`SessionEnd::Failed`），之后任何调用都拿到 `Closed`。
    Storage(StorageError),
    /// 存进去的记录读不回我们自己的类型 —— 只可能是记录形状和类型定义分叉了。
    Shape(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Closed => f.write_str("session is closed"),
            SessionError::DeferredUnsupported => {
                f.write_str("deferred commit persistence is not supported yet")
            }
            SessionError::Storage(error) => write!(f, "{error}"),
            SessionError::Shape(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StorageError> for SessionError {
    fn from(error: StorageError) -> Self {
        SessionError::Storage(error)
    }
}

/// 一条会话的公开面：提交、关、等它关、读一份文档、读一条 entry。
///
/// ## 为什么不是「一次调用一个回调」
///
/// 上游 `commit((tx) => ...)` 把事务句柄交给调用方。本库的 `12b` 已经把事务封在
/// [`Storage::commit`] 里，再开一层就是**两处管事务** —— 所以这里只收一批写
/// （[`StorageWrite`]），不暴露 `Tx`、也不暴露 `Storage`。
///
/// ## `close()` 的三条语义
///
/// 1. 之后的 [`commit`](Self::commit) 拿到 [`SessionError::Closed`]：明确错误，不 panic、不挂住；
/// 2. **幂等**：第二次 `close()` 返回 `Ok`（不是「已经关过」的错误）；
/// 3. **在飞的提交走完再关**：提交与关共用一把异步锁，先拿到锁的提交整个跑完，`close()` 才拿到锁。
pub struct Session {
    /// `None` = 关了。关的时候存储被**消费掉**（[`Storage::close`] 消费自己），于是不会留一个
    /// 「已关但还能用」的状态。
    storage: tokio::sync::Mutex<Option<Storage>>,
    observers: Arc<ObserverRegistry>,
    /// settle 点。写进去之后不再改动（先写再 `notify`）。
    end: tokio::sync::Mutex<Option<SessionEnd>>,
    settled: tokio::sync::Notify,
}

impl Session {
    /// 打开一份库并起一条会话（观察者名单现造一个）。
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, SessionError> {
        Ok(Self::with_observers(
            Storage::open(path)?,
            ObserverRegistry::new(),
        ))
    }

    /// 用现成的存储与观察者名单。观察者名单要由调用方留着 —— `Session` 不提供「给我名单」的
    /// 访问器，因为公开面就是五个方法，而挂观察者是构造期的事。
    pub fn with_observers(storage: Storage, observers: Arc<ObserverRegistry>) -> Self {
        Self {
            storage: tokio::sync::Mutex::new(Some(storage)),
            observers,
            end: tokio::sync::Mutex::new(None),
            settled: tokio::sync::Notify::new(),
        }
    }

    /// 提交一批写。落库成功后**先**把 publication 交给观察者，**再**返回 —— 「先落库后可见」。
    pub async fn commit(
        &self,
        batch: Vec<StorageWrite>,
        options: CommitOptions,
    ) -> Result<CommitOutcome, SessionError> {
        if !matches!(options.persistence, CommitPersistence::Immediate) {
            return Err(SessionError::DeferredUnsupported);
        }
        let mut guard = self.storage.lock().await;
        if guard.is_none() {
            return Err(SessionError::Closed);
        }
        let committed = {
            let storage = guard.as_mut().expect("checked above");
            storage.commit(&[batch], |_, _| {})
        };
        let result = match committed {
            Ok(result) => result,
            Err(error) => {
                // 存储失败 → 会话自己关掉（上游同一语义）。先放掉锁再收场，否则 `settle` 会撞锁。
                let storage = guard.take();
                drop(guard);
                if let Some(storage) = storage {
                    let _ = storage.close();
                }
                self.end_all(&WatchEnd::Failed {
                    error: error.to_string(),
                });
                self.settle(SessionEnd::Failed {
                    error: error.to_string(),
                })
                .await;
                return Err(SessionError::Storage(error));
            }
        };
        let outcome = match result.batches.first() {
            None => CommitOutcome {
                stored: result.stored,
                refused: result.refused,
                publication: None,
            },
            Some(batch) => {
                let publication = publication_of(batch)?;
                // 在释放锁之前发布：提交是串行的， 发布的顺序就是落库的顺序。
                self.observers.publish(&publication, &Context::new());
                CommitOutcome {
                    stored: result.stored,
                    refused: result.refused,
                    publication: Some(publication),
                }
            }
        };
        drop(guard);
        Ok(outcome)
    }

    /// 关会话：密封入场、等**在飞的提交走完**、然后关存储。幂等。
    pub async fn close(&self) -> Result<(), SessionError> {
        let mut guard = self.storage.lock().await;
        let Some(storage) = guard.take() else {
            // 已经关过（或已经失败收场）：第二次 `close()` 返回 `Ok`。
            return Ok(());
        };
        drop(guard);
        match storage.close() {
            Ok(()) => {
                self.end_all(&WatchEnd::Ended);
                self.settle(SessionEnd::Closed).await;
                Ok(())
            }
            Err(error) => {
                self.end_all(&WatchEnd::Failed {
                    error: error.to_string(),
                });
                self.settle(SessionEnd::Failed {
                    error: error.to_string(),
                })
                .await;
                Err(SessionError::Storage(error))
            }
        }
    }

    /// 等它关。**一个 settle 点**，不是 watch 通道：`SessionEnd` 只出现一次。
    pub async fn closed(&self) -> SessionEnd {
        loop {
            // 先登记再查：`notify_waiters` 只叫已经等着的，漏掉这一下就会永远等下去。
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(end) = self.end.lock().await.clone() {
                return end;
            }
            notified.await;
        }
    }

    /// 一份文档的当前值（`materialize(Current)`）。退役的、不存在的都是 `None`。
    pub async fn snapshot(&self, id: Id) -> Result<Option<StoredDocument>, SessionError> {
        let guard = self.storage.lock().await;
        let storage = guard.as_ref().ok_or(SessionError::Closed)?;
        Ok(storage.materialize(id, DocumentPoint::Current)?)
    }

    /// 一条 entry 与它落库时的 `seq`。
    pub async fn get_entry(&self, id: Id) -> Result<Option<(EntryRecord, Seq)>, SessionError> {
        let guard = self.storage.lock().await;
        let storage = guard.as_ref().ok_or(SessionError::Closed)?;
        let Some((record, seq)) = storage.entry(id)? else {
            return Ok(None);
        };
        let record: EntryRecord = serde_json::from_value(record)
            .map_err(|error| SessionError::Shape(format!("entry {id}: {error}")))?;
        Ok(Some((record, seq)))
    }

    fn end_all(&self, end: &WatchEnd) {
        self.observers.end_all(end);
    }

    /// 写 settle 点并叫醒在等的人。**只写一次**：最先到的那个定调。
    async fn settle(&self, end: SessionEnd) {
        let mut slot = self.end.lock().await;
        if slot.is_some() {
            return;
        }
        *slot = Some(end);
        drop(slot);
        self.settled.notify_waiters();
    }
}

/// 把「存进去的样子」翻成类型的 publication。
///
/// 记录是**从库里回来的**（不是提交时那份副本）：`document` 的 `createdAt` / `retiredAt` 是提交
/// 盖的章，delta 的 `value` 是重放出来的 —— 拿提交时的副本会漏掉这两样。
fn publication_of(batch: &StoredBatch) -> Result<CommitPublication, SessionError> {
    let mut changes = Vec::with_capacity(batch.changes.len());
    for change in &batch.changes {
        changes.push(match change {
            StoredChange::Conversation(record) => CommitChange::Conversation(typed(record, "conversation")?),
            StoredChange::Entry(record) => CommitChange::Entry(typed(record, "entry")?),
            StoredChange::Task(record) => CommitChange::Task(typed(record, "task")?),
            StoredChange::Submission(record) => CommitChange::Submission(typed(record, "submission")?),
            StoredChange::Document(change) => CommitChange::Document(match change {
                StoredDocumentChange::Change {
                    record,
                    conversation_id,
                    version,
                    value,
                    ops,
                } => DocumentCommitChange::Change {
                    record: typed(record, "document")?,
                    conversation_id: *conversation_id,
                    version: *version,
                    value: value.clone(),
                    ops: ops.clone(),
                },
                StoredDocumentChange::Retire {
                    record,
                    conversation_id,
                } => DocumentCommitChange::Retire {
                    record: typed(record, "document")?,
                    conversation_id: *conversation_id,
                },
                StoredDocumentChange::Copy {
                    record,
                    conversation_id,
                    source,
                } => DocumentCommitChange::Copy {
                    record: typed(record, "document")?,
                    conversation_id: *conversation_id,
                    source: source.clone(),
                },
            }),
        });
    }
    Ok(CommitPublication::new(batch.seq, changes))
}

fn typed<T: serde::de::DeserializeOwned>(
    record: &serde_json::Value,
    what: &str,
) -> Result<T, SessionError> {
    serde_json::from_value(record.clone()).map_err(|error| {
        SessionError::Shape(format!("stored {what} record does not fit its type: {error}"))
    })
}
