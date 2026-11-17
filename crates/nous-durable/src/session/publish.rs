//! 一次提交「**存了什么**」的事实，以及谁在旁边看。上游 `types.ts` 的 `CommitPublication` /
//! `CommitChange` / `DocumentCommitChange` / `CommitObserver` / `CommitObservation`。
//!
//! ## publication 不是 SSE，也不许被 SSE 塑形
//!
//! publication 是提交的**事实**：这次事务存了哪几条记录、哪份文档改了、改成什么。SSE 帧是
//! 「客户端看到什么」的**投影**，那是 `nous-server` 的事（#22）。两者分家的三条理由，写在这里
//! 免得下次又有人想「顺手按事件表裁一裁」：
//!
//! 1. 投影里有一半东西 publication 里本来就没有：`snapshot` 帧、`message_start`、
//!    `message_update.changes[]` —— 那些是生成态（harness）的产物，不是 commit 的产物。按投影
//!    倒推，会逼着存储层长出「客户端概念」。
//! 2. `document` 变更带的 `value` + `ops` 是 live projection（`harness/partial-changes`）的原料，
//!    裁掉它，#14 得回头把 diff 重算一遍。
//! 3. 存储层不该知道传输。`#22` 能薄，是因为它只做 `publication → 帧`，不参与「存了什么」。
//!
//! ## 形状照上游，一个字不裁
//!
//! ```text
//! CommitPublication = { seq, changes[] }
//! CommitChange = {type:"conversation"|"entry"|"task"|"submission", value}
//!              | DocumentCommitChange
//! DocumentCommitChange = {type:"document", kind:"change", record, conversationId?, version, value, ops[]}
//!                      | {type:"document", kind:"retire", record, conversationId?}
//!                      | {type:"document.copy", record, conversationId, source}
//! ```
//!
//! `conversationId` 在 **session 作用域**的文档上**缺席**（上游是 `ConversationId | undefined`），
//! 写成 `null` 就不是同一个形状了。`copy` 的那个是必填的（上游没有 `| undefined`）。
//!
//! ## 比上游严：多一个键就是错
//!
//! 上游这些对象是类型，运行时多一个键没人管。本库反序列化时**逐键对账**，多余的键报错
//! （`docs/storage.md` §12.6 的同一条规矩）。理由：这个类型的唯一用途就是给别的层读，静默丢掉
//! 一个别人写进来的键，等于让两份实现悄悄分叉。
//!
//! ## 观察者：同步、不重入
//!
//! 上游 `CommitObserver.advance(publication, context)` 是**同步**回调：不许 await、不许回调
//! Session、不许抛。Rust 里前两条由签名钉住（`fn advance(&self, …)` 不是 `async fn`）。
//! 第三条做不到 —— Rust 的观察者没有「抛错」这个通道，`panic` 会直接冒到 `commit` 的调用方。
//! 我们**不**用 `catch_unwind` 兜住：把库里的 bug 变成「事件静默丢失」比崩掉更难查。于是
//! [`WatchEnd::ListenerError`] 只在真有错误通道那天才会有值，现在没有观察者能走到它。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde::de::DeserializeOwned;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::types::{
    ConversationRecord, DocumentRecord, EntryRecord, SubmissionRecord, TaskRecord,
};
use crate::{CopySource, Id, Seq};

use nous_chord::context::Context;
use nous_chord::watch::WatchEnd;

/// 一次提交存下的全部改动，加它自己的 `seq`。上游 `CommitPublication`。
///
/// 改动顺序上游没规定（`Change order is unspecified`），别在测试里断言顺序之外的东西。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitPublication {
    pub seq: Seq,
    pub changes: Vec<CommitChange>,
}

impl CommitPublication {
    pub fn new(seq: Seq, changes: Vec<CommitChange>) -> Self {
        Self { seq, changes }
    }
}

/// 一条改动。上游 `CommitChange` = 四种表记录 + 文档。
#[derive(Debug, Clone, PartialEq)]
pub enum CommitChange {
    Conversation(ConversationRecord),
    Entry(EntryRecord),
    Task(TaskRecord),
    Submission(SubmissionRecord),
    Document(DocumentCommitChange),
}

