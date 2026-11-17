//! Google Generative AI (Gemini) provider — `POST
//! {base}/models/{model}:streamGenerateContent?alt=sse`.
//!
//! Mirrors `packages/ai/src/api/google-generative-ai.ts`（`stream` /
//! `streamSimple` / `createClient`）与 `google-shared.ts` 的共享部分。
//!
//! 结构差异（不是语义差异）：
//!
//! - TS 走 `@google/genai` SDK（`client.models.generateContentStream(params)`）；
//!   Rust 直接说 REST，所以 **请求体在 [`build_params`] 里摊平**、**流在 [`mapper`]
//!   里解**，中间没有 SDK 那层。
//! - TS 的 `retryGoogleRequest` 是对 SDK 调用的包装；Rust 复用 anthropic 的
//!   [`crate::providers::anthropic::retry::retry_provider_request`]（同一套
//!   408/409/429/5xx + `retry-after` 策略），差别只在错误分类：SDK 的 `ApiError`
//!   补一个 `headers` 字段就能进同一判定，REST 版拿到的就是状态码。
//! - `abortSignal` 是 SDK 侧字段、不上线；取消走 `CancellationToken`。
//! - **已知缺口**：`toolChoice`（`auto`/`none`/`any`）在 TS 是 `GoogleOptions` 的
//!   字段，本库的 `SimpleStreamOptions` 没有对应字段（`rpi-ai` 也没有），冻结类型
//!   不轻改 —— 见 [`build_params::resolve_function_calling_mode`] 的注释与 #5 票面。

pub mod build_params;
pub mod mapper;

use async_trait::async_trait;
use std::time::Instant;

use crate::error::AiError;
use crate::event_stream::{
    create_assistant_message_event_stream_with_cancellation, AssistantMessageEventStream,
    AssistantMessageEventStreamProducer,
};
use crate::model::Model;
use crate::provider::{Provider, SimpleStreamOptions};
use crate::types::{AssistantMessageEvent, Context, DoneReason, ErrorReason};
use crate::providers::anthropic::retry::retry_provider_request;
use crate::providers::anthropic::sse::SseEventStream;

use build_params::{build_request, BuiltRequest};
use mapper::{GoogleMapper, now_ms};

/// Built-in Gemini catalog. **价格留 0 且 `cost_configured = false`**：定价由
/// Nous 的模型编目（`nous-runtime`，#17）注入，这里不假装知道。
pub fn google_models() -> Vec<Model> {
    let base = "https://generativelanguage.googleapis.com/v1beta";
    let mut models = Vec::new();
    for (id, name, reasoning, context_window, max_tokens) in [
        ("gemini-2.5-pro", "Gemini 2.5 Pro", true, 1_048_576u64, 65_536u64),
        ("gemini-2.5-flash", "Gemini 2.5 Flash", true, 1_048_576, 65_536),
        ("gemini-2.5-flash-lite", "Gemini 2.5 Flash Lite", true, 1_048_576, 65_536),
        ("gemini-3-pro-preview", "Gemini 3 Pro (preview)", true, 1_048_576, 65_536),
        ("gemini-3-flash-preview", "Gemini 3 Flash (preview)", true, 1_048_576, 65_536),
        ("gemma-4-27b-it", "Gemma 4 27B", true, 131_072, 8_192),
    ] {
        let mut model = Model::new(id, name, crate::types::Api::GoogleGenerativeAi, "google", base);
        model.reasoning = reasoning;
        model.context_window = context_window;
        model.max_tokens = max_tokens;
        model.input = vec![crate::types::InputModality::Text, crate::types::InputModality::Image];
        models.push(model);
    }
    models
}

/// The Gemini provider. Mirrors TS `createClient` + the `stream` entry point.
pub struct GoogleGenerativeAiProvider {
    api_key: Option<String>,
    allow_env_api_key: bool,
    http: reqwest::Client,
    models: Vec<Model>,
}

impl GoogleGenerativeAiProvider {
    /// Explicit default key + client; serves [`google_models`].
    pub fn new(api_key: Option<String>, http: reqwest::Client) -> Self {
        Self {
            api_key,
            allow_env_api_key: true,
            http,
            models: google_models(),
        }
    }

