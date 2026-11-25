//! 文档在某个点上的值：base + 其后的 delta 重放一次（上游 `SqliteStorage.materializeDocument`
//! 与 chord 的 `loadContent`）。
//!
//! ## 为什么不是「读最新 revision 就行」
//!
//! `document_revisions` 里存的是 **base 的整份值** 与 **delta 的操作数组**（`docs/storage.md` §4）。
//! 一条 delta 相对的是**它上面最近的 base**，不是上一条 revision。所以一次读取是：
//!
//! 1. 找 `seq <= 点` 的最新 base；
//! 2. 取 `base.seq < seq <= 点` 的所有 revision，按 seq 升序；
//! 3. 每条必须是 `delta` 且 `version == base.version` —— 否则那是一次跨版本的接续，
//!    按规矩必须有新 base 兜着（[`StorageError::VersionCrossing`]）；
//! 4. 用 [`nous_chord::delta`] 逐条 apply。
//!
//! ## 点与在世
//!
//! [`DocumentPoint::Current`] 看最新状态；[`DocumentPoint::At`] 看某个 seq 上的状态。
//! 只留最新态的文档（`is_current_only`）在非 current 的点上**明确失败**，不悄悄给最新值 ——
//! 给了就等于伪造历史。
//!
//! ## `Current` 的缓存（`#25`）
//!
//! 重放的性质：`value` 是「最新一条 base + 其后的 delta」的函数，而 delta 提交每次都要拿这个值
//! 当 publication 的 `value` —— 链一直长的话就是 O(n²)（`docs/bench-commit.md`：1,000 次 delta
//! 提交 1,805-1,989 ms）。
//!
//! 所以 [`materialize_cached`] 把 `Current` 的结果按文档 id 记在 [`DocumentCache`] 里。**缓存
//! 永远是库的从属物**，它的有效性只有一条判据：**库里这份文档最新那条 revision 的 `seq` 等于
//! 缓存记下的 `seq`**。revision 只插不改（主键 `(document_id, seq)`），所以「最新 revision 还是
//! 我算值的那一条」⇒「中间没有任何人写过」⇒ 值仍然成立。外部进程写的 delta 会把 `latest_seq`
//! 推过去，缓存当场作废。
//!
//! 退役**不写 revision**，`retired_at` 也不动 `seq`。只留最新态的文档退役时 revision 会被一起删掉
//! （`MAX(seq)` 变 NULL，恰好也能挡住），但 `rewindable` 的文档退役后 revision 全在 —— 光比 `seq`
//! 就会把已退役的文档"复活"。所以 [`materialize_cached`] 每次都读 `retired_at`：退役的一律
//! `Ok(None)`，并从缓存里删掉。
//!
//! 缓存只服务 `Current`。`At(seq)` 是历史读取（拿它判一条 delta 的 `version`），没有「每次都读
//! 同一个点」的性质，缓存它没有收益却有伪造历史的风险。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use nous_chord::delta::apply::{apply_immutable_trusted, TrustedOps};
use nous_chord::delta::codec::Decoder;
use nous_chord::sqlite::{SqliteExecutor, SqliteValue as Cell};

use super::sqlite::rows::{is_current_only, parse_json};
use super::sqlite::schema::SqliteSchema;
use super::writes::{Id, Seq};
use crate::error::{Result, StorageError};

/// 读文档的哪个点。上游 `DocumentPoint = Seq | "current"`。
///
/// 序列化形状就是上游那个联合：`At(seq)` → `seq`，`Current` → `"current"`（字符串，不是
/// `{"kind":"current"}`）。`CommitPublication` 里的 `document.copy` 带着它过线，所以必须在这
/// 一层就对得上，不能在拼 publication 的地方临时凑一个。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentPoint {
    /// 最新状态。
    Current,
    /// 某个 seq 上的状态（含那个 seq 的 revision）。
    At(Seq),
}

