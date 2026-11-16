//! Request building for the Google Generative AI (Gemini) dialect.
//!
//! Mirrors `packages/ai/src/api/google-shared.ts`（`convertMessages` /
//! `convertTools` / thinking 解析 / `mapStopReason` 等）与
//! `packages/ai/src/api/google-generative-ai.ts` 的 `buildParams`。**只有请求形状，
//! 没有 HTTP** —— 发送在 [`super`] 的 runner 里。
//!
//! 与上游的一处结构差异（不是语义差异）：TS 走 `@google/genai` SDK，config 由 SDK
//! 摊平成 REST body；Rust 直接说 REST，所以这里手写摊平的结果：
//!
//! ```text
//! { contents, systemInstruction:{parts:[{text}]},
//!   tools:[{functionDeclarations:[…]}],
//!   toolConfig:{functionCallingConfig:{mode}},
//!   generationConfig:{temperature?, maxOutputTokens?, thinkingConfig?} }
//! ```
//!
//! `thinkingConfig` 在 REST 里住在 `generationConfig` 下（SDK 负责搬），
//! `abortSignal` 是 SDK 侧字段、不上线（我们靠 `CancellationToken`）。

use serde_json::{json, Map, Value};

use crate::model::Model;
use crate::provider::SimpleStreamOptions;
use crate::types::{
    Content, Context, Message, ModelCost, ThinkingBudgets, ThinkingLevel, Tool, UserContent,
};

/// The finished request: URL + JSON body. Auth headers are assembled by the runner.
pub struct BuiltRequest {
    pub url: String,
    pub body: Value,
}

/// A resolved Google thinking level — the four Gemini API levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleLevel {
    Minimal,
    Low,
    Medium,
    High,
}

impl GoogleLevel {
    pub fn api_name(self) -> &'static str {
        match self {
            GoogleLevel::Minimal => "MINIMAL",
            GoogleLevel::Low => "LOW",
            GoogleLevel::Medium => "MEDIUM",
            GoogleLevel::High => "HIGH",
        }
    }
}

/// Build the `streamGenerateContent` request. Mirrors TS `buildParams`.
pub fn build_request(model: &Model, ctx: &Context, opts: &SimpleStreamOptions) -> BuiltRequest {
    BuiltRequest {
        url: stream_url(model),
        body: build_body(model, ctx, opts),
    }
}

/// `{base_url}/models/{id}:streamGenerateContent?alt=sse`.
///
/// TS sets `apiVersion = ""` whenever `model.baseUrl` is given, because the base
/// URL already carries the version path (`…/v1beta`); we keep the same rule by
/// never inserting a version ourselves.
pub fn stream_url(model: &Model) -> String {
    format!(
        "{}/models/{}:streamGenerateContent?alt=sse",
        model.base_url.trim_end_matches('/'),
        model.id
    )
}