    /// Custom catalog (e.g. the generated models snapshot).
    pub fn with_models(api_key: Option<String>, http: reqwest::Client, models: Vec<Model>) -> Self {
        Self {
            api_key,
            allow_env_api_key: true,
            http,
            models,
        }
    }

    /// Build with a default client, resolving the key at call time
    /// (`opts.api_key` → provider default → `GOOGLE_API_KEY` / `GEMINI_API_KEY`).
    pub fn from_env() -> Self {
        let http = crate::http::build_client("https://generativelanguage.googleapis.com", None, None)
            .unwrap_or_else(|_| reqwest::Client::new());
        Self::new(None, http)
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }
}

#[async_trait]
impl Provider for GoogleGenerativeAiProvider {
    fn id(&self) -> &str {
        "google"
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    async fn stream_simple(
        &self,
        model: &Model,
        ctx: &Context,
        opts: &SimpleStreamOptions,
    ) -> AssistantMessageEventStream {
        let (mut prod, stream) =
            create_assistant_message_event_stream_with_cancellation(&opts.signal);

        let api_key = resolve_api_key(
            &self.api_key,
            opts,
            self.allow_env_api_key,
        );
        let prepared = match api_key.clone() {
            None => Err(format!("No API key for provider: {}", model.provider)),
            Some(_) => Ok(build_request(model, ctx, opts)),
        };

        let http = self.http.clone();
        let model = model.clone();
        let mut opts = opts.clone();
        opts.signal = prod.cancellation_token();

        tokio::spawn(async move {
            run_google_stream(&mut prod, http, &model, prepared, api_key, &opts).await;
        });

        stream
    }
}

/// `opts.api_key` → provider default → env. Mirrors `getApiKey` on the TS side,
/// restricted to the two env vars the Gemini SDK honours.
fn resolve_api_key(
    provider_key: &Option<String>,
    opts: &SimpleStreamOptions,
    allow_env: bool,
) -> Option<String> {
    if let Some(key) = opts.api_key.as_ref().filter(|key| !key.is_empty()) {
        return Some(key.clone());
    }
    if let Some(key) = provider_key.as_ref().filter(|key| !key.is_empty()) {
        return Some(key.clone());
    }
    if !allow_env {
        return None;
    }
    std::env::var("GOOGLE_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .ok()
        .filter(|key| !key.is_empty())
}

/// The producer task. Mirrors the TS `stream` async IIFE:
/// start → POST with retry → drive SSE → Done/Error.
async fn run_google_stream(
    prod: &mut AssistantMessageEventStreamProducer,
    http: reqwest::Client,
    model: &Model,
    prepared: Result<BuiltRequest, String>,
    api_key: Option<String>,
    opts: &SimpleStreamOptions,
) {
    let mut mapper = GoogleMapper::new(model, now_ms());
    let started = Instant::now();

    let BuiltRequest { url, body } = match prepared {
        Ok(request) => {
            let _ = started;
            request
        }
        Err(error) => {
            let event = mapper.error_event(ErrorReason::Error, error);
            prod.push(event).await;
            return;
        }
    };

    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(key) = api_key.as_deref() {
        headers.push(("x-goog-api-key".into(), key.to_string()));
    }
    if let Some(extra) = model.headers.as_ref() {
        for (name, value) in extra {
            headers.push((name.clone(), value.clone()));
        }
    }
    if let Some(extra) = opts.headers.as_ref() {
        for (name, value) in extra {
            headers.push((name.clone(), value.clone()));
        }
    }

    prod.push(mapper.start_event()).await;

    let signal = opts.signal.clone();
    let body = std::sync::Arc::new(body);
    let headers = std::sync::Arc::new(headers);
    let response = retry_provider_request(
        || {
            let http = http.clone();
            let url = url.clone();
            let body = body.clone();
            let headers = headers.clone();
            async move { send_once(http, url, body, headers).await }
        },
        opts.max_retries,
        opts.max_retry_delay,
        &signal,
    )
    .await;

    let response = match response {
        Ok(response) => response,
        Err(error) => {
            let reason = if matches!(error, AiError::Abort { .. }) {
                ErrorReason::Aborted
            } else {
                ErrorReason::Error
            };
            let event = mapper.error_event(reason, error.to_string());
            prod.push(event).await;
            return;
        }
    };

    let mut sse = SseEventStream::new(response, opts.signal.clone())
        .with_idle_timeout(opts.stream_idle_timeout());

    loop {
        match sse.next_event().await {
            Ok(Some(event)) => {
                let data = event.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    if data == "[DONE]" {
                        break;
                    }
                    continue;
                }
                let chunk: serde_json::Value = match serde_json::from_str(data) {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        let event = mapper.error_event(
                            ErrorReason::Error,
                            format!("Malformed Google stream chunk: {error}"),
                        );
                        prod.push(event).await;
                        return;
                    }
                };
                for mapped in mapper.on_chunk(&chunk, model) {
                    prod.push(mapped).await;
                }
            }
            Ok(None) => break,
            Err(error) => {
                let reason = if matches!(error, AiError::Abort { .. }) {
                    ErrorReason::Aborted
                } else {
                    ErrorReason::Error
                };
                let event = mapper.error_event(reason, error.to_string());
                prod.push(event).await;
                return;
            }
        }
    }

