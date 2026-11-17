//! 一次提交里的写，和「这一批合法吗」的判定（上游 `storage/sqlite/commit.ts` 的 `claimOf` /
//! `prepareDocumentActions` 与 `CommitState.#checkIds` / `#checkDocuments`）。
//!
//! ## 记录是 JSON，索引列从记录里取
//!
//! `docs/storage.md` §4 定的：`record` 列原样存一条记录的 JSON，`WHERE` / `ORDER BY` / 联结用
//! 得到的那几列单独拆出来。所以这里的写只拿 `serde_json::Value`，列由
//! `sqlite::tables::*_row` 从同一个值里**重算** —— 与导入器用同一套函数，两边不可能漂。
//!
//! ## 拒绝 vs 失败
//!
//! [`Refusal`] 是「这一批不合法」：调用方的问题，不写、不抛，随 [`commit`](super::storage)
//! 的结果一起回去（上游 `CommitResult.refused`）。数据库自己的失败（约束、IO、抢锁）是
//! [`StorageError`](crate::StorageError)，整批回滚。
//!
//! ## `document.copy`（12c）
//!
//! 复制 = 「把源文档在 `source.at` 上的值 materialize 出来，当成新文档的第一条 base」。
//! 于是它同时是 [创建](StorageWrite::DocumentCopy) 和 [内容](DocumentContent::Base)，
//! 但内容要**在事务里读源文档之后**才确定 —— 见 [`DocumentAction::copy`] 与
//! `commit::resolve_copies`。

use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

use super::sqlite::rows::{address_key, required_i64};

pub type Id = i64;
pub type Seq = i64;

/// 一条记录属于哪张表，`record_ids.record_type` 用的就是这几个词。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableName {
    Conversation,
    Entry,
    Task,
    Submission,
    Document,
}

impl TableName {
    pub fn as_str(self) -> &'static str {
        match self {
            TableName::Conversation => "conversation",
            TableName::Entry => "entry",
            TableName::Task => "task",
            TableName::Submission => "submission",
            TableName::Document => "document",
        }
    }
}

impl fmt::Display for TableName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一条记录的新内容：整份 base 值，或者相对上一版的一批 delta 操作。
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentContent {
    Base { version: i64, value: Value },
    Delta { version: i64, ops: Value },
}

impl DocumentContent {
    pub fn version(&self) -> i64 {
        match self {
            DocumentContent::Base { version, .. } | DocumentContent::Delta { version, .. } => *version,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            DocumentContent::Base { .. } => "base",
            DocumentContent::Delta { .. } => "delta",
        }
    }

    pub fn encoded(&self) -> String {
        match self {
            DocumentContent::Base { value, .. } => super::sqlite::rows::encode_json(value),
            DocumentContent::Delta { ops, .. } => super::sqlite::rows::encode_json(ops),
        }
    }
}

/// 一次提交里的一条写。上游 `StorageWrite`，去掉 `document.copy`（见模块注释）。
#[derive(Debug, Clone, PartialEq)]
pub enum StorageWrite {
    Conversation(Value),
    Entry(Value),
    Task(Value),
    Submission(Value),
    /// 新造一份文档。`record` 里**没有** `createdAt` / `retiredAt`，由提交盖章。
    DocumentCreate {
        id: Id,
        record: Value,
        content: DocumentContent,
    },
    /// 新造一份文档，值来自另一份文档在 `source.at` 上的 materialize 结果。
    DocumentCopy {
        id: Id,
        record: Value,
        source: CopySource,
    },
    DocumentChange { id: Id, content: DocumentContent },
    DocumentRetire { id: Id },
}