/// The request body. Mirrors TS `buildParams`'s `GenerateContentParameters`.
pub fn build_body(model: &Model, ctx: &Context, opts: &SimpleStreamOptions) -> Value {
    let contents = convert_messages(model, ctx);
    let mut generation_config = Map::new();
    if let Some(temperature) = opts.temperature {
        generation_config.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = opts.max_tokens {
        generation_config.insert("maxOutputTokens".into(), json!(max_tokens));
    }

    let supports_strict = supports_strict_tool_sampling(&model.id);
    let mut body = Map::new();
    body.insert("contents".into(), Value::Array(contents));
    if let Some(system) = ctx.system_prompt.as_deref().filter(|s| !s.is_empty()) {
        body.insert(
            "systemInstruction".into(),
            json!({ "parts": [{ "text": sanitize_surrogates(system) }] }),
        );
    }
    if !ctx.tools.is_empty() {
        body.insert("tools".into(), convert_tools(&ctx.tools, supports_strict));
        if let Some(mode) = resolve_function_calling_mode(&ctx.tools, supports_strict) {
            body.insert(
                "toolConfig".into(),
                json!({ "functionCallingConfig": { "mode": mode } }),
            );
        }
    }

    // ---- thinking (mirrors streamSimple → buildParams) ----
    if let Some(config) = thinking_config(model, opts) {
        generation_config.insert("thinkingConfig".into(), config);
    }

    if !generation_config.is_empty() {
        body.insert("generationConfig".into(), Value::Object(generation_config));
    }
    Value::Object(body)
}

/// Mirrors `streamSimple`'s thinking decision + `buildParams`'s `thinkingConfig`.
///
/// `None` = no `thinkingConfig` key at all (model can't reason). `Some(..)` is
/// either the disabled form (`thinkingBudget: 0`) or the enabled form (discrete
/// `thinkingLevel` for Gemini ≥3 naming, token `thinkingBudget` otherwise).
pub fn thinking_config(model: &Model, opts: &SimpleStreamOptions) -> Option<Value> {
    if !model.reasoning {
        return None;
    }
    // `!options.reasoning` in TS == `None` here.
    let requested = match opts.reasoning {
        Some(level) if level != ThinkingLevel::Off => level,
        Some(_) => return Some(disabled_thinking_config(model)),
        None => return Some(disabled_thinking_config(model)),
    };
    let clamped = model.clamp_thinking_level(requested);
    if clamped == ThinkingLevel::Off {
        return Some(disabled_thinking_config(model));
    }
    let level = resolve_thinking_level(model, clamped).ok()?;
    let mut config = Map::new();
    config.insert("includeThoughts".into(), Value::Bool(true));
    if uses_thinking_level(model) {
        config.insert("thinkingLevel".into(), json!(level.api_name()));
    } else {
        config.insert(
            "thinkingBudget".into(),
            json!(budget_for(model, level, opts.thinking_budgets.as_ref())),
        );
    }
    Some(Value::Object(config))
}

/// Mirrors `getDisabledGoogleThinkingConfig`.
pub fn disabled_thinking_config(model: &Model) -> Value {
    if !uses_thinking_level(model) {
        return json!({ "thinkingBudget": 0 });
    }
    let fallback = model.clamp_thinking_level(ThinkingLevel::Off);
    if fallback == ThinkingLevel::Off {
        return json!({ "thinkingBudget": 0 });
    }
    match resolve_thinking_level(model, fallback) {
        Ok(level) => json!({ "thinkingLevel": level.api_name() }),
        Err(_) => json!({ "thinkingBudget": 0 }),
    }
}

/// Mirrors `resolveGoogleThinkingLevel`: a model-level mapping wins, and the
/// result must be one of the four API levels.
pub fn resolve_thinking_level(model: &Model, level: ThinkingLevel) -> Result<GoogleLevel, String> {
    let mapped = model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(&level))
        .and_then(|value| value.clone());
    let resolved = mapped
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| thinking_level_name(level).to_string());
    match resolved.as_str() {
        "minimal" => Ok(GoogleLevel::Minimal),
        "low" => Ok(GoogleLevel::Low),
        "medium" => Ok(GoogleLevel::Medium),
        "high" => Ok(GoogleLevel::High),
        other => Err(format!(
            "Unsupported Google thinking level mapping for {}/{}: {level:?} -> {other}",
            model.provider, model.id
        )),
    }
}

fn thinking_level_name(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

/// Mirrors `usesGoogleThinkingLevel`: Gemini ≥3 / Gemini 4+ / gemma-4 take the
/// discrete level; everything else (2.5 family) takes a token budget.
pub fn uses_thinking_level(model: &Model) -> bool {
    let id = model.id.to_ascii_lowercase();
    if id == "gemini-flash-latest" || id == "gemini-flash-lite-latest" {
        return true;
    }
    gemini_3_pro_or_flash(&id) || gemini_4_or_later(&id) || gemma_4(&id)
}

/// `/gemini-3(?:\.\d+)?-(?:pro|flash)/`
fn gemini_3_pro_or_flash(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("gemini-3") else {
        return false;
    };
    let rest = match rest.strip_prefix('.') {
        Some(after_dot) => match after_dot.find(|c: char| !c.is_ascii_digit()) {
            Some(idx) => &after_dot[idx..],
            None => return false,
        },
        None => rest,
    };
    rest.starts_with("-pro") || rest.starts_with("-flash")
}

/// `/gemini-(?:[4-9]|[1-9]\d)/`
fn gemini_4_or_later(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("gemini-") else {
        return false;
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return false;
    }
    // 第一段数字：4..9 或 10+（两位以上且不以 0 开头）
    if digits.len() == 1 {
        matches!(digits.as_bytes()[0], b'4'..=b'9')
    } else {
        digits.as_bytes()[0] != b'0'
    }
}

/// `/gemma-?4/`
fn gemma_4(id: &str) -> bool {
    id.contains("gemma-4") || id.contains("gemma4")
}

/// Mirrors `getGoogleBudget`.
pub fn budget_for(
    model: &Model,
    level: GoogleLevel,
    custom: Option<&ThinkingBudgets>,
) -> i64 {
    if let Some(custom) = custom {
        let pick = match level {
            GoogleLevel::Minimal => custom.minimal,
            GoogleLevel::Low => custom.low,
            GoogleLevel::Medium => custom.medium,
            GoogleLevel::High => custom.high,
        };
        if let Some(value) = pick {
            return value as i64;
        }
    }
    let id = model.id.as_str();
    let budget = |minimal: i64, low: i64, medium: i64, high: i64| match level {
        GoogleLevel::Minimal => minimal,
        GoogleLevel::Low => low,
        GoogleLevel::Medium => medium,
        GoogleLevel::High => high,
    };
    if id.contains("2.5-pro") {
        return budget(128, 2048, 8192, 32768);
    }
    if id.contains("2.5-flash-lite") {
        return budget(512, 2048, 8192, 24576);
    }
    if id.contains("2.5-flash") {
        return budget(128, 2048, 8192, 24576);
    }
    -1
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// `parametersJsonSchema` for each tool. Mirrors `convertTools(useParameters=false)`.
pub fn convert_tools(tools: &[Tool], supports_strict: bool) -> Value {
    let declarations: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let strict = supports_strict && strict_tool_parameters(tool).is_some();
            let parameters = if strict {
                strict_tool_parameters(tool).unwrap_or_else(|| tool.parameters.as_value().clone())
            } else {
                tool.parameters.as_value().clone()
            };
            json!({
                "name": tool.name,
                "description": tool.description,
                "parametersJsonSchema": rewrite_const_as_enum(&parameters),
            })
        })
        .collect();
    json!([{ "functionDeclarations": declarations }])
}

