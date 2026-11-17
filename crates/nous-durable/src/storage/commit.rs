//! 一次 `commit()` 的判定与落盘（上游 `storage/sqlite/commit.ts`，486 行）。
//!
//! ## 一条调用、几条语句，不管里面几批
//!
//! 上游的注释值得抄一遍：检查要读的状态**一次读全**（[`CommitState::read`]，三条语句），之后
//! 每一批只在内存里对着「前面几批留下的状态」检查（[`CommitState::check`]），通过的批按表攒成
//! 净效果（[`PendingWrites`]），再写出去。远端 SQLite 上每条语句一次往返，而写锁一直被握着 ——
//! 所以「一条写一次往返」是不能接受的成本。
//!
//! **每批 flush 一次**（不是整个调用一次）：每批的 [`StoredBatch`] 要说出「这份文档这一批之后
//! 的值」，而 delta 的值 = base + 到这一批为止的重放 —— 不 flush 就得在内存里再维护一份文档值，
//! 那就是第二份真相。`Session` 的常态是一次调用一批，语句数与「整个调用 flush 一次」完全相同；
//! 只有一次调用多批时多几条语句。
//!
//! ## 检查在前、写在后的原因
//!
//! 上游不能用 savepoint（Durable Objects 不给），所以**拒绝不是回滚**：第 N 批不合法时，前 N-1
//! 批照常提交。这就是「半写回滚」那条判据的边界 —— 数据库自己的失败才整批回滚（`COMMIT`
//! 之前的任何 `Err` 都让整个事务消失），而**拒绝只是这一段没写**。
//!
//! ## 净效果
//!
//! 同一批里同一份 task / submission 写两次，落库只写一次（后写的算数）；只读态（`history:
//! "latest"`）的文档在 base 更新或退役时，它之前的 revision 直接丢掉。

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};
use nous_chord::sqlite::{SqliteExecutor, SqliteValue as Cell};

use super::cursor::ScanOrder;
use super::sqlite::rows::{
    address_columns_key, address_key, address_parts, document_row, is_current_only, parse_json,
    required_i64,
};
use super::sqlite::schema::SqliteSchema;
use super::sqlite::statements::{insert_rows, read_rows, read_rows_parts, run_for_each};
use super::sqlite::tables::{
    conversation_row, entry_row, record_id_row, revision_row, submission_row, task_row,
    CONVERSATION_COLUMNS, ENTRY_COLUMNS, RECORD_ID_COLUMNS, SUBMISSION_COLUMNS, TASK_COLUMNS,
    DOCUMENT_COLUMNS, DOCUMENT_REVISION_COLUMNS,
};
use super::writes::{
    check_documents, check_ids, prepare_document_actions, CopySource, DocumentAction,
    DocumentContent, Id, Refusal, Seq, StorageWrite, TableName,
};
use crate::error::{Result, StorageError};

/// 一次提交的结果。`stored` 是真正落库的批数，`refused` 是第一段被拒的原因，
/// `batches` 是**存进去的样子**（被拒的那些不在里面）。
#[derive(Debug, Clone, PartialEq)]
pub struct CommitResult {
    pub stored: usize,
    pub refused: Option<Refusal>,
    pub batches: Vec<StoredBatch>,
}

/// 一批落库的写，按「存进去的样子」给出。**不是** publication（那是 `session` 的类型）：
/// 这里只有 JSON 与表名，`session` 负责翻成类型的 `CommitChange`。
///
/// 分开的理由：`storage` 不认识 `DocumentRecord` / `EntryRecord` 这些类型（`docs/storage.md`
/// §12.5：记录内容对存储层是不透明 JSON）。让存储层直接产出 publication 会把「记录的语义」搬进
/// 「记录怎么存」那一层。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredBatch {
    pub seq: Seq,
    pub changes: Vec<StoredChange>,
}

