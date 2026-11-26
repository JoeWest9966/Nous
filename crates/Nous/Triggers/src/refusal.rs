//! 「哪一种失败回哪个状态码」的唯一一处（搬自 `server/src/error.rs` 的语义）。
//!
//! 插件不能依赖 `server`，也不该各自再写一份映射 —— 上一票（`refusal` 收口）刚把
//! env / combos / archive 三份私有 `Refusal` 收进 `nous_routes::HttpRefusal`，
//! 再长出一份新的就是往回走。所以这里只做**语法糖**：把「一个消息」包成带状态码的
//! 拒绝，判定规则本身仍是 `wire.md` 那一条（400 / 404 / 409 / 412 / 500）。
//!
//! 为什么不直接写 `HttpRefusal::bad_request(...)`：搬过来的代码里有 60 多处
//! `bad(...)`，逐处展开只会让 diff 变成噪音，而搬家的判据是「行为一字未改」。

use nous_routes::HttpRefusal;

/// 参数 / 盘面写法不对：400。
pub(crate) fn bad(message: impl std::fmt::Display) -> HttpRefusal {
    HttpRefusal::bad_request(message.to_string())
}

/// 没有这东西：404。**不许降级成 400**，也不许空 200。
pub(crate) fn not_found(message: impl std::fmt::Display) -> HttpRefusal {
    HttpRefusal::not_found(message.to_string())
}

/// 现在不行（版本过期以外的状态冲突）：409。
pub(crate) fn conflict(message: impl std::fmt::Display) -> HttpRefusal {
    HttpRefusal::conflict(message.to_string())
}

/// 内部错误：500。原文保留在 `error` 里 —— 客户端看到的是能贴进 issue 的一句话。
pub(crate) fn internal(error: impl std::fmt::Display) -> HttpRefusal {
    HttpRefusal::internal(error.to_string())
}

/// ETag / If-Match 不匹配：412。409 是「现在不行」，412 是「你手里的版本过期了」。
pub(crate) fn precondition_failed(message: impl std::fmt::Display) -> HttpRefusal {
    HttpRefusal::new(412, serde_json::json!({ "error": message.to_string() }))
}
