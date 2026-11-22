//! stats 内部共用的只读 SQL 小件：开库、读列、`pragma`、`dbstat`、mtime。
//!
//! 只读打开（`docs/storage.md` §8.3）；写操作在 SQLite 层就失败，不靠调用方自律。
//! 这里不造业务概念，只把「从 sqlite 读一个数」这件事收敛到一处。

use std::path::Path;

use nous_chord::sqlite::{LocalDatabase, SqliteExecutor, SqliteRow, SqliteValue};
use serde_json::Value;

use crate::{Result, StatsError};

/// 只读打开一个库；失败带路径。
pub(crate) fn open_read_only(path: &Path) -> Result<LocalDatabase> {
    LocalDatabase::open_read_only(path)
        .map_err(|error| StatsError::Storage(format!("open {}: {error}", path.display())))
}

/// 跑一条查询取全部行。
pub(crate) fn all(db: &dyn SqliteExecutor, sql: &str) -> Result<Vec<SqliteRow>> {
    db.all(sql, &[]).map_err(|error| StatsError::Storage(error.to_string()))
}

/// 跑一条查询取第一行。
pub(crate) fn get(db: &dyn SqliteExecutor, sql: &str) -> Result<Option<SqliteRow>> {
    db.get(sql, &[]).map_err(|error| StatsError::Storage(error.to_string()))
}

/// 取整数列（缺席 / 非整数 → 0）。只用于读数聚合，缺值当零是故意的。
pub(crate) fn col_i64(row: &SqliteRow, column: &str) -> i64 {
    row.get(column).and_then(SqliteValue::as_i64).unwrap_or(0)
}

/// 取文本列（缺席 → 空串）。
pub(crate) fn col_str(row: &SqliteRow, column: &str) -> String {
    row.get(column)
        .and_then(SqliteValue::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `PRAGMA <name>` 的第一个整数。`name` 只来自本库写死的字面量，不拼用户输入。
pub(crate) fn pragma_i64(db: &dyn SqliteExecutor, name: &str) -> i64 {
    get(db, &format!("PRAGMA {name}"))
        .ok()
        .flatten()
        .and_then(|row| row.values().next().and_then(SqliteValue::as_i64))
        .unwrap_or(0)
}

/// `PRAGMA <name>` 的第一个文本（`journal_mode`）。
pub(crate) fn pragma_text(db: &dyn SqliteExecutor, name: &str) -> String {
    get(db, &format!("PRAGMA {name}"))
        .ok()
        .flatten()
        .and_then(|row| {
            row.values()
                .next()
                .and_then(SqliteValue::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// 主文件字节数（不存在 → 0）。
pub(crate) fn file_bytes(path: &Path) -> i64 {
    std::fs::metadata(path).map(|meta| meta.len() as i64).unwrap_or(0)
}

/// `<文件>-wal` / `<文件>-shm` 的字节数。
pub(crate) fn sidecar_bytes(path: &Path, suffix: &str) -> i64 {
    let mut side = path.as_os_str().to_owned();
    side.push(suffix);
    file_bytes(Path::new(&side))
}

/// 文件 mtime（epoch 毫秒，浮点；不存在 → 0）。
pub(crate) fn mtime_ms(path: &Path) -> f64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs_f64() * 1000.0)
        .unwrap_or(0.)
}

/// 一条 `record` 文本 → JSON。坏 JSON 返回 `None`（那一条跳过，不整屏崩）。
pub(crate) fn parse_record(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok()
}

/// `tasks.kind` / `documents.kind` 在盘上是 JSON 字面量（`"pi.generation"`），去掉引号。
pub(crate) fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        if let Ok(Value::String(text)) = serde_json::from_str::<Value>(trimmed) {
            return text;
        }
        return trimmed[1..trimmed.len() - 1].to_string();
    }
    trimmed.to_string()
}
