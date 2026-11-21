//! 会话归档 generations（#67）——选项甲：每代一份独立 SQLite 文件。
//!
//! `archive` = 把会话闭包复印进 `archives/gen-YYYYMMDD.db` + 在活库写墓碑；
//! `restore` = 从 gen 文件原样搬回活库 + 抹墓碑。`purge` 走 #64 的
//! [`Storage::purge_conversation`]（要求先有墓碑）。gen 文件用 durable 自己的迁移链建
//! schema，但**不进 durable 的迁移链**，也不被 `--check` / `--backup` 看见。
//!
//! # 单写者
//`
//! 所有活库写都经同一个 `Storage`：墓碑用 `StorageWrite::DocumentChange` 走 `Session::commit`
//! 那条路径（上层命令层持有同一把写锁），从 gen 文件搬回行则在 `Storage` 的连接上跑一个事务。
//! gen 文件只在 archive 时由本模块写，restore 只读它。不另开进程、不另开活库连接。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nous_chord::sqlite::{SqliteDatabase, SqliteExecutor, SqliteRow, SqliteValue as Cell};
use serde_json::{json, Value};

use super::import::ImportedStore;
use super::sqlite::rows::{document_row, parse_json};
use super::sqlite::schema::SqliteSchema;
use super::sqlite::statements::insert_rows;
use super::sqlite::tables::{
    conversation_row, entry_row, record_id_row, revision_row, submission_row, task_row,
    CONVERSATION_COLUMNS, DOCUMENT_REVISION_COLUMNS, ENTRY_COLUMNS, RECORD_ID_COLUMNS,
    SUBMISSION_COLUMNS, TASK_COLUMNS,
};
use super::SqliteStore;
use crate::error::StorageError;
use crate::{DocumentContent, DocumentPoint, Storage, StorageWrite};

/// 归档目录名（相对 `NOUS_DB` 的父目录）；归位制登记在 `docs/plugins.md`。
pub const ARCHIVE_DIR: &str = "archives";

pub type ArchiveResult<T> = std::result::Result<T, ArchiveError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveOutcome {
    pub gen: String,
    pub ids: Vec<i64>,
    pub rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    pub gen: String,
    pub items: Vec<ArchiveItem>,
}

/// `GET /v1/archive` 的一条。四态判据由 `archived` + `live` 给出，客户端不推。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveItem {
    pub id: i64,
    pub parent: Option<(i64, i64)>,
    pub entries: usize,
    pub last: Option<(i64, String, String)>,
    pub name: String,
    pub archived: Option<ArchivedTombstone>,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedTombstone {
    pub at: i64,
    pub gen: String,
    pub purged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    NotFound(i64),
    Busy(i64),
    LiveDescendants(i64, Vec<i64>),
    MissingGeneration { gen: String, id: i64 },
    AlreadyLive(i64),
    Storage(StorageError),
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "session {id} not found"),
            Self::Busy(id) => write!(f, "session {id} 忙，不能归档"),
            Self::LiveDescendants(id, children) => {
                write!(f, "会话 {id} 有活后代 {children:?}，先处理它的活分支")
            }
            Self::MissingGeneration { gen, id } => write!(f, "generation {gen} 里没有会话 {id}"),
            Self::AlreadyLive(id) => write!(f, "会话 {id} 已经在活库里 —— 不用捞"),
            Self::Storage(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ArchiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self { Self::Storage(error) => Some(error), _ => None }
    }
}

impl From<StorageError> for ArchiveError {
    fn from(error: StorageError) -> Self { Self::Storage(error) }
}

impl From<nous_chord::sqlite::SqliteError> for ArchiveError {
    fn from(error: nous_chord::sqlite::SqliteError) -> Self { Self::Storage(error.into()) }
}

pub fn archive_dir(db: &Path) -> PathBuf {
    db.parent().unwrap_or_else(|| Path::new(".")).join(ARCHIVE_DIR)
}

fn gen_path(db: &Path, gen: &str) -> PathBuf {
    archive_dir(db).join(format!("gen-{gen}.db"))
}

fn open_generation(path: &Path) -> ArchiveResult<SqliteStore> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| StorageError::Shape(format!(
            "create archive dir {}: {error}", parent.display()
        )))?;
    }
    // 同日重写按 id 先清后写（幂等）。gen 文件是本库自己建的，writer 认门照旧。
    // 只有 restore 会只读打开它，这里始终是可写打开。
    Ok(SqliteStore::open_writer(path)?)
}