impl serde::Serialize for DocumentPoint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            DocumentPoint::Current => serializer.serialize_str("current"),
            DocumentPoint::At(seq) => serializer.serialize_i64(*seq),
        }
    }
}

impl<'de> serde::Deserialize<'de> for DocumentPoint {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        match <serde_json::Value as serde::Deserialize>::deserialize(deserializer)? {
            serde_json::Value::Number(number) => number
                .as_i64()
                .map(DocumentPoint::At)
                .ok_or_else(|| serde::de::Error::custom("document point seq must be an integer")),
            serde_json::Value::String(text) if text == "current" => Ok(DocumentPoint::Current),
            other => Err(serde::de::Error::custom(format!(
                "document point must be a seq or \"current\", got {other}"
            ))),
        }
    }
}

impl DocumentPoint {
    /// 查询用的上界：`At(seq)` 就是 `seq`，`Current` 是「没有上界」。
    fn upper(self) -> i64 {
        match self {
            DocumentPoint::Current => i64::MAX,
            DocumentPoint::At(seq) => seq,
        }
    }
}

/// 一份文档在某个点上的值。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredDocument {
    /// 文档记录本身（`documents.record` 那一列）。
    pub record: Value,
    /// 值来自哪个 base 的版本。
    pub version: i64,
    /// 重放后的整份值。
    pub value: Value,
    /// 这个 base 之后已经叠了几条 delta（读的人靠它决定「该写新的 base 了吗」）。
    pub deltas_since_base: usize,
}

/// 缓存里的一条：**这份值来自哪个 seq**。判据就是它 —— 见模块文档。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CachedDocument {
    /// 值等于「`seq` 那一条 revision 之后的状态」。库里这份文档的 `MAX(seq)` 变了，它就不成立了。
    pub seq: Seq,
    pub version: i64,
    pub value: Value,
    pub deltas_since_base: usize,
}

/// 缓存最多留几份文档。**按条目数，不按字节数**：一份值有多大由调用方决定（`pi.live` 在长答案
/// 期间就是整段正文），把字节数写进判据只会变成一个「按负载调的数字」。最坏情况的算术在
/// `docs/storage.md` §10.6。
pub const CACHED_DOCUMENTS: usize = 256;

/// `materialize(Current)` 的结果，按文档 id 存，**最多 [`CACHED_DOCUMENTS`] 条**。
///
/// 满了两件事同时成立：① **最近用过的留**（LRU，`Slot::touched` 是单调计数，命中也会推它 ——
/// 否则「最近」只记得写过谁，读一次就再也不读的文档会被反复留下）；② **淘汰只影响成本，不影响
/// 正确性**：被淘汰的文档下次读走的是完整重放，那条路一直在（缓存不是唯一的读法）。
///
/// **每个 `Storage` 一份**：它属于一条连接的生命，跨连接（= 跨进程）的失效靠
/// [`materialize_cached`] 每次对库的那一问。
///
/// 为什么要有上限：没有上限的话「不释放」是个沉默的漏，而不是一个决定。fixture 里 961 份文档
/// 加起来才 53 KB，看不出问题；长驻进程里 5,000 个会话 × 一份 100 KB 的 `pi.live` 就是 500 MB，
/// 永不释放，且没有任何一步会报错。
#[derive(Debug, Default)]
pub(crate) struct DocumentCache {
    entries: HashMap<Id, Slot>,
    /// 单调递增的「第几次碰」，用来挑最久没被碰过的那个。
    clock: u64,
}

#[derive(Debug)]
struct Slot {
    touched: u64,
    document: CachedDocument,
}

