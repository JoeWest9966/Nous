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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteError {
    pub kind: SqliteErrorKind,
    pub message: String,
}

impl SqliteError {
    pub fn new(kind: SqliteErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn backend(message: impl std::fmt::Display) -> Self {
        Self::new(SqliteErrorKind::Backend, message.to_string())
    }
}

impl std::fmt::Display for SqliteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sqlite {:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for SqliteError {}

impl From<rusqlite::Error> for SqliteError {
    fn from(error: rusqlite::Error) -> Self {
        SqliteError::backend(error)
    }
}
