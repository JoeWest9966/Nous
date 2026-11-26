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

/// 一条端点拒绝理由的**语义**：状态码 + 一句人话（+ 内容闸的两格定位）。
///
/// `RepoError` 的种类是这一层的语义，所以「哪种错误回哪个码」的判断也该在这里 ——
/// 插件只负责把它翻成自己的 HTTP 回包，不各自再算一遍。
///
/// 为什么不在 `nous-routes`：那一层被 `nous-env` 依赖（`env` 插件自带端点），
/// 而 `nous-env` 又是本 crate 的依赖 —— 放那里会成环（`env → routes → repo → env`）。
/// 本 crate 拥有 `RepoError`，翻译自己家的错误，依赖方向永远是对的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// HTTP 状态码的数值（`nous-repo` 不引入 HTTP 框架的类型，只给数）。
    pub status: u16,
    pub message: String,
    /// 内容闸未过时带上的两格（`RepoError::Validation`）：`{field, problem}`。
    pub validation: Option<(String, String)>,
    /// `RepoError::Conflict` 的回包富化：磁盘当前正文（票面 #75 要求 409 带它）。
    ///
    /// Desk 靠这一格在 If-Match 失败时显示「盘上现在是什么」，所以搬到插件里
    /// 也得照搬——少了它客户端只能重新拉一次。
    pub content: Option<String>,
    /// `RepoError::Conflict` 的回包富化：磁盘当前 etag。
    pub etag: Option<String>,
}

impl Refusal {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            validation: None,
            content: None,
            etag: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(409, message)
    }

    pub fn locked(message: impl Into<String>) -> Self {
        Self::new(423, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(500, message)
    }

    /// `RepoError` → 拒绝理由。**搬家前后逐字一致**（含 409 那句「先重新打开这个文件」）：
    ///
    /// - `BadRequest` / `InvalidPath` / `Validation` → 400（后者多带 `{field, problem}`）
    /// - `NotFound` → 404
    /// - `Conflict` → 409（`{error}` 只留一句人话，Desk 原样显示它）
    /// - `Busy` → 409；`Locked` → 423
    /// - `Io` / `LockPoisoned` → 500
    pub fn from_repo(error: RepoError) -> Self {
        match error {
            RepoError::BadRequest(m) | RepoError::InvalidPath(m) => Self::bad_request(m),
            RepoError::Validation { field, problem } => Self {
                status: 400,
                message: "内容闸未过".to_string(),
                validation: Some((field, problem)),
                content: None,
                etag: None,
            },
            RepoError::NotFound(m) => Self::not_found(m),
            // 409 的三格照搬：`{error}` 之外还带磁盘当前正文与 etag（#75 票面）。
            RepoError::Conflict { message, current, etag } => Self {
                status: 409,
                message: format!("{message} —— 先重新打开这个文件，再存一次"),
                validation: None,
                content: current.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                etag,
            },
            RepoError::Busy(m) => Self::conflict(m),
            RepoError::Locked(m) => Self::locked(m),
            RepoError::Io(m) => Self::internal(m),
            RepoError::LockPoisoned => Self::internal("私库写锁中毒"),
        }
    }

    /// 回包体：`{"error": …}`（+ 可选 `{field, problem}`）。插件直接 `Json` 它。
    pub fn body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({ "error": self.message });
        if let Some((field, problem)) = &self.validation {
            body["field"] = serde_json::json!(field);
            body["problem"] = serde_json::json!(problem);
        }
        if let Some(content) = &self.content {
            body["content"] = serde_json::json!(content);
        }
        if let Some(etag) = &self.etag {
            body["etag"] = serde_json::json!(etag);
        }
        body
    }
}
