//! 一个 `StorageError`，手写 `Display`（不引 `thiserror`，和 `nous-env` / `nous-chord` 同一个取舍）。

use nous_chord::sqlite::SqliteError;

/// 存储层的失败。调用方按变体分流，不靠读消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// SQLite 层（含 `BEGIN IMMEDIATE` 抢锁失败、约束冲突）。
    Sqlite(SqliteError),
    /// 记录的 JSON 解析/编码失败。
    Json(String),
    /// 记录形状不对：缺字段、类型不对、超出文档的取值集合。
    Shape(String),
    /// 写模式打开的库不是我们写的（`durable_schema.writer != 'nous'`）。
    WriterMismatch { found: Option<String> },
    /// 要读的文档最新一条 revision 是 `delta`，本版还不会重放（12c）。
    UndecodedDelta { document_id: i64 },
    /// 迁移后版本与代码期望不一致。
    SchemaVersion { found: i64, supported: i64 },
}

impl From<SqliteError> for StorageError {
    fn from(error: SqliteError) -> Self {
        StorageError::Sqlite(error)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        StorageError::Json(error.to_string())
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Sqlite(error) => write!(f, "storage sqlite: {error}"),
            StorageError::Json(message) => write!(f, "storage json: {message}"),
            StorageError::Shape(message) => write!(f, "storage record shape: {message}"),
            StorageError::WriterMismatch { found } => match found {
                Some(found) => write!(
                    f,
                    "this database was not written by nous (writer = {found:?}); open it read-only"
                ),
                None => write!(
                    f,
                    "this database has no durable_schema.writer column; open it read-only"
                ),
            },
            StorageError::UndecodedDelta { document_id } => write!(
                f,
                "document {document_id} has a delta as its newest revision; delta replay is not wired yet"
            ),
            StorageError::SchemaVersion { found, supported } => write!(
                f,
                "schema version {found} is not the supported version {supported}"
            ),
        }
    }
}

impl std::error::Error for StorageError {}

pub type Result<T> = std::result::Result<T, StorageError>;