impl DocumentCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 读一条，**命中就推一次「最近用过」**。
    fn get(&mut self, id: Id) -> Option<&CachedDocument> {
        self.clock = self.clock.wrapping_add(1);
        let touched = self.clock;
        let slot = self.entries.get_mut(&id)?;
        slot.touched = touched;
        Some(&slot.document)
    }

    /// 放一条，满了就淘汰最久没被碰过的那一条。
    fn insert(&mut self, id: Id, document: CachedDocument) {
        self.clock = self.clock.wrapping_add(1);
        let touched = self.clock;
        self.entries.insert(id, Slot { touched, document });
        while self.entries.len() > CACHED_DOCUMENTS {
            // 溢出最多比上限多一条，所以每一次插入最多淘汰一条；`while` 只是把不变量写死。
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, slot)| slot.touched)
                .map(|(id, _)| *id);
            match victim {
                Some(victim) => {
                    self.entries.remove(&victim);
                }
                None => break,
            }
        }
    }

    fn remove(&mut self, id: Id) {
        self.entries.remove(&id);
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// 缓存里现在有几条。**只给判据用**（`#25`：上限必须机器可判，不然「不会永不释放」只能靠读代码）。
pub(crate) fn cache_len(cache: &Mutex<DocumentCache>) -> usize {
    lock(cache).len()
}

/// 这份文档现在在缓存里吗。**只给判据用**：LRU 与 FIFO 只差「命中是否推时间戳」那一句，差别只有
/// 读得到它才能判。
pub(crate) fn caches_document(cache: &Mutex<DocumentCache>, id: Id) -> bool {
    lock(cache).entries.contains_key(&id)
}

/// 一条 delta revision 的 wire ops（`[{op}, …]`）作用在 `value` 上。
///
/// 重放（[`materialize`]）与「缓存 + 本批 ops」（[`values_after_batch`]）都走这一条 —— 两条路
/// 如果各写一遍 apply，缓存的值和重放的值迟早会不一样，而这种不一致只有流式 UI 上「少了一段
/// 字」才看得见。
fn apply_wire_ops(id: Id, value: &Value, ops: &Value) -> Result<Value> {
    let wire = ops.as_array().ok_or_else(|| StorageError::DeltaDecode {
        document_id: id,
        message: "a delta revision is not an array of ops".to_string(),
    })?;
    let decoded = Decoder::new().decode(wire).map_err(|error| StorageError::DeltaDecode {
        document_id: id,
        message: error.to_string(),
    })?;
    let trusted = TrustedOps::from_decoded(decoded).map_err(|error| StorageError::DeltaDecode {
        document_id: id,
        message: error.to_string(),
    })?;
    apply_immutable_trusted(value, &trusted).map_err(|error| StorageError::DeltaDecode {
        document_id: id,
        message: error.to_string(),
    })
}

/// 一份文档「现在在哪」：最新 revision 的 seq、是否已退役、记录本身。**一条语句。**
struct CurrentPointer {
    record: Value,
    retired: bool,
    latest_seq: Option<Seq>,
}

fn current_pointer(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    id: Id,
) -> Result<Option<CurrentPointer>> {
    let Some(row) = executor.get(
        &format!(
            "SELECT document.record AS record, document.retired_at AS retired_at,
                (SELECT MAX(seq) FROM {revisions} WHERE document_id = document.id) AS latest_seq
             FROM {documents} AS document WHERE document.id = ?",
            documents = schema.table("documents"),
            revisions = schema.table("document_revisions")
        ),
        &[Cell::Integer(id)],
    )?
    else {
        return Ok(None);
    };
    Ok(Some(CurrentPointer {
        record: parse_json(
            row.get("record")
                .and_then(Cell::as_str)
                .ok_or_else(|| StorageError::Shape("document row has no record".to_string()))?,
        )?,
        retired: row.get("retired_at").map(|cell| !matches!(cell, Cell::Null)).unwrap_or(false),
        latest_seq: row.get("latest_seq").and_then(Cell::as_i64),
    }))
}

/// 锁一下缓存。毒化只可能来自「持锁期间 panic」，而锁区里只有 map 的读写 —— 真毒化了就是有
/// 别处出了大问题，喊出来比继续往下猜好。
fn lock(cache: &Mutex<DocumentCache>) -> std::sync::MutexGuard<'_, DocumentCache> {
    cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// [`materialize`] 的缓存版：**库是真相，缓存是它的从属物**。
///
/// 有效性判据只有一条 —— 库里这份文档 `MAX(seq)` 等于缓存记下的 `seq`（revision 只插不改，所以
/// 「最新那条还是我算值的那一条」⇒「中间没人写过」）。退役不写 revision，于是这条判据单独看会漏掉
/// 它（`history: "latest"` 的文档退役时 revision 会被删掉，`MAX(seq)` 变 NULL 恰好挡住；但
/// `rewindable` 的文档退役后 revision 全在）：退役的文档一律 `Ok(None)`，并把它从缓存里删掉。
pub(crate) fn materialize_cached(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    cache: &Mutex<DocumentCache>,
    id: Id,
    at: DocumentPoint,
) -> Result<Option<StoredDocument>> {
    if at != DocumentPoint::Current {
        // 历史读取：没有「每次都读同一个点」的性质，缓存它只会多一份伪造历史的办法。
        return materialize(executor, schema, id, at);
    }
    let Some(pointer) = current_pointer(executor, schema, id)? else {
        lock(cache).remove(id);
        return Ok(None);
    };
    if pointer.retired {
        lock(cache).remove(id);
        return Ok(None);
    }
    let hit = lock(cache)
        .get(id)
        .filter(|entry| Some(entry.seq) == pointer.latest_seq)
        .cloned();
    if let Some(entry) = hit {
        return Ok(Some(StoredDocument {
            record: pointer.record,
            version: entry.version,
            value: entry.value,
            deltas_since_base: entry.deltas_since_base,
        }));
    }
    let stored = materialize(executor, schema, id, at)?;
    if let (Some(stored), Some(seq)) = (&stored, pointer.latest_seq) {
        lock(cache).insert(
            id,
            CachedDocument {
                seq,
                version: stored.version,
                value: stored.value.clone(),
                deltas_since_base: stored.deltas_since_base,
            },
        );
    }
    Ok(stored)
}

/// **一批写落库之前**：把这一批要碰的文档的缓存对着库校准一遍，返回仍然有效的那几条。
///
/// 返回的就是「这批之前的值」—— delta 提交拿它当底板，加上本批 ops 就得到这批之后的值，不必
/// 重放整条链。调用点在事务里（写锁在手上），所以校准与使用之间没有人能插进来写。
pub(crate) fn refresh(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    cache: &Mutex<DocumentCache>,
    ids: &[Id],
) -> Result<HashMap<Id, CachedDocument>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let params: Vec<Cell> = ids.iter().map(|id| Cell::Integer(*id)).collect();
    let rows = super::sqlite::statements::read_rows(
        executor,
        &format!(
            "SELECT document.id AS id, document.retired_at AS retired_at,
                (SELECT MAX(seq) FROM {revisions} WHERE document_id = document.id) AS latest_seq
             FROM {documents} AS document WHERE document.id IN (SELECT value FROM json_each(?))",
            documents = schema.table("documents"),
            revisions = schema.table("document_revisions")
        ),
        &params,
    )?;
    let mut live = HashMap::new();
    let mut guarded = lock(cache);
    for row in &rows {
        let id = row
            .get("id")
            .and_then(Cell::as_i64)
            .ok_or_else(|| StorageError::Shape("document row has no id".to_string()))?;
        let retired = row.get("retired_at").map(|cell| !matches!(cell, Cell::Null)).unwrap_or(false);
        let latest_seq = row.get("latest_seq").and_then(Cell::as_i64);
        let entry = guarded
            .get(id)
            .filter(|entry| !retired && Some(entry.seq) == latest_seq)
            .cloned();
        match entry {
            Some(entry) => {
                live.insert(id, entry);
            }
            None => {
                guarded.remove(id);
            }
        }
    }
    // 这次提交里**不在库中**的文档 id（新造的）：库里查不到就没有可校准的，缓存里也不该有。
    for id in ids {
        if !rows.iter().any(|row| row.get("id").and_then(Cell::as_i64) == Some(*id)) {
            guarded.remove(*id);
        }
    }
    Ok(live)
}

