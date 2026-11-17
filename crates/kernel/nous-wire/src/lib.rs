//! `wire.md` 的 Rust 投影 —— 本库唯一对外的事件类型层。
//!
//! 权威是仓库根的 `wire.md`（宪法）。这个 crate 和它不一致时，**改这里，不改宪法**。
//!
//! 契约面只有一条：**序列化后的字段名**（`camelCase`，`Option` 字段不吐 `null`）。
//! 所以这里没有逻辑、没有 IO、没有内部依赖 —— 只有 `serde` + `serde_json`，
//! `cargo tree -p nous-wire` 里一旦出现 `nous-ai` / `tokio` / `reqwest` 就是契约图破了。
//!
//! 范围：只做 `GET /v1/sessions/:id/events` 的事件类型。REST 响应体
//! （`/v1/sessions` 的会话树、`/v1/entries` 的历史…）等真服务落地时再投影，
//! 现在具名等于替引擎冻结构型。
//!
//! ```no_run
//! let line = r#"{"type":"message_update","changes":[{"type":"text_delta","contentIndex":0,"delta":"hi"}]}"#;
//! let event: nous_wire::RemoteEvent = serde_json::from_str(line).unwrap();
//! assert!(event.is_known());
//! ```

pub mod event;

pub use event::{
    entry_kind, message_role, Generation, MessageChange, RemoteEvent, SubmissionRecord,
};