/// 一条存进去的改动。顺序与这批里写的顺序一致；文档按 id 升序排在表的记录之后。
#[derive(Debug, Clone, PartialEq)]
pub enum StoredChange {
    Conversation(Value),
    Entry(Value),
    Task(Value),
    Submission(Value),
    Document(StoredDocumentChange),
}

/// 文档这一批发生了什么。对应上游 `DocumentCommitChange` 的三个分支。
#[derive(Debug, Clone, PartialEq)]
pub enum StoredDocumentChange {
    /// 新建或改内容。`ops` 是**这一批交上来的那批**：创建 / base 更新是空数组，delta 就是那批 op。
    Change {
        record: Value,
        /// 拥有它的会话。session 作用域的文档是 `None`。
        conversation_id: Option<Id>,
        version: i64,
        /// 这一批之后的值（delta 按 base + 重放算出来，不是把 ops 原样端出去）。
        value: Value,
        ops: Value,
    },
    /// 这一批把它退役了（记录里已带 `retiredAt`）。
    Retire {
        record: Value,
        conversation_id: Option<Id>,
    },
    /// 这一批造了一份复制。`conversationId` 必填。
    Copy {
        record: Value,
        conversation_id: Id,
        source: CopySource,
    },
}

/// 一次调用里所有批会碰到的状态，**开始前读一次**，之后每一批在内存里推进。
#[derive(Debug, Default)]
pub struct CommitState {
    /// 每个被占用的 id 属于哪张表。缺席 = 没人占。
    record_ids: HashMap<Id, TableName>,
    /// 每份被碰到的文档的记录。缺席 = 没这份文档。
    documents: HashMap<Id, Value>,
    /// 每份被 delta 改的文档最新一条 revision 的版本号。缺席 = 一条 revision 都没有。
    versions: HashMap<Id, i64>,
    /// 每个被创建地址的当前在世化身（键是 `address_key`）。`None` = 没有在世化身。
    current: HashMap<String, Option<Id>>,
}

impl CommitState {
    /// 读 `batches` 的检查要用的状态：四类状态各一条语句（地址那条一次 join）。
    pub fn read(
        executor: &dyn SqliteExecutor,
        schema: &SqliteSchema,
        batches: &[Vec<StorageWrite>],
    ) -> Result<Self> {
        let mut ids: Vec<Cell> = Vec::new();
        let mut document_ids: Vec<Cell> = Vec::new();
        let mut delta_ids: Vec<Cell> = Vec::new();
        let mut addresses: BTreeMap<String, [Value; 5]> = BTreeMap::new();
        for writes in batches {
            for write in writes {
                if let Some((id, _)) = write.claim()? {
                    ids.push(id.into());
                }
                match write {
                    StorageWrite::DocumentCreate { id, record, .. } => {
                        document_ids.push((*id).into());
                        let parts = address_parts(record)?;
                        addresses.insert(
                            address_key(record)?,
                            [
                                Value::String(parts.kind.clone()),
                                Value::String(parts.scope_kind.to_string()),
                                parts.owner_id.into(),
                                parts.family.into(),
                                Value::String(parts.key_value.clone()),
                            ],
                        );
                    }
                    StorageWrite::DocumentChange { id, content } => {
                        document_ids.push((*id).into());
                        if matches!(content, DocumentContent::Delta { .. }) {
                            delta_ids.push((*id).into());
                        }
                    }
                    StorageWrite::DocumentRetire { id } => document_ids.push((*id).into()),
                    _ => {}
                }
            }
        }

        let mut record_ids = HashMap::new();
        for row in read_rows(
            executor,
            &format!(
                "SELECT id, record_type FROM {} WHERE id IN (SELECT value FROM json_each(?))",
                schema.table("record_ids")
            ),
            &ids,
        )? {
            record_ids.insert(integer(&row, "id")?, table_name(text(&row, "record_type")?)?);
        }

        let mut documents = HashMap::new();
        if !document_ids.is_empty() {
            for row in read_rows(
                executor,
                &format!(
                    "SELECT record FROM {} WHERE id IN (SELECT value FROM json_each(?))",
                    schema.table("documents")
                ),
                &document_ids,
            )? {
                let record: Value = parse_json(text(&row, "record")?)?;
                documents.insert(required_i64(&record, "id")?, record);
            }
        }

        let mut versions = HashMap::new();
        if !delta_ids.is_empty() {
            // 每个被点名的 id 各一次主键降序探测。
            for row in read_rows(
                executor,
                &format!(
                    "SELECT requested.value AS document_id, (
                        SELECT version FROM {revisions} WHERE document_id = requested.value
                        ORDER BY seq DESC LIMIT 1
                    ) AS version FROM json_each(?) AS requested",
                    revisions = schema.table("document_revisions")
                ),
                &delta_ids,
            )? {
                if let Some(version) = row.get("version").and_then(Cell::as_i64) {
                    versions.insert(integer(&row, "document_id")?, version);
                }
            }
        }