impl StorageWrite {
    /// 这条写会占用哪个 id、记在哪张表下。不占 id 的写返回 `None`。
    pub fn claim(&self) -> crate::error::Result<Option<(Id, TableName)>> {
        Ok(match self {
            StorageWrite::Conversation(record) => {
                Some((required_i64(record, "id")?, TableName::Conversation))
            }
            StorageWrite::Entry(record) => Some((required_i64(record, "id")?, TableName::Entry)),
            StorageWrite::Task(record) => Some((required_i64(record, "id")?, TableName::Task)),
            StorageWrite::Submission(record) => {
                Some((required_i64(record, "id")?, TableName::Submission))
            }
            StorageWrite::DocumentCreate { id, .. } | StorageWrite::DocumentCopy { id, .. } => {
                Some((*id, TableName::Document))
            }
            StorageWrite::DocumentChange { .. } | StorageWrite::DocumentRetire { .. } => None,
        })
    }
}

/// 复制的源：哪份文档、哪个点。上游 `document.copy` 的 `source`（`{id, at}`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CopySource {
    pub id: Id,
    pub at: super::documents::DocumentPoint,
}

/// 一批写对一份文档做了什么：创建（含内容）、复制（内容待定）、改内容、退役。
/// 同一份文档在一批里最多各一次。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DocumentAction {
    pub create: Option<Value>,
    /// 复制还没解出来的源；解开之后 `content` 会变成一条 base。
    pub copy: Option<CopySource>,
    pub content: Option<DocumentContent>,
    pub retire: bool,
}

impl DocumentAction {
    /// 这一批要不要在事务里读数据库（只有复制要）。
    pub fn needs_copy(&self) -> bool {
        self.copy.is_some()
    }
}

/// 把一批写按文档归拢。上游 `prepareDocumentActions`：同一份文档两条内容命令、或退役两次 ——
/// 都是拒绝（调用方写错了，不是数据库的问题）。
pub fn prepare_document_actions(writes: &[StorageWrite]) -> Result<HashMap<Id, DocumentAction>, Refusal> {
    let mut actions: HashMap<Id, DocumentAction> = HashMap::new();
    for write in writes {
        let id = match write {
            StorageWrite::DocumentCreate { id, .. } | StorageWrite::DocumentCopy { id, .. } => *id,
            StorageWrite::DocumentChange { id, .. } | StorageWrite::DocumentRetire { id } => *id,
            _ => continue,
        };
        let action = actions.entry(id).or_default();
        match write {
            StorageWrite::DocumentCreate { record, content, .. } => {
                if action.create.is_some() || action.content.is_some() || action.copy.is_some() {
                    return Err(Refusal::DuplicateContent { document_id: id });
                }
                action.create = Some(record.clone());
                action.content = Some(content.clone());
            }
            StorageWrite::DocumentCopy { record, source, .. } => {
                if action.create.is_some() || action.content.is_some() || action.copy.is_some() {
                    return Err(Refusal::DuplicateContent { document_id: id });
                }
                action.create = Some(record.clone());
                action.copy = Some(*source);
            }
            StorageWrite::DocumentChange { content, .. } => {
                if action.content.is_some() || action.copy.is_some() {
                    return Err(Refusal::DuplicateContent { document_id: id });
                }
                action.content = Some(content.clone());
            }
            StorageWrite::DocumentRetire { .. } => {
                if action.retire {
                    return Err(Refusal::DuplicateRetire { document_id: id });
                }
                action.retire = true;
            }
            _ => unreachable!("已经过滤过"),
        }
    }
    Ok(actions)
}