fn strict_tool_parameters(tool: &Tool) -> Option<Value> {
    crate::strict_schema::strict_tool_parameters(tool)
}

/// Mirrors `resolveGoogleFunctionCallingMode` with `toolChoice` absent.
///
/// ⚠ 与上游的一处**已知缺口**：TS 的 `toolChoice`（`auto` / `none` / `any`）住在
/// `GoogleOptions` 上，而本库的 `SimpleStreamOptions` 没有这个字段（`rpi-ai`
/// 也没有）—— 冻结类型不轻改。所以这里固定按「没给 toolChoice」走：
/// 有 strict 工具 → `VALIDATED`，否则不出 `toolConfig`。
/// 要支持显式 `none`/`any`，得先给 provider 层加一条 per-provider 选项通道，
/// 那是一次改契约的活，记在 #5 的票里。
pub fn resolve_function_calling_mode(tools: &[Tool], supports_strict: bool) -> Option<&'static str> {
    resolve_function_calling_mode_with(tools, None, supports_strict)
}

/// The `mapToolChoice` + strict-wins rule, split out so it is testable without
/// constructing a whole `SimpleStreamOptions`.
pub fn resolve_function_calling_mode_with(
    tools: &[Tool],
    tool_choice: Option<&str>,
    supports_strict: bool,
) -> Option<&'static str> {
    if tools.is_empty() {
        return None;
    }
    if matches!(tool_choice, Some("none") | Some("any")) {
        return Some(map_tool_choice(tool_choice));
    }
    let use_strict = supports_strict
        && tools
            .iter()
            .any(|tool| strict_tool_parameters(tool).is_some());
    if use_strict {
        return Some("VALIDATED");
    }
    tool_choice.map(|choice| map_tool_choice(Some(choice)))
}

/// Mirrors `mapToolChoice`.
pub fn map_tool_choice(choice: Option<&str>) -> &'static str {
    match choice {
        Some("none") => "NONE",
        Some("any") => "ANY",
        _ => "AUTO",
    }
}

/// Keywords whose values map names to subschemas — their keys are data, not schema keywords.
const NAME_MAPS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
    "dependencies",
];

/// Keywords whose value is a subschema (or an array of them).
const SUBSCHEMAS: &[&str] = &[
    "additionalProperties",
    "unevaluatedProperties",
    "propertyNames",
    "items",
    "prefixItems",
    "additionalItems",
    "unevaluatedItems",
    "contains",
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "if",
    "then",
    "else",
    "contentSchema",
];

/// Mirrors `rewriteConstAsEnum`: Gemini ignores `const` in `parametersJsonSchema`
/// but honours `enum`, so string/number `const` becomes a one-value `enum`.
pub fn rewrite_const_as_enum(schema: &Value) -> Value {
    rewrite_const_inner(schema, false)
}