        let current = read_current(executor, schema, &addresses)?;
        Ok(Self {
            record_ids,
            documents,
            versions,
            current,
        })
    }

    /// Durable 自己的检查（不需要源文档，所以在这里）：先 id，再文档。
    pub fn check(
        &self,
        writes: &[StorageWrite],
        actions: &HashMap<Id, DocumentAction>,
    ) -> Result<Option<Refusal>> {
        if let Some(refusal) = check_ids(writes, &self.record_ids)? {
            return Ok(Some(refusal));
        }
        if let Some(refusal) = check_documents(actions, &self.documents, &self.versions, &self.current)? {
            return Ok(Some(refusal));
        }
        Ok(None)
    }

    /// 一批被接受、存成 `seq` 之后，把状态推进过去。
    pub fn apply(
        &mut self,
        writes: &[StorageWrite],
        actions: &HashMap<Id, DocumentAction>,
        seq: Seq,
    ) -> Result<()> {
        for write in writes {
            // 被接受的批为它「落在哪张表」占用 id（`check` 已经确认没有冲突）。
            if let Some((id, table)) = write.claim()? {
                self.record_ids.insert(id, table);
            }
        }
        for (id, action) in actions {
            if let Some(create) = &action.create {
                // 上游在这里盖章：`createdAt` 一定是这次提交的 seq，退役也用它。
                let mut record = create.clone();
                stamp(&mut record, "createdAt", seq)?;
                if action.retire {
                    stamp(&mut record, "retiredAt", seq)?;
                }
                self.documents.insert(*id, record.clone());
                if !action.retire {
                    self.current.insert(address_key(&record)?, Some(*id));
                }
            } else if action.retire {
                let mut record = self
                    .documents
                    .get(id)
                    .cloned()
                    .ok_or(StorageError::Shape(format!("document {id} vanished")))?;
                stamp(&mut record, "retiredAt", seq)?;
                self.documents.insert(*id, record.clone());
                // 这一批可能已经建了同一地址的下一个化身：只让这一份离开。
                let key = address_key(&record)?;
                if self.current.get(&key).copied().flatten() == Some(*id) {
                    self.current.insert(key, None);
                }
            }
            // 退役文档的版本不会再被读：之后任何动作都会先被拒绝。
            if let Some(content) = &action.content {
                self.versions.insert(*id, content.version());
            }
        }
        Ok(())
    }

    /// 一批被接受之后，文档的记录（已盖章）。
    pub fn document(&self, id: Id) -> Option<&Value> {
        self.documents.get(&id)
    }
}

