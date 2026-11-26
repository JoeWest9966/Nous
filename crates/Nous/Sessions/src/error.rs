//! `sessions` 模块自己的错误：端点、命令面、数据源三层共用同一种。
//!
//! ## 为什么不复用 `server::ServerError`
//!
//! 端点住在插件里（Magento 2「模块自带 controller」），插件不许 `use nous_server::…`。
//! 而这个错误要穿过三层（`SessionSource` / `CommandSource` / handler），三层的返回类型必须
//! 是同一个，所以它得住在**拥有这三层**的地方 —— 本 crate。
//!
//! ## 十个变体是照抄，不是重新设计
//!
//! 与 `ServerError` 一对一（搬家前后状态码与文案逐字不变）；`server` 侧有一层
//! `From<SessionError> for ServerError` 把老调用点接住，等 17 条端点全搬完就只剩
//! `into_refusal()` 一个出口。

use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    BadRequest(String),
    /// 远端机器不可达（ssh 连不上 / 超时）：502，不是退回本机的 200。
    BadGateway(String),
    /// 没有权限读目标资源：403，不是 500。
    Forbidden(String),
    NotFound(String),
    /// 状态冲突：会话已关闭、正在跑、没有可中止的任务。**不许降级成 400**。
    Conflict(String),
    /// 内容闸未过：400 + `{field, problem}`（私库发布前校验）。
    Validation { field: String, problem: String },
    /// HTTP 412：If-Match 乐观锁不匹配。409 是「现在不行」，412 是「你手里的版本过期了」。
    PreconditionFailed(String),
    /// HTTP 423：目标被锁定。不是 403 —— 403 是「你没权限」，锁是「谁都得先解锁」。
    Locked(String),
    /// 没有任何可用资源（#70：一个模型来源都没有），503。
    ServiceUnavailable(String),
    Internal(String),
}

impl SessionError {
    /// 状态码的数值（本 crate 不引入 HTTP 框架的类型，只给数）。
    pub fn status(&self) -> u16 {
        match self {
            SessionError::BadRequest(_) => 400,
            SessionError::BadGateway(_) => 502,
            SessionError::Forbidden(_) => 403,
            SessionError::NotFound(_) => 404,
            SessionError::Conflict(_) => 409,
            SessionError::Validation { .. } => 400,
            SessionError::PreconditionFailed(_) => 412,
            SessionError::Locked(_) => 423,
            SessionError::ServiceUnavailable(_) => 503,
            SessionError::Internal(_) => 500,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            SessionError::BadRequest(message)
            | SessionError::BadGateway(message)
            | SessionError::Forbidden(message)
            | SessionError::NotFound(message)
            | SessionError::Conflict(message)
            | SessionError::PreconditionFailed(message)
            | SessionError::Locked(message)
            | SessionError::ServiceUnavailable(message)
            | SessionError::Internal(message) => message,
            SessionError::Validation { .. } => "内容闸未过",
        }
    }

    /// 回包体：`{"error": …}`（+ 可选 `{field, problem}`）。与 `ServerError` 的
    /// `IntoResponse` 逐字一致。
    pub fn body(&self) -> serde_json::Value {
        let mut body = json!({ "error": self.message() });
        if let SessionError::Validation { field, problem } = self {
            body["field"] = json!(field);
            body["problem"] = json!(problem);
        }
        body
    }
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for SessionError {}

pub type Result<T> = std::result::Result<T, SessionError>;

/// 会话层的存储错误（`nous_durable::session::SessionError`）→ 本层的 409/500。
///
/// 逐字照抄搬家前 `commands.rs` 的那两个分支：`Closed` 是「现在不行」（409），
/// 其余是「后端出问题了」（500）。
pub fn from_session(error: nous_durable::session::SessionError) -> SessionError {
    match error {
        nous_durable::session::SessionError::Closed => {
            SessionError::Conflict("session is closed".to_string())
        }
        other => SessionError::Internal(other.to_string()),
    }
}

/// 写命令错误（`nous_durable::session::commands::CommandError`）→ 本层。
///
/// 逐字照抄搬家前 `commands.rs` 的 `command_error`：`Busy` 与 `Closed` 都是 409
/// （「正在跑」/「已关闭」），不是 400。
pub fn from_command(error: nous_durable::session::commands::CommandError) -> SessionError {
    use nous_durable::session::commands::CommandError;
    match error {
        CommandError::BadRequest(message) => SessionError::BadRequest(message),
        CommandError::NotFound(message) => SessionError::NotFound(message),
        CommandError::Conflict(message) => SessionError::Conflict(message),
        CommandError::Session(nous_durable::session::SessionError::Closed) => {
            SessionError::Conflict("session is closed".to_string())
        }
        CommandError::Session(other) => SessionError::Internal(other.to_string()),
    }
}