/// 「这一批不合法」。每条都说得清是哪条写、为什么 —— 上游是一句 `Error` 文本，我们给变体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// id 已经被别的记录占了。
    IdAlreadyBelongs { id: Id, table: TableName },
    /// 同一批里同一个 id 写了两遍（会话 / entry / 文档是后写覆盖前的天敌）。
    IdWrittenTwice { id: Id },
    /// 同一批里同一个 id 被当成两种记录写（task 与 submission 可以多写，但不许串类型）。
    IdTwoTypes { id: Id },
    /// 同一份文档在一批里给了两条内容命令。
    DuplicateContent { document_id: Id },
    /// 同一份文档在一批里退役了两次。
    DuplicateRetire { document_id: Id },
    /// 改一份不存在的文档。
    UnknownDocument { document_id: Id },
    /// 建一份已经存在的文档。
    DocumentExists { document_id: Id },
    /// 动一份已经退役的文档。
    DocumentRetired { document_id: Id },
    /// delta 找不到底座（这份文档一条 revision 都没有）。
    DeltaWithoutBase { document_id: Id },
    /// delta 的版本接不上最新一条 revision。
    DeltaVersionMismatch { document_id: Id, previous: i64, wanted: i64 },
    /// 复制的源文档在同一批里也被改了 —— 那这一批的复制没有确定的基准。
    CopySourceChanged { document_id: Id },
    /// 复制的源读不出来（不存在、不在世、没留历史）或形状对不上。
    CopyRejected { document_id: Id, reason: String },
    /// 同一个地址（kind + scope + key）已经有在世的化身了。
    AddressAlreadyCurrent { address: String },
    /// 游标不是本库编出来的。
    InvalidCursor,
    /// 游标接着的是另一个方向。
    CursorOrderMismatch,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::IdAlreadyBelongs { id, table } => {
                write!(f, "ID {id} already belongs to {table}")
            }
            Refusal::IdWrittenTwice { id } => write!(f, "ID {id} is written more than once"),
            Refusal::IdTwoTypes { id } => write!(f, "ID {id} is written as two record types"),
            Refusal::DuplicateContent { document_id } => {
                write!(f, "Document {document_id} has more than one content command")
            }
            Refusal::DuplicateRetire { document_id } => {
                write!(f, "Document {document_id} is retired more than once")
            }
            Refusal::UnknownDocument { document_id } => write!(f, "Unknown document: {document_id}"),
            Refusal::DocumentExists { document_id } => write!(f, "Document {document_id} already exists"),
            Refusal::DocumentRetired { document_id } => write!(f, "Document {document_id} is retired"),
            Refusal::DeltaWithoutBase { document_id } => {
                write!(f, "Document {document_id} delta has no base")
            }
            Refusal::DeltaVersionMismatch { document_id, previous, wanted } => write!(
                f,
                "Document {document_id} version transition requires a base ({previous} -> {wanted})"
            ),
            Refusal::CopySourceChanged { document_id } => write!(
                f,
                "Document copy {document_id} source is changed in the copy batch"
            ),
            Refusal::CopyRejected { document_id, reason } => {
                write!(f, "Document copy {document_id} was rejected: {reason}")
            }
            Refusal::AddressAlreadyCurrent { address } => {
                write!(f, "Document address already has a current incarnation: {address}")
            }
            Refusal::InvalidCursor => f.write_str("Invalid storage cursor"),
            Refusal::CursorOrderMismatch => {
                f.write_str("The cursor continues a scan in another order")
            }
        }
    }
}

impl std::error::Error for Refusal {}

/// 一批写对一份文档的检查，加上「state 里已有的东西」。上游 `CommitState.#checkIds` /
/// `#checkDocuments` 的判定顺序照抄：先 id，再文档。
///
/// 返回 `Ok(None)` 放行、`Ok(Some(_))` 拒绝、`Err(_)` 是数据库或记录形状的失败（不是拒绝）。
pub(crate) fn check_ids(
    writes: &[StorageWrite],
    existing: &HashMap<Id, TableName>,
) -> crate::error::Result<Option<Refusal>> {
    let mut claimed: HashMap<Id, TableName> = HashMap::new();
    for write in writes {
        let Some((id, table)) = write.claim()? else {
            continue;
        };
        let earlier = claimed.get(&id).copied();
        match table {
            // 这几张表的 id 是「一条记录一个 id」，重复就是错。
            TableName::Conversation | TableName::Entry | TableName::Document => {
                if let Some(owner) = existing.get(&id) {
                    return Ok(Some(Refusal::IdAlreadyBelongs { id, table: *owner }));
                }
                if earlier.is_some() {
                    return Ok(Some(Refusal::IdWrittenTwice { id }));
                }
            }
            // task / submission 可以被同一批写多次（状态推进），但类型不许串。
            TableName::Task | TableName::Submission => {
                if let Some(owner) = existing.get(&id) {
                    if *owner != table {
                        return Ok(Some(Refusal::IdAlreadyBelongs { id, table: *owner }));
                    }
                }
                if let Some(earlier) = earlier {
                    if earlier != table {
                        return Ok(Some(Refusal::IdTwoTypes { id }));
                    }
                }
            }
        }
        claimed.insert(id, table);
    }
    Ok(None)
}

