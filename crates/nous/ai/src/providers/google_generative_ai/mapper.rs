//! Chunk → event mapping for the Google Generative AI dialect.
//!
//! Mirrors the streaming half of `packages/ai/src/api/google-generative-ai.ts`
//! (the `for await (const chunk of googleStream)` loop) + `mapStopReason` from
//! `google-shared.ts`.
//!
//! 与上游的一处结构差异：TS 的 mapper 直接推事件进 `AssistantMessageEventStream`；
//! Rust 侧把「解析一个 chunk」做成纯函数式方法（`on_chunk` 返回事件向量），
//! runner 再负责 push。好处是**整条映射路径可以离线测**（不用起 HTTP），
//! 代价是每次事件都要 snapshot 一次 `Arc<AssistantMessage>` —— 与 anthropic
//! mapper 的代价相同。

use serde_json::Value;
use std::sync::Arc;

use crate::model::Model;
use crate::types::{
    AssistantMessage, AssistantMessageEvent, Content, DoneReason, ErrorReason, ImageContent,
    ImageContentType, StopReason, TextContent, TextContentType, ThinkingContent, ThinkingContentType,
    ToolCall, ToolCallType, Usage,
};

use super::build_params::{
    calculate_cost, retain_thought_signature, sanitize_surrogates,
};

/// Which block the stream is currently filling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    Text,
    Thinking,
}

/// The mapper's mutable state: the message under construction plus the open block.
pub struct GoogleMapper {
    output: AssistantMessage,
    current: Option<Block>,
    tool_call_counter: u64,
    clock_ms: fn() -> i64,
}

impl GoogleMapper {
    pub fn new(model: &Model, timestamp: i64) -> Self {
        Self {
            output: AssistantMessage::empty(
                model.api.clone(),
                model.provider.clone(),
                model.id.clone(),
                timestamp,
            ),
            current: None,
            tool_call_counter: 0,
            clock_ms: now_ms,
        }
    }

    /// The message so far (for the terminal event and for tests).
    pub fn message(&self) -> &AssistantMessage {
        &self.output
    }

    pub fn into_message(self) -> AssistantMessage {
        self.output
    }

    /// Emit the initial `Start` event.
    pub fn start_event(&self) -> AssistantMessageEvent {
        AssistantMessageEvent::Start {
            partial: Arc::new(self.output.clone()),
        }
    }

    /// Apply one streamed chunk and return the events it produces, in order.
    pub fn on_chunk(&mut self, chunk: &Value, model: &Model) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();

        if let Some(response_id) = chunk.get("responseId").and_then(Value::as_str) {
            if !response_id.is_empty() && self.output.response_id.is_none() {
                self.output.response_id = Some(response_id.to_string());
            }
        }

        let candidate = chunk
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
            .cloned()
            .unwrap_or(Value::Null);