/// 每个地址当前在世的化身。上游 `readCurrent`。
fn read_current(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    addresses: &BTreeMap<String, [Value; 5]>,
) -> Result<HashMap<String, Option<Id>>> {
    let mut current: HashMap<String, Option<Id>> = addresses
        .keys()
        .map(|key| (key.clone(), None))
        .collect();
    if addresses.is_empty() {
        return Ok(current);
    }
    let parts: Vec<String> = addresses
        .values()
        .map(|parts| serde_json::to_string(&parts).map_err(StorageError::from))
        .collect::<Result<_>>()?;
    let rows = read_rows_parts(
        executor,
        &format!(
            "SELECT document.id, document.kind, document.scope_kind, document.owner_id,
                document.family, document.key_value
            FROM json_each(?) AS address JOIN {documents} AS document
            ON document.kind = json_extract(address.value, '$[0]')
            AND document.scope_kind = json_extract(address.value, '$[1]')
            AND document.owner_id = json_extract(address.value, '$[2]')
            AND document.family = json_extract(address.value, '$[3]')
            AND document.key_value = json_extract(address.value, '$[4]')
            WHERE document.retired_at IS NULL",
            documents = schema.table("documents")
        ),
        parts,
    )?;
    for row in rows {
        let key = address_columns_key(
            text(&row, "kind")?,
            text(&row, "scope_kind")?,
            integer(&row, "owner_id")?,
            integer(&row, "family")?,
            text(&row, "key_value")?,
        );
        current.insert(key, Some(integer(&row, "id")?));
    }
    Ok(current)
}

/// 通过检查的那些批**还没写出去**的净效果，按表攒好。
#[derive(Debug, Default)]
pub struct PendingWrites {
    record_ids: HashMap<Id, TableName>,
    conversations: Vec<Vec<Cell>>,
    entries: Vec<Vec<Cell>>,
    /// task / submission 会被同一批写多次：只留最后一次。
    tasks: HashMap<Id, Vec<Cell>>,
    submissions: HashMap<Id, Vec<Cell>>,
    /// 被创建或退役的文档的记录。
    documents: HashMap<Id, Value>,
    /// 库里的 revision 要整份删掉的文档（只读态的 base 更新或退役）。
    cleared: HashSet<Id>,
    /// 每份文档的新 revision，按加入顺序。
    revisions: Vec<Vec<Cell>>,
}

impl PendingWrites {
    /// 加入一批（`state` 已经推进过它）。
    pub fn add(
        &mut self,
        writes: &[StorageWrite],
        actions: &HashMap<Id, DocumentAction>,
        seq: Seq,
        state: &CommitState,
    ) -> Result<()> {
        for write in writes {
            self.add_write(write, seq)?;
        }
        for (id, action) in actions {
            let record = state
                .document(*id)
                .ok_or_else(|| StorageError::Shape(format!("document {id} has no record")))?;
            self.add_document(*id, action, seq, record)?;
        }
        Ok(())
    }

    fn add_write(&mut self, write: &StorageWrite, seq: Seq) -> Result<()> {
        if let Some((id, table)) = write.claim()? {
            self.record_ids.insert(id, table);
        }
        match write {
            StorageWrite::Conversation(record) => self.conversations.push(conversation_row(record)?),
            StorageWrite::Entry(record) => self.entries.push(entry_row(record, seq)?),
            StorageWrite::Task(record) => {
                self.tasks.insert(required_i64(record, "id")?, task_row(record)?);
            }
            StorageWrite::Submission(record) => {
                self.submissions
                    .insert(required_i64(record, "id")?, submission_row(record)?);
            }
            _ => {}
        }
        Ok(())
    }

    fn add_document(
        &mut self,
        id: Id,
        action: &DocumentAction,
        seq: Seq,
        record: &Value,
    ) -> Result<()> {
        if action.create.is_some() || action.retire {
            self.documents.insert(id, record.clone());
        }
        if let Some(content) = &action.content {
            // 只读态的文档换了 base：之前的 revision 不再需要（也不该留在库里）。
            if matches!(content, DocumentContent::Base { .. }) && is_current_only(record) {
                self.clear(id);
            }
            // base 存整份值，delta 存操作数组 —— 与上游 `content.kind === "base" ? value : ops` 一致。
            let stored = match content {
                DocumentContent::Base { value, .. } => value,
                DocumentContent::Delta { ops, .. } => ops,
            };
            self.revisions
                .push(revision_row(id, seq, content.kind(), content.version(), stored));
        }
        if action.retire && is_current_only(record) {
            self.clear(id);
        }
        Ok(())
    }