/// 会话级文档：`(化身 id, 当前值)`。墓碑住在 `qrd3wh.sessions.archived`。
pub(crate) fn session_document(storage: &Storage) -> ArchiveResult<(Option<i64>, Value)> {
    let Some(record) = storage.find_document(
        &json!({ "kind": "qrd3wh.sessions", "scope": { "kind": "session" } }),
        DocumentPoint::Current,
    )? else { return Ok((None, json!({}))) };
    let id = record.get("id").and_then(Value::as_i64)
        .ok_or_else(|| StorageError::Shape("session document has no id".into()))?;
    let value = storage.materialize(id, DocumentPoint::Current)?
        .map(|doc| doc.value).unwrap_or_else(|| json!({}));
    Ok((Some(id), value))
}

pub(crate) fn tombstone(value: &Value, id: i64) -> Option<ArchivedTombstone> {
    let entry = value.get("archived")?.get(&id.to_string())?;
    Some(ArchivedTombstone {
        at: entry.get("at")?.as_i64()?,
        gen: entry.get("gen")?.as_str()?.to_string(),
        purged: entry.get("purged")?.as_bool()?,
    })
}

/// 会话闭包：七张有数据的表的原始行 + id 台账。
#[derive(Debug, Default)]
pub(crate) struct Closure {
    pub conversations: Vec<SqliteRow>,
    pub entries: Vec<SqliteRow>,
    pub tasks: Vec<SqliteRow>,
    pub submissions: Vec<SqliteRow>,
    pub documents: Vec<SqliteRow>,
    pub revisions: Vec<SqliteRow>,
    pub record_ids: BTreeSet<(i64, String)>,
    pub rows: usize,
}

fn row_i64(row: &SqliteRow, key: &str) -> Option<i64> {
    row.get(key).and_then(Cell::as_i64)
}

pub(crate) fn row_id(row: &SqliteRow) -> Option<i64> { row_i64(row, "id") }

fn record_of(row: &SqliteRow) -> ArchiveResult<Value> {
    Ok(parse_json(row.get("record").and_then(Cell::as_str)
        .ok_or_else(|| StorageError::Shape("row has no record".into()))?)?)
}

fn owner_of(row: &SqliteRow) -> ArchiveResult<Option<i64>> {
    let record = record_of(row)?;
    Ok(record.get("conversationId").and_then(Value::as_i64)
        .or_else(|| row_i64(row, "conversation_id")))
}

fn document_owner(row: &SqliteRow) -> ArchiveResult<Option<i64>> {
    let record = record_of(row)?;
    Ok(record.get("scope").and_then(Value::as_object)
        .and_then(|scope| scope.get("conversationId").or_else(|| scope.get("conversation_id")))
        .and_then(Value::as_i64)
        .or_else(|| record.get("conversationId").and_then(Value::as_i64)))
}

/// fork 祖先链的会话 id（从 root 往上走，带环保护）。
pub(crate) fn ancestor_ids(all: &ImportedStore, root: i64) -> ArchiveResult<BTreeSet<i64>> {
    let mut ids = BTreeSet::new();
    ids.insert(root);
    let mut cursor = root;
    for _ in 0..256 {
        let Some(row) = all.conversations.iter()
            .find(|row| row_i64(row, "id") == Some(cursor)) else { break };
        let parent = record_of(row)?
            .get("parent").and_then(|p| p.get("conversationId")).and_then(Value::as_i64);
        let Some(parent) = parent else { break };
        if !ids.insert(parent) { break; }
        cursor = parent;
    }
    Ok(ids)
}

