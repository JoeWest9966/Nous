//! 存储门面：提交、id 分配、会话与 entry 的扫描（上游 `storage/sqlite/storage.ts` 的 12b 部分）。
//!
//! ## 一次提交的四步
//!
//! 1. 在**一个** `BEGIN IMMEDIATE` 事务里读 `durable_metadata`（`next_id` / `next_seq`）；
//! 2. 校验 seq 单调：`next_seq` 必须大于库里已有的最大 seq（会话的 `commit_seq`、文档修订的
//!    `seq`）。倒退 = 明确失败，绝不「接着用一个旧号」（见 [`StorageError::SeqRegression`]）；
//! 3. 逐批检查（[`CommitState`]），通过的批盖 seq、攒进 [`PendingWrites`]；
//! 4. 一次写出去，**同一事务**里把 `durable_metadata.next_seq` / `next_id` 推上去。
//!
//! ## 回执在 `COMMIT` 之后
//!
//! `receive(index, seq)` 在事务提交**之后**才被调用 —— 上游同一条规矩：回执的语义是「已落盘」，
//! 在事务里回调会让调用方看到还没提交的东西。
//!
//! ## id 分配器
//!
//! [`Storage::mint_id`] 只在内存里推进，随**下一次**提交一起落盘（上游 `mintId` 就是这样）。
//! 于是「分配出去又没用」的 id 可能被再用一次 —— 但那个 id 从没离开过本进程，用它是安全的。
//! 一旦某个 id 进了 `record_ids`（哪怕记录后来被删），它**永远不会**再被分配：`next_id` 只会
//! 往前推，而且和 `record_ids` 的插入在同一个事务里。

use serde_json::Value;

use nous_chord::sqlite::{SqliteDatabase, SqliteExecutor, SqliteValue as Cell};

use super::commit::{resolve_copies, CommitResult, CommitState, ConversationQuery, PendingWrites};
use super::documents::{materialize, DocumentPoint, StoredDocument};
use super::cursor::{cut_page, scan_start, Cursor, Page, ScanOrder};
use super::sqlite::rows::{parse_json, required_i64};
use super::sqlite::schema::SqliteSchema;
use super::writes::{Refusal, Seq, StorageWrite};
use super::SqliteStore;
use crate::error::{Result, StorageError};

/// 一条 entry 扫描的过滤条件。
#[derive(Debug, Clone, Copy)]
pub struct EntryQuery {
    pub conversation_id: i64,
    pub min_entry_id: Option<i64>,
    pub max_entry_id: Option<i64>,
    pub order: Option<ScanOrder>,
}

/// 一份 nous 自己的库 + 提交语义。
pub struct Storage {
    store: SqliteStore,
    /// 内存里的下一个 id（提交时才落盘）。
    next_id: i64,
    /// 本次调用里已经取走的 id 的下界 —— 提交时 `next_id` 取它和库里的较大者。
    base_next_id: i64,
}