    fn clear(&mut self, id: Id) {
        self.cleared.insert(id);
        self.revisions.retain(|row| row[0] != Cell::Integer(id));
    }

    /// 每张表几条语句（[`insert_rows`]），写完就忘掉。
    pub fn flush(&mut self, executor: &dyn SqliteExecutor, schema: &SqliteSchema) -> Result<()> {
        if !self.record_ids.is_empty() {
            let rows: Vec<Vec<Cell>> = self
                .record_ids
                .iter()
                .map(|(id, table)| record_id_row(*id, table.as_str()))
                .collect();
            insert_rows(
                executor,
                &schema.table("record_ids"),
                RECORD_ID_COLUMNS,
                &rows,
                "ON CONFLICT(id) DO NOTHING",
            )?;
        }
        if !self.conversations.is_empty() {
            insert_rows(
                executor,
                &schema.table("conversations"),
                CONVERSATION_COLUMNS,
                &self.conversations,
                "",
            )?;
        }
        if !self.entries.is_empty() {
            insert_rows(executor, &schema.table("entries"), ENTRY_COLUMNS, &self.entries, "")?;
        }
        if !self.tasks.is_empty() {
            let rows: Vec<Vec<Cell>> = self.tasks.values().cloned().collect();
            insert_rows(
                executor,
                &schema.table("tasks"),
                TASK_COLUMNS,
                &rows,
                "ON CONFLICT(id) DO UPDATE SET conversation_id = excluded.conversation_id,
                    kind = excluded.kind, status = excluded.status,
                    abort_requested = excluded.abort_requested, background = excluded.background,
                    record = excluded.record",
            )?;
        }
        if !self.submissions.is_empty() {
            let rows: Vec<Vec<Cell>> = self.submissions.values().cloned().collect();
            insert_rows(
                executor,
                &schema.table("submissions"),
                SUBMISSION_COLUMNS,
                &rows,
                "ON CONFLICT(id) DO UPDATE SET conversation_id = excluded.conversation_id,
                    request_id = excluded.request_id, status = excluded.status,
                    record = excluded.record",
            )?;
        }
        if !self.documents.is_empty() {
            // 新建的插入、已存的那份退役：一个 upsert 写两种情况。
            let rows: Vec<Vec<Cell>> = self
                .documents
                .values()
                .map(document_row)
                .collect::<Result<_>>()?;
            insert_rows(
                executor,
                &schema.table("documents"),
                DOCUMENT_COLUMNS,
                &rows,
                "ON CONFLICT(id) DO UPDATE SET retired_at = excluded.retired_at, record = excluded.record",
            )?;
        }
        if !self.cleared.is_empty() {
            let ids: Vec<Cell> = self.cleared.iter().map(|id| Cell::Integer(*id)).collect();
            run_for_each(
                executor,
                &format!(
                    "DELETE FROM {} WHERE document_id IN (SELECT value FROM json_each(?))",
                    schema.table("document_revisions")
                ),
                &ids,
            )?;
        }
        if !self.revisions.is_empty() {
            insert_rows(
                executor,
                &schema.table("document_revisions"),
                DOCUMENT_REVISION_COLUMNS,
                &self.revisions,
                "",
            )?;
        }
        self.record_ids.clear();
        self.conversations.clear();
        self.entries.clear();
        self.tasks.clear();
        self.submissions.clear();
        self.documents.clear();
        self.cleared.clear();
        self.revisions.clear();
        Ok(())
    }
}