    if opts.signal.is_cancelled() {
        let event = mapper.error_event(ErrorReason::Aborted, "Request was aborted".into());
        prod.push(event).await;
        return;
    }

    match mapper.finish() {
        Ok(events) => {
            for event in events {
                prod.push(event).await;
            }
            let reason = mapper.done_reason().unwrap_or(DoneReason::Stop);
            let message = mapper.into_message();
            let event = AssistantMessageEvent::Done {
                reason,
                message,
            };
            prod.push(event).await;
        }
        Err(error) => {
            let event = mapper.error_event(ErrorReason::Error, error);
            prod.push(event).await;
        }
    }
}

/// One HTTP attempt. Non-2xx becomes a retryable `AiError::Http` carrying the
/// status code + the response body (mirrors the TS `ApiError` normalization).
async fn send_once(
    http: reqwest::Client,
    url: String,
    body: std::sync::Arc<serde_json::Value>,
    headers: std::sync::Arc<Vec<(String, String)>>,
) -> Result<reqwest::Response, AiError> {
    let mut request = http
        .post(&url)
        .header("content-type", "application/json")
        .json(&*body);
    for (name, value) in headers.iter() {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.send().await.map_err(|error| AiError::Http {
        status: None,
        message: error.to_string(),
    })?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let code = status.as_u16();
    let body = response.text().await.unwrap_or_default();
    Err(AiError::Http {
        status: Some(code),
        message: body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::SimpleStreamOptions;
    use crate::providers::anthropic::sse::SseDecoderState;

    #[test]
    fn catalog_covers_the_three_wire_families() {
        let models = google_models();
        assert!(models.iter().any(|m| m.id == "gemini-2.5-flash"));
        assert!(models
            .iter()
            .any(|m| build_params::uses_thinking_level(m)));
        assert!(models.iter().all(|m| m.api == crate::types::Api::GoogleGenerativeAi));
        // 不假装知道价格。
        assert!(models.iter().all(|m| !m.cost_configured));
    }

    #[test]
    fn explicit_opts_key_wins_over_provider_key_and_env() {
        std::env::set_var("GOOGLE_API_KEY", "env-key");
        let provider = Some("provider-key".to_string());
        let mut opts = SimpleStreamOptions::default();
        opts.api_key = Some("opts-key".into());
        assert_eq!(
            resolve_api_key(&provider, &opts, true).as_deref(),
            Some("opts-key")
        );
        opts.api_key = None;
        assert_eq!(
            resolve_api_key(&provider, &opts, true).as_deref(),
            Some("provider-key")
        );
        assert_eq!(
            resolve_api_key(&None, &opts, true).as_deref(),
            Some("env-key")
        );
        // Non-env providers must not pick the ambient key up.
        assert_eq!(resolve_api_key(&None, &opts, false), None);
        std::env::remove_var("GOOGLE_API_KEY");
    }

    #[test]
    fn raw_sse_frames_decode_into_the_same_events_the_mapper_makes() {
        // This is the wire path without HTTP: SSE bytes → decoded data → mapper.
        let raw = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}\n\n\
                   data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" there\"}]},\"finishReason\":\"STOP\"}],\
                   \"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":2,\"totalTokenCount\":5}}\n\n";
        let mut decoder = SseDecoderState::new();
        let mut chunks = Vec::new();
        for line in raw.split('\n') {
            if let Some(event) = decoder.decode_line(line) {
                chunks.push(event.data);
            }
        }
        assert_eq!(chunks.len(), 2);

        let model = google_models()
            .into_iter()
            .find(|m| m.id == "gemini-2.5-flash")
            .unwrap();
        let mut mapper = GoogleMapper::new(&model, 0);
        let mut kinds = Vec::new();
        for chunk in &chunks {
            let value: serde_json::Value = serde_json::from_str(chunk).unwrap();
            for event in mapper.on_chunk(&value, &model) {
                kinds.push(event_kind(&event));
            }
        }
        for event in mapper.finish().unwrap() {
            kinds.push(event_kind(&event));
        }
        assert_eq!(
            kinds,
            vec!["text_start", "text_delta", "text_delta", "text_end"]
        );
        let message = mapper.into_message();
        assert_eq!(
            crate::types::Content::text_only(&message.content, ""),
            "hi there"
        );
        assert_eq!(message.usage.total_tokens, 5);
    }

    /// 方言之间的工具声明必须同义：同一个 `Tool` 出去，每个方言的声明里
    /// name / description / 参数内容一致，只是外壳不同（Anthropic 是
    /// `input_schema`，Gemini 是 `parametersJsonSchema`）。
    ///
    /// ⚠ 覆盖范围：anthropic 与 google 的 builder 是 `pub`，可以直接对拍；
    /// `openai_completions` / `openai_responses` 的 builder 在冻结文件里是私有的，
    /// 为了让它们可对拍就得改冻结文件的可见性 —— 那要记台账，不该为了测试顺手做。
    /// 这两个方言的声明形状由它们自己的（frozen，上游原样）单测守着。
    #[test]
    fn tool_declaration_parity_anthropic_vs_google() {
        use crate::types::{Schema, Tool};
        let tool = Tool {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: Schema::new(serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            })),
            constrained_sampling: None,
        };
        let anthropic = crate::providers::anthropic::build_params::convert_tools(
            std::slice::from_ref(&tool),
            false,
            false,
            false,
            None,
            false,
        );
        let anthropic = serde_json::to_value(&anthropic).unwrap();
        let google = build_params::convert_tools(std::slice::from_ref(&tool), false);

        let a = &anthropic[0];
        let g = &google[0]["functionDeclarations"][0];
        assert_eq!(a["name"], g["name"]);
        assert_eq!(a["description"], g["description"]);
        // Same parameter tree, different外壳.
        assert_eq!(
            a["input_schema"]["properties"]["path"],
            g["parametersJsonSchema"]["properties"]["path"]
        );
        assert_eq!(a["input_schema"]["required"], g["parametersJsonSchema"]["required"]);
        assert!(a.get("parametersJsonSchema").is_none());
        assert!(g.get("input_schema").is_none());
    }

    fn event_kind(event: &AssistantMessageEvent) -> &'static str {
        match event {
            AssistantMessageEvent::Start { .. } => "start",
            AssistantMessageEvent::TextStart { .. } => "text_start",
            AssistantMessageEvent::TextDelta { .. } => "text_delta",
            AssistantMessageEvent::TextEnd { .. } => "text_end",
            AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
            AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
            AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
            AssistantMessageEvent::ToolCallStart { .. } => "toolcall_start",
            AssistantMessageEvent::ToolCallDelta { .. } => "toolcall_delta",
            AssistantMessageEvent::ToolCallEnd { .. } => "toolcall_end",
            AssistantMessageEvent::Done { .. } => "done",
            AssistantMessageEvent::Error { .. } => "error",
        }
    }
}