/// **一批写落库之后**：算出这批改到的每份文档的 `(version, value)`（publication 的 `value`），
/// 同时把缓存推到这一批的 `seq` 上。
///
/// delta 的底板来自 `before`（[`refresh`] 在 flush 之前交出来的）；缓存里没有才退回重放一次。
/// 这条「退回」不是异常路径 —— 进程刚起来、或外部刚写过、或刚打了 base，都会走到这里。
pub(crate) fn values_after_batch(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    cache: &Mutex<DocumentCache>,
    actions: &HashMap<Id, super::writes::DocumentAction>,
    seq: Seq,
    before: &HashMap<Id, CachedDocument>,
) -> Result<HashMap<Id, (i64, Value)>> {
    use super::writes::DocumentContent;

    let mut values = HashMap::new();
    let mut ids: Vec<Id> = actions.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let action = &actions[&id];
        if action.retire {
            // 记录里已经带了 `retiredAt`：值不再有意义，缓存也不该留着它。
            lock(cache).remove(id);
            continue;
        }
        let Some(content) = &action.content else { continue };
        match content {
            DocumentContent::Base { version, value } => {
                values.insert(id, (*version, value.clone()));
                lock(cache).insert(
                    id,
                    CachedDocument {
                        seq,
                        version: *version,
                        value: value.clone(),
                        deltas_since_base: 0,
                    },
                );
            }
            DocumentContent::Delta { version, ops } => {
                let (value, deltas_since_base) = match before.get(&id) {
                    Some(entry) => (apply_wire_ops(id, &entry.value, ops)?, entry.deltas_since_base + 1),
                    None => {
                        // 这一批已经 flush：此刻读 `Current` 拿到的就是这批之后的值。
                        let stored = materialize(executor, schema, id, DocumentPoint::Current)?
                            .ok_or_else(|| {
                                StorageError::Shape(format!("document {id} vanished right after commit"))
                            })?;
                        (stored.value, stored.deltas_since_base)
                    }
                };
                values.insert(id, (*version, value.clone()));
                lock(cache).insert(
                    id,
                    CachedDocument {
                        seq,
                        version: *version,
                        value,
                        deltas_since_base,
                    },
                );
            }
        }
    }
    Ok(values)
}

