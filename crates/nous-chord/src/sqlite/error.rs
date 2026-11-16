//! 一个 `SqliteError`，不引 `thiserror`（和 `nous-env` 同一个取舍：手写 `Display`）。

/// 错误分类。调用方按它决定「重试 / 报错 / 迁移」，不靠读消息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteErrorKind {
    /// 后端（`rusqlite` / SQLite 自己）报的错，含约束冲突、IO、SQL 语法。
    Backend,
    /// schema 前缀不合法或占用了保留名。
    InvalidPrefix,
    /// 迁移版本不是从 1 开始的连续整数。
    NonContiguousVersions,
    /// 版本元数据表存在但没有 singleton 行。
    MissingMetadata,
    /// 库里的 schema 版本比代码支持的更新（**拒绝运行**，不降级）。
    SchemaTooNew,
    /// 事务没提交也没回滚地结束了（`commit` 失败）。
    Transaction,
}

/// 底层错误**原样留着**，只压成字符串就等于把 `source()` 链剪断：调用方想按
/// `rusqlite::ErrorCode` 分流（比如 `SQLITE_BUSY` 要不要重试）就只剩正则解析消息一条路。
#[derive(Debug, Clone)]
pub struct SqliteError {
    pub kind: SqliteErrorKind,
    pub message: String,
    /// `rusqlite` 报的原始错误。我们自己的判定（版本过新、前缀不合法）没有它。
    cause: Option<std::sync::Arc<rusqlite::Error>>,
}

impl SqliteError {
    pub fn new(kind: SqliteErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            cause: None,
        }
    }

    pub fn backend(message: impl std::fmt::Display) -> Self {
        Self::new(SqliteErrorKind::Backend, message.to_string())
    }

    /// 挂上底层错误。`message` 由调用方给（有时要加一句上下文），`source()` 走这里。
    pub fn with_cause(mut self, cause: rusqlite::Error) -> Self {
        self.cause = Some(std::sync::Arc::new(cause));
        self
    }

    pub fn cause(&self) -> Option<&rusqlite::Error> {
        self.cause.as_deref()
    }
}

/// 只比「哪一类 + 消息」：底层错误的类型没有 `PartialEq`，且测试要断言的是分类。
impl PartialEq for SqliteError {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.message == other.message
    }
}

impl Eq for SqliteError {}

impl std::fmt::Display for SqliteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sqlite {:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for SqliteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

impl From<rusqlite::Error> for SqliteError {
    fn from(error: rusqlite::Error) -> Self {
        let message = error.to_string();
        SqliteError::new(SqliteErrorKind::Backend, message).with_cause(error)
    }
}