pub(crate) fn closure_from(all: &ImportedStore, ids: &BTreeSet<i64>) -> ArchiveResult<Closure> {
    let mut closure = Closure::default();
    closure.conversations = all.conversations.iter()
        .filter(|row| row_i64(row, "id").is_some_and(|id| ids.contains(&id)))
        .cloned().collect();
    closure.entries = all.entries.iter()
        .filter(|row| owner_of(row).ok().flatten().is_some_and(|id| ids.contains(&id)))
        .cloned().collect();
    closure.tasks = all.tasks.iter()
        .filter(|row| owner_of(row).ok().flatten().is_some_and(|id| ids.contains(&id)))
        .cloned().collect();
    closure.submissions = all.submissions.iter()
        .filter(|row| owner_of(row).ok().flatten().is_some_and(|id| ids.contains(&id)))
        .cloned().collect();
    let document_ids: BTreeSet<i64> = all.documents.iter()
        .filter(|row| document_owner(row).ok().flatten().is_some_and(|id| ids.contains(&id)))
        .filter_map(|row| row_i64(row, "id")).collect();
    closure.documents = all.documents.iter()
        .filter(|row| row_i64(row, "id").is_some_and(|id| document_ids.contains(&id)))
        .cloned().collect();
    closure.revisions = all.document_revisions.iter()
        .filter(|row| row_i64(row, "document_id").is_some_and(|id| document_ids.contains(&id)))
        .cloned().collect();
    let entry_ids: BTreeSet<i64> = closure.entries.iter().filter_map(|r| row_i64(r, "id")).collect();
    let task_ids: BTreeSet<i64> = closure.tasks.iter().filter_map(|r| row_i64(r, "id")).collect();
    let submission_ids: BTreeSet<i64> = closure.submissions.iter().filter_map(|r| row_i64(r, "id")).collect();
    closure.record_ids = all.record_ids.iter().filter_map(|row| {
        let id = row_i64(row, "id")?;
        let kind = row.get("record_type")?.as_str()?.to_string();
        let keep = match kind.as_str() {
            "conversation" => ids.contains(&id),
            "entry" => entry_ids.contains(&id),
            "task" => task_ids.contains(&id),
            "submission" => submission_ids.contains(&id),
            "document" => document_ids.contains(&id),
            _ => false,
        };
        keep.then_some((id, kind))
    }).collect();
    closure.rows = closure.conversations.len() + closure.entries.len() + closure.tasks.len()
        + closure.submissions.len() + closure.documents.len() + closure.revisions.len()
        + closure.record_ids.len();
    Ok(closure)
}

/// 写入任意库：按 id 先清后写，保证同日二次归档不产生重复行。
pub(crate) fn write_closure(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    closure: &Closure,
) -> ArchiveResult<()> {
    let delete = |table: &str, ids: Vec<i64>| -> ArchiveResult<()> {
        for id in ids {
            executor.run(&format!("DELETE FROM {} WHERE id = ?", schema.table(table)), &[Cell::Integer(id)])?;
        }
        Ok(())
    };
    delete("conversations", closure.conversations.iter().filter_map(|r| row_i64(r, "id")).collect())?;
    delete("entries", closure.entries.iter().filter_map(|r| row_i64(r, "id")).collect())?;
    delete("tasks", closure.tasks.iter().filter_map(|r| row_i64(r, "id")).collect())?;
    delete("submissions", closure.submissions.iter().filter_map(|r| row_i64(r, "id")).collect())?;
    delete("documents", closure.documents.iter().filter_map(|r| row_i64(r, "id")).collect())?;
    for row in &closure.revisions {
        executor.run(
            &format!("DELETE FROM {} WHERE document_id = ? AND seq = ?", schema.table("document_revisions")),
            &[row_i64(row, "document_id").unwrap_or(0).into(), row_i64(row, "seq").unwrap_or(0).into()],
        )?;
    }
    for (id, _) in &closure.record_ids {
        executor.run(&format!("DELETE FROM {} WHERE id = ?", schema.table("record_ids")), &[Cell::Integer(*id)])?;
    }
    let mapped: Vec<Vec<Cell>> = closure.conversations.iter()
        .map(|row| -> ArchiveResult<Vec<Cell>> { Ok(conversation_row(&record_of(row)?)?) })
        .collect::<ArchiveResult<_>>()?;
    insert_rows(executor, schema.table("conversations"), CONVERSATION_COLUMNS, &mapped, "")?;
    let mapped: Vec<Vec<Cell>> = closure.entries.iter()
        .map(|row| -> ArchiveResult<Vec<Cell>> {
            Ok(entry_row(&record_of(row)?, row_i64(row, "commit_seq").unwrap_or(1))?)
        })
        .collect::<ArchiveResult<_>>()?;
    insert_rows(executor, schema.table("entries"), ENTRY_COLUMNS, &mapped, "")?;
    let mapped: Vec<Vec<Cell>> = closure.tasks.iter()
        .map(|row| -> ArchiveResult<Vec<Cell>> { Ok(task_row(&record_of(row)?)?) })
        .collect::<ArchiveResult<_>>()?;
    insert_rows(executor, schema.table("tasks"), TASK_COLUMNS, &mapped, "")?;
    let mapped: Vec<Vec<Cell>> = closure.submissions.iter()
        .map(|row| -> ArchiveResult<Vec<Cell>> { Ok(submission_row(&record_of(row)?)?) })
        .collect::<ArchiveResult<_>>()?;
    insert_rows(executor, schema.table("submissions"), SUBMISSION_COLUMNS, &mapped, "")?;
    let mapped: Vec<Vec<Cell>> = closure.documents.iter()
        .map(|row| -> ArchiveResult<Vec<Cell>> { Ok(document_row(&record_of(row)?)?) })
        .collect::<ArchiveResult<_>>()?;
    insert_rows(executor, schema.table("documents"), super::sqlite::tables::DOCUMENT_COLUMNS, &mapped, "")?;
    let mapped: Vec<Vec<Cell>> = closure.revisions.iter().map(|row| -> ArchiveResult<Vec<Cell>> {
        let content: Value = parse_json(row.get("content").and_then(Cell::as_str)
            .ok_or_else(|| StorageError::Shape("revision has no content".into()))?)?;
        Ok(revision_row(
            row_i64(row, "document_id").ok_or_else(|| StorageError::Shape("revision has no document_id".into()))?,
            row_i64(row, "seq").ok_or_else(|| StorageError::Shape("revision has no seq".into()))?,
            row.get("kind").and_then(Cell::as_str).unwrap_or("base"),
            row_i64(row, "version").unwrap_or(1),
            &content,
        ))
    }).collect::<ArchiveResult<_>>()?;
    insert_rows(executor, schema.table("document_revisions"), DOCUMENT_REVISION_COLUMNS, &mapped, "")?;
    let mapped: Vec<Vec<Cell>> = closure.record_ids.iter().map(|(id, kind)| record_id_row(*id, kind)).collect();
    insert_rows(executor, schema.table("record_ids"), RECORD_ID_COLUMNS, &mapped, "")?;
    Ok(())
}

