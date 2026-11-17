//! 扫描与按地址查：四个 `scan_*` 的读一半（上游 `storage/store/storage.ts:245-358`）。
//!
//! ## 为什么单独一个文件
//!
//! [`storage`](super::storage) 那个门面已经装着提交、id 分配器、缓存；扫描是**纯读**，只依赖
//! 表名 + 索引列 + [`scan_start`](super::cursor::scan_start) 那套不透明游标。放这里，`storage.rs`
//! 里剩下的就是几行委托 —— 读的人一眼看得出哪些是「提交那一半」、哪些是「读那一半」。
//!
//! ## 三条共同规矩（上游 `#scan` 的形状）
//!
//! 1. **多取一条**：`LIMIT limit + 1`，多出来的那一条只用来决定「还有没有下一页」，不进结果
//!    （[`cut_page`]）。
//! 2. **游标接着 id**：起点是 `id > after`（升）或 `id < after`（降）。游标不透明，只编码 id
//!    （[`super::cursor`]），这里不做「偏移量」「时间戳」那种调用方读得懂的翻页。
//! 3. **默认方向各不相同**：任务与提交是**升序**（最老的先，上游 `scanStart(query.order, cursor,
//!    "ascending")`），entry 是**降序**（最新的先）。这不是口味 —— 写反了翻页照样能翻，只是顺序
//!    错了，所以要各写一遍、各测一遍。
//!
//! ## 索引列
//!
//! `tasks` / `submissions` 的过滤列都是拆出来的列（`docs/storage.md` §2）：`status` / `abort_requested`
//! / `background` 是原值，`kind` / `request_id` 是**编码过的**索引字符串（[`encode_indexed_string`]），
//! 所以过滤时也必须编码 —— 拿原文去比会一条都匹配不上，而且不报错。

use serde_json::Value;

use nous_chord::sqlite::{SqliteExecutor, SqliteValue as Cell};

use super::cursor::{cut_page, scan_start, Cursor, Page, ScanOrder};
use super::documents::DocumentPoint;
use super::sqlite::rows::{encode_indexed_string, parse_json, required_i64, scope_columns};
use super::sqlite::schema::SqliteSchema;
use super::writes::Refusal;
use crate::error::{Result, StorageError};

/// 一次任务扫描的过滤条件（上游 `TaskQuery`）。全空 = 全表按 id 扫。
#[derive(Debug, Clone, Default)]
pub struct TaskQuery {
    pub conversation_id: Option<i64>,
    /// 记录里的 `kind` 原文（不是编码体）：这里编码，SQL 里比的是编码体。
    pub kind: Option<String>,
    pub status: Option<String>,
    pub abort_requested: Option<bool>,
    pub background: Option<bool>,
    pub order: Option<ScanOrder>,
}

/// 一次提交记录扫描的过滤条件（上游 `SubmissionQuery`）。
#[derive(Debug, Clone, Default)]
pub struct SubmissionQuery {
    pub conversation_id: Option<i64>,
    pub status: Option<String>,
    pub order: Option<ScanOrder>,
}

/// 一次文档扫描的过滤条件（上游 `DocumentQuery`）。
///
/// `scope` 是**作用域**而不是地址：一条 `cache` 文档只要作用域对就进结果，`key` 不参与 —— 按地址
/// 精确找是 [`find_document`] 的事。这两条路不许混（上游那条案名就叫 `indexes logical addresses
/// and exact-scope scans independently`）。
#[derive(Debug, Clone)]
pub struct DocumentQuery {
    /// `{kind: "session"}` / `{kind: "conversation", conversationId}` / `{kind: "task", taskId}`。
    pub scope: Value,
    /// `kind` 原文；缺席 = 作用域里的全部种类。
    pub kind: Option<String>,
    pub at: DocumentPoint,
}

