//! 私库错误：HTTP 层按变体映射 400/404/409/429/500。

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoError {
    /// 路径逃逸 / 二进制 / 超限等输入问题 → 400。
    BadRequest(String),
    /// 内容闸未过 → 400，带 `{field, problem}` 定位信息。
    Validation { field: String, problem: String },
    /// 仓根不存在或不可访问 → 400。
    InvalidPath(String),
    NotFound(String),
    /// If-Match 不符 → 409；回包要带磁盘当前内容。
    Conflict { message: String, current: Option<Vec<u8>>, etag: Option<String> },
    /// 目标被锁定（元素库的 `locked`）→ 423：改内容与删除都拒绝，
    /// 解锁只能走 `/lock`（谁都能调，所以不能拿 403 骗人）。
    Locked(String),
    /// 同一仓已有写在进行，且调用方选择不等待 → 429。
    Busy(String),
    LockPoisoned,
    /// git / IO 失败 → 500；push 失败不在这里冒 500，见 `CommitResult.pushed`。
    Io(String),
}

impl RepoError {
    pub fn conflict(message: impl Into<String>, current: Vec<u8>, etag: String) -> Self {
        Self::Conflict { message: message.into(), current: Some(current), etag: Some(etag) }
    }
}

impl std::fmt::Display for RepoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(m) | Self::NotFound(m) | Self::Busy(m) | Self::Io(m) | Self::InvalidPath(m) => f.write_str(m),
            Self::Validation { field, problem } => write!(f, "内容闸未过：{field} / {problem}"),
            Self::Conflict { message, .. } => f.write_str(message),
            Self::Locked(m) => f.write_str(m),
            Self::LockPoisoned => f.write_str("私库写锁中毒"),
        }
    }
}

impl std::error::Error for RepoError {}

pub type Result<T> = std::result::Result<T, RepoError>;

/// 内容闸失败时给客户端的定位信息。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ValidationIssue {
    pub field: String,
    pub problem: String,
}

/// 归档三代的结果。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PathInfo {
    pub path: PathBuf,
}