/// 这一批**存进去的样子**（[`StoredBatch`]）。调用前这一批必须已经 flush：delta 的值要从库里读回来。
///
/// 顺序：表记录按写的顺序，文档按 id 升序（`actions` 是 `HashMap`，不排序就等于让发布顺序随机）。
pub(crate) fn stored_batch(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    writes: &[StorageWrite],
    actions: &HashMap<Id, DocumentAction>,
    seq: Seq,
    state: &CommitState,
    values: &HashMap<Id, (i64, Value)>,
) -> Result<StoredBatch> {
    let mut changes: Vec<StoredChange> = Vec::new();
    for write in writes {
        changes.push(match write {
            StorageWrite::Conversation(record) => StoredChange::Conversation(record.clone()),
            StorageWrite::Entry(record) => StoredChange::Entry(record.clone()),
            StorageWrite::Task(record) => StoredChange::Task(record.clone()),
            StorageWrite::Submission(record) => StoredChange::Submission(record.clone()),
            _ => continue,
        });
    }
    let mut ids: Vec<Id> = actions.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let action = &actions[&id];
        let record = state
            .document(id)
            .cloned()
            .ok_or_else(|| StorageError::Shape(format!("document {id} has no record")))?;
        let change = if let Some(copy) = &action.copy {
            StoredDocumentChange::Copy {
                conversation_id: publishing_conversation(executor, schema, &record)?
                    .ok_or_else(|| {
                        StorageError::Shape(format!(
                            "document {id} copy has no owning conversation"
                        ))
                    })?,
                record,
                source: copy.clone(),
            }
        } else if action.retire {
            // 退役只发退役：这一批同时改了内容也不发内容（上游 observeCommits 的同一句话）。
            StoredDocumentChange::Retire {
                conversation_id: publishing_conversation(executor, schema, &record)?,
                record,
            }
        } else if let Some(content) = &action.content {
            let (version, value, ops) = match content {
                DocumentContent::Base { version, value } => {
                    (*version, value.clone(), Value::Array(Vec::new()))
                }
                DocumentContent::Delta { version, ops } => {
                    // 这批之后的值由 `values_after_batch` 算好（缓存 + 本批 ops，或重放一次）：
                    // 这里不再自己读库，也不在持锁区间里重放整条链（`#25` O(n²) 的根）。
                    let value = values
                        .get(&id)
                        .map(|(_, value)| value.clone())
                        .ok_or_else(|| {
                            StorageError::Shape(format!("document {id} has no value for this batch"))
                        })?;
                    (*version, value, ops.clone())
                }
            };
            StoredDocumentChange::Change {
                conversation_id: publishing_conversation(executor, schema, &record)?,
                record,
                version,
                value,
                ops,
            }
        } else {
            continue;
        };
        changes.push(StoredChange::Document(change));
    }
    Ok(StoredBatch { seq, changes })
}

/// 这份文档归哪条会话：session 作用域 → `None`；conversation → 自己的 id；task → **问那条 task**。
///
/// 上游也这么推（`conversationId` 的注释：「task documents derive it from their task record」）。
/// task 记录不在库里就报错 —— 悄悄给 `None` 会让 publication 看起来像 session 文档。
fn publishing_conversation(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    record: &Value,
) -> Result<Option<Id>> {
    let scope = record
        .get("scope")
        .ok_or_else(|| StorageError::Shape("document record has no scope".to_string()))?;
    match scope.get("kind").and_then(Value::as_str) {
        Some("session") => Ok(None),
        Some("conversation") => Ok(Some(required_i64(scope, "conversationId")?)),
        Some("task") => {
            let task_id = required_i64(scope, "taskId")?;
            let row = executor.get(
                &format!(
                    "SELECT conversation_id FROM {} WHERE id = ?",
                    schema.table("tasks")
                ),
                &[Cell::Integer(task_id)],
            )?;
            let row = row.ok_or_else(|| {
                StorageError::Shape(format!(
                    "document of task {task_id} has no task record to derive its conversation"
                ))
            })?;
            Ok(Some(integer(&row, "conversation_id")?))
        }
        other => Err(StorageError::Shape(format!(
            "document scope kind must be session/conversation/task, got {other:?}"
        ))),
    }
}