/// 取最多 `rows` 条记录（`SELECT record` 那一列），按 id 走。
///
/// **`rows` 是原样的 `LIMIT`**，不是页大小：分页要「多取一条」，那个 `+ 1` 由调用方加，读起来才
/// 看得出哪一处是「多取的那一条」。
fn fetch_records(
    executor: &dyn SqliteExecutor,
    table: &str,
    clauses: &[String],
    params: &[Cell],
    order: ScanOrder,
    rows: usize,
) -> Result<Vec<Value>> {
    let rows = executor.all(
        &format!(
            "SELECT record FROM {table} WHERE {} ORDER BY id {} LIMIT ?",
            clauses.join(" AND "),
            order.direction()
        ),
        &[params, &[Cell::Integer(rows as i64)]].concat(),
    )?;
    rows.iter()
        .map(|row| {
            parse_json(
                row.get("record")
                    .and_then(Cell::as_str)
                    .ok_or_else(|| StorageError::Shape("row has no record".to_string()))?,
            )
        })
        .collect()
}

/// 按 id 取一条记录的 `record` 列（不解释内容）。
pub fn record_by_id(executor: &dyn SqliteExecutor, table: &str, id: i64) -> Result<Option<Value>> {
    let row = executor.get(&format!("SELECT record FROM {table} WHERE id = ?"), &[Cell::Integer(id)])?;
    row.as_ref().map(|row| {
        parse_json(
            row.get("record")
                .and_then(Cell::as_str)
                .ok_or_else(|| StorageError::Shape("row has no record".to_string()))?,
        )
    })
    .transpose()
}

/// 游标的起点子句：升序 `id > after`，降序 `id < after`；没有游标就是整表。
fn start_clause(order: ScanOrder, after: Option<i64>) -> (String, Cell) {
    let opening = match order {
        ScanOrder::Ascending => -1,
        ScanOrder::Descending => i64::MAX,
    };
    let comparison = match order {
        ScanOrder::Ascending => "id > ?",
        ScanOrder::Descending => "id < ?",
    };
    (comparison.to_string(), Cell::Integer(after.unwrap_or(opening)))
}

fn page_of(values: Vec<Value>, limit: usize, order: ScanOrder) -> Page<Value> {
    cut_page(values, limit, order, |record| {
        record.get("id").and_then(Value::as_i64).unwrap_or(0)
    })
}

/// 一条任务记录（上游 `storage.task(id)`）。不解释 `state`：它是调用方的不透明 JSON。
pub fn task(executor: &dyn SqliteExecutor, schema: &SqliteSchema, id: i64) -> Result<Option<Value>> {
    record_by_id(executor, schema.table("tasks"), id)
}

