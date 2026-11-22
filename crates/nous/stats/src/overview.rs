//! `GET /v1/stats/overview` 的读口：durable 库的 X 光片（#数据监控室复原）。
//!
//! 六屏里的「解剖」：把 `nous.db` 摊开 —— 文件字节、pragma、每张表的真页数与行数、
//! 索引、entries 的 kind 分布、integrity。数据只有一处来源：`sqlite_master` / `dbstat` /
//! `pragma` / `fs::metadata`，**不落快照、不建索引、不写库**。
//!
//! `dbstat` 是 SQLite 编译进去的虚表（rusqlite `bundled` 默认有）；真拿不到时退化为
//! 「表有行数、没有按页字节」，不整屏崩 —— 判据看 `dbstatOk`。

use std::collections::BTreeMap;
use std::path::Path;

use nous_chord::sqlite::SqliteExecutor;
use serde_json::{json, Value};

use crate::util;
use crate::Result;

/// `PRAGMA integrity_check` 只在库小于这个体积时跑（大了会几分钟，面板同规矩）。
const INTEGRITY_MAX_BYTES: i64 = 64 * 1024 * 1024;

/// 一张表的读数：名字、真字节、真页数、行数。
fn table_entry(name: &str, bytes: i64, pages: i64, rows: i64) -> Value {
    json!({ "name": name, "bytes": bytes, "pages": pages, "rows": rows })
}