/// 文档改动的三种收法。上游 `DocumentCommitChange`。
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentCommitChange {
    /// 新建或改内容。`ops` 是**这一次采纳的原始批次**：创建时是空数组，普通更新时正好是提交
    /// 上来的那批 op —— 不是重算的 diff。
    Change {
        record: DocumentRecord,
        /// 拥有这份文档的会话；**只有 session 作用域的文档**是 `None`（序列化后键缺席）。
        conversation_id: Option<Id>,
        /// `value` 的文档定义版本。
        version: i64,
        /// 采纳后的完整值。
        value: Value,
        /// 采纳的那批 op（chord 线上形状，`["s", ["a"], 1]` 这样）。
        ops: Value,
    },
    /// 这次提交把这份化身退役了。
    Retire {
        record: DocumentRecord,
        conversation_id: Option<Id>,
    },
    /// 子初始化：定义无关，内容等 watch 去补水。`conversationId` 必填。
    Copy {
        record: DocumentRecord,
        conversation_id: Id,
        source: CopySource,
    },
}

impl DocumentCommitChange {
    /// 这份文档归哪条会话。session 作用域的文档没有。
    pub fn conversation_id(&self) -> Option<Id> {
        match self {
            DocumentCommitChange::Change {
                conversation_id, ..
            }
            | DocumentCommitChange::Retire {
                conversation_id, ..
            } => *conversation_id,
            DocumentCommitChange::Copy {
                conversation_id, ..
            } => Some(*conversation_id),
        }
    }
}

/// 逐键对账：`required` 全在、`optional` 可以有、其余一律报错。返回这份对象（借用）。
fn expect_keys<'a>(
    value: &'a Value,
    required: &[&str],
    optional: &[&str],
    what: &str,
) -> Result<&'a Map<String, Value>, String> {
    let map = value
        .as_object()
        .ok_or_else(|| format!("{what} must be an object"))?;
    for key in map.keys() {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(format!("{what} has unexpected key {key:?}"));
        }
    }
    for key in required {
        if !map.contains_key(*key) {
            return Err(format!("{what} is missing key {key:?}"));
        }
    }
    Ok(map)
}

fn typed<T: DeserializeOwned>(map: &Map<String, Value>, key: &str, what: &str) -> Result<T, String> {
    let value = map
        .get(key)
        .ok_or_else(|| format!("{what} is missing key {key:?}"))?;
    serde_json::from_value(value.clone()).map_err(|error| format!("{what}.{key}: {error}"))
}

fn optional_id(map: &Map<String, Value>, key: &str, what: &str) -> Result<Option<Id>, String> {
    match map.get(key) {
        None => Ok(None),
        // 上游是 `T | undefined`；`null` 不是同一个东西，那是「有键但没有值」。
        Some(Value::Null) => Err(format!("{what}.{key} must be absent, not null")),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{what}.{key} must be an integer id")),
        Some(other) => Err(format!("{what}.{key} must be an integer id, got {other}")),
    }
}

impl Serialize for DocumentCommitChange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = Map::new();
        match self {
            DocumentCommitChange::Change {
                record,
                conversation_id,
                version,
                value,
                ops,
            } => {
                map.insert("type".into(), Value::from("document"));
                map.insert("kind".into(), Value::from("change"));
                map.insert("record".into(), serde_json::to_value(record).map_err(serde::ser::Error::custom)?);
                if let Some(id) = conversation_id {
                    map.insert("conversationId".into(), Value::from(*id));
                }
                map.insert("version".into(), Value::from(*version));
                map.insert("value".into(), value.clone());
                map.insert("ops".into(), ops.clone());
            }
            DocumentCommitChange::Retire {
                record,
                conversation_id,
            } => {
                map.insert("type".into(), Value::from("document"));
                map.insert("kind".into(), Value::from("retire"));
                map.insert("record".into(), serde_json::to_value(record).map_err(serde::ser::Error::custom)?);
                if let Some(id) = conversation_id {
                    map.insert("conversationId".into(), Value::from(*id));
                }
            }
            DocumentCommitChange::Copy {
                record,
                conversation_id,
                source,
            } => {
                map.insert("type".into(), Value::from("document.copy"));
                map.insert("record".into(), serde_json::to_value(record).map_err(serde::ser::Error::custom)?);
                map.insert("conversationId".into(), Value::from(*conversation_id));
                map.insert("source".into(), serde_json::to_value(source).map_err(serde::ser::Error::custom)?);
            }
        }
        let mut serializer = serializer.serialize_map(Some(map.len()))?;
        for (key, value) in map {
            serializer.serialize_entry(&key, &value)?;
        }
        serializer.end()
    }
}