/// 按 id 升/降扫任务，可按会话、种类、状态、`abortRequested`、`background` 过滤。
pub fn scan_tasks(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    query: &TaskQuery,
    limit: usize,
    cursor: Option<&Cursor>,
) -> Result<std::result::Result<Page<Value>, Refusal>> {
    let (order, after) = match scan_start(query.order, cursor, ScanOrder::Ascending) {
        Ok(start) => start,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let (clause, after_value) = start_clause(order, after);
    let mut clauses = vec![clause];
    let mut params = vec![after_value];
    if let Some(conversation_id) = query.conversation_id {
        clauses.push("conversation_id = ?".to_string());
        params.push(Cell::Integer(conversation_id));
    }
    if let Some(kind) = &query.kind {
        clauses.push("kind = ?".to_string());
        params.push(Cell::Text(encode_indexed_string(kind)));
    }
    if let Some(status) = &query.status {
        clauses.push("status = ?".to_string());
        params.push(Cell::Text(status.clone()));
    }
    if let Some(abort_requested) = query.abort_requested {
        clauses.push("abort_requested = ?".to_string());
        params.push(Cell::Integer(if abort_requested { 1 } else { 0 }));
    }
    if let Some(background) = query.background {
        clauses.push("background = ?".to_string());
        params.push(Cell::Integer(if background { 1 } else { 0 }));
    }
    let values = fetch_records(executor, schema.table("tasks"), &clauses, &params, order, limit + 1)?;
    Ok(Ok(page_of(values, limit, order)))
}

/// 一条提交记录（上游 `storage.submission(id)`）。
pub fn submission(executor: &dyn SqliteExecutor, schema: &SqliteSchema, id: i64) -> Result<Option<Value>> {
    record_by_id(executor, schema.table("submissions"), id)
}

/// `(conversationId, requestId)` 的幂等查找（上游 `submissionByRequest`）。
///
/// 这一条是 `POST /messages` 的幂等键：同一个 `requestId` 打两次必须拿到同一条 submission。
/// 表里**不建**唯一索引（`docs/storage.md` §9），所以这里是「取 id 最小的那一条」而不是「唯一成立」。
pub fn submission_by_request(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    conversation_id: i64,
    request_id: &str,
) -> Result<Option<Value>> {
    let row = executor.get(
        &format!(
            "SELECT record FROM {} WHERE conversation_id = ? AND request_id = ? ORDER BY id LIMIT 1",
            schema.table("submissions")
        ),
        &[Cell::Integer(conversation_id), Cell::Text(encode_indexed_string(request_id))],
    )?;
    row.as_ref()
        .map(|row| {
            parse_json(
                row.get("record")
                    .and_then(Cell::as_str)
                    .ok_or_else(|| StorageError::Shape("row has no record".to_string()))?,
            )
        })
        .transpose()
}

/// 按 id 升/降扫提交记录，可按会话与状态过滤。
pub fn scan_submissions(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    query: &SubmissionQuery,
    limit: usize,
    cursor: Option<&Cursor>,
) -> Result<std::result::Result<Page<Value>, Refusal>> {
    let (order, after) = match scan_start(query.order, cursor, ScanOrder::Ascending) {
        Ok(start) => start,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let (clause, after_value) = start_clause(order, after);
    let mut clauses = vec![clause];
    let mut params = vec![after_value];
    if let Some(conversation_id) = query.conversation_id {
        clauses.push("conversation_id = ?".to_string());
        params.push(Cell::Integer(conversation_id));
    }
    if let Some(status) = &query.status {
        clauses.push("status = ?".to_string());
        params.push(Cell::Text(status.clone()));
    }
    let values = fetch_records(executor, schema.table("submissions"), &clauses, &params, order, limit + 1)?;
    Ok(Ok(page_of(values, limit, order)))
}

/// 按**地址**精确找一份在世的文档（上游 `storage.findDocument`）。
///
/// 地址 = `kind` + `scope` + `key`（`key` 缺席与 `key: ""` 是**两个不同的地址**：索引列里
/// `family` 把它们分开，见 [`super::sqlite::rows::address_parts`]）。所以这里比的是五个索引列，
/// 不是「拿出来之后再看一眼」。
pub fn find_document(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    address: &Value,
    at: DocumentPoint,
) -> Result<Option<Value>> {
    let parts = super::sqlite::rows::address_parts(address)?;
    let mut clauses = vec![
        "kind = ?".to_string(),
        "scope_kind = ?".to_string(),
        "owner_id = ?".to_string(),
        "family = ?".to_string(),
        "key_value = ?".to_string(),
    ];
    let mut params = vec![
        Cell::Text(parts.kind.clone()),
        Cell::Text(parts.scope_kind.to_string()),
        Cell::Integer(parts.owner_id),
        Cell::Integer(parts.family),
        Cell::Text(parts.key_value.clone()),
    ];
    alive_clauses(at, &mut clauses, &mut params);
    // 同一个地址在 `at` 上可能有多个化身（左闭右开区间里先后相接的两个）：取**最新**那个
    // （`ORDER BY created_at DESC` —— 照抄上游 `FIND_ENTRY_AT`；按 id 排也能得到同样的结果，
    // 但那是巧合，不是保证）。
    let row = executor.get(
        &format!(
            "SELECT record FROM {} WHERE {} ORDER BY created_at DESC LIMIT 1",
            schema.table("documents"),
            clauses.join(" AND ")
        ),
        &params,
    )?;
    row.as_ref()
        .map(|row| {
            parse_json(
                row.get("record")
                    .and_then(Cell::as_str)
                    .ok_or_else(|| StorageError::Shape("row has no record".to_string()))?,
            )
        })
        .transpose()
}

/// 按**作用域**扫在世的文档（上游 `storage.scanDocuments`）：`kind` 可选，`key` 不参与。
///
/// 上游这条扫描**没有方向**（`cursorId(cursor) ?? -1` + 固定升序），所以游标带 `d` 前缀
/// （降序）会被拒绝 —— 不是「忽略方向」，那会让调用方以为自己在降序翻页。
pub fn scan_documents(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    query: &DocumentQuery,
    limit: usize,
    cursor: Option<&Cursor>,
) -> Result<std::result::Result<Page<Value>, Refusal>> {
    if let Some(cursor) = cursor {
        if cursor.order != ScanOrder::Ascending {
            return Ok(Err(Refusal::CursorOrderMismatch));
        }
    }
    let columns = scope_columns(&query.scope)?;
    let after = cursor.map(|cursor| cursor.after).unwrap_or(-1);
    let mut clauses = vec![
        "id > ?".to_string(),
        "scope_kind = ?".to_string(),
        "owner_id = ?".to_string(),
    ];
    let mut params = vec![
        Cell::Integer(after),
        Cell::Text(columns.scope_kind.to_string()),
        Cell::Integer(columns.owner_id),
    ];
    if let Some(kind) = &query.kind {
        clauses.push("kind = ?".to_string());
        params.push(Cell::Text(encode_indexed_string(kind)));
    }
    alive_clauses(query.at, &mut clauses, &mut params);
    let values = fetch_records(
        executor,
        schema.table("documents"),
        &clauses,
        &params,
        ScanOrder::Ascending,
        limit + 1,
    )?;
    Ok(Ok(page_of(values, limit, ScanOrder::Ascending)))
}

/// 「在这个点上在世」的两条子句，**照抄上游 `LIST_ENTRIES` / `FIND_ENTRY_*` 的 SQL**。
///
/// 生命周期是左闭右开：`created_at <= at` 且（没退役 或 `retired_at > at`）。`Current` 只看
/// 「没退役」——**不看 `created_at`**，因为「现在」永远在创建之后。反过来写成 `created_at <= 现在`
/// 也能跑，但那要求调用方给一个「现在」的 seq，而 `Current` 根本没有那样的数。
fn alive_clauses(at: DocumentPoint, clauses: &mut Vec<String>, params: &mut Vec<Cell>) {
    match at {
        DocumentPoint::Current => clauses.push("retired_at IS NULL".to_string()),
        DocumentPoint::At(seq) => {
            clauses.push("created_at <= ?".to_string());
            params.push(Cell::Integer(seq));
            clauses.push("(retired_at IS NULL OR retired_at > ?)".to_string());
            params.push(Cell::Integer(seq));
        }
    }
}

/// fork 链上的一条 entry 段：`conversation_id` 的 `[lower, upper]`（两端都可缺席）。
struct Segment {
    conversation_id: i64,
    upper: Option<i64>,
}

/// 一条会话记录里 `parent` 的 `(conversationId, at)`；没有父就是 `None`。
fn parent_of(record: &Value) -> Option<(i64, i64)> {
    let parent = record.get("parent")?;
    let conversation_id = parent.get("conversationId").and_then(Value::as_i64)?;
    let at = parent.get("at").and_then(Value::as_i64)?;
    Some((conversation_id, at))
}

fn min_of(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(one), None) | (None, Some(one)) => Some(one),
        (None, None) => None,
    }
}

