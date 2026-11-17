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
    NotFound(String),
    Internal(String),
}

impl ServerError {
    pub fn status(&self) -> StatusCode {
        match self {
            ServerError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ServerError::NotFound(_) => StatusCode::NOT_FOUND,
            ServerError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ServerError::BadRequest(message)
            | ServerError::NotFound(message)
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
