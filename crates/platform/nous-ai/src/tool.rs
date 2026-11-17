//! The tool contract: what a tool is, what it returns, and how failures are
//! reported — **moved here** from pi-rust 的 `rpi-agent`，只取工具层需要的部分。
//!
//! 来源：`bigfish1913/pi-rust@v0.24.2` 的
//! `crates/rpi-agent/src/{agent_tool.rs,error.rs,types.rs}`。
//! 搬进来的是 5 项 + 2 项跑不掉的东西（零逻辑改动）：
//!
//! 1. [`AgentTool`] —— trait 本体，`agent_tool.rs`。
//! 2. [`AgentToolResult`] —— `types.rs` 的 `AgentToolResult`。
//! 3. [`TextContentOrImage`] —— 同上，工具内容的受限联合。
//! 4. [`ToolResultPartial`] —— 同上，`on_update` 回调的载荷（就是 `AgentToolResult`）。
//! 5. [`AgentError`] —— `error.rs` 整个枚举。
//! 6. [`ToolExecutionMode`] —— `AgentTool::execution_mode()` 的返回类型，**跑不掉**：没有它 trait 编不过。
//! 7. `impl AgentToolResult` 的三个便捷方法 + `From<AgentToolResult> for Result<…>` —— 与类型同源，拆开更怪。
//!
//! 为什么放在 `nous-ai` 而不是 `nous-tools`：`AGENTS.md` §二 规定
//! `nous-tools → ai, env`，而工具实现必须实现 `AgentTool`。这 250 行是本库
//! 契约图能成立的前提，台账里记作 `crates/nous-ai/src/tool.rs`（`已改`）。
//!
//! 上游原文（`agent_tool.rs` 头）：
//!
//! > TS `AgentTool` extends `Tool` with `label`, optional `prepareArguments`,
//! > `execute`, and per-tool `executionMode`. The Rust port keeps the same shape:
//! > a struct-less `#[async_trait]` `AgentTool` returning the public `Tool`
//! > schema from `nous_ai`. Tools are held behind `Arc<dyn AgentTool>` so the agent
//! > can store + dispatch them without generics.
//! >
//! > The `execute` signature carries an `on_update` callback (`&dyn Fn`) for
//! > streaming partial results. The loop wraps it with an
//! > `accepting_updates: Arc<AtomicBool>` gate so calls made after `execute`
//! > resolves are no-ops — the late-update suppression invariant (plan §5.3).

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::types::{Content, ImageContent, TextContent, TextContentType, Tool, Usage};
use std::sync::Arc;

/// How a batch of tool calls from one assistant message are executed.
///
/// - `Sequential`: each call is prepared, executed, finalized before the next starts.
/// - `Parallel`: calls are prepared sequentially, then allowed tools execute
///   concurrently. `tool_execution_end` fires in completion order; tool-result
///   `MessageEnd` fires later in assistant source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

/// Result returned from a tool `execute`. Mirrors TS `AgentToolResult<T>`:
/// `content` goes back to the model, `details` are structured log/UI payload,
/// `usage` is optionally reported, and `terminate` hints the batch should stop.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    pub content: Vec<TextContentOrImage>,
    pub details: serde_json::Value,
    pub usage: Option<Usage>,
    pub added_tool_names: Vec<String>,
    pub terminate: bool,
}

/// an enum so tool authors stay within the provider-content language without
/// pulling in the full `Content` union (which adds Thinking/ToolCall).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum TextContentOrImage {
    Text(TextContent),
    Image(ImageContent),
}

impl TextContentOrImage {
    pub fn text<S: Into<String>>(s: S) -> Self {
        TextContentOrImage::Text(TextContent {
            kind: TextContentType,
            text: s.into(),
            text_signature: None,
        })
    }
}

impl AgentToolResult {
    /// Convenience: a single text block, empty details.
    pub fn text(message: impl Into<String>) -> Self {
        Self {
            content: vec![TextContentOrImage::text(message)],
            details: serde_json::Value::Null,
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        }
    }

    /// Convenience: an error text block. `is_error` is carried on the
    /// `ToolResultMessage`, not the result; this just builds the content.
    pub fn error_text(message: impl Into<String>) -> Self {
        Self::text(message)
    }

    pub fn into_content(self) -> Vec<Content> {
        self.content
            .into_iter()
            .map(|c| match c {
                TextContentOrImage::Text(t) => Content::Text(t),
                TextContentOrImage::Image(i) => Content::Image(i),
            })
            .collect()
    }
}

impl From<AgentToolResult> for Result<AgentToolResult, AgentError> {
    fn from(r: AgentToolResult) -> Self {
        Ok(r)
    }
}

/// Partial result pushed by a tool's `on_update` callback during execution.
/// Mirrors TS `AgentToolUpdateCallback<T>` payload.
pub type ToolResultPartial = AgentToolResult;

/// Failures raised inside the agent loop. Cannot cross the provider boundary
/// (provider failures arrive as `AssistantMessageEvent::Error`); these surface
/// from tool execution, argument validation, queue/state misuse, or abort.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AgentError {
    #[error("tool error: {0}")]
    Tool(String),

    /// A failed tool that still owns native content and accounting metadata.
    #[error("structured tool error")]
    StructuredTool(Box<AgentToolResult>),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("operation aborted")]
    Abort,

    #[error("provider error: {0}")]
    Provider(String),

    #[error("queue error: {0}")]
    Queue(String),

    #[error("invalid state: {0}")]
    State(String),
}

impl AgentError {
    pub fn tool(message: impl Into<String>) -> Self {
        AgentError::Tool(message.into())
    }

    /// True when the error originated from an abort. Used by the loop to decide
    /// the terminal event shape (`Aborted` vs `Error`).
    pub fn is_abort(&self) -> bool {
        matches!(self, AgentError::Abort)
    }
}

/// A tool the agent can call. Mirrors TS `AgentTool<TParameters, TDetails>`.
///
/// Implementations are `Send + Sync` and conventionally held as
/// `Arc<dyn AgentTool>`. The loop looks up tools by `schema().name` matching a
/// `ToolCall::name`.
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// The provider-facing tool definition (name, description, parameters schema).
    fn schema(&self) -> &Tool;

    /// Human-readable label for UI display.
    fn label(&self) -> &str;

    /// Per-tool execution-mode override. `Sequential` forces one-at-a-time for
    /// the whole batch when any tool in it returns `Sequential`; `Parallel`
    /// (default) allows concurrency.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Parallel
    }

    /// Optional compatibility shim for raw tool-call arguments before schema
    /// validation. Default is identity. Mirrors TS `prepareArguments`.
    fn prepare_arguments(&self, args: serde_json::Value) -> Result<serde_json::Value, AgentError> {
        Ok(args)
    }

    /// Execute the tool call. Throw via `Err(AgentError)` on failure — the loop
    /// encodes the error message into an error `ToolResultMessage`.
    ///
    /// `on_update` streams partial results. It is passed as an `Arc<dyn Fn>`
    /// (not a borrow) so a tool may clone it into a background task that emits
    /// progress after `execute` has returned its main result — e.g. a bash tool
    /// whose throttled output flusher outlives the `await` point. Calls made
    /// after the loop has flipped its `accepting_updates` gate to false are
    /// silently dropped by the loop (late-update suppression, invariant §5.3);
    /// the tool never needs to track settlement itself.
    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        signal: CancellationToken,
        on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError>;
}