/// 一条 entry 段里 `[lower, upper]` 的记录，按 `order` 取。
fn segment_records(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    segment: &Segment,
    lower: Option<i64>,
    order: ScanOrder,
    remaining: usize,
) -> Result<Vec<Value>> {
    let mut clauses = vec!["conversation_id = ?".to_string()];
    let mut params = vec![Cell::Integer(segment.conversation_id)];
    if let Some(upper) = segment.upper {
        clauses.push("id <= ?".to_string());
        params.push(Cell::Integer(upper));
    }
    if let Some(lower) = lower {
        clauses.push("id >= ?".to_string());
        params.push(Cell::Integer(lower));
    }
    fetch_records(executor, schema.table("entries"), &clauses, &params, order, remaining)
}

/// fork 链上的 entry 扫描（上游 `#entriesDescending` / `#entriesAscending`）。
///
/// ## 为什么一条会话不够
///
/// fork 出来的会话只存**自己那一段**的 entry；它之前的历史住在父会话里，父的那一段到 fork 点
/// （`parent.at`）为止。所以「这个会话的历史」是一条**链**：从自己往上走到没有父为止，每一段
/// 上界分别是「上一段的 fork 点」。
///
/// 两条方向的做法不一样，因为它们读库的顺序不同：
///
/// - **降序**（最新的先）：从自己开始往下读，读满就停；没读满就把 `upper` 收到 `parent.at` 换去
///   父会话。`upper` 一路取 `min`，所以更上面的段绝不会把下面已经跨过的 fork 点之后的东西吐出来。
/// - **升序**（最老的先）：先把链走完、收集成段，再**反过来**从最老的读 —— 升序的第一页必须来自
///   根，而根是走到链尾才知道的。游标起点 `after + 1` 是每一段的共同下界（更上面的段本来就更老，
///   多一道下界不改变结果）。
pub fn scan_entry_chain(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    query: &super::storage::EntryQuery,
    limit: usize,
    cursor: Option<&Cursor>,
) -> Result<std::result::Result<Page<Value>, Refusal>> {
    let (order, after) = match scan_start(query.order, cursor, ScanOrder::Descending) {
        Ok(start) => start,
        Err(refusal) => return Ok(Err(refusal)),
    };
    // 会话不在就按「空页」处理（`#21` 的取舍，写进 `docs/storage.md` §11.4）：本库不区分
    // 「这个会话没有 entry」与「这个会话不存在」，两者都读出一页空。
    let Some(mut conversation) = record_by_id(executor, schema.table("conversations"), query.conversation_id)? else {
        return Ok(Ok(Page {
            items: Vec::new(),
            next: None,
        }));
    };

    match order {
        ScanOrder::Descending => {
            let mut upper = min_of(query.max_entry_id, after.map(|after| after - 1));
            let mut values: Vec<Value> = Vec::new();
            loop {
                let segment = Segment {
                    conversation_id: required_i64(&conversation, "id")?,
                    upper,
                };
                let mut rows = segment_records(
                    executor,
                    schema,
                    &segment,
                    query.min_entry_id,
                    order,
                    limit + 1 - values.len(),
                )?;
                values.append(&mut rows);
                let Some((parent_id, parent_at)) = parent_of(&conversation) else {
                    break;
                };
                if values.len() > limit {
                    break;
                }
                upper = min_of(upper, Some(parent_at));
                if let (Some(min), Some(upper)) = (query.min_entry_id, upper) {
                    if upper < min {
                        break;
                    }
                }
                conversation = record_by_id(executor, schema.table("conversations"), parent_id)?
                    .ok_or_else(|| {
                        StorageError::Shape(format!("conversation {parent_id} is missing from its own fork chain"))
                    })?;
            }
            Ok(Ok(page_of(values, limit, order)))
        }
        ScanOrder::Ascending => {
            let mut segments: Vec<Segment> = Vec::new();
            let mut upper = query.max_entry_id;
            loop {
                segments.push(Segment {
                    conversation_id: required_i64(&conversation, "id")?,
                    upper,
                });
                let Some((parent_id, parent_at)) = parent_of(&conversation) else {
                    break;
                };
                upper = min_of(upper, Some(parent_at));
                if let (Some(min), Some(upper)) = (query.min_entry_id, upper) {
                    if upper < min {
                        break;
                    }
                }
                conversation = record_by_id(executor, schema.table("conversations"), parent_id)?
                    .ok_or_else(|| {
                        StorageError::Shape(format!("conversation {parent_id} is missing from its own fork chain"))
                    })?;
            }
            let lower = match (query.min_entry_id, after) {
                (Some(min), Some(after)) => Some(min.max(after + 1)),
                (Some(min), None) => Some(min),
                (None, Some(after)) => Some(after + 1),
                (None, None) => None,
            };
            let mut values: Vec<Value> = Vec::new();
            for segment in segments.iter().rev() {
                let mut rows =
                    segment_records(executor, schema, segment, lower, order, limit + 1 - values.len())?;
                values.append(&mut rows);
                if values.len() > limit {
                    break;
                }
            }
            Ok(Ok(page_of(values, limit, order)))
        }
    }
}
