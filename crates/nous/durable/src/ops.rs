//! 运维动作（D13）：直连 `Storage`，不经过 `Session` / 轮次 / provider。
//!
//! 三个动作共用 durable 的 migrations 代码，备份的 schema 版本与读到的版本同源。

use std::path::Path;

use nous_chord::sqlite::{SqliteExecutor, SqliteValue};

use crate::error::{Result, StorageError};
use crate::storage::SqliteStore;

/// 读写打开源库 → backup API 写到 `path`。源库必须读写打开（WAL checkpoint 的前提）。
pub fn backup(source: &str, destination: &Path) -> Result<()> {
    let store = SqliteStore::open_writer(source)?;
    store.db().backup_to(destination)?;
    Ok(())
}

/// 只读打开，跑 `PRAGMA quick_check` 并读 schema 版本。
/// `Ok(())` 表示 quick_check == "ok" 且 schema 可读（由调用方按 `schema` 打第二行）。
pub fn check(path: &Path) -> Result<i64> {
    let store = SqliteStore::open_read_only(path)?;
    let row = store
        .db()
        .get("PRAGMA quick_check", &[])?
        .ok_or_else(|| StorageError::Shape("PRAGMA quick_check returned no row".into()))?;
    let result = row
        .values()
        .next()
        .and_then(SqliteValue::as_str)
        .unwrap_or_default();
    if result != "ok" {
        return Err(StorageError::Shape(format!("quick_check={result}")));
    }
    store.schema_version()
}

/// 只数**在飞**的任务：`pending` / `running` / `waiting` / `completing`。
///
/// 不算 `sleeping`（触发器排队 —— 它排着队不代表有活在跑；#57 上线后库里能攒到几千条，
/// 按「非 terminal」数会让升级永久被拒，2026-11-22 实测 7,805 条）与 `terminal`（已收尾）。
/// 状态域见 `storage/sqlite/migrations.rs` 的 CHECK 约束，这里是它的读口（#91）。
pub fn active_tasks(path: &str) -> Result<i64> {
    let store = SqliteStore::open_read_only(path)?;
    let row = store.db().get(
        "SELECT count(*) AS n FROM tasks \
         WHERE status IN ('pending', 'running', 'waiting', 'completing')",
        &[],
    )?;
    row.and_then(|row| row.get("n").and_then(SqliteValue::as_i64))
        .ok_or_else(|| StorageError::Shape("active-tasks count is not an integer".into()))
}
