//! `nous-ai` 的**纯半**：只有类型、事件流与容错 JSON。
//!
//! 判据（#18）：依赖树里没有 `reqwest` / `hyper`，也没有 `tokio` 的 `time|net` 特性。
//! 真相层（`crates/kernel/*`）只许依赖这一半 —— `nous-ai` 的 provider 半会 sleep，
//! 把它拖进真相层，三层分层就不成立（`docs/spec/kernel-rust.md` 判据 6b）。
//!
//! 旧路径不变：`nous_ai::types::*` / `nous_ai::event_stream::*` / `nous_ai::utils::*`
//! 在 `nous-ai` 里是 `pub use`，所以调用点一行都不用改（#18 的边界）。

pub mod event_stream;
pub mod json_parse;
pub mod types;
pub mod utils;

// 旧路径的平铺再导出：调用点写 `nous_ai_types::Message` 或 `nous_ai::Message` 都是同一个类型。
pub use event_stream::{AssistantMessageEventStream, AssistantMessageEventStreamProducer};
pub use types::{
    Api, AssistantMessage, AssistantMessageEvent, Content, Context, DeferredHandle, DoneReason,
    ErrorReason, ImageContent, InputModality, Message, ModelCost, ModelCostRates, ModelCostTier,
    ProviderId, Schema, StopReason, TextContent, ThinkingBudgets, ThinkingContent, ThinkingLevel,
    ThinkingLevelMap, Tool, ToolCall, ToolCallType, ToolResultMessage, Usage, UsageCost,
    UserContent, UserMessage,
};