/// 文档在 `at` 上是否在世（左闭右开的生命周期）。上游 `isAliveAt`。
pub fn alive_at(record: &Value, at: DocumentPoint) -> bool {
    let created = record.get("createdAt").and_then(Value::as_i64);
    let retired = record.get("retiredAt").filter(|value| !value.is_null()).and_then(Value::as_i64);
    match at {
        DocumentPoint::Current => retired.is_none(),
        DocumentPoint::At(seq) => {
            let Some(created) = created else { return false };
            created <= seq && retired.map(|retired| seq < retired).unwrap_or(true)
        }
    }
}

/// 读 `id` 在 `at` 上的值：不存在 / 不在世给 `Ok(None)`，形状不对给 `Err`。
pub(crate) fn materialize(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    id: Id,
    at: DocumentPoint,
) -> Result<Option<StoredDocument>> {
    materialize_inner(executor, schema, id, at, false)
}

/// 只读的历史重放：允许 current-only 文档在某个 seq 上读值。
///
/// 用途只有上层 documents 的 history/diff；它读的是**已经落库的 revision**，
/// 不会把“当前值”冒充成过去 —— `as_of` 走的是和 `materialize(At)` 同一条 base+delta 重放。
pub(crate) fn revision_value(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    id: Id,
    seq: Seq,
) -> Result<Option<StoredDocument>> {
    materialize_inner(executor, schema, id, DocumentPoint::At(seq), true)
}

