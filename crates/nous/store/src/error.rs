//! 一个 `StoreError`，手写 `Display`（和 `nous-durable` / `nous-env` 同一个取舍：不引 `thiserror`）。
//!
//! 调用方按**变体**分流，不靠读消息 —— 消息是给人看的。

use std::fmt;

use rusqlite::ErrorCode;

/// 这个库的失败。每一变体都在说「怎么办」，不是「哪里疼」。
#[derive(Debug)]
pub enum StoreError {
    /// SQLite 层（含约束冲突、`VACUUM INTO` 失败）。
    Sqlite(rusqlite::Error),
    /// 文件层（临时目录没了、路径不是 UTF-8）。
    Io(std::io::Error),
    /// 库比代码新：拒绝打开，别让写入落在看不懂的表结构上。
    SchemaTooNew { found: u32, supported: u32 },
    /// 迁移链断了（版本号不连续）：链是按顺序排的，跳号就是有人改坏了。
    MigrationChain { expected: u32, found: u32 },
    /// 迁移跑完后的版本不等于 `CURRENT_VERSION`。
    SchemaVersion { found: u32, supported: u32 },
    /// `backup_to` 的目标已经存在。**不静默覆盖**：上一份备份也是备份。
    BackupTargetExists { path: String },
    /// `code` 不合 `^[a-z][a-z0-9_]{2,63}$`。
    InvalidCode { code: String, rule: &'static str },
    /// `code` 被占着。`deleted_at` 非空 = 那是**墓碑**占着（删了也不许复用）。
    CodeTaken { code: String, deleted_at: Option<i64> },
    /// 树里没有这个 `code`。
    CodeUnknown(String),
    /// 账本里没有这笔账（`(code, fire_at)` 这个主键）。
    FireUnknown { trigger_code: String, fire_at: i64 },
    /// 库里的 `state` 不在三值里（有人绕过 CHECK 写进去了）。
    StateUnknown { state: String },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "sqlite: {error}"),
            Self::Io(error) => write!(f, "io: {error}"),
            Self::SchemaTooNew { found, supported } => write!(
                f,
                "库的 schema 版本是 {found}，本版代码只支持到 {supported} —— 拒绝打开（先升代码）"
            ),
            Self::MigrationChain { expected, found } => {
                write!(f, "迁移链跳号：期望 {expected}，实得 {found}")
            }
            Self::SchemaVersion { found, supported } => {
                write!(f, "迁移后的版本是 {found}，代码期望 {supported}")
            }
            Self::BackupTargetExists { path } => {
                write!(f, "备份目标已存在：{path}（不覆盖上一份；换个路径或自己先搬走）")
            }
            Self::InvalidCode { code, rule } => write!(f, "code {code:?} 不合规：{rule}"),
            Self::CodeTaken { code, deleted_at: Some(deleted_at) } => write!(
                f,
                "code {code:?} 已被占用：它的触发器在 {deleted_at} 被删了（墓碑仍占着这个字符串，\
                 它挂着历史 requestId / actor.id / 审计）"
            ),
            Self::CodeTaken { code, deleted_at: None } => write!(f, "code {code:?} 已被占用"),
            Self::CodeUnknown(code) => write!(f, "没有 code = {code:?} 的触发器"),
            Self::FireUnknown { trigger_code, fire_at } => {
                write!(f, "账本里没有 ({trigger_code:?}, {fire_at}) 这笔账")
            }
            Self::StateUnknown { state } => {
                write!(f, "库里的 fires.state = {state:?} 不在 pending/done/orphaned 里")
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// 约束冲突（UNIQUE / PRIMARY KEY / CHECK）。别再拿字符串认错。
pub(crate) fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _) if failure.code == ErrorCode::ConstraintViolation
    )
}

pub type Result<T> = std::result::Result<T, StoreError>;
