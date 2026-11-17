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
    /// 只留最新态的文档没有历史可读（要过去的点 = 伪造）。
    HistoryNotRetained { document_id: i64 },
    /// 文档一条 base 都没有，可它的值只能从 base 起算。
    MissingBase { document_id: i64 },
    /// revision 序列里出现跨版本的 delta：中间缺一个 base。
    VersionCrossing { document_id: i64 },
    /// 存着的 delta 解不出来（不是合法的 op 数组 / op 形状不对 / 路径不可解）。
    DeltaDecode { document_id: i64, message: String },
    /// 库里的 seq 已经走到 `floor`，`next_seq` 却是 `next_seq` —— 接着写就是复用旧号。
    SeqRegression { next_seq: i64, floor: i64 },
    /// `durable_metadata` 的 singleton 行不在，或它的值读不出来。
    MissingMetadata,
    /// 迁移后版本与代码期望不一致。
    SchemaVersion { found: i64, supported: i64 },
    /// 库已经关了：`close()` 之后的每一次调用都明确失败，不「装作还能用」。
    Closed,
    /// 按会话读 entry / 扫 entry 时那个会话不存在（上游 `Unknown conversation`）。
    ///
    /// 这一条与「会话存在但没有 entry」**必须分开**：都读成空页的话，客户端分不出「历史到头了」
    /// 与「会话 id 写错了」。
    UnknownConversation { id: i64 },
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
            StorageError::HistoryNotRetained { document_id } => write!(
                f,
                "Document {document_id} does not retain historical content"
            ),
            StorageError::MissingBase { document_id } => {
                write!(f, "Document {document_id} is missing a required base")
            }
            StorageError::VersionCrossing { document_id } => write!(
                f,
                "Document {document_id} crosses a stored version boundary without a base"
            ),
            StorageError::DeltaDecode { document_id, message } => {
                write!(f, "Document {document_id} has an undecodable delta: {message}")
            }
            StorageError::SeqRegression { next_seq, floor } => write!(
                f,
                "next_seq {next_seq} would reuse a seq already stored (the highest stored seq is {floor})"
            ),
            StorageError::MissingMetadata => {
                f.write_str("durable_metadata has no singleton row, or next_id/next_seq is unreadable")
            }
            StorageError::SchemaVersion { found, supported } => write!(
                f,
                "schema version {found} is not the supported version {supported}"
            ),
            StorageError::Closed => f.write_str("storage is closed"),
            StorageError::UnknownConversation { id } => write!(f, "Unknown conversation: {id}"),
        }
    }
}

impl std::error::Error for StorageError {
    /// 把 `SqliteError` 交出去 —— 事务边界的错误也要能一路问到底。
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StorageError::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;
