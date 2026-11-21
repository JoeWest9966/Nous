//! 端点错误 → `wire.md` 的 `{ error: string }`。
//!
//! 宪法只有一条形状要求：错误体统一 `{ error: string }`，状态码跟着语义走
//! （400 / 404 / 409 / 500…）。所以这里不发明错误码表，也不把内部错误串成 HTML。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// 端点层能返回的错误。`Storage` / `Harness` 的错误都收敛到 [`ServerError::Internal`]，
/// 但原文保留在 `error` 里 —— 客户端看到的是能贴进 issue 的一句话，不是 "Internal error"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerError {
    BadRequest(String),
    /// 远端机器不可达（ssh 连不上 / 超时）：502，不是退回本机的 200。
    BadGateway(String),
    /// 没有权限读目标资源：403，不是 500。
    Forbidden(String),
    NotFound(String),
    /// 状态冲突：会话已关闭、正在跑、没有可中止的任务。**不许降级成 400** ——
    /// 400 是「参数这么写不行」，409 是「现在不行」。
    Conflict(String),
    /// HTTP 412：If-Match 乐观锁不匹配（#60 / #61）。409 是「现在不行」，412 是「你手里的版本过期了」。
    PreconditionFailed(String),
    Internal(String),
}

impl ServerError {
    pub fn status(&self) -> StatusCode {
        match self {
            ServerError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ServerError::BadGateway(_) => StatusCode::BAD_GATEWAY,
            ServerError::Forbidden(_) => StatusCode::FORBIDDEN,
            ServerError::NotFound(_) => StatusCode::NOT_FOUND,
            ServerError::Conflict(_) => StatusCode::CONFLICT,
            ServerError::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            ServerError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ServerError::BadRequest(message)
            | ServerError::BadGateway(message)
            | ServerError::Forbidden(message)
            | ServerError::NotFound(message)
            | ServerError::Conflict(message)
            | ServerError::PreconditionFailed(message)
            | ServerError::Internal(message) => message,
        }
    }
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let status = self.status();
        (status, Json(json!({ "error": self.message() }))).into_response()
    }
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ServerError {}

pub type Result<T> = std::result::Result<T, ServerError>;