/// 文档检查。`existing` 是库里的记录，`versions` 是每份文档最新一条 revision 的版本号，
/// `current` 是每个地址当前在世的化身 id（键是 `address_key`）。
pub(crate) fn check_documents(
    actions: &HashMap<Id, DocumentAction>,
    existing: &HashMap<Id, Value>,
    versions: &HashMap<Id, i64>,
    current: &HashMap<String, Option<Id>>,
) -> crate::error::Result<Option<Refusal>> {
    let mut live: HashMap<String, i64> = HashMap::new();
    for (id, action) in actions {
        // 复制在同一批里改自己的源：这一批的基准不确定，直接拒绝（上游先查这条）。
        if let Some(copy) = &action.copy {
            if actions.contains_key(&copy.id) {
                return Ok(Some(Refusal::CopySourceChanged { document_id: *id }));
            }
        }
        let stored = existing.get(id);
        if action.create.is_none() && stored.is_none() {
            return Ok(Some(Refusal::UnknownDocument { document_id: *id }));
        }
        if action.create.is_some() && stored.is_some() {
            return Ok(Some(Refusal::DocumentExists { document_id: *id }));
        }
        if let Some(retired_at) = stored.and_then(|record| record.get("retiredAt")) {
            if !retired_at.is_null() {
                return Ok(Some(Refusal::DocumentRetired { document_id: *id }));
            }
        }
        if let Some(DocumentContent::Delta { version, .. }) = &action.content {
            match versions.get(id) {
                None => return Ok(Some(Refusal::DeltaWithoutBase { document_id: *id })),
                Some(previous) if previous != version => {
                    return Ok(Some(Refusal::DeltaVersionMismatch {
                        document_id: *id,
                        previous: *previous,
                        wanted: *version,
                    }))
                }
                Some(_) => {}
            }
        }
        // 普通的内容变更不改变地址的在世数量：只有创建与退役才参与计数。
        if action.create.is_none() && !action.retire {
            continue;
        }
        let record = action.create.as_ref().or(stored).expect("上面已经检查过");
        let key = address_key(record)?;
        let mut count = match live.get(&key) {
            Some(count) => *count,
            // 只读过被创建的地址：库里已有的在世家化这里取初值。
            None => {
                if current.get(&key).copied().flatten().is_some() {
                    1
                } else {
                    0
                }
            }
        };
        if action.retire && stored.is_some() {
            count -= 1;
        }
        if action.create.is_some() && !action.retire {
            count += 1;
        }
        live.insert(key, count);
    }
    for key in live.keys() {
        if live[key] > 1 {
            return Ok(Some(Refusal::AddressAlreadyCurrent {
                address: key.clone(),
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conversation(id: i64) -> StorageWrite {
        StorageWrite::Conversation(json!({ "id": id }))
    }

    #[test]
    fn a_batch_may_not_write_the_same_conversation_twice() {
        let writes = vec![conversation(2), conversation(2)];
        assert_eq!(check_ids(&writes, &HashMap::new()), Ok(Some(Refusal::IdWrittenTwice { id: 2 })));
    }

    #[test]
    fn an_id_that_belongs_to_another_table_is_refused_with_that_table() {
        let writes = vec![conversation(2)];
        let existing = HashMap::from([(2, TableName::Entry)]);
        assert_eq!(
            check_ids(&writes, &existing),
            Ok(Some(Refusal::IdAlreadyBelongs { id: 2, table: TableName::Entry }))
        );
    }

    #[test]
    fn a_task_may_be_written_twice_but_not_as_a_submission() {
        let task = |status: &str| StorageWrite::Task(json!({ "id": 9, "state": { "status": status } }));
        assert_eq!(check_ids(&[task("running"), task("terminal")], &HashMap::new()), Ok(None));
        assert_eq!(
            check_ids(
                &[task("running"), StorageWrite::Submission(json!({ "id": 9 }))],
                &HashMap::new()
            ),
            Ok(Some(Refusal::IdTwoTypes { id: 9 }))
        );
    }

    #[test]
    fn one_document_may_not_get_two_content_commands_or_two_retirements() {
        let content = DocumentContent::Base {
            version: 1,
            value: json!({}),
        };
        let writes = vec![
            StorageWrite::DocumentCreate {
                id: 5,
                record: json!({ "id": 5 }),
                content: content.clone(),
            },
            StorageWrite::DocumentChange { id: 5, content },
        ];
        assert_eq!(
            prepare_document_actions(&writes),
            Err(Refusal::DuplicateContent { document_id: 5 })
        );
        let writes = vec![
            StorageWrite::DocumentRetire { id: 5 },
            StorageWrite::DocumentRetire { id: 5 },
        ];
        assert_eq!(
            prepare_document_actions(&writes),
            Err(Refusal::DuplicateRetire { document_id: 5 })
        );
    }

    #[test]
    fn a_delta_must_continue_the_base_version_the_last_revision_left() {
        let record = json!({
            "id": 5, "kind": "pi.agent", "scope": { "kind": "session" },
            "history": "latest", "fork": "current", "createdAt": 1
        });
        let actions = HashMap::from([(
            5,
            DocumentAction {
                create: None,
                content: Some(DocumentContent::Delta {
                    version: 2,
                    ops: json!([]),
                }),
                copy: None,
                retire: false,
            },
        )]);
        let existing = HashMap::from([(5, record)]);
        // 没有 revision → 没有底座。
        assert_eq!(
            check_documents(&actions, &existing, &HashMap::new(), &HashMap::new()),
            Ok(Some(Refusal::DeltaWithoutBase { document_id: 5 }))
        );
        // 库里的 base 是 1、delta 说 2 → 拒绝（换版本必须先来一条 base）。
        assert_eq!(
            check_documents(&actions, &existing, &HashMap::from([(5, 1)]), &HashMap::new()),
            Ok(Some(Refusal::DeltaVersionMismatch { document_id: 5, previous: 1, wanted: 2 }))
        );
        // 版本对上（delta 接着 base 2）→ 放行。
        assert_eq!(
            check_documents(&actions, &existing, &HashMap::from([(5, 2)]), &HashMap::new()),
            Ok(None)
        );
    }

    #[test]
    fn a_second_live_incarnation_of_one_address_is_refused() {
        let record = |id: i64| {
            json!({
                "id": id, "kind": "pi.agent", "scope": { "kind": "session" },
                "history": "latest", "fork": "current", "createdAt": 1
            })
        };
        let actions = HashMap::from([(
            6,
            DocumentAction {
                create: Some(record(6)),
                content: None,
                copy: None,
                retire: false,
            },
        )]);
        let key = address_key(&record(5)).unwrap();
        let current = HashMap::from([(key, Some(5))]);
        assert_eq!(
            check_documents(&actions, &HashMap::new(), &HashMap::new(), &current),
            Ok(Some(Refusal::AddressAlreadyCurrent {
                address: address_key(&record(6)).unwrap()
            }))
        );
    }
}