/// 把一批里的复制解成 base：读源文档在 `source.at` 上的值（上游 `resolveCopies`）。
///
/// 顺序照抄：**先 flush 前面已接受的批**，源才读得到 —— 一次调用里的多条 fork 于是每条一次读，
/// 语句数按 fork 数走，不按批数走。
///
/// 失败分两类：数据库自己的失败（[`StorageError::Sqlite`]）原样往上抛（整次回滚）；其余
/// （源不存在 / 不在世 / 没留历史 / 形状对不上 / delta 解不出来）是**拒绝**，不抛。
pub(crate) fn resolve_copies(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    actions: &mut HashMap<Id, DocumentAction>,
) -> Result<Option<Refusal>> {
    let copies: Vec<(Id, CopySource, Value)> = actions
        .iter()
        .filter_map(|(id, action)| {
            let copy = action.copy?;
            let create = action.create.clone()?;
            Some((*id, copy, create))
        })
        .collect();
    for (id, source, create) in copies {
        let stored = match super::documents::materialize(executor, schema, source.id, source.at) {
            Ok(stored) => stored,
            Err(error @ StorageError::Sqlite(_)) => return Err(error),
            Err(error) => {
                return Ok(Some(Refusal::CopyRejected {
                    document_id: id,
                    reason: error.to_string(),
                }))
            }
        };
        let Some(stored) = stored else {
            return Ok(Some(Refusal::CopyRejected {
                document_id: id,
                reason: format!("fork source document {} cannot be read", source.id),
            }));
        };
        // 源与目标必须是「同一类文档的另一个 id」：换 scope 就不是复制，是另造一份。
        let matches = stored.record.get("scope").and_then(|scope| scope.get("kind")).and_then(Value::as_str)
            == Some("conversation")
            && create.get("scope").and_then(|scope| scope.get("kind")).and_then(Value::as_str)
                == Some("conversation")
            && ["kind", "key", "history", "fork"].iter().all(|field| {
                stored.record.get(*field).unwrap_or(&Value::Null)
                    == create.get(*field).unwrap_or(&Value::Null)
            });
        if !matches {
            return Ok(Some(Refusal::CopyRejected {
                document_id: id,
                reason: format!(
                    "fork source document {} does not match the copied record",
                    source.id
                ),
            }));
        }
        if let Some(action) = actions.get_mut(&id) {
            action.copy = None;
            action.content = Some(DocumentContent::Base {
                version: stored.version,
                value: stored.value,
            });
        }
    }
    Ok(None)
}

/// 预先算好一批写会碰到的文档动作（`prepareDocumentActions` 的可失败版）。
pub fn prepare(writes: &[StorageWrite]) -> Result<std::result::Result<HashMap<Id, DocumentAction>, Refusal>> {
    match prepare_document_actions(writes) {
        Ok(actions) => Ok(Ok(actions)),
        Err(refusal) => Ok(Err(refusal)),
    }
}

/// 一条会话扫描的过滤条件，和「从哪儿开始」一起。
#[derive(Debug, Clone, Default)]
pub struct ConversationQuery {
    pub owner_conversation_id: Option<Id>,
    pub owner_task_id: Option<Id>,
    pub order: Option<ScanOrder>,
}

fn stamp(record: &mut Value, key: &str, seq: Seq) -> Result<()> {
    let object: &mut Map<String, Value> = record
        .as_object_mut()
        .ok_or_else(|| StorageError::Shape("document record is not an object".to_string()))?;
    object.insert(key.to_string(), seq.into());
    Ok(())
}

fn table_name(text: &str) -> Result<TableName> {
    Ok(match text {
        "conversation" => TableName::Conversation,
        "entry" => TableName::Entry,
        "task" => TableName::Task,
        "submission" => TableName::Submission,
        "document" => TableName::Document,
        other => {
            return Err(StorageError::Shape(format!(
                "record_ids.record_type has an unknown value {other:?}"
            )))
        }
    })
}

fn integer(row: &nous_chord::sqlite::SqliteRow, column: &str) -> Result<i64> {
    row.get(column)
        .and_then(Cell::as_i64)
        .ok_or_else(|| StorageError::Shape(format!("column {column} is not an integer")))
}

