//! Folding and re-playing `SystemMessage`s. Mirrors `packages/ai/src/utils/transcript.ts`.
//!
//! # What this does
//!
//! A transcript can carry more than one system message: the first one holds the prompt and
//! the tool set, later ones patch `sections`, add or remove tools, or append more prompt text
//! (a compaction handoff is one of those). Providers differ in whether the model accepts
//! system messages mid-conversation, and upstream resolves that with two tiers:
//!
//! - the model accepts them → send them in place (`renderSystemMessageUpdate`, plus a
//!   hold-back rule in the Anthropic transport);
//! - it does not → [`collapse_system_messages`] replays every system message into **one**
//!   leading message and drops the rest.
//!
//! **Only the second tier is implemented here.** That is a scope decision, not an omission:
//! the in-place tier has no witness in this workspace — we have no model that accepts a
//! mid-conversation system message to compare against — so porting it would produce code no
//! test could ever turn red. When a real transport needs it, implement it next to
//! [`collapse_system_messages`] and give it a provider test.
//!
//! # The replay rules (each one is a place implementations drift)
//!
//! | thing | rule |
//! |---|---|
//! | `content` | appended in transcript order, joined with `"\n\n"`, empty strings skipped |
//! | `sections` | patched **by name in place**; `null` deletes; a deleted name that returns lands at the **end** |
//! | `toolsAdded` / `toolsRemoved` | replayed into the **request tool list** (never rendered as prompt text) |
//! | `timestamp` | the first system message's |

use crate::types::{Context, Message, Sections, SystemContent, SystemMessage, Tool, ToolReference};

/// The text a **leading** system message contributes: its content, then its `sections`
/// values, joined by a blank line. Mirrors upstream `getSystemMessageText`.
pub fn get_system_message_text(message: &SystemMessage) -> String {
    message.text()
}

/// The leading system message for a prompt and tool set; `None` when both are empty, so an
/// empty transcript stays empty. Mirrors upstream `createInitialSystemMessage`.
pub fn create_initial_system_message(
    system_prompt: Option<&str>,
    tools: &[Tool],
) -> Option<SystemMessage> {
    let has_prompt = system_prompt.is_some_and(|prompt| !prompt.is_empty());
    let has_tools = !tools.is_empty();
    if !has_prompt && !has_tools {
        return None;
    }
    let mut message = SystemMessage::new(system_prompt.unwrap_or("").to_string(), 0);
    if has_tools {
        message.tools_added = Some(tools.to_vec());
    }
    Some(message)
}

/// Fold `Context.systemPrompt` / `Context.tools` into a leading system message. Mirrors
/// upstream `normalizeContext` — the only entry point that produces a transcript.
pub fn normalize_context(context: &Context) -> Vec<Message> {
    let initial = create_initial_system_message(
        context.system_prompt.as_deref(),
        context.tools.as_slice(),
    );
    let mut messages = Vec::with_capacity(context.messages.len() + 1);
    if let Some(initial) = initial {
        messages.push(Message::System(Box::new(initial)));
    }
    messages.extend(context.messages.iter().cloned());
    messages
}

/// The leading system message, if the transcript starts with one.
pub fn get_initial_system_message(messages: &[Message]) -> Option<&SystemMessage> {
    match messages.first() {
        Some(Message::System(message)) => Some(message),
        _ => None,
    }
}

/// The transcript without its leading system message (for transports that carry the prompt
/// out of band).
pub fn without_initial_system_message(messages: &[Message]) -> &[Message] {
    if get_initial_system_message(messages).is_some() {
        &messages[1..]
    } else {
        messages
    }
}

/// The tools available after replaying every system message in order.
///
/// A redefinition keeps the tool's original position (JS `Map.set` on an existing key);
/// `toolsRemoved` deletes by name. This is the order the request must send — not "the last
/// batch", which would make every removal a no-op that still compiles.
pub fn get_current_tools(messages: &[Message]) -> Vec<Tool> {
    let mut tools: Vec<Tool> = Vec::new();
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        for removed in system.tools_removed.iter().flatten() {
            tools.retain(|tool| tool.name != removed.name);
        }
        for added in system.tools_added.iter().flatten() {
            match tools.iter_mut().find(|tool| tool.name == added.name) {
                Some(slot) => *slot = added.clone(),
                None => tools.push(added.clone()),
            }
        }
    }
    tools
}

