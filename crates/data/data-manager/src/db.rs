//! `GET /v1/stats/db` 的读口：两库各一份 `{bytes, rows, quickCheck, schema}`。
//!
//! durable 那份直接调 [`nous_durable::ops::check`] —— 与 `nous-app --check` 同一函数；
//! plugins 那份用 `nous_store::Store::conn()` 跑同一条 `PRAGMA quick_check`，不另开写连接。
//! 库不存在时按空库报告（空库判据：三条端点都 200），不把「没建」伪装成「坏了」。

use std::collections::BTreeMap;
use std::path::Path;

use nous_chord::sqlite::{LocalDatabase, SqliteExecutor, SqliteValue};
use serde_json::{json, Value};

use crate::{Result, StatsError};

/// durable 库的九张表 + plugins 库的两张表（关键表计数）。
const DURABLE_TABLES: &[&str] = &[
    "record_ids", "conversations", "entries", "tasks", "submissions", "documents",
    "document_revisions",
];
const PLUGIN_TABLES: &[&str] = &["triggers", "fires"];

fn file_bytes(path: &Path) -> i64 {
    std::fs::metadata(path).map(|meta| meta.len() as i64).unwrap_or(0)
}

fn table_rows(db: &dyn SqliteExecutor, table: &str) -> i64 {
    db.get(&format!("SELECT count(*) AS n FROM {table}"), &[])
        .ok()
        .flatten()
        .and_then(|row| row.get("n").and_then(SqliteValue::as_i64))
        .unwrap_or(0)
}

fn table_counts(db: &dyn SqliteExecutor, tables: &[&str]) -> Value {
    let mut rows = BTreeMap::new();
    for table in tables {
        rows.insert(table.to_string(), table_rows(db, table));
    }
    Value::Object(rows.into_iter().map(|(k, v)| (k, json!(v))).collect())
}

fn durable_report(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({
            "bytes": 0,
            "rows": table_counts_missing(DURABLE_TABLES),
            "quickCheck": "ok",
            "schema": 0,
        }));
    }
    // 与 `nous-app --check` 同一个读口（票面约束 3/契约第 3 条）。
    let schema = nous_durable::ops::check(path)?;
    let db = LocalDatabase::open_read_only(path).map_err(|error| {
        StatsError::Storage(format!("open {}: {error}", path.display()))
    })?;
    Ok(json!({
        "bytes": file_bytes(path),
        "rows": table_counts(&db, DURABLE_TABLES),
        "quickCheck": "ok",
        "schema": schema,
    }))
}

fn table_counts_missing(tables: &[&str]) -> Value {
    let mut rows = BTreeMap::new();
    for table in tables {
        rows.insert(table.to_string(), 0_i64);
    }
    Value::Object(rows.into_iter().map(|(k, v)| (k, json!(v))).collect())
}

fn plugins_report(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({
            "bytes": 0,
            "rows": table_counts_missing(PLUGIN_TABLES),
            "quickCheck": "ok",
            "schema": 0,
        }));
    }
    let store = nous_store::Store::open(path)?;
    let schema = store.schema_version()?;
    let quick_check = nous_store::presets::quick_check(&store)
        .map_err(|error| StatsError::Storage(error.to_string()))?;
    let mut rows = BTreeMap::new();
    for table in PLUGIN_TABLES {
        let count = nous_store::presets::table_count(&store, table).unwrap_or(0);
        rows.insert(table.to_string(), json!(count));
    }
    Ok(json!({
        "bytes": file_bytes(path),
        "rows": Value::Object(rows.into_iter().collect()),
        "quickCheck": quick_check,
        "schema": schema,
    }))
}

pub fn report(durable_db: &Path, plugins_db: &Path) -> Result<Value> {
    Ok(json!({
        "durable": durable_report(durable_db)?,
        "plugins": plugins_report(plugins_db)?,
    }))
}