impl Storage {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let store = SqliteStore::open_writer(path)?;
        let (next_id, _) = store.metadata()?;
        let next_id = next_id.parse::<i64>().map_err(|_| {
            StorageError::Shape(format!("durable_metadata.next_id is not a number: {next_id:?}"))
        })?;
        Ok(Self {
            store,
            next_id,
            base_next_id: next_id,
        })
    }

    pub fn open_read_only(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let store = SqliteStore::open_read_only(path)?;
        let next_id = store
            .metadata()
            .ok()
            .and_then(|(next_id, _)| next_id.parse::<i64>().ok())
            .unwrap_or(1);
        Ok(Self {
            store,
            next_id,
            base_next_id: next_id,
        })
    }

    pub fn store(&self) -> &SqliteStore {
        &self.store
    }

    pub fn next_id(&self) -> i64 {
        self.next_id
    }

    /// 取一个 id。**不落盘**：跟着下一次 [`commit`](Self::commit) 一起推上去。
    pub fn mint_id(&mut self) -> Result<i64> {
        if self.next_id >= i64::MAX {
            return Err(StorageError::Shape("ID space is exhausted".to_string()));
        }
        let id = self.next_id;
        self.next_id += 1;
        Ok(id)
    }

    /// 一次提交若干批，每批盖自己的 seq。第一段被拒的批到此为止，**前面通过的照常提交**。
    pub fn commit<F>(&mut self, batches: &[Vec<StorageWrite>], mut receive: F) -> Result<CommitResult>
    where
        F: FnMut(usize, Seq),
    {
        let schema = self.store.schema().clone();
        let base_next_id = self.next_id;

        let (seqs, refused, candidate_next_id) = self.store.db_mut().transaction(
            |tx| -> Result<(Vec<Seq>, Option<Refusal>, i64)> {
                let (stored_next_id, mut next_seq) = read_metadata(tx, &schema)?;
                let floor = seq_floor(tx, &schema)?;
                // 负例二：seq 绝不倒退、绝不复用。
                if next_seq <= floor {
                    return Err(StorageError::SeqRegression { next_seq, floor });
                }
                let mut state = CommitState::read(tx, &schema, batches)?;
                let mut pending = PendingWrites::default();
                let mut seqs: Vec<Seq> = Vec::new();
                let mut refused: Option<Refusal> = None;
                let mut candidate_next_id = base_next_id.max(stored_next_id);
                for writes in batches {
                    let actions = match super::commit::prepare(writes)? {
                        Ok(actions) => actions,
                        Err(refusal) => {
                            refused = Some(refusal);
                            break;
                        }
                    };
                    if let Some(refusal) = state.check(writes, &actions)? {
                        refused = Some(refusal);
                        break;
                    }
                    let mut actions = actions;
                    // 复制要读源文档：先把前面已接受的批写下去，源才看得见（上游 `checkBatch`）。
                    if actions.values().any(|action| action.needs_copy()) {
                        pending.flush(tx, &schema)?;
                        if let Some(refusal) = resolve_copies(tx, &schema, &mut actions)? {
                            refused = Some(refusal);
                            break;
                        }
                    }
                    let seq = next_seq;
                    next_seq += 1;
                    state.apply(writes, &actions, seq)?;
                    pending.add(writes, &actions, seq, &state)?;
                    seqs.push(seq);
                    candidate_next_id = candidate_next_id.max(candidate_next_id_of(writes, base_next_id)?);
                }
                pending.flush(tx, &schema)?;
                if !seqs.is_empty() {
                    write_metadata(tx, &schema, candidate_next_id.max(stored_next_id), next_seq)?;
                }
                Ok((seqs, refused, candidate_next_id))
            },
        )?;

        self.base_next_id = self.base_next_id.max(candidate_next_id);
        self.next_id = self.next_id.max(candidate_next_id);
        // `COMMIT` 已经返回：现在叫回执，落盘的东西才是「已落盘」。
        for (index, seq) in seqs.iter().enumerate() {
            receive(index, *seq);
        }
        Ok(CommitResult {
            stored: seqs.len(),
            refused,
        })
    }

    /// 一条会话的记录。
    pub fn conversation(&self, id: i64) -> Result<Option<Value>> {
        let row = self.store.db().get(
            &format!("SELECT record FROM {} WHERE id = ?", self.schema().table("conversations")),
            &[Cell::Integer(id)],
        )?;
        row.as_ref().map(record_of).transpose()
    }

    /// 按 id 升/降序扫描会话，游标不透明。
    pub fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<std::result::Result<Page<Value>, Refusal>> {
        let (order, after) = match scan_start(query.order, cursor, ScanOrder::Ascending) {
            Ok(start) => start,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let mut clauses = vec![match order {
            ScanOrder::Ascending => "id > ?",
            ScanOrder::Descending => "id < ?",
        }
        .to_string()];
        let mut params: Vec<Cell> = vec![Cell::Integer(after.unwrap_or(match order {
            ScanOrder::Ascending => -1,
            ScanOrder::Descending => i64::MAX,
        }))];
        if let Some(owner) = query.owner_conversation_id {
            clauses.push("owner_conversation_id = ?".to_string());
            params.push(Cell::Integer(owner));
        }
        if let Some(owner) = query.owner_task_id {
            clauses.push("owner_task_id = ?".to_string());
            params.push(Cell::Integer(owner));
        }
        params.push(Cell::Integer(limit as i64 + 1));
        let rows = self.store.db().all(
            &format!(
                "SELECT record FROM {} WHERE {} ORDER BY id {} LIMIT ?",
                self.schema().table("conversations"),
                clauses.join(" AND "),
                order.direction()
            ),
            &params,
        )?;
        let records = rows
            .iter()
            .map(record_of)
            .collect::<Result<Vec<Value>>>()?;
        Ok(Ok(cut_page(records, limit, order, |record| {
            record.get("id").and_then(Value::as_i64).unwrap_or(0)
        })))
    }

    /// 一条 entry 与它的 `commit_seq`。
    pub fn entry(&self, id: i64) -> Result<Option<(Value, Seq)>> {
        let row = self.store.db().get(
            &format!(
                "SELECT record, commit_seq FROM {} WHERE id = ?",
                self.schema().table("entries")
            ),
            &[Cell::Integer(id)],
        )?;
        let Some(row) = row else { return Ok(None) };
        let record = parse_json(row.get("record").and_then(Cell::as_str).unwrap_or("null"))?;
        let seq = row
            .get("commit_seq")
            .and_then(Cell::as_i64)
            .ok_or_else(|| StorageError::Shape("entry has no commit_seq".to_string()))?;
        Ok(Some((record, seq)))
    }

    /// 一个会话的 entry，按 id 升/降序，游标不透明。
    pub fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<std::result::Result<Page<Value>, Refusal>> {
        let (order, after) = match scan_start(query.order, cursor, ScanOrder::Descending) {
            Ok(start) => start,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let mut clauses = vec!["conversation_id = ?".to_string()];
        let mut params = vec![Cell::Integer(query.conversation_id)];
        clauses.push(
            match order {
                ScanOrder::Ascending => "id > ?",
                ScanOrder::Descending => "id < ?",
            }
            .to_string(),
        );
        params.push(Cell::Integer(after.unwrap_or(match order {
            ScanOrder::Ascending => -1,
            ScanOrder::Descending => i64::MAX,
        })));
        if let Some(min) = query.min_entry_id {
            clauses.push("id >= ?".to_string());
            params.push(Cell::Integer(min));
        }
        if let Some(max) = query.max_entry_id {
            clauses.push("id <= ?".to_string());
            params.push(Cell::Integer(max));
        }
        params.push(Cell::Integer(limit as i64 + 1));
        let rows = self.store.db().all(
            &format!(
                "SELECT record FROM {} WHERE {} ORDER BY id {} LIMIT ?",
                self.schema().table("entries"),
                clauses.join(" AND "),
                order.direction()
            ),
            &params,
        )?;
        let records = rows
            .iter()
            .map(record_of)
            .collect::<Result<Vec<Value>>>()?;
        Ok(Ok(cut_page(records, limit, order, |record| {
            record.get("id").and_then(Value::as_i64).unwrap_or(0)
        })))
    }

    /// 一份文档在 `at` 上的值（base + 其后的 delta 重放）。不存在 / 不在世 → `None`。
    pub fn materialize(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>> {
        materialize(self.store.db(), self.schema(), id, at)
    }

    /// 一份文档的记录（不重放内容）。
    pub fn document(&self, id: i64) -> Result<Option<Value>> {
        let row = self.store.db().get(
            &format!("SELECT record FROM {} WHERE id = ?", self.schema().table("documents")),
            &[Cell::Integer(id)],
        )?;
        row.as_ref().map(record_of).transpose()
    }

    fn schema(&self) -> &SqliteSchema {
        self.store.schema()
    }
}

fn record_of(row: &nous_chord::sqlite::SqliteRow) -> Result<Value> {
    parse_json(
        row.get("record")
            .and_then(Cell::as_str)
            .ok_or_else(|| StorageError::Shape("row has no record".to_string()))?,
    )
}

fn read_metadata(executor: &dyn SqliteExecutor, schema: &SqliteSchema) -> Result<(i64, Seq)> {
    let row = executor
        .get(
            &format!(
                "SELECT next_id, next_seq FROM {} WHERE singleton = 1",
                schema.table("durable_metadata")
            ),
            &[],
        )?
        .ok_or(StorageError::MissingMetadata)?;
    let next_id = row
        .get("next_id")
        .and_then(Cell::as_str)
        .ok_or(StorageError::MissingMetadata)?
        .parse::<i64>()
        .map_err(|_| StorageError::MissingMetadata)?;
    let next_seq = row
        .get("next_seq")
        .and_then(Cell::as_i64)
        .ok_or(StorageError::MissingMetadata)?;
    Ok((next_id, next_seq))
}

fn write_metadata(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    next_id: i64,
    next_seq: Seq,
) -> Result<()> {
    executor.run(
        &format!(
            "UPDATE {} SET next_id = ?, next_seq = ? WHERE singleton = 1",
            schema.table("durable_metadata")
        ),
        &[Cell::Text(next_id.to_string()), Cell::Integer(next_seq)],
    )?;
    Ok(())
}

/// 库里已有的最大 seq：会话的 `commit_seq` 与文档修订的 `seq` 取大者。
fn seq_floor(executor: &dyn SqliteExecutor, schema: &SqliteSchema) -> Result<Seq> {
    let row = executor
        .get(
            &format!(
                "SELECT
                    (SELECT COALESCE(MAX(commit_seq), 0) FROM {entries}) AS entry_floor,
                    (SELECT COALESCE(MAX(seq), 0) FROM {revisions}) AS revision_floor",
                entries = schema.table("entries"),
                revisions = schema.table("document_revisions")
            ),
            &[],
        )?
        .ok_or(StorageError::MissingMetadata)?;
    let entry_floor = row.get("entry_floor").and_then(Cell::as_i64).unwrap_or(0);
    let revision_floor = row.get("revision_floor").and_then(Cell::as_i64).unwrap_or(0);
    Ok(entry_floor.max(revision_floor))
}

/// 一批写里最大的 id + 1，和库里的 `next_id` 取大者。
fn candidate_next_id_of(writes: &[StorageWrite], base: i64) -> Result<i64> {
    let mut candidate = base;
    for write in writes {
        if let Some((id, _)) = write.claim()? {
            candidate = candidate.max(id + 1);
        }
    }
    Ok(candidate)
}

/// 只读查询用得上的一条 helper：会话下的 entry id 上界。
pub fn latest_entry_id(executor: &dyn SqliteExecutor, schema: &SqliteSchema, conversation_id: i64) -> Result<Option<i64>> {
    let row = executor.get(
        &format!(
            "SELECT MAX(id) AS id FROM {} WHERE conversation_id = ?",
            schema.table("entries")
        ),
        &[Cell::Integer(conversation_id)],
    )?;
    Ok(row.and_then(|row| row.get("id").and_then(Cell::as_i64)))
}

/// `Value` 的 `id`（测试与调用方都要，别各自写一遍）。
pub fn id_of(record: &Value) -> Result<i64> {
    required_i64(record, "id")
}