/// Replay every system message into one leading system message holding the current prompt,
/// sections and tools. Mirrors upstream `getCurrentSystemMessage`.
///
/// Returns `None` when the transcript carries no system message at all (upstream returns
/// `undefined` in that case rather than an empty message).
pub fn get_current_system_message(messages: &[Message]) -> Option<SystemMessage> {
    let mut content: Vec<String> = Vec::new();
    let mut sections = Sections::new();
    let mut timestamp: Option<i64> = None;
    for message in messages {
        let Message::System(system) = message else {
            continue;
        };
        timestamp.get_or_insert(system.timestamp);
        let text = crate::types::system_content_text(&system.content);
        if !text.is_empty() {
            content.push(text);
        }
        if let Some(patch) = &system.sections {
            for (name, value) in patch.iter() {
                match value {
                    Some(value) => sections.set(name, value),
                    None => sections.remove(name),
                }
            }
        }
    }
    let tools = get_current_tools(messages);
    let timestamp = timestamp?;
    let mut message = SystemMessage::new(content.join("\n\n"), timestamp);
    if !sections.is_empty() {
        message.sections = Some(sections);
    }
    if !tools.is_empty() {
        message.tools_added = Some(tools);
    }
    Some(message)
}

/// The replayed leading prompt as text. Mirrors upstream `getCurrentSystemPrompt`.
pub fn get_current_system_prompt(messages: &[Message]) -> String {
    get_current_system_message(messages)
        .map(|message| get_system_message_text(&message))
        .unwrap_or_default()
}

/// Rebuild the transcript for transports without mid-conversation system messages: the
/// replayed system message leads and **every** system message is dropped from the body.
/// Mirrors upstream `collapseSystemMessages`.
pub fn collapse_system_messages(messages: &[Message]) -> Vec<Message> {
    let head = get_current_system_message(messages);
    let mut collapsed = Vec::with_capacity(messages.len() + 1);
    if let Some(head) = head {
        collapsed.push(Message::System(Box::new(head)));
    }
    collapsed.extend(
        messages
            .iter()
            .filter(|message| !matches!(message, Message::System(_)))
            .cloned(),
    );
    collapsed
}

/// What a provider actually sends: the leading prompt (folded and replayed), the body without
/// any system message, and the request tool list.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedContext {
    /// `None` when the context carried neither a prompt nor tools nor system messages.
    pub system: Option<SystemMessage>,
    /// Body messages. **Contains no `Message::System`** — [`collapse_system_messages`] moved
    /// them all into `system`.
    pub messages: Vec<Message>,
    /// The replay result of `toolsAdded` / `toolsRemoved`; falls back to `Context.tools`
    /// (which `normalize_context` already folded into the leading message), so an empty vector
    /// means "no tools".
    pub tools: Vec<Tool>,
}

impl PreparedContext {
    /// The leading prompt as text (`""` when there is none).
    pub fn system_text(&self) -> String {
        self.system
            .as_ref()
            .map(get_system_message_text)
            .unwrap_or_default()
    }
}

/// Fold + collapse a context into what a provider sends. **Every provider starts here.**
pub fn prepare(context: &Context) -> PreparedContext {
    let normalized = normalize_context(context);
    let tools = get_current_tools(&normalized);
    let collapsed = collapse_system_messages(&normalized);
    let mut messages = collapsed;
    let system = match messages.first() {
        Some(Message::System(message)) => {
            let system = (**message).clone();
            let _ = messages.remove(0);
            Some(system)
        }
        _ => None,
    };
    PreparedContext {
        system,
        messages,
        tools,
    }
}

/// A system message that only patches `sections` — the shape later system messages take.
pub fn section_patch(sections: &[(&str, Option<&str>)], timestamp: i64) -> SystemMessage {
    let mut message = SystemMessage::new(String::new(), timestamp);
    let patch = Sections::with(
        sections
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.map(str::to_string))),
    );
    message.sections = Some(patch);
    message
}

/// A content-only system message (a compaction handoff is one): no sections, no tools.
pub fn appended_text(text: impl Into<String>, timestamp: i64) -> SystemMessage {
    SystemMessage::new(text.into(), timestamp)
}

/// Convenience for the removal half of a tool change.
pub fn removed(names: &[&str]) -> Vec<ToolReference> {
    names.iter().map(|name| ToolReference::from(*name)).collect()
}

/// The system content as loose text — providers that must flatten blocks use this.
pub fn content_text(content: &SystemContent) -> String {
    crate::types::system_content_text(content)
}