/// `dbstat` 的一次读：按名字分「表」与「索引」两组。
///
/// 返回 `(tables, indexes, ok, why)`；`ok=false` 时两组为空、`why` 是原因。
fn dbstat(db: &dyn SqliteExecutor) -> (BTreeMap<String, (i64, i64)>, BTreeMap<String, (i64, i64, String)>, bool, String) {
    let rows = match db.all(
        "SELECT name, sum(pgsize) AS bytes, count(*) AS pages FROM dbstat GROUP BY name",
        &[],
    ) {
        Ok(rows) => rows,
        Err(error) => return (BTreeMap::new(), BTreeMap::new(), false, error.to_string()),
    };
    let table_names: Vec<String> = util::all(
        db,
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .unwrap_or_default()
    .iter()
    .map(|row| util::col_str(row, "name"))
    .collect();
    let table_set: std::collections::BTreeSet<String> = table_names.into_iter().collect();
    let index_owner: BTreeMap<String, String> = util::all(
        db,
        "SELECT name, tbl_name FROM sqlite_master WHERE type='index' AND tbl_name NOT LIKE 'sqlite_%'",
    )
    .unwrap_or_default()
    .iter()
    .map(|row| (util::col_str(row, "name"), util::col_str(row, "tbl_name")))
    .collect();

    let mut tables = BTreeMap::new();
    let mut indexes = BTreeMap::new();
    for row in rows {
        let name = util::col_str(&row, "name");
        let bytes = util::col_i64(&row, "bytes");
        let pages = util::col_i64(&row, "pages");
        if table_set.contains(&name) {
            tables.insert(name, (bytes, pages));
        } else if let Some(owner) = index_owner.get(&name) {
            indexes.insert(name, (bytes, pages, owner.clone()));
        }
    }
    (tables, indexes, true, String::new())
}

/// `entries` 的 kind 分布：条数 + 字节（`record` 是 JSON，一条 40KB 一条 20 字节）。
fn entries_by_kind(db: &dyn SqliteExecutor) -> Vec<Value> {
    util::all(
        db,
        "SELECT coalesce(json_extract(record, '$.kind'), '?') AS kind, \
         count(*) AS n, sum(length(record)) AS bytes FROM entries GROUP BY 1 ORDER BY bytes DESC",
    )
    .unwrap_or_default()
    .iter()
    .map(|row| {
        json!({
            "kind": util::col_str(row, "kind"),
            "count": util::col_i64(row, "n"),
            "bytes": util::col_i64(row, "bytes"),
        })
    })
    .collect()
}

/// 关键表行数（`sqlite_master` 里所有非 `sqlite_` 表）。
fn table_counts(db: &dyn SqliteExecutor) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::new();
    for row in util::all(
        db,
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .unwrap_or_default()
    {
        let name = util::col_str(&row, "name");
        let n = util::get(db, &format!("SELECT count(*) AS n FROM \"{}\"", name.replace('"', "\"\"")))
            .ok()
            .flatten()
            .map(|row| util::col_i64(&row, "n"))
            .unwrap_or(0);
        counts.insert(name, n);
    }
    counts
}

/// `PRAGMA integrity_check`；库大就不跑（面板同阈值）。
fn integrity(db: &dyn SqliteExecutor, total_bytes: i64) -> Value {
    if total_bytes >= INTEGRITY_MAX_BYTES {
        return json!({ "checked": false, "ok": true, "messages": [] });
    }
    match db.all("PRAGMA integrity_check", &[]) {
        Ok(rows) => {
            let messages: Vec<String> = rows
                .iter()
                .map(|row| {
                    row.values()
                        .next()
                        .map(|value| match value {
                            nous_chord::sqlite::SqliteValue::Text(text) => text.clone(),
                            other => format!("{other:?}"),
                        })
                        .unwrap_or_default()
                })
                .collect();
            let ok = messages.len() == 1 && messages[0] == "ok";
            json!({ "checked": true, "ok": ok, "messages": messages.into_iter().take(4).collect::<Vec<_>>() })
        }
        Err(error) => json!({ "checked": true, "ok": false, "messages": [error.to_string()] }),
    }
}

/// 骨架外的提示（WAL 大 / 空闲页多 / schema 旧 / 旁路）。
fn attention(bytes: &Value, version: i64) -> Vec<String> {
    let mut out = Vec::new();
    let wal = bytes.get("wal").and_then(Value::as_i64).unwrap_or(0);
    let free = bytes.get("free").and_then(Value::as_i64).unwrap_or(0);
    let total = bytes.get("db").and_then(Value::as_i64).unwrap_or(0);
    if wal > 4 * 1024 * 1024 {
        out.push(format!("WAL 已 {wal} 字节：没有别的读者在等它 checkpoint"));
    }
    if total > 0 && free > total / 5 {
        out.push(format!("空闲页 {free} 字节（{}%）：VACUUM 能拿回来", (free * 100) / total));
    }
    if version < 2 {
        out.push(format!("schema v{version}：比这页认识的 v2 旧"));
    }
    out
}

/// `GET /v1/stats/overview`：durable 库的一整份 X 光片。
pub fn overview(durable_db: &Path) -> Result<Value> {
    let db_bytes = util::file_bytes(durable_db);
    let wal = util::sidecar_bytes(durable_db, "-wal");
    let shm = util::sidecar_bytes(durable_db, "-shm");

    if !durable_db.exists() {
        return Ok(json!({
            "target": { "id": stem(durable_db), "path": durable_db.display().to_string() },
            "file": { "db": 0, "wal": 0, "shm": 0, "mtime": 0., "walMtime": 0. },
            "pragmas": { "pageSize": 0, "pageCount": 0, "freelist": 0, "journalMode": "", "schemaVersion": 0 },
            "bytes": { "db": 0, "wal": 0, "shm": 0, "free": 0, "used": 0, "onDisk": 0 },
            "counts": {},
            "tables": [],
            "indexes": [],
            "entriesByKind": [],
            "integrity": { "checked": false, "ok": true, "messages": [] },
            "attention": ["库文件还不存在：引擎第一次写入前就是这个状态"],
            "dbstatOk": false,
        }));
    }

    let db = util::open_read_only(durable_db)?;
    let page_size = util::pragma_i64(&db, "page_size");
    let page_count = util::pragma_i64(&db, "page_count");
    let freelist = util::pragma_i64(&db, "freelist_count");
    let journal_mode = util::pragma_text(&db, "journal_mode");
    let version = util::get(&db, "SELECT version FROM durable_schema WHERE singleton = 1")
        .ok()
        .flatten()
        .map(|row| util::col_i64(&row, "version"))
        .unwrap_or(0);

    let free_bytes = freelist * page_size;
    let used_bytes = (db_bytes - free_bytes).max(0);
    let bytes = json!({
        "db": db_bytes,
        "wal": wal,
        "shm": shm,
        "free": free_bytes,
        "used": used_bytes,
        "onDisk": db_bytes + wal + shm,
    });

    let (stat_tables, stat_indexes, dbstat_ok, dbstat_why) = dbstat(&db);
    let counts = table_counts(&db);

    let tables: Vec<Value> = counts
        .iter()
        .map(|(name, rows)| {
            let (bytes, pages) = stat_tables.get(name).copied().unwrap_or((0, 0));
            table_entry(name, bytes, pages, *rows)
        })
        .collect();
    // 按真字节降序（没有 dbstat 时全 0，退回按名字序，稳定）。
    let mut tables = tables;
    tables.sort_by(|a, b| {
        b["bytes"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["bytes"].as_i64().unwrap_or(0))
            .then_with(|| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")))
    });

    let indexes: Vec<Value> = stat_indexes
        .iter()
        .map(|(name, (bytes, pages, owner))| {
            json!({ "name": name, "bytes": bytes, "pages": pages, "table": owner })
        })
        .collect();

    let mut out = json!({
        "target": { "id": stem(durable_db), "path": durable_db.display().to_string() },
        "file": {
            "db": db_bytes,
            "wal": wal,
            "shm": shm,
            "mtime": util::mtime_ms(durable_db),
            "walMtime": util::mtime_ms(&with_suffix(durable_db, "-wal")),
        },
        "pragmas": {
            "pageSize": page_size,
            "pageCount": page_count,
            "freelist": freelist,
            "journalMode": journal_mode,
            "schemaVersion": version,
        },
        "bytes": bytes,
        "counts": counts,
        "tables": tables,
        "indexes": indexes,
        "entriesByKind": entries_by_kind(&db),
        "integrity": integrity(&db, db_bytes),
        "dbstatOk": dbstat_ok,
    });
    if !dbstat_ok {
        out["dbstatWhy"] = json!(dbstat_why);
    }
    out["attention"] = json!(attention(&out["bytes"], version));
    Ok(out)
}

fn stem(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("nous.db")
        .to_string()
}

fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut owned = path.as_os_str().to_owned();
    owned.push(suffix);
    std::path::PathBuf::from(owned)
}
