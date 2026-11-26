//! Mirrors `packages/ai/src` — the unified, multi-provider LLM layer.
//!
//! Provider-agnostic types (`Content`, `Message`, `AssistantMessage`,
//! `AssistantMessageEvent`, `Context`, `Tool`, `Usage`) live in [`types`];
//! model-layer configuration (`Model`, compat structs) lives in [`model`].
//! Schema validation + LLM-sloppy-JSON coercion (mirroring `validation.ts`) is
//! in [`schema`]; the SPSC streaming queue mirroring `event-stream.ts` is in
//! [`event_stream`]; the `Provider` trait seam is in [`provider`]; concrete
//! providers live under [`providers`] (faux now, anthropic in M3).
//!
//! Every module's doc comment names the TS file it mirrors.

#![forbid(unsafe_code)]

pub mod constrained;
pub mod error;
pub mod frames;
#[cfg(feature = "providers")]
pub mod http;
#[cfg(feature = "providers")]
pub mod images;
pub mod model;
pub mod provider;
pub mod providers;
pub(crate) mod redact;
pub mod schema;
#[cfg(feature = "providers")]
pub(crate) mod strict_schema;
pub mod tool;
pub mod transform;

// Flat re-exports — consumers do `use nous_ai::Message` not `nous_ai::types::Message`.
pub use error::AiError;

/// faux provider 的模型 id（`Api::Faux` 那一路回复里带的就是它）。
///
/// 放在这里而不是 `server`：点火路径（`trigger-core`）与 faux 轮次装配
/// （`server`）都要用同一个 id，两边各写一份字符串迟早会漂移 —— 而
/// `FauxProvider` 自己就在本 crate 里，「假 provider 叫什么」本来就归它答。
pub const FAUX_MODEL_ID: &str = "echo";
// 纯半搬进了 `nous-ai-types`；这里保持旧路径（`nous_ai::types::…`）。
pub use nous_ai_types::{event_stream, json_parse, types, utils};
pub use event_stream::{AssistantMessageEventStream, AssistantMessageEventStreamProducer};
pub use model::{Model, StreamingProtocolCompat};
pub use provider::{
    CacheRetention, ContextPatch, NoopProviderHooks, Provider, ProviderHooks, SimpleStreamOptions,
    SimpleStreamOptionsPatch, DEFAULT_LLM_API_TIMEOUT, DEFAULT_STREAM_IDLE_TIMEOUT,
};
pub use schema::validate_tool_arguments;
pub use tool::{
    AgentError, AgentTool, AgentToolResult, TextContentOrImage, ToolExecutionMode, ToolResultPartial,
};
pub use types::{
    Api, AssistantMessage, AssistantMessageEvent, Content, Context, DeferredHandle, DoneReason,
    ErrorReason, ImageContent, InputModality, Message, ModelCost, ModelCostRates, ModelCostTier,
    ProviderId, Schema, StopReason, TextContent, ThinkingBudgets, ThinkingContent, ThinkingLevel,
    ThinkingLevelMap, Tool, ToolCall, ToolCallType, ToolResultMessage, Usage, UsageCost,
    UserContent, UserMessage,
};