        if let Some(parts) = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    let is_thinking = part.get("thought").and_then(Value::as_bool) == Some(true);
                    let wanted = if is_thinking {
                        Block::Thinking
                    } else {
                        Block::Text
                    };
                    if self.current != Some(wanted) {
                        self.close_block(&mut events);
                        self.open_block(wanted, &mut events);
                    }
                    let signature = part.get("thoughtSignature").and_then(Value::as_str);
                    match wanted {
                        Block::Thinking => {
                            if let Some(Content::Thinking(block)) = self.output.content.last_mut() {
                                block.thinking.push_str(&sanitize_surrogates(text));
                                block.thinking_signature = retain_thought_signature(
                                    block.thinking_signature.take(),
                                    signature,
                                );
                            }
                            events.push(AssistantMessageEvent::ThinkingDelta {
                                content_index: self.last_index(),
                                delta: text.to_string(),
                                partial: self.snapshot(),
                            });
                        }
                        Block::Text => {
                            if let Some(Content::Text(block)) = self.output.content.last_mut() {
                                block.text.push_str(&sanitize_surrogates(text));
                                block.text_signature = retain_thought_signature(
                                    block.text_signature.take(),
                                    signature,
                                );
                            }
                            events.push(AssistantMessageEvent::TextDelta {
                                content_index: self.last_index(),
                                delta: text.to_string(),
                                partial: self.snapshot(),
                            });
                        }
                    }
                }

                if let Some(function_call) = part.get("functionCall") {
                    self.close_block(&mut events);

                    let provided_id = function_call.get("id").and_then(Value::as_str);
                    let duplicate = provided_id.is_some_and(|id| {
                        self.output.content.iter().any(|block| {
                            matches!(block, Content::ToolCall(call) if call.id == id)
                        })
                    });
                    let id = match provided_id {
                        Some(id) if !id.is_empty() && !duplicate => id.to_string(),
                        _ => {
                            self.tool_call_counter += 1;
                            format!(
                                "{}_{}_{}",
                                function_call.get("name").and_then(Value::as_str).unwrap_or(""),
                                (self.clock_ms)(),
                                self.tool_call_counter
                            )
                        }
                    };

                    let args = function_call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    let call = ToolCall {
                        kind: ToolCallType,
                        id,
                        name: function_call
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        arguments: args.clone(),
                        thought_signature: part
                            .get("thoughtSignature")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        namespace: None,
                        recorded: None,
                    };
                    self.output
                        .content
                        .push(Content::ToolCall(call.clone()));
                    events.push(AssistantMessageEvent::ToolCallStart {
                        content_index: self.last_index(),
                        partial: self.snapshot(),
                    });
                    events.push(AssistantMessageEvent::ToolCallDelta {
                        content_index: self.last_index(),
                        delta: serde_json::to_string(&args).unwrap_or_else(|_| "{}".into()),
                        partial: self.snapshot(),
                    });
                    events.push(AssistantMessageEvent::ToolCallEnd {
                        content_index: self.last_index(),
                        tool_call: call,
                        partial: self.snapshot(),
                    });
                }
            }
        }

        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.output.raw_stop_reason = Some(reason.to_string());
            self.output.stop_reason = map_stop_reason(reason);
            let has_tool_call = self
                .output
                .content
                .iter()
                .any(|block| matches!(block, Content::ToolCall(_)));
            if has_tool_call && self.output.stop_reason == StopReason::Stop {
                self.output.stop_reason = StopReason::ToolUse;
            }
        }

        if let Some(metadata) = chunk.get("usageMetadata") {
            let number = |key: &str| -> i64 {
                metadata.get(key).and_then(Value::as_i64).unwrap_or(0)
            };
            let cached = number("cachedContentTokenCount");
            let thoughts = number("thoughtsTokenCount");
            self.output.usage = Usage {
                input: number("promptTokenCount") - cached,
                output: number("candidatesTokenCount") + thoughts,
                cache_read: cached,
                cache_write: 0,
                cache_write_1h: None,
                reasoning: Some(thoughts),
                total_tokens: number("totalTokenCount"),
                cost: Default::default(),
            };
            self.output.usage.cost = calculate_cost(&model.cost, &self.output.usage);
        }

        events
    }

    /// Close the open block and validate the terminal state. Mirrors the tail of
    /// the TS stream loop: a stream with no finish reason or a failed stop
    /// reason is an error, not a silent empty answer.
    pub fn finish(&mut self) -> Result<Vec<AssistantMessageEvent>, String> {
        let mut events = Vec::new();
        self.close_block(&mut events);
        if self.output.stop_reason == StopReason::Pending {
            return Err("Google stream ended without a finish reason".to_string());
        }
        if matches!(self.output.stop_reason, StopReason::Aborted | StopReason::Error) {
            return Err(match self.output.raw_stop_reason.as_deref() {
                Some(reason) => format!("Provider stopped with: {reason}"),
                None => "An unknown error occurred".to_string(),
            });
        }
        Ok(events)
    }

    /// The terminal `Done` event. `StopReason` has no `Deferred` path here.
    pub fn done_reason(&self) -> Option<DoneReason> {
        match self.output.stop_reason {
            StopReason::Stop => Some(DoneReason::Stop),
            StopReason::Length => Some(DoneReason::Length),
            StopReason::ToolUse => Some(DoneReason::ToolUse),
            _ => None,
        }
    }

    pub fn error_event(&mut self, reason: ErrorReason, message: String) -> AssistantMessageEvent {
        self.output.stop_reason = match reason {
            ErrorReason::Aborted => StopReason::Aborted,
            ErrorReason::Error => StopReason::Error,
        };
        self.output.error_message = Some(message);
        AssistantMessageEvent::Error {
            reason,
            error: self.output.clone(),
        }
    }

    fn snapshot(&self) -> Arc<AssistantMessage> {
        Arc::new(self.output.clone())
    }

    fn last_index(&self) -> usize {
        self.output.content.len().saturating_sub(1)
    }

    fn open_block(&mut self, block: Block, events: &mut Vec<AssistantMessageEvent>) {
        match block {
            Block::Text => {
                self.output.content.push(Content::Text(TextContent {
                    kind: TextContentType,
                    text: String::new(),
                    text_signature: None,
                }));
                events.push(AssistantMessageEvent::TextStart {
                    content_index: self.last_index(),
                    partial: self.snapshot(),
                });
            }
            Block::Thinking => {
                self.output.content.push(Content::Thinking(ThinkingContent {
                    kind: ThinkingContentType,
                    thinking: String::new(),
                    thinking_signature: None,
                    redacted: false,
                }));
                events.push(AssistantMessageEvent::ThinkingStart {
                    content_index: self.last_index(),
                    partial: self.snapshot(),
                });
            }
        }
        self.current = Some(block);
    }

    fn close_block(&mut self, events: &mut Vec<AssistantMessageEvent>) {
        match self.current.take() {
            Some(Block::Text) => {
                let content = match self.output.content.last() {
                    Some(Content::Text(block)) => block.text.clone(),
                    _ => String::new(),
                };
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: self.last_index(),
                    content,
                    partial: self.snapshot(),
                });
            }
            Some(Block::Thinking) => {
                let content = match self.output.content.last() {
                    Some(Content::Thinking(block)) => block.thinking.clone(),
                    _ => String::new(),
                };
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: self.last_index(),
                    content,
                    partial: self.snapshot(),
                });
            }
            None => {}
        }
    }
}