fn text<'a>(row: &'a nous_chord::sqlite::SqliteRow, column: &str) -> Result<&'a str> {
    row.get(column)
        .and_then(Cell::as_str)
        .ok_or_else(|| StorageError::Shape(format!("column {column} is not text")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::migrations::migrations;
    use nous_chord::sqlite::{apply_sqlite_migrations, LocalDatabase};

    fn fresh() -> LocalDatabase {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        let schema = SqliteSchema::unprefixed();
        apply_sqlite_migrations(&mut db, &migrations(&schema), schema.table("durable_schema")).unwrap();
        db
    }

    #[test]
    fn one_statement_reads_the_state_of_every_kind_the_batches_touch() {
        let db = fresh();
        let schema = SqliteSchema::unprefixed();
        let batches = vec![vec![
            StorageWrite::Conversation(serde_json::json!({ "id": 2 })),
            StorageWrite::Entry(serde_json::json!({ "id": 3, "conversationId": 2, "kind": "pi.user" })),
            StorageWrite::DocumentCreate {
                id: 4,
                record: serde_json::json!({
                    "id": 4, "kind": "pi.agent", "scope": { "kind": "session" },
                    "history": "latest", "fork": "current"
                }),
                content: DocumentContent::Base {
                    version: 1,
                    value: serde_json::json!({}),
                },
            },
        ]];
        let state = CommitState::read(&db, &schema, &batches).unwrap();
        assert!(state.record_ids.is_empty());
        assert!(state.documents.is_empty());
        assert!(state.versions.is_empty());
        // 地址读到了：库里没有 → `None`，但键在（否则 `check_documents` 会当成「没读过」）。
        assert_eq!(state.current.len(), 1);
        assert_eq!(state.current.values().next().unwrap(), &None);
    }

    #[test]
    fn a_current_only_document_keeps_one_revision_and_a_rewindable_one_keeps_them_all() {
        let db = fresh();
        let schema = SqliteSchema::unprefixed();
        let latest = serde_json::json!({
            "id": 4, "kind": "pi.agent", "scope": { "kind": "session" },
            "history": "latest", "fork": "current", "createdAt": 1
        });
        // `history: "rewindable"` 只对 conversation 作用域有意义（`is_current_only` 的规矩）。
        let rewindable = serde_json::json!({
            "id": 5, "kind": "pi.agent",
            "scope": { "kind": "conversation", "conversationId": 7 },
            "history": "rewindable", "fork": "asOf", "createdAt": 1
        });
        let mut pending = PendingWrites::default();
        let base = DocumentContent::Base {
            version: 1,
            value: serde_json::json!({ "a": 1 }),
        };
        let next = DocumentContent::Base {
            version: 2,
            value: serde_json::json!({ "a": 2 }),
        };
        for (id, record) in [(4, latest), (5, rewindable)] {
            let mut state = CommitState::default();
            state.documents.insert(id, record.clone());
            let actions = HashMap::from([(
                id,
                DocumentAction {
                    create: None,
                    copy: None,
                    content: Some(base.clone()),
                    retire: false,
                },
            )]);
            pending.add(&[], &actions, 1, &state).unwrap();
            let actions = HashMap::from([(
                id,
                DocumentAction {
                    create: None,
                    copy: None,
                    content: Some(next.clone()),
                    retire: false,
                },
            )]);
            pending.add(&[], &actions, 2, &state).unwrap();
        }
        pending.flush(&db, &schema).unwrap();
        let rows = db
            .all(
                "SELECT document_id, COUNT(*) AS n FROM document_revisions GROUP BY document_id ORDER BY document_id",
                &[],
            )
            .unwrap();
        // 只读态只留最新一条 base；可回放的两条都在。
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("n").unwrap().as_i64(), Some(1));
        assert_eq!(rows[1].get("n").unwrap().as_i64(), Some(2));
    }
}