impl<'de> Deserialize<'de> for DocumentCommitChange {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        DocumentCommitChange::from_value(&value).map_err(serde::de::Error::custom)
    }
}

impl DocumentCommitChange {
    fn from_value(value: &Value) -> Result<Self, String> {
        let tag = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "document change is missing key \"type\"".to_string())?;
        match tag {
            "document" => {
                let kind = value
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "document change is missing key \"kind\"".to_string())?;
                match kind {
                    "change" => {
                        let map = expect_keys(
                            value,
                            &["type", "kind", "record", "version", "value", "ops"],
                            &["conversationId"],
                            "document change",
                        )?;
                        let record: DocumentRecord = typed(map, "record", "document change")?;
                        DocumentRecord::validate(record.scope, record.history, record.fork)?;
                        Ok(DocumentCommitChange::Change {
                            record,
                            conversation_id: optional_id(map, "conversationId", "document change")?,
                            version: typed(map, "version", "document change")?,
                            value: typed(map, "value", "document change")?,
                            ops: typed(map, "ops", "document change")?,
                        })
                    }
                    "retire" => {
                        let map = expect_keys(
                            value,
                            &["type", "kind", "record"],
                            &["conversationId"],
                            "document retire",
                        )?;
                        let record: DocumentRecord = typed(map, "record", "document retire")?;
                        DocumentRecord::validate(record.scope, record.history, record.fork)?;
                        Ok(DocumentCommitChange::Retire {
                            record,
                            conversation_id: optional_id(map, "conversationId", "document retire")?,
                        })
                    }
                    other => Err(format!("unknown document kind {other:?}")),
                }
            }
            "document.copy" => {
                let map = expect_keys(
                    value,
                    &["type", "record", "conversationId", "source"],
                    &[],
                    "document copy",
                )?;
                let record: DocumentRecord = typed(map, "record", "document copy")?;
                DocumentRecord::validate(record.scope, record.history, record.fork)?;
                Ok(DocumentCommitChange::Copy {
                    record,
                    conversation_id: typed(map, "conversationId", "document copy")?,
                    source: typed(map, "source", "document copy")?,
                })
            }
            other => Err(format!("unknown document change type {other:?}")),
        }
    }
}

impl Serialize for CommitChange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CommitChange::Document(change) => change.serialize(serializer),
            other => {
                let (tag, value) = match other {
                    CommitChange::Conversation(record) => {
                        ("conversation", serde_json::to_value(record).map_err(serde::ser::Error::custom)?)
                    }
                    CommitChange::Entry(record) => ("entry", serde_json::to_value(record).map_err(serde::ser::Error::custom)?),
                    CommitChange::Task(record) => ("task", serde_json::to_value(record).map_err(serde::ser::Error::custom)?),
                    CommitChange::Submission(record) => ("submission", serde_json::to_value(record).map_err(serde::ser::Error::custom)?),
                    CommitChange::Document(_) => unreachable!("document handled above"),
                };
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", tag)?;
                map.serialize_entry("value", &value)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for CommitChange {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        CommitChange::from_value(&value).map_err(serde::de::Error::custom)
    }
}