/// Mirrors `mapStopReason`.
pub fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        // Everything Gemini can filter or refuse: blocklist, prohibited content,
        // SPII, safety (text + image), recitation, language, malformed/too many
        // tool calls, FINISH_REASON_UNSPECIFIED, OTHER, NO_IMAGE.
        "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "SAFETY"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION"
        | "IMAGE_OTHER"
        | "RECITATION"
        | "FINISH_REASON_UNSPECIFIED"
        | "OTHER"
        | "LANGUAGE"
        | "MALFORMED_FUNCTION_CALL"
        | "UNEXPECTED_TOOL_CALL"
        | "TOO_MANY_TOOL_CALLS"
        | "NO_IMAGE" => StopReason::Error,
        // Unknown future reasons: the TS switch throws (exhaustive check), which
        // the adapter turns into an error terminal event — same outcome here.
        _ => StopReason::Error,
    }
}

/// Mirrors `mapStopReasonString` (used by raw API responses).
pub fn map_stop_reason_string(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        _ => StopReason::Error,
    }
}

/// Re-exported so the runner can construct image content without importing types.
pub fn image_content(mime_type: &str, data: &str) -> Content {
    Content::Image(ImageContent {
        kind: ImageContentType,
        mime_type: mime_type.to_string(),
        data: std::sync::Arc::from(data),
    })
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Api, ModelCost, ModelCostRates};
    use serde_json::json;

    fn model() -> Model {
        Model::new(
            "gemini-2.5-flash",
            "Gemini 2.5 Flash",
            Api::GoogleGenerativeAi,
            "google",
            "https://generativelanguage.googleapis.com/v1beta",
        )
    }

    fn priced_model() -> Model {
        let mut m = model();
        m.cost = ModelCost {
            rates: ModelCostRates {
                input: 1.0,
                output: 2.0,
                cache_read: 0.5,
                cache_write: 0.0,
            },
            tiers: Vec::new(),
        };
        m
    }

    fn mapper() -> GoogleMapper {
        GoogleMapper::new(&model(), 1_700_000_000_000)
    }

    #[test]
    fn text_deltas_open_and_close_one_block() {
        let mut m = mapper();
        let first = m.on_chunk(&json!({"candidates":[{"content":{"parts":[{"text":"he"}]}}]}), &model());
        assert!(matches!(first[0], AssistantMessageEvent::TextStart { content_index: 0, .. }));
        assert!(matches!(first[1], AssistantMessageEvent::TextDelta { ref delta, .. } if delta == "he"));

        let second = m.on_chunk(
            &json!({"candidates":[{"content":{"parts":[{"text":"llo"}]},"finishReason":"STOP"}]}),
            &model(),
        );
        assert_eq!(second.len(), 1, "same block must not emit a second TextStart");
        assert!(matches!(second[0], AssistantMessageEvent::TextDelta { ref delta, .. } if delta == "llo"));

        let tail = m.finish().unwrap();
        assert!(matches!(tail[0], AssistantMessageEvent::TextEnd { ref content, .. } if content == "hello"));
        assert_eq!(m.message().content.len(), 1);
    }

    #[test]
    fn thinking_then_text_switches_blocks_and_keeps_signature() {
        let mut m = mapper();
        let events = m.on_chunk(
            &json!({"candidates":[{"content":{"parts":[
                {"text":"pondering","thought":true,"thoughtSignature":"c2lnbg=="}
            ]}}]}),
            &model(),
        );
        assert!(matches!(events[0], AssistantMessageEvent::ThinkingStart { .. }));
        assert!(matches!(events[1], AssistantMessageEvent::ThinkingDelta { .. }));

        // A later thinking delta with no signature must not erase the first one.
        m.on_chunk(
            &json!({"candidates":[{"content":{"parts":[{"text":" more","thought":true}]}}]}),
            &model(),
        );
        let events = m.on_chunk(
            &json!({"candidates":[{"content":{"parts":[{"text":"answer"}]}}]}),
            &model(),
        );
        // ThinkingEnd, then TextStart, then TextDelta.
        assert!(matches!(events[0], AssistantMessageEvent::ThinkingEnd { .. }));
        assert!(matches!(events[1], AssistantMessageEvent::TextStart { content_index: 1, .. }));
        assert!(matches!(events[2], AssistantMessageEvent::TextDelta { .. }));

        match &m.message().content[0] {
            Content::Thinking(block) => {
                assert_eq!(block.thinking, "pondering more");
                assert_eq!(block.thinking_signature.as_deref(), Some("c2lnbg=="));
            }
            other => panic!("expected thinking block, got {other:?}"),
        }
    }

    #[test]
    fn function_call_closes_text_and_emits_start_delta_end() {
        let mut m = mapper();
        m.on_chunk(&json!({"candidates":[{"content":{"parts":[{"text":"let me look"}]}}]}), &model());
        let events = m.on_chunk(
            &json!({"candidates":[{"content":{"parts":[
                {"functionCall":{"name":"read","args":{"path":"a.txt"},"id":"call_1"}}
            ]}}]}),
            &model(),
        );
        assert!(matches!(events[0], AssistantMessageEvent::TextEnd { .. }));
        assert!(matches!(events[1], AssistantMessageEvent::ToolCallStart { content_index: 1, .. }));
        assert!(matches!(events[2], AssistantMessageEvent::ToolCallDelta { ref delta, .. }
            if delta.contains("a.txt")));
        assert!(matches!(events[3], AssistantMessageEvent::ToolCallEnd { .. }));
        match &m.message().content[1] {
            Content::ToolCall(call) => {
                assert_eq!(call.id, "call_1");
                assert_eq!(call.name, "read");
                assert_eq!(call.arguments["path"], "a.txt");
            }
            other => panic!("expected tool call, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_or_missing_tool_call_ids_get_a_generated_one() {
        let mut m = mapper();
        let chunk = json!({"candidates":[{"content":{"parts":[
            {"functionCall":{"name":"read","args":{},"id":"same"}},
            {"functionCall":{"name":"read","args":{},"id":"same"}},
            {"functionCall":{"name":"ls","args":{}}}
        ]}}]});
        m.on_chunk(&chunk, &model());
        let ids: Vec<String> = m
            .message()
            .content
            .iter()
            .filter_map(|block| match block {
                Content::ToolCall(call) => Some(call.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], "same", "the first one keeps the provider id");
        assert_ne!(ids[1], "same", "a duplicate must be renamed");
        assert!(ids[1].starts_with("read_"));
        assert!(ids[2].starts_with("ls_"));
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );
    }

    #[test]
    fn finish_reason_and_tool_use_override() {
        let mut m = mapper();
        m.on_chunk(
            &json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{}}}]},
                                   "finishReason":"STOP"}]}),
            &model(),
        );
        assert_eq!(m.message().raw_stop_reason.as_deref(), Some("STOP"));
        assert_eq!(m.message().stop_reason, StopReason::ToolUse);
        assert_eq!(m.done_reason(), Some(DoneReason::ToolUse));
    }

    #[test]
    fn stop_reason_matrix() {
        assert_eq!(map_stop_reason("STOP"), StopReason::Stop);
        assert_eq!(map_stop_reason("MAX_TOKENS"), StopReason::Length);
        for filtered in [
            "SAFETY",
            "RECITATION",
            "BLOCKLIST",
            "PROHIBITED_CONTENT",
            "SPII",
            "IMAGE_SAFETY",
            "MALFORMED_FUNCTION_CALL",
            "TOO_MANY_TOOL_CALLS",
            "FINISH_REASON_UNSPECIFIED",
            "OTHER",
            "SOMETHING_NEW_IN_2027",
        ] {
            assert_eq!(map_stop_reason(filtered), StopReason::Error, "{filtered}");
        }
        assert_eq!(map_stop_reason_string("STOP"), StopReason::Stop);
        assert_eq!(map_stop_reason_string("SAFETY"), StopReason::Error);
    }

    #[test]
    fn usage_mapping_counts_cached_and_thought_tokens_and_costs_them() {
        let mut m = GoogleMapper::new(&priced_model(), 0);
        m.on_chunk(
            &json!({"candidates":[{"content":{"parts":[{"text":"x"}]},"finishReason":"STOP"}],
                    "usageMetadata":{
                        "promptTokenCount":100,
                        "cachedContentTokenCount":40,
                        "candidatesTokenCount":10,
                        "thoughtsTokenCount":5,
                        "totalTokenCount":115
                    }}),
            &priced_model(),
        );
        let usage = &m.message().usage;
        assert_eq!(usage.input, 60, "prompt minus cached");
        assert_eq!(usage.cache_read, 40);
        assert_eq!(usage.output, 15, "candidates plus thoughts");
        assert_eq!(usage.reasoning, Some(5));
        assert_eq!(usage.total_tokens, 115);
        // 60 in @1.0/M + 15 out @2.0/M + 40 cache-read @0.5/M
        assert!((usage.cost.total - (60e-6 + 30e-6 + 20e-6)).abs() < 1e-12);
    }

    #[test]
    fn response_id_keeps_the_first_non_empty_value() {
        let mut m = mapper();
        m.on_chunk(&json!({"responseId":"first","candidates":[]}), &model());
        m.on_chunk(&json!({"responseId":"second","candidates":[]}), &model());
        assert_eq!(m.message().response_id.as_deref(), Some("first"));
    }

    #[test]
    fn stream_without_finish_reason_is_an_error() {
        let mut m = mapper();
        m.on_chunk(&json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}), &model());
        let err = m.finish().unwrap_err();
        assert!(err.contains("without a finish reason"), "{err}");
    }

    #[test]
    fn filtered_stop_reason_finishes_as_error_not_done() {
        let mut m = mapper();
        m.on_chunk(
            &json!({"candidates":[{"finishReason":"SAFETY"}]}),
            &model(),
        );
        let err = m.finish().unwrap_err();
        assert_eq!(err, "Provider stopped with: SAFETY");
        assert_eq!(m.done_reason(), None);
    }

    #[test]
    fn error_event_sets_terminal_state_and_message() {
        let mut m = mapper();
        match m.error_event(ErrorReason::Error, "boom".into()) {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, ErrorReason::Error);
                assert_eq!(error.stop_reason, StopReason::Error);
                assert_eq!(error.error_message.as_deref(), Some("boom"));
            }
            other => panic!("expected error, got {other:?}"),
        }
        match m.error_event(ErrorReason::Aborted, "stopped".into()) {
            AssistantMessageEvent::Error { error, .. } => {
                assert_eq!(error.stop_reason, StopReason::Aborted)
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn empty_chunk_produces_no_events() {
        let mut m = mapper();
        assert!(m.on_chunk(&json!({"candidates":[]}), &model()).is_empty());
        assert!(m.on_chunk(&json!({}), &model()).is_empty());
        assert!(m.message().content.is_empty());
    }
}