fn materialize_inner(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    id: Id,
    at: DocumentPoint,
    allow_past: bool,
) -> Result<Option<StoredDocument>> {
    let Some(row) = executor.get(
        &format!("SELECT record FROM {} WHERE id = ?", schema.table("documents")),
        &[Cell::Integer(id)],
    )?
    else {
        return Ok(None);
    };
    let record = parse_json(
        row.get("record")
            .and_then(Cell::as_str)
            .ok_or_else(|| StorageError::Shape("document row has no record".to_string()))?,
    )?;
    // 只留最新态的文档没有历史可读：给最新的值就是伪造。
    if at != DocumentPoint::Current && is_current_only(&record) && !allow_past {
        return Err(StorageError::HistoryNotRetained { document_id: id });
    }
    if !alive_at(&record, at) {
        return Ok(None);
    }
    let upper = at.upper();
    let base = executor
        .get(
            &format!(
                "SELECT seq, version, content FROM {} WHERE document_id = ? AND kind = 'base' AND seq <= ?
                 ORDER BY seq DESC LIMIT 1",
                schema.table("document_revisions")
            ),
            &[Cell::Integer(id), Cell::Integer(upper)],
        )?
        .ok_or(StorageError::MissingBase { document_id: id })?;
    let base_seq = base
        .get("seq")
        .and_then(Cell::as_i64)
        .ok_or_else(|| StorageError::Shape("revision has no seq".to_string()))?;
    let version = base
        .get("version")
        .and_then(Cell::as_i64)
        .ok_or_else(|| StorageError::Shape("revision has no version".to_string()))?;
    let mut value = parse_json(
        base.get("content")
            .and_then(Cell::as_str)
            .ok_or_else(|| StorageError::Shape("revision has no content".to_string()))?,
    )?;
    let tail = executor.all(
        &format!(
            "SELECT seq, kind, version, content FROM {} WHERE document_id = ? AND seq > ? AND seq <= ?
             ORDER BY seq",
            schema.table("document_revisions")
        ),
        &[Cell::Integer(id), Cell::Integer(base_seq), Cell::Integer(upper)],
    )?;
    for revision in &tail {
        let kind = revision.get("kind").and_then(Cell::as_str).unwrap_or("");
        let revision_version = revision.get("version").and_then(Cell::as_i64).unwrap_or(-1);
        if kind != "delta" || revision_version != version {
            // 版本中间没有 base 兜着：说明这份库被写坏了，或者我们的写入漏了 base。
            return Err(StorageError::VersionCrossing { document_id: id });
        }
        let ops: Value = parse_json(
            revision
                .get("content")
                .and_then(Cell::as_str)
                .ok_or_else(|| StorageError::Shape("revision has no content".to_string()))?,
        )?;
        value = apply_wire_ops(id, &value, &ops)?;
    }
    Ok(Some(StoredDocument {
        record,
        version,
        value,
        deltas_since_base: tail.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_lifetime_is_half_open_and_current_ignores_created_at() {
        let live = json!({ "id": 1, "createdAt": 10 });
        assert!(alive_at(&live, DocumentPoint::Current));
        assert!(alive_at(&live, DocumentPoint::At(10)));
        assert!(!alive_at(&live, DocumentPoint::At(9)));

        let retired = json!({ "id": 1, "createdAt": 10, "retiredAt": 20 });
        assert!(!alive_at(&retired, DocumentPoint::Current));
        assert!(alive_at(&retired, DocumentPoint::At(10)));
        assert!(alive_at(&retired, DocumentPoint::At(19)));
        // 左闭右开：退役那个 seq 上已经不在世了。
        assert!(!alive_at(&retired, DocumentPoint::At(20)));
    }
}