impl CommitChange {
    fn from_value(value: &Value) -> Result<Self, String> {
        let tag = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "commit change is missing key \"type\"".to_string())?;
        match tag {
            "document" | "document.copy" => Ok(CommitChange::Document(
                DocumentCommitChange::from_value(value)?,
            )),
            "conversation" | "entry" | "task" | "submission" => {
                let map = expect_keys(value, &["type", "value"], &[], "commit change")?;
                Ok(match tag {
                    "conversation" => CommitChange::Conversation(typed(map, "value", "commit change")?),
                    "entry" => CommitChange::Entry(typed(map, "value", "commit change")?),
                    "task" => CommitChange::Task(typed(map, "value", "commit change")?),
                    _ => CommitChange::Submission(typed(map, "value", "commit change")?),
                })
            }
            other => Err(format!("unknown commit change type {other:?}")),
        }
    }
}

/// 观察已存提交的回调。上游 `CommitObserver`。
///
/// `advance` 每次提交调一次（拿到整份 publication），`end` 只调一次：Session 关闭
/// （`failed` 带 "session closed"）或 Session 失败（带那个错）。`detach()` 之后两个都不再调。
pub trait CommitObserver: Send + Sync {
    fn advance(&self, publication: &CommitPublication, context: &Context);
    fn end(&self, end: &WatchEnd);
}

/// 一个已挂上的观察者：怎么摘。上游 `CommitObservation`。`detach()` 幂等。
#[derive(Clone, Debug)]
pub struct CommitObservation {
    registry: Weak<ObserverRegistry>,
    id: u64,
    detached: Arc<AtomicBool>,
}

impl CommitObservation {
    /// 摘掉这个观察者。之后再 `advance` / `end` 都不会到它手上。重复调没事。
    pub fn detach(&self) {
        self.detached.store(true, Ordering::SeqCst);
        if let Some(registry) = self.registry.upgrade() {
            registry.remove(self.id);
        }
    }

    /// 还没摘吗。
    pub fn is_attached(&self) -> bool {
        !self.detached.load(Ordering::SeqCst)
    }
}

struct Slot {
    id: u64,
    observer: Arc<dyn CommitObserver>,
}

#[derive(Default)]
struct RegistryState {
    next_id: u64,
    slots: Vec<Slot>,
}

/// 观察者名单。Session 持一份；`publish` / `end_all` 由 Session 在提交的关键点上调。
///
/// 名字叫 registry 而不是 session：「谁在看」是提交发布的一部分，不是会话状态机的一部分，
/// 单独拎出来才能在 #13 的 commit ② 里不依赖尚未存在的 `Session` 就测完。
#[derive(Default)]
pub struct ObserverRegistry {
    state: Mutex<RegistryState>,
}

impl ObserverRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 挂一个观察者。返回的句柄负责摘。
    pub fn attach(self: &Arc<Self>, observer: Arc<dyn CommitObserver>) -> CommitObservation {
        let id = {
            let mut state = self.lock();
            let id = state.next_id;
            state.next_id += 1;
            state.slots.push(Slot { id, observer });
            id
        };
        CommitObservation {
            registry: Arc::downgrade(self),
            id,
            detached: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 把这份 publication 交给每个还挂着的观察者。
    ///
    /// 先快照再逐个调：观察者自己的回调里 `detach()` 是合法的（上游也这么用），不能让持锁的
    /// 迭代碰到重入。已经摘掉的不再收到。
    pub fn publish(&self, publication: &CommitPublication, context: &Context) {
        let snapshot: Vec<Arc<dyn CommitObserver>> = {
            let state = self.lock();
            state.slots.iter().map(|slot| slot.observer.clone()).collect()
        };
        for observer in snapshot {
            observer.advance(publication, context);
        }
    }

    /// 收尾：每个还挂着的观察者收一次 `end`，然后名单清空（不会收到第二次）。
    pub fn end_all(&self, end: &WatchEnd) {
        let slots = {
            let mut state = self.lock();
            std::mem::take(&mut state.slots)
        };
        for slot in slots {
            slot.observer.end(end);
        }
    }

    /// 还挂着几个。
    pub fn len(&self) -> usize {
        self.lock().slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn remove(&self, id: u64) {
        let mut state = self.lock();
        state.slots.retain(|slot| slot.id != id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        // 反序列化与观察者回调都不持这份锁（publish 先快照再调用），所以中毒只可能是别处
        // panic 时撞上了；那种情况下继续用旧数据比二次 panic 更容易定位。
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