fn rewrite_const_inner(schema: &Value, is_name_map: bool) -> Value {
    match schema {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| rewrite_const_inner(item, false))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                if is_name_map || SUBSCHEMAS.contains(&key.as_str()) {
                    out.insert(key.clone(), rewrite_const_inner(value, false));
                } else if NAME_MAPS.contains(&key.as_str()) {
                    out.insert(key.clone(), rewrite_const_inner(value, true));
                } else {
                    out.insert(key.clone(), value.clone());
                }
            }
            if !is_name_map && !out.contains_key("enum") {
                if let Some(constant) = out.get("const") {
                    if constant.is_string() || constant.is_number() {
                        let constant = constant.clone();
                        out.remove("const");
                        out.insert("enum".into(), Value::Array(vec![constant]));
                    }
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// (Unused by the Gemini path today; kept for the Cloud-Code-Assist variant of
/// `convertTools(useParameters=true)`, which strips JSON-Schema meta keywords.)
#[allow(dead_code)]
pub fn sanitize_for_open_api(schema: &Value) -> Value {
    const META: &[&str] = &[
        "$schema",
        "$id",
        "$anchor",
        "$dynamicAnchor",
        "$vocabulary",
        "$comment",
        "$defs",
        "definitions",
    ];
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !META.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), sanitize_for_open_api(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Mirrors `convertMessages`: pi messages → Gemini `contents[]`.
pub fn convert_messages(model: &Model, ctx: &Context) -> Vec<Value> {
    let require_id = requires_tool_call_id(&model.id);
    let multimodal_response = supports_multimodal_function_response(&model.id);
    let mut contents: Vec<Value> = Vec::new();

    for message in &ctx.messages {
        match message {
            Message::User(user) => match &user.content {
                UserContent::Text(text) => {
                    if text.is_empty() {
                        continue;
                    }
                    contents.push(json!({
                        "role": "user",
                        "parts": [{ "text": sanitize_surrogates(text) }],
                    }));
                }
                UserContent::Blocks(blocks) => {
                    let parts: Vec<Value> = blocks
                        .iter()
                        .filter_map(|block| match block {
                            Content::Text(text) => {
                                Some(json!({ "text": sanitize_surrogates(&text.text) }))
                            }
                            Content::Image(image) => Some(json!({
                                "inlineData": { "mimeType": image.mime_type, "data": image.data },
                            })),
                            _ => None,
                        })
                        .collect();
                    if parts.is_empty() {
                        continue;
                    }
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            },
            Message::Assistant(assistant) => {
                let same_model =
                    assistant.provider == model.provider && assistant.model == model.id;
                let mut parts: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        Content::Text(text) => {
                            let signature =
                                resolve_thought_signature(same_model, text.text_signature.as_deref());
                            // 空文本块只在没有签名时丢；签名挂在空 part 上是合法的，丢了会断思维链。
                            if text.text.trim().is_empty() && signature.is_none() {
                                continue;
                            }
                            let mut part = Map::new();
                            part.insert("text".into(), json!(sanitize_surrogates(&text.text)));
                            if let Some(signature) = signature {
                                part.insert("thoughtSignature".into(), json!(signature));
                            }
                            parts.push(Value::Object(part));
                        }
                        Content::Thinking(thinking) => {
                            let signature = resolve_thought_signature(
                                same_model,
                                thinking.thinking_signature.as_deref(),
                            );
                            if thinking.thinking.trim().is_empty() && signature.is_none() {
                                continue;
                            }
                            if same_model {
                                let mut part = Map::new();
                                part.insert("thought".into(), Value::Bool(true));
                                part.insert(
                                    "text".into(),
                                    json!(sanitize_surrogates(&thinking.thinking)),
                                );
                                if let Some(signature) = signature {
                                    part.insert("thoughtSignature".into(), json!(signature));
                                }
                                parts.push(Value::Object(part));
                            } else {
                                // 跨模型：签名不可用，降级成普通文本（不加标签，免得模型模仿）。
                                parts.push(json!({ "text": sanitize_surrogates(&thinking.thinking) }));
                            }
                        }
                        Content::ToolCall(call) => {
                            let signature = resolve_thought_signature(
                                same_model,
                                call.thought_signature.as_deref(),
                            );
                            let mut function_call = Map::new();
                            function_call.insert("name".into(), json!(call.name));
                            function_call.insert("args".into(), call.arguments.clone());
                            if require_id {
                                function_call.insert("id".into(), json!(normalize_tool_call_id(&call.id)));
                            }
                            let mut part = Map::new();
                            part.insert("functionCall".into(), Value::Object(function_call));
                            if let Some(signature) = signature {
                                part.insert("thoughtSignature".into(), json!(signature));
                            }
                            parts.push(Value::Object(part));
                        }
                        Content::Image(_) => {}
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            Message::ToolResult(result) => {
                let text = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let images: Vec<Value> = if model.input.contains(&crate::types::InputModality::Image)
                {
                    result
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            Content::Image(image) => Some(json!({
                                "inlineData": { "mimeType": image.mime_type, "data": image.data },
                            })),
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let has_text = !text.trim().is_empty();
                let has_images = !images.is_empty();
                let response_value = if has_text {
                    json!(sanitize_surrogates(&text))
                } else if has_images {
                    json!("(see attached image)")
                } else {
                    json!("")
                };
                let mut function_response = Map::new();
                function_response.insert("name".into(), json!(result.tool_name));
                function_response.insert(
                    if result.is_error { "error" } else { "output" }.into(),
                    response_value,
                );
                if has_images && multimodal_response {
                    function_response.insert("parts".into(), Value::Array(images.clone()));
                }
                if require_id {
                    function_response
                        .insert("id".into(), json!(normalize_tool_call_id(&result.tool_call_id)));
                }
                let part = json!({ "functionResponse": Value::Object(function_response) });

                let can_merge = matches!(
                    contents.last(),
                    Some(Value::Object(last))
                        if last.get("role").and_then(Value::as_str) == Some("user")
                            && last
                                .get("parts")
                                .and_then(Value::as_array)
                                .is_some_and(|parts| {
                                    parts.iter().any(|p| p.get("functionResponse").is_some())
                                })
                );
                if can_merge {
                    if let Some(Value::Object(last)) = contents.last_mut() {
                        if let Some(Value::Array(parts)) = last.get_mut("parts") {
                            parts.push(part);
                        }
                    }
                } else {
                    contents.push(json!({ "role": "user", "parts": [part] }));
                }

                // Gemini < 3 不接受 image 嵌在 functionResponse 里：另起一个 user turn。
                if has_images && !multimodal_response {
                    let mut parts = vec![json!({ "text": "Tool result image:" })];
                    parts.extend(images);
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }
    }
    contents
}

/// Mirrors `requiresToolCallId`.
pub fn requires_tool_call_id(model_id: &str) -> bool {
    model_id.starts_with("claude-")
        || model_id.starts_with("gpt-oss-")
        || gemini_major_version(model_id).is_some_and(|major| major >= 3)
}

/// Mirrors `getGeminiMajorVersion`.
pub fn gemini_major_version(model_id: &str) -> Option<u32> {
    let lower = model_id.to_ascii_lowercase();
    let rest = lower.strip_prefix("gemini-")?;
    let rest = rest.strip_prefix("live-").unwrap_or(rest);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Mirrors `supportsGoogleStrictToolSampling`: Gemini 3+ enforces required
/// function parameters in validated tool-calling modes.
pub fn supports_strict_tool_sampling(model_id: &str) -> bool {
    gemini_major_version(model_id).is_some_and(|major| major >= 3)
}

fn supports_multimodal_function_response(model_id: &str) -> bool {
    gemini_major_version(model_id).map_or(true, |major| major >= 3)
}

/// TS `normalizeToolCallId`: only for models that require ids, and the API caps
/// them at 64 chars of `[A-Za-z0-9_-]`.
fn normalize_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// Mirrors `resolveThoughtSignature`: signatures only replay for the same
/// provider+model and must be valid base64 (length multiple of 4).
pub fn resolve_thought_signature(same_provider_and_model: bool, signature: Option<&str>) -> Option<String> {
    let signature = signature?;
    if !same_provider_and_model {
        return None;
    }
    if signature.is_empty() || signature.len() % 4 != 0 {
        return None;
    }
    let valid = signature
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
        || signature.ends_with('=')
        || signature.ends_with("==");
    valid.then(|| signature.to_string())
}

/// Mirrors `retainThoughtSignature`: a later empty/absent signature must not
/// erase the one an earlier delta carried.
pub fn retain_thought_signature(existing: Option<String>, incoming: Option<&str>) -> Option<String> {
    match incoming {
        Some(value) if !value.is_empty() => Some(value.to_string()),
        _ => existing,
    }
}

/// Rust `String`s cannot hold lone surrogates, so this is a pass-through kept
/// for parity with the TS `sanitizeSurrogates` call sites.
pub fn sanitize_surrogates(text: &str) -> String {
    text.to_string()
}

/// Cost is provider-agnostic math living under the anthropic module; Gemini
/// calls the same function (TS `calculateCost`).
pub fn calculate_cost(cost: &ModelCost, usage: &crate::types::Usage) -> crate::types::UsageCost {
    crate::providers::anthropic::cost::calculate_cost(cost, usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        Api, AssistantMessage, Content, ImageContent, ImageContentType, InputModality, Schema,
        TextContent, TextContentType, ThinkingContent, ThinkingContentType, ToolCall, ToolCallType,
        ToolResultMessage, ToolResultRole, Usage, UserMessage, UserRole,
    };
    use std::collections::BTreeMap;

    fn model(id: &str) -> Model {
        Model::new(
            id,
            id,
            Api::GoogleGenerativeAi,
            "google",
            "https://generativelanguage.googleapis.com/v1beta",
        )
    }

    fn opts() -> SimpleStreamOptions {
        SimpleStreamOptions::default()
    }

    fn tool(name: &str, parameters: Value) -> Tool {
        Tool {
            name: name.into(),
            description: format!("{name} tool"),
            parameters: Schema::new(parameters),
            constrained_sampling: None,
        }
    }

    fn text(t: &str) -> Content {
        Content::Text(TextContent {
            kind: TextContentType,
            text: t.into(),
            text_signature: None,
        })
    }

    fn assistant(content: Vec<Content>, provider: &str, model_id: &str) -> Message {
        Message::Assistant(Box::new(AssistantMessage {
            role: crate::types::AssistantRole,
            content,
            api: Api::GoogleGenerativeAi,
            provider: provider.into(),
            model: model_id.into(),
            response_model: None,
            response_id: None,
            usage: Usage::zero(),
            stop_reason: crate::types::StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        }))
    }

    #[test]
    fn url_never_double_appends_a_version() {
        let m = model("gemini-2.5-flash");
        assert_eq!(
            stream_url(&m),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn system_prompt_becomes_system_instruction() {
        let m = model("gemini-2.5-flash");
        let ctx = Context {
            system_prompt: Some("be terse".into()),
            messages: vec![Message::User(UserMessage::new("hi", 0))],
            tools: Vec::new(),
        };
        let body = build_body(&m, &ctx, &opts());
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be terse");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn tools_become_function_declarations_with_const_rewritten_to_enum() {
        let m = model("gemini-2.5-flash");
        let ctx = Context {
            system_prompt: None,
            messages: vec![],
            tools: vec![
                tool(
                    "read",
                    json!({
                        "$schema": "http://json-schema.org/draft-07/schema#",
                        "type": "object",
                        "properties": {
                            "mode": { "const": "fast", "type": "string" },
                            "limit": { "const": 10, "type": "number" }
                        }
                    }),
                ),
                tool("ls", json!({ "type": "object", "properties": {} })),
            ],
        };
        let body = build_body(&m, &ctx, &opts());
        let decls = &body["tools"][0]["functionDeclarations"];
        assert_eq!(decls[0]["name"], "read");
        // `parametersJsonSchema` carries FULL JSON Schema, so `$schema` stays
        // (upstream only strips meta keywords on the OpenAPI `parameters` path).
        assert_eq!(
            decls[0]["parametersJsonSchema"]["$schema"],
            "http://json-schema.org/draft-07/schema#"
        );
        // `const` ignored by Gemini → single-value enum.
        assert_eq!(
            decls[0]["parametersJsonSchema"]["properties"]["mode"]["enum"][0],
            "fast"
        );
        assert_eq!(
            decls[0]["parametersJsonSchema"]["properties"]["limit"]["enum"][0],
            10
        );
        assert!(decls[0]["parametersJsonSchema"]["properties"]["limit"]
            .get("const")
            .is_none());
        assert_eq!(decls[1]["name"], "ls");
        // Tools present but no explicit choice and no strict tool → no toolConfig.
        assert!(body.get("toolConfig").is_none());
    }

    #[test]
    fn tool_choice_maps_to_function_calling_mode() {
        let tools = vec![tool("ls", json!({"type": "object"}))];
        assert_eq!(
            resolve_function_calling_mode_with(&tools, Some("none"), false),
            Some("NONE")
        );
        assert_eq!(
            resolve_function_calling_mode_with(&tools, Some("any"), false),
            Some("ANY")
        );
        assert_eq!(
            resolve_function_calling_mode_with(&tools, Some("auto"), false),
            Some("AUTO")
        );
        assert_eq!(resolve_function_calling_mode_with(&tools, None, false), None);
        // No tools → no mode at all, even with a choice.
        assert_eq!(
            resolve_function_calling_mode_with(&[], Some("any"), false),
            None
        );
    }

    #[test]
    fn no_thinking_config_without_reasoning() {
        let m = model("gemini-2.5-flash");
        assert!(thinking_config(&m, &opts()).is_none());
    }

    #[test]
    fn disabled_thinking_uses_budget_zero_for_2_5_and_level_for_gemini_3() {
        let mut m = model("gemini-2.5-flash");
        m.reasoning = true;
        assert_eq!(thinking_config(&m, &opts()), Some(json!({ "thinkingBudget": 0 })));

        let mut m3 = model("gemini-3-pro-preview");
        m3.reasoning = true;
        m3.thinking_level_map = Some(BTreeMap::from([(ThinkingLevel::Off, None)]));
        assert_eq!(thinking_config(&m3, &opts()), Some(json!({ "thinkingBudget": 0 })));
    }

    #[test]
    fn enabled_thinking_picks_level_vs_budget_by_model_family() {
        let mut m = model("gemini-2.5-pro");
        m.reasoning = true;
        let mut o = opts();
        o.reasoning = Some(ThinkingLevel::Medium);
        assert_eq!(
            thinking_config(&m, &o),
            Some(json!({ "includeThoughts": true, "thinkingBudget": 8192 }))
        );

        let mut m3 = model("gemini-3-flash-preview");
        m3.reasoning = true;
        assert_eq!(
            thinking_config(&m3, &o),
            Some(json!({ "includeThoughts": true, "thinkingLevel": "MEDIUM" }))
        );

        // Custom budgets win over the model defaults.
        let mut m = model("gemini-2.5-flash");
        m.reasoning = true;
        o.thinking_budgets = Some(ThinkingBudgets {
            minimal: None,
            low: None,
            medium: Some(1234),
            high: None,
            xhigh: None,
            max: None,
        });
        assert_eq!(
            thinking_config(&m, &o),
            Some(json!({ "includeThoughts": true, "thinkingBudget": 1234 }))
        );
    }

    #[test]
    fn unsupported_thinking_mapping_is_an_error_not_a_silent_default() {
        let mut m = model("gemini-2.5-flash");
        m.reasoning = true;
        m.thinking_level_map = Some(BTreeMap::from([(
            ThinkingLevel::High,
            Some("xhigh".to_string()),
        )]));
        assert!(resolve_thinking_level(&m, ThinkingLevel::High).is_err());
    }

    #[test]
    fn thinking_level_families() {
        for id in [
            "gemini-3-pro-preview",
            "gemini-3.1-pro-preview",
            "gemini-3-flash-preview",
            "gemini-4-argon",
            "gemini-flash-latest",
            "gemma-4-27b",
            "gemma4-27b",
        ] {
            assert!(uses_thinking_level(&model(id)), "{id} should use thinkingLevel");
        }
        for id in ["gemini-2.5-pro", "gemini-2.5-flash", "gemini-2.0-flash"] {
            assert!(!uses_thinking_level(&model(id)), "{id} should use thinkingBudget");
        }
    }

    #[test]
    fn temperature_and_max_tokens_land_in_generation_config() {
        let m = model("gemini-2.5-flash");
        let mut o = opts();
        o.temperature = Some(0.25);
        o.max_tokens = Some(512);
        let body = build_body(&m, &Context::new(vec![]), &o);
        assert_eq!(body["generationConfig"]["temperature"], 0.25);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 512);
    }

    #[test]
    fn tool_result_merges_into_a_preceding_function_response_turn() {
        let m = model("gemini-2.5-flash");
        let result = |id: &str, body: &str| {
            Message::ToolResult(Box::new(ToolResultMessage {
                role: ToolResultRole,
                tool_call_id: id.into(),
                tool_name: "read".into(),
                content: vec![text(body)],
                details: None,
                usage: None,
                added_tool_names: Vec::new(),
                is_error: false,
                timestamp: 0,
            }))
        };
        let ctx = Context::new(vec![result("a", "one"), result("b", "two")]);
        let contents = convert_messages(&m, &ctx);
        // One merged user turn, both responses in it.
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["parts"].as_array().unwrap().len(), 2);
        assert_eq!(contents[0]["parts"][0]["functionResponse"]["output"], "one");
        assert_eq!(contents[0]["parts"][1]["functionResponse"]["output"], "two");
    }

    #[test]
    fn tool_result_error_uses_error_key_and_ids_only_where_required() {
        let result = Message::ToolResult(Box::new(ToolResultMessage {
            role: ToolResultRole,
            tool_call_id: "call/1".into(),
            tool_name: "read".into(),
            content: vec![text("boom")],
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            is_error: true,
            timestamp: 0,
        }));
        let ctx = Context::new(vec![result]);

        let two_five = convert_messages(&model("gemini-2.5-flash"), &ctx);
        assert_eq!(two_five[0]["parts"][0]["functionResponse"]["error"], "boom");
        assert!(two_five[0]["parts"][0]["functionResponse"].get("id").is_none());

        let three = convert_messages(&model("gemini-3-pro-preview"), &ctx);
        assert_eq!(three[0]["parts"][0]["functionResponse"]["id"], "call_1");
    }

    #[test]
    fn cross_model_thinking_downgrades_to_plain_text_and_drops_signatures() {
        let m = model("gemini-2.5-flash");
        let thinking = Content::Thinking(ThinkingContent {
            kind: ThinkingContentType,
            thinking: "hmm".into(),
            thinking_signature: Some("c2lnbg==".into()),
            redacted: false,
        });
        let from_other = assistant(vec![thinking.clone()], "google", "gemini-3-pro-preview");
        let contents = convert_messages(&m, &Context::new(vec![from_other]));
        assert_eq!(contents[0]["parts"][0]["text"], "hmm");
        assert!(contents[0]["parts"][0].get("thought").is_none());
        assert!(contents[0]["parts"][0].get("thoughtSignature").is_none());

        let from_same = assistant(vec![thinking], "google", "gemini-2.5-flash");
        let contents = convert_messages(&m, &Context::new(vec![from_same]));
        assert_eq!(contents[0]["parts"][0]["thought"], true);
        assert_eq!(contents[0]["parts"][0]["thoughtSignature"], "c2lnbg==");
    }

    #[test]
    fn empty_text_block_survives_only_when_it_carries_a_signature() {
        let m = model("gemini-2.5-flash");
        let bare = assistant(vec![text("   ")], "google", "gemini-2.5-flash");
        assert!(convert_messages(&m, &Context::new(vec![bare])).is_empty());

        let signed = assistant(
            vec![Content::Text(TextContent {
                kind: TextContentType,
                text: "".into(),
                text_signature: Some("c2lnbg==".into()),
            })],
            "google",
            "gemini-2.5-flash",
        );
        let contents = convert_messages(&m, &Context::new(vec![signed]));
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["parts"][0]["thoughtSignature"], "c2lnbg==");
    }

    #[test]
    fn tool_call_args_and_id_shape() {
        let call = Content::ToolCall(ToolCall {
            kind: ToolCallType,
            id: "call/with space".into(),
            name: "read".into(),
            arguments: json!({"path": "a.txt"}),
            thought_signature: None,
            namespace: None,
            recorded: None,
        });
        let from_gemini_3 = assistant(vec![call.clone()], "google", "gemini-3-pro-preview");
        let contents = convert_messages(&model("gemini-3-pro-preview"), &Context::new(vec![from_gemini_3]));
        assert_eq!(contents[0]["parts"][0]["functionCall"]["name"], "read");
        assert_eq!(contents[0]["parts"][0]["functionCall"]["args"]["path"], "a.txt");
        assert_eq!(
            contents[0]["parts"][0]["functionCall"]["id"],
            "call_with_space"
        );

        let from_two_five = assistant(vec![call], "google", "gemini-2.5-flash");
        let contents = convert_messages(&model("gemini-2.5-flash"), &Context::new(vec![from_two_five]));
        assert!(contents[0]["parts"][0]["functionCall"].get("id").is_none());
    }

    #[test]
    fn images_nest_inside_function_response_only_for_gemini_3_plus() {
        let image = Content::Image(ImageContent {
            kind: ImageContentType,
            mime_type: "image/png".into(),
            data: "AAAA".into(),
        });
        let result = Message::ToolResult(Box::new(ToolResultMessage {
            role: ToolResultRole,
            tool_call_id: "c1".into(),
            tool_name: "screenshot".into(),
            content: vec![text("done"), image],
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            is_error: false,
            timestamp: 0,
        }));

        let mut m3 = model("gemini-3-flash-preview");
        m3.input = vec![InputModality::Image, InputModality::Text];
        let contents = convert_messages(&m3, &Context::new(vec![result.clone()]));
        assert_eq!(
            contents[0]["parts"][0]["functionResponse"]["parts"][0]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(contents.len(), 1);

        let mut m2 = model("gemini-2.5-flash");
        m2.input = vec![InputModality::Image, InputModality::Text];
        let contents = convert_messages(&m2, &Context::new(vec![result]));
        assert!(contents[0]["parts"][0]["functionResponse"].get("parts").is_none());
        // Second turn carries the image for < 3.
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[1]["parts"][0]["text"], "Tool result image:");
    }

    #[test]
    fn thought_signature_rules() {
        assert_eq!(
            resolve_thought_signature(true, Some("c2lnbg==")),
            Some("c2lnbg==".to_string())
        );
        assert_eq!(resolve_thought_signature(false, Some("c2lnbg==")), None);
        assert_eq!(resolve_thought_signature(true, Some("bad")), None);
        assert_eq!(resolve_thought_signature(true, Some("")), None);
        assert_eq!(resolve_thought_signature(true, None), None);

        assert_eq!(
            retain_thought_signature(Some("keep==".into()), Some("")),
            Some("keep==".to_string())
        );
        assert_eq!(
            retain_thought_signature(Some("keep==".into()), Some("new==")),
            Some("new==".to_string())
        );
        assert_eq!(
            retain_thought_signature(Some("keep==".into()), None),
            Some("keep==".to_string())
        );
    }

    #[test]
    fn requires_tool_call_id_matrix() {
        assert!(requires_tool_call_id("claude-sonnet-4"));
        assert!(requires_tool_call_id("gpt-oss-120b"));
        assert!(requires_tool_call_id("gemini-3-pro-preview"));
        assert!(requires_tool_call_id("gemini-live-3-flash"));
        assert!(!requires_tool_call_id("gemini-2.5-flash"));
        assert!(!requires_tool_call_id("gemma-4-27b"));
    }

    #[test]
    fn user_blocks_keep_image_parts_and_skip_empty_sets() {
        let image = Content::Image(ImageContent {
            kind: ImageContentType,
            mime_type: "image/jpeg".into(),
            data: "QQ==".into(),
        });
        let message = Message::User(UserMessage {
            role: UserRole,
            content: UserContent::Blocks(vec![text("look"), image]),
            timestamp: 0,
        });
        let contents = convert_messages(&model("gemini-2.5-flash"), &Context::new(vec![message]));
        assert_eq!(contents[0]["parts"][0]["text"], "look");
        assert_eq!(contents[0]["parts"][1]["inlineData"]["mimeType"], "image/jpeg");

        let empty = Message::User(UserMessage {
            role: UserRole,
            content: UserContent::Blocks(vec![]),
            timestamp: 0,
        });
        assert!(convert_messages(&model("gemini-2.5-flash"), &Context::new(vec![empty])).is_empty());
    }
}