/// 前置条件：会话存在、不忙、没有活后代。
pub(crate) fn check_archive(storage: &Storage, id: i64) -> ArchiveResult<()> {
    if storage.conversation(id)?.is_none() {
        return Err(ArchiveError::NotFound(id));
    }
    let busy = storage.tasks(id)?.iter().any(|record| {
        record.get("state").and_then(Value::as_object)
            .and_then(|state| state.get("status")).and_then(Value::as_str)
            .is_some_and(|status| !matches!(status, "terminal" | "aborted"))
    });
    if busy { return Err(ArchiveError::Busy(id)); }
    let all = ImportedStore::read(storage.db_path())?;
    let children: Vec<i64> = all.conversations.iter().filter_map(|row| {
        let record = record_of(row).ok()?;
        (record.get("parent").and_then(|p| p.get("conversationId")).and_then(Value::as_i64) == Some(id))
            .then_some(row_i64(row, "id").unwrap_or(0))
    }).collect();
    if !children.is_empty() {
        return Err(ArchiveError::LiveDescendants(id, children));
    }
    Ok(())
}

/// 墓碑写回的 `StorageWrite`。调用方拿 `Session` 的写锁、执行、发布；这里只造写。
pub(crate) fn tombstone_write(
    storage: &mut Storage,
    id: i64,
    at: i64,
    gen: &str,
    purged: bool,
    clear: bool,
) -> ArchiveResult<StorageWrite> {
    let (existing, mut value) = session_document(storage)?;
    if !value.is_object() { value = json!({}); }
    let mut archived = value.get("archived").cloned().unwrap_or_else(|| json!({}));
    if !archived.is_object() { archived = json!({}); }
    if clear {
        if let Some(object) = archived.as_object_mut() { object.remove(&id.to_string()); }
    } else {
        archived[&id.to_string()] = json!({ "at": at, "gen": gen, "purged": purged });
    }
    value["archived"] = archived;
    let content = DocumentContent::Base { version: 1, value };
    Ok(match existing {
        Some(doc_id) => StorageWrite::DocumentChange { id: doc_id, content },
        None => StorageWrite::DocumentCreate {
            id: {
                let id = storage.mint_id()?;
                id
            },
            record: json!({ "id": storage.next_id() - 1, "kind": "qrd3wh.sessions", "scope": { "kind": "session" } }),
            content,
        },
    })
}

