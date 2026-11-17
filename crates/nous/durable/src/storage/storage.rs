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

use std::sync::Mutex;

use serde_json::Value;

use nous_chord::sqlite::{SqliteDatabase, SqliteExecutor, SqliteValue as Cell};

use super::commit::{
    resolve_copies, stored_batch, CommitResult, CommitState, ConversationQuery, PendingWrites,
    StoredBatch,
};
use super::documents::{materialize_cached, DocumentCache, DocumentPoint, StoredDocument};
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
    /// `materialize(Current)` 的结果，按文档 id 存（`#25`）。有效性每次提交都在事务里对着库校准：
    /// 库是真相，缓存只是「上次算过的值」，不是第二份真相。
    cache: Mutex<DocumentCache>,
    /// `close()` 之后为 true。**这就是「关了」的全部状态**：没有连接池、没有重连、没有后台任务，
    /// 只有一个门闩（理由见 [`Storage::close`]）。
    closed: bool,
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
            cache: Mutex::new(DocumentCache::new()),
            closed: false,
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
            cache: Mutex::new(DocumentCache::new()),
            closed: false,
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
        self.ensure_open()?;
        if self.next_id >= i64::MAX {
            return Err(StorageError::Shape("ID space is exhausted".to_string()));
        }
        let id = self.next_id;
        self.next_id += 1;
        Ok(id)
    }

    /// 关库：**之后每一次调用都明确失败**（[`StorageError::Closed`]），包括提交与取 id。
    ///
    /// ## 这里只有一个门闩，不是生命周期管理
    ///
    /// 上游 `StoreStorage.close()` 做三件事：拒绝新调用、等正在飞的提交走完、放掉声明。本库是
    /// **单进程 + 单写者**，flight 的等待由调用方本来的 `&mut self` 借阅保证（一次只能有一个提交
    /// 在跑），所以这里只剩第一件：置 [`Storage::closed`]，随后每个入口 `ensure_open()`。
    ///
    /// **`Drop` 仍然是收尾的正路**：不开 `close()` 时连接由 [`SqliteStore`] 的析构关掉，语义与这里
    /// 一样（SQLite 会自己关）。`close()` 存在的理由是「关掉之后还拿着这个值的代码必须报错」——
    /// 例如一个已经收摊的 `Session` 手里漏出来的 `Storage`，它的下一次提交该失败，而不是悄悄写进
    /// 一份已经交给别人的库。所以它**不消费 `self`**：消费掉 `self` 的写法根本测不到这条语义
    /// （上游那条案 `rejects every operation after close` 就是关完之后再调三个入口）。
    ///
    /// **重复调用返回 `Ok(())`**：已经关了就不是错误。
    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.store.db_mut().close()?;
        Ok(())
    }

    /// 关过之后一切入口都走这里。`Ok` 之后调用方才继续。
    fn ensure_open(&self) -> Result<()> {
        if self.closed {
            return Err(StorageError::Closed);
        }
        Ok(())
    }

    /// 一次提交若干批，每批盖自己的 seq。第一段被拒的批到此为止，**前面通过的照常提交**。
    pub fn commit<F>(&mut self, batches: &[Vec<StorageWrite>], mut receive: F) -> Result<CommitResult>
    where
        F: FnMut(usize, Seq),
    {
        self.ensure_open()?;
        let schema = self.store.schema().clone();
        let base_next_id = self.next_id;

        let (seqs, refused, candidate_next_id, batches) = self.store.db_mut().transaction(
            |tx| -> Result<(Vec<Seq>, Option<Refusal>, i64, Vec<StoredBatch>)> {
                let (stored_next_id, mut next_seq) = read_metadata(tx, &schema)?;
                let floor = seq_floor(tx, &schema)?;
                // 负例二：seq 绝不倒退、绝不复用。
                if next_seq <= floor {
                    return Err(StorageError::SeqRegression { next_seq, floor });
                }
                let mut state = CommitState::read(tx, &schema, batches)?;
                let mut pending = PendingWrites::default();
                let mut seqs: Vec<Seq> = Vec::new();
                let mut stored: Vec<StoredBatch> = Vec::new();
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
                    // 复制要读源文档：每批结束都会 flush，所以此刻前面几批已经在库里了（上游
                    // `checkBatch` 的顺序：先写前面的，再解复制）。
                    if actions.values().any(|action| action.needs_copy()) {
                        if let Some(refusal) = resolve_copies(tx, &schema, &mut actions)? {
                            refused = Some(refusal);
                            break;
                        }
                    }
                    let seq = next_seq;
                    next_seq += 1;
                    state.apply(writes, &actions, seq)?;
                    pending.add(writes, &actions, seq, &state)?;
                    // 这批要碰的文档：把缓存对着库校准（写锁在手上，校准与使用之间没人能插进来），
                    // 于是 delta 的「批前值」通常直接来自缓存，不必重放整条链（`#25`）。
                    let before =
                        super::documents::refresh(tx, &schema, &self.cache, &touched_documents(writes))?;
                    pending.flush(tx, &schema)?;
                    // 这批之后的值（publication 的 `value`）：缓存 + 本批 ops，同一条 apply。
                    let values =
                        super::documents::values_after_batch(tx, &schema, &self.cache, &actions, seq, &before)?;
                    seqs.push(seq);
                    stored.push(stored_batch(tx, &schema, writes, &actions, seq, &state, &values)?);
                    candidate_next_id = candidate_next_id.max(candidate_next_id_of(writes, base_next_id)?);
                }
                pending.flush(tx, &schema)?;
                if !seqs.is_empty() {
                    write_metadata(tx, &schema, candidate_next_id.max(stored_next_id), next_seq)?;
                }
                Ok((seqs, refused, candidate_next_id, stored))
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
            batches,
        })
    }

    /// 一条会话的记录。
    pub fn conversation(&self, id: i64) -> Result<Option<Value>> {
        self.ensure_open()?;
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
        self.ensure_open()?;
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
        self.ensure_open()?;
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

    /// 这个会话最新的一条**带终态**的 entry（`record.reason` 非 null）。
    ///
    /// 走 `entry_terminals_by_conversation` 这个 partial expression index：
    /// `... WHERE json_extract(record,'$.reason') IS NOT NULL ORDER BY id DESC LIMIT 1`。
    /// 只命中带 reason 的行，普通 entry 不进索引 —— abort 热路径上不再 O(n) 扫全会话。
    pub fn terminal_entry_in(&self, conversation_id: i64) -> Result<Option<Value>> {
        self.ensure_open()?;
        let row = self.store.db().get(
            &format!(
                "SELECT record FROM {} WHERE conversation_id = ? \
                 AND json_extract(record, '$.reason') IS NOT NULL \
                 ORDER BY id DESC LIMIT 1",
                self.schema().table("entries")
            ),
            &[Cell::Integer(conversation_id)],
        )?;
        let Some(row) = row else { return Ok(None) };
        let text = row
            .get("record")
            .and_then(Cell::as_str)
            .ok_or_else(|| StorageError::Shape("entry has no record".to_string()))?;
        parse_json(text).map(Some)
    }

    /// 一条 entry，**只在它于这个会话的 fork 链上看得见时**返回。
    ///
    /// 与 [`entry`](Self::entry) 的差别是「可见性」：`entry(id)` 是全局按 id 取（导入器与测试要它），
    /// 这一条问的是「这个会话的历史里有没有它」—— fork 点之后写在别的分支上的 entry 不在其中。
    /// 会话不存在 → [`StorageError::UnknownConversation`]。
    pub fn entry_in_conversation(&self, conversation_id: i64, id: i64) -> Result<Option<(Value, Seq)>> {
        self.ensure_open()?;
        let found = super::scans::visible_entry(self.store.db(), self.schema(), conversation_id, id)?;
        Ok(found.map(|(record, seq)| (record, seq as Seq)))
    }

    /// 这个会话的 fork 链上最新的、带 `head` 的那条 entry —— 一段历史的**真实下界**。
    ///
    /// `at_or_before` 给了就在那个 id（含）以下找。压缩/重置留下的 `head` 标记就是靠它定位的：
    /// 调用方拿 `minEntryId = marker.head` 去扫，才不会把已经压缩掉的正文再读一遍。
    pub fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before: Option<i64>,
    ) -> Result<Option<Value>> {
        self.ensure_open()?;
        super::scans::latest_head_marker(self.store.db(), self.schema(), conversation_id, at_or_before)
    }

    /// 一条会话（含它的 fork 祖先链）的 entry，按 id 升/降序，游标不透明。
    ///
    /// **不只是一条会话**：fork 出来的会话只存自己那一段，之前的历史住在父会话里。链怎么走、
    /// 两条方向为什么不对称，见 [`super::scans::scan_entry_chain`] 的模块文档。
    pub fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<std::result::Result<Page<Value>, Refusal>> {
        self.ensure_open()?;
        super::scans::scan_entry_chain(self.store.db(), self.schema(), query, limit, cursor)
    }

    /// 一个会话的提交记录（`submissions.record`），按 id 升序。
    ///
    /// `request_id` 的幂等查找要走这条：`#22` 的 `POST /messages` 同一个 `requestId` 打两次
    /// 必须拿到同一条 submission（表里不建唯一索引的理由见 `docs/storage.md`）。
    pub fn submissions(&self, conversation_id: i64) -> Result<Vec<Value>> {
        self.ensure_open()?;
        let rows = self.store.db().all(
            &format!(
                "SELECT record FROM {} WHERE conversation_id = ? ORDER BY id",
                self.schema().table("submissions")
            ),
            &[Cell::Integer(conversation_id)],
        )?;
        rows.iter().map(record_of).collect()
    }

    /// 一个会话的任务记录（`tasks.record`），按 id 升序。状态是不透明 JSON，这里不解释。
    pub fn tasks(&self, conversation_id: i64) -> Result<Vec<Value>> {
        self.ensure_open()?;
        let rows = self.store.db().all(
            &format!(
                "SELECT record FROM {} WHERE conversation_id = ? ORDER BY id",
                self.schema().table("tasks")
            ),
            &[Cell::Integer(conversation_id)],
        )?;
        rows.iter().map(record_of).collect()
    }

    /// 会话里 id 最大的那条 entry（fork 不给 `entryId` 时用）。
    pub fn latest_entry_id(&self, conversation_id: i64) -> Result<Option<i64>> {
        self.ensure_open()?;
        latest_entry_id(self.store.db(), self.schema(), conversation_id)
    }

    /// 一份文档在 `at` 上的值。不存在 / 不在世 → `None`。
    ///
    /// `Current` 走缓存（[`materialize_cached`]）：命中与否**只看库里这份文档最新一条 revision
    /// 的 `seq`**，所以外部连接写过的文档会当场失效。`At(seq)` 永远重放，不缓存。
    pub fn materialize(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>> {
        self.ensure_open()?;
        materialize_cached(self.store.db(), self.schema(), &self.cache, id, at)
    }

    /// 缓存里现在留了几份文档。**给判据用**：`#25` 要求上限机器可判（`tests/materialize_cache.rs`），
    /// 不然「不会永不释放」只能靠读代码。上限本身是 [`super::documents::CACHED_DOCUMENTS`]。
    pub fn cached_documents(&self) -> usize {
        super::documents::cache_len(&self.cache)
    }

    /// 这份文档现在在缓存里吗。**给判据用**：`#25` 的淘汰策略是 LRU，而 LRU 与 FIFO 只差一句
    /// 「命中是否推时间戳」—— 差别只有读得到它才能判。
    pub fn caches_document(&self, id: i64) -> bool {
        super::documents::caches_document(&self.cache, id)
    }

    /// 一条任务记录。`state` 是不透明 JSON：这一层不解释它，状态机的语义在调用方。
    pub fn task(&self, id: i64) -> Result<Option<Value>> {
        self.ensure_open()?;
        super::scans::task(self.store.db(), self.schema(), id)
    }

    /// 一条提交记录。
    pub fn submission(&self, id: i64) -> Result<Option<Value>> {
        self.ensure_open()?;
        super::scans::submission(self.store.db(), self.schema(), id)
    }

    /// `(conversationId, requestId)` 的幂等查找 —— `POST /messages` 的幂等键。
    pub fn submission_by_request(&self, conversation_id: i64, request_id: &str) -> Result<Option<Value>> {
        self.ensure_open()?;
        super::scans::submission_by_request(self.store.db(), self.schema(), conversation_id, request_id)
    }

    /// 按 id 升/降扫任务。过滤条件全是拆出来的索引列（`kind` 与 `request_id` 是编码体）。
    pub fn scan_tasks(
        &self,
        query: &super::scans::TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<std::result::Result<Page<Value>, Refusal>> {
        self.ensure_open()?;
        super::scans::scan_tasks(self.store.db(), self.schema(), query, limit, cursor)
    }

    /// 按 id 升/降扫提交记录。
    pub fn scan_submissions(
        &self,
        query: &super::scans::SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<std::result::Result<Page<Value>, Refusal>> {
        self.ensure_open()?;
        super::scans::scan_submissions(self.store.db(), self.schema(), query, limit, cursor)
    }

    /// 按**作用域**扫在世的文档（`key` 不参与）。
    pub fn scan_documents(
        &self,
        query: &super::scans::DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<std::result::Result<Page<Value>, Refusal>> {
        self.ensure_open()?;
        super::scans::scan_documents(self.store.db(), self.schema(), query, limit, cursor)
    }

    /// 按**地址**（`kind` + `scope` + `key`）精确找一份在世的文档。
    ///
    /// 与 [`scan_documents`](Self::scan_documents) 是两条独立的路：一个比五个索引列，一个比作用域
    /// 三列。混成一条就会出现「按地址找」返回一串候选，调用方还得自己挑。
    pub fn find_document(&self, address: &Value, at: DocumentPoint) -> Result<Option<Value>> {
        self.ensure_open()?;
        super::scans::find_document(self.store.db(), self.schema(), address, at)
    }

    /// 一份文档的记录（不重放内容）。
    pub fn document(&self, id: i64) -> Result<Option<Value>> {
        self.ensure_open()?;
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

/// 一批写碰到的文档 id（去重）：缓存只用得着这些。
fn touched_documents(writes: &[StorageWrite]) -> Vec<i64> {
    let mut ids: Vec<i64> = writes
        .iter()
        .filter_map(|write| match write {
            StorageWrite::DocumentCreate { id, .. }
            | StorageWrite::DocumentCopy { id, .. }
            | StorageWrite::DocumentChange { id, .. }
            | StorageWrite::DocumentRetire { id } => Some(*id),
            _ => None,
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
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