/// `gen` 名字：8 位 `YYYYMMDD`，由调用方注入的时钟算（kernel 零直接时钟）。
pub(crate) fn gen_for(at_ms: i64) -> String {
    let (y, m, d) = civil_from_days(at_ms.div_euclid(86_400_000));
    format!("{y:04}{m:02}{d:02}")
}

/// 校验 generation slug（来自 HTTP body，必须 8 位数字）。
pub(crate) fn read_gen(gen: &str) -> ArchiveResult<()> {
    if gen.len() == 8 && gen.bytes().all(|b| b.is_ascii_digit()) { Ok(()) }
    else { Err(StorageError::Shape(format!("invalid generation {gen:?}")).into()) }
}

pub(crate) fn generation_path(db: &Path, gen: &str) -> PathBuf { gen_path(db, gen) }

/// 读闭包（活库）。调用方持有写锁。
pub(crate) fn read_live_closure(storage: &Storage, id: i64) -> ArchiveResult<Closure> {
    let all = ImportedStore::read(storage.db_path())?;
    closure_from(&all, &ancestor_ids(&all, id)?)
}

/// `GET /v1/archive` 的数据装配（只读）。墓碑从活库文档读，live 从活库会话读。
pub fn list(storage: &Storage) -> ArchiveResult<Vec<Generation>> {
    let (_, session_value) = session_document(storage)?;
    let Ok(entries) = std::fs::read_dir(archive_dir(&storage.db_path())) else { return Ok(Vec::new()) };
    let mut files = entries.filter_map(|entry| entry.ok()).map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("db")
            && path.file_name().and_then(|s| s.to_str()).is_some_and(|n| n.starts_with("gen-")))
        .collect::<Vec<_>>();
    files.sort();
    let mut generations = Vec::new();
    for path in files {
        let gen = path.file_stem().and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("gen-")).unwrap_or_default().to_string();
        let Ok(from) = ImportedStore::read(&path) else { continue };
        let mut items = Vec::new();
        for row in &from.conversations {
            let id = row_i64(row, "id").unwrap_or(0);
            let record = record_of(row)?;
            let name = record.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let parent = record.get("parent").and_then(|p| Some((
                p.get("conversationId")?.as_i64()?, p.get("at")?.as_i64()?,
            )));
            let own: Vec<&SqliteRow> = from.entries.iter()
                .filter(|r| owner_of(r).ok().flatten() == Some(id)).collect();
            let last = own.iter().filter_map(|r| {
                let record = record_of(r).ok()?;
                Some((row_i64(r, "id")?, record.get("kind")?.as_str()?.to_string(),
                    record.get("text").and_then(Value::as_str).unwrap_or_default().to_string()))
            }).max_by_key(|item| item.0);
            items.push(ArchiveItem {
                id, parent, entries: own.len(), last, name,
                archived: tombstone(&session_value, id),
                live: storage.conversation(id)?.is_some(),
            });
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.id));
        generations.push(Generation { gen, items });
    }
    Ok(generations)
}

/// 从 generation 文件读闭包（只读）。
pub(crate) fn read_generation_closure(db: &Path, id: i64, gen: &str) -> ArchiveResult<Closure> {
    read_gen(gen)?;
    let from = ImportedStore::read(generation_path(db, gen))?;
    if !from.conversations.iter().any(|row| row_i64(row, "id") == Some(id)) {
        return Err(ArchiveError::MissingGeneration { gen: gen.to_string(), id });
    }
    closure_from(&from, &ancestor_ids(&from, id)?)
}

/// 把闭包写进活库（调用方持有写锁 + 事务）。
pub(crate) fn restore_into(storage: &mut Storage, closure: &Closure) -> ArchiveResult<()> {
    let schema = storage.store().schema().clone();
    storage.store_mut().db_mut().transaction(|tx| write_closure(tx, &schema, closure))?;
    Ok(())
}

/// 把闭包写进 generation 文件（同日按 id 先清后写，不重复）。
pub(crate) fn write_generation(db: &Path, gen: &str, closure: &Closure) -> ArchiveResult<()> {
    let mut generation = open_generation(&generation_path(db, gen))?;
    let schema = generation.schema().clone();
    generation.db_mut().transaction(|tx| write_closure(tx, &schema, closure))?;
    Ok(())
}

/// Howard Hinnant 的 civil calendar 算法（UTC，不引 chrono）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + i64::from(m <= 2), m, d)
}
