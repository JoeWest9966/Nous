//! `PartialChanges`：把流式事件里"这一节流间隔内变了什么"攒起来，让一次 partial 提交只花新文本的钱。
//!
//! 上游：`packages/durable/src/harness/partial-changes.ts`（185 行）。
//!
//! 两条成本原则（上游注释里的原话，这里是它们的落点）：
//!
//! 1. **正文/思考**：只记 `text_delta` / `thinking_delta` 的增量，提交时交给
//!    [`assign_json`](super::json::assign_json) 按 `Append` 计划写 —— 已提交的字符串加上增量正好等于
//!    新字符串的长度就发 `a`。整块 `set` 会让 chord 每次都拿新旧两条字符串比长度，O(长度)/次。
//!    块的 `*_end` 事件把这个块的增量丢掉：**块结束时 provider 可能替换整段文本**，那时增量不再可信。
//! 2. **工具参数**：每个调用一个 [`StreamingJsonParser`]，喂 `toolcall_delta` 的原始参数文本，
//!    它报的就是"这一片改了什么"（`Set` / `Append`）。提交时按顺序把这些改动应用到**已提交的**参数上，
//!    参数本身从消息赋值里排除（`Keep`），于是已提交的参数恰好等于"到目前为止解析出的文本"。
//!    `toolcall_end` 结束跟踪；那次提交把最终参数写上。
//!
//! ## 与上游的两处转译
//!
//! - 上游 `message` 是一个可变的 JS 对象（`PartialMessage`），`take()` 就地清掉被跟踪调用的
//!   `arguments`。这里是 `&mut Value`，同样的就地改。
//! - 上游 `applyChange()` 拿到的是"已提交消息里那一块"的 `Draft`，直接 `parent[key] = …` 写；
//!   本库的 `Draft` 只能按绝对路径写，所以先按路径**重新算一遍**再 `set` / `append_text`。
//!   数组里的字符串仍然整值赋值（上游注释：`appendText()` 只寻址对象成员，所以数组元素要整条比）。

use std::collections::BTreeMap;

use nous_ai::utils::streaming_json::{JsonChange, StreamingJsonParser};
use nous_ai::AssistantMessageEvent;
use nous_chord::delta::draft::Draft;
use nous_chord::delta::{OpError, Seg};
use serde_json::{Map, Value};

use super::json::{assign_json, child, value_at, AssignPlan};
use super::under;
use super::seg;

/// 正在攒增量的那个字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    /// 正文 `text`。
    Text,
    /// 思考 `thinking`。
    Thinking,
}

impl Field {
    /// 字段名（就是消息块里的键名）。
    fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Thinking => "thinking",
        }
    }
}

/// 一个内容块攒着的增量。
struct PendingText {
    field: Field,
    text: String,
}

/// 一个被跟踪的工具调用的状态。
struct ToolCallState {
    /// 解析参数文本的增量解析器。
    parser: StreamingJsonParser,
    /// 上一次提交之后攒下的改动。
    changes: Vec<JsonChange>,
    /// 有没有哪次提交把这些改动拿走过。
    committed: bool,
}

/// 一次 partial 提交除了消息本身还要写的东西，来自 [`PartialChanges::take`]。
#[derive(Clone, Debug)]
pub struct PartialCommit {
    /// 写消息用的计划：哪些字符串是追加，工具参数留给 `changes`。
    pub plan: AssignPlan,
    /// 每个被跟踪的工具调用：它自上次提交以来的参数改动（按顺序），以及它的快照。
    pub tool_calls: BTreeMap<usize, ToolCallCommit>,
}

/// 一个工具调用在这次提交里要写的东西。
#[derive(Clone, Debug)]
pub struct ToolCallCommit {
    /// 自上次提交以来的参数改动。
    pub changes: Vec<JsonChange>,
    /// 上次提交时已提交的参数：提交失败后从这里重来，或已提交消息里那块丢了时整块写这个。
    pub snapshot: Value,
}

/// 从一条响应的流式事件里攒"自上次提交以来 partial 消息变了什么"。
#[derive(Default)]
pub struct PartialChanges {
    text: BTreeMap<usize, PendingText>,
    tool_calls: BTreeMap<usize, ToolCallState>,
}

impl PartialChanges {
    /// 一个什么都还没观察到的收集器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 记下 `event` 改变了什么。
    pub fn observe(&mut self, event: &AssistantMessageEvent) {
        match event {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
                ..
            } => self.push_text(*content_index, Field::Text, delta),
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
                ..
            } => self.push_text(*content_index, Field::Thinking, delta),
            // provider 可能在块结束时替换整段文本，所以攒的增量到这里就不再可信。
            AssistantMessageEvent::TextEnd { content_index, .. }
            | AssistantMessageEvent::ThinkingEnd { content_index, .. } => {
                self.text.remove(content_index);
            }
            AssistantMessageEvent::ToolCallStart { content_index, .. } => {
                self.tool_calls.insert(
                    *content_index,
                    ToolCallState {
                        parser: StreamingJsonParser::new(),
                        changes: Vec::new(),
                        committed: false,
                    },
                );
            }
            AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
                ..
            } => {
                let Some(call) = self.tool_calls.get_mut(content_index) else {
                    return;
                };
                for change in call.parser.append(delta) {
                    // 参数必须是对象：根上一旦出现别的形状，这一片就不算参数 —— 停止跟踪，
                    // 下一次提交整块写快照（`commit` 里 `call.committed` 那条分支）。
                    if change.is_root() {
                        let object = matches!(&change, JsonChange::Set { value, .. } if value.is_object());
                        if !object {
                            self.tool_calls.remove(content_index);
                            return;
                        }
                    }
                    call.changes.push(change);
                }
            }
            AssistantMessageEvent::ToolCallEnd { content_index, .. } => {
                self.tool_calls.remove(content_index);
            }
            _ => {}
        }
    }

    fn push_text(&mut self, content_index: usize, field: Field, delta: &str) {
        match self.text.get_mut(&content_index) {
            Some(pending) if pending.field == field => pending.text.push_str(delta),
            _ => {
                self.text.insert(
                    content_index,
                    PendingText {
                        field,
                        text: delta.to_string(),
                    },
                );
            }
        }
    }

    /// 取走"自上次调用以来"的改动，用来提交 `message`（partial 的一份副本）。被跟踪的工具调用的
    /// `arguments` 会在 `message` 里清空：提交时按改动逐条写。提交失败就调
    /// [`restore`](Self::restore) 把它们放回去。
    pub fn take(&mut self, message: &mut Value) -> PartialCommit {
        let mut blocks: BTreeMap<Seg, AssignPlan> = BTreeMap::new();
        for (index, pending) in std::mem::take(&mut self.text) {
            blocks.insert(
                Seg::index(index),
                AssignPlan::Fields(BTreeMap::from([(
                    Seg::key(pending.field.name()),
                    AssignPlan::Append { text: pending.text },
                )])),
            );
        }
        let mut tool_calls = BTreeMap::new();
        for (index, call) in self.tool_calls.iter_mut() {
            let Some(block) = message
                .get_mut("content")
                .and_then(|content| content.get_mut(*index))
            else {
                continue;
            };
            // 被跟踪的调用就是 `toolcall_start` 点名的那个块；这里只是确认它还是工具调用。
            if block.get("type").and_then(Value::as_str) != Some("toolCall") {
                continue;
            }
            let snapshot = block
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new()));
            if let Some(object) = block.as_object_mut() {
                object.insert("arguments".to_string(), Value::Object(Map::new()));
            }
            tool_calls.insert(
                *index,
                ToolCallCommit {
                    changes: std::mem::take(&mut call.changes),
                    snapshot,
                },
            );
            blocks.insert(
                Seg::index(*index),
                AssignPlan::Fields(BTreeMap::from([(Seg::key("arguments"), AssignPlan::Keep)])),
            );
        }
        PartialCommit {
            plan: AssignPlan::Fields(BTreeMap::from([(
                Seg::key("content"),
                AssignPlan::Fields(blocks),
            )])),
            tool_calls,
        }
    }

    /// 把 `commit` 取走的东西写进 `generation.message`：先按计划逐叶写消息，再逐条应用参数改动。
    ///
    /// 已提交消息里**新出现**的调用，它的参数从这次改动开始写（之前的改动不在已提交消息里）；
    /// 已提交消息**丢了**一个已经提交过改动的调用（别的写者重置了它）时，从此整块写快照。
    pub fn commit(
        &mut self,
        generation: &mut Draft,
        prefix: &[Seg],
        message: &mut Value,
        commit: PartialCommit,
    ) -> Result<(), OpError> {
        let PartialCommit { plan, tool_calls } = commit;
        let message_path = under(prefix, &[Seg::key("message")]);
        let committed_blocks = value_at(generation.value(), &message_path)
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let mut apply: Vec<(usize, Vec<JsonChange>)> = Vec::new();
        for (index, taken) in tool_calls {
            let Some(call) = self.tool_calls.get_mut(&index) else {
                continue;
            };
            let block = message.get("content").and_then(|content| content.get(index));
            let is_tool_call = block
                .and_then(|block| block.get("type"))
                .and_then(Value::as_str)
                == Some("toolCall");
            if index >= committed_blocks && call.committed && is_tool_call {
                // 之前那些改动不在已提交消息里：整块写快照，并停止跟踪这次调用。
                self.tool_calls.remove(&index);
                if let Some(object) = message
                    .get_mut("content")
                    .and_then(|content| content.get_mut(index))
                    .and_then(Value::as_object_mut)
                {
                    object.insert("arguments".to_string(), taken.snapshot);
                }
                continue;
            }
            call.committed = true;
            apply.push((index, taken.changes));
        }

        assign_json(generation, &message_path, message, Some(&plan))?;

        let content = value_at(generation.value(), &message_path)
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (index, changes) in apply {
            let Some(block) = content.get(index) else {
                continue;
            };
            if block.get("type").and_then(Value::as_str) != Some("toolCall") {
                continue;
            }
            let block_path = under(&message_path, &[Seg::key("content"), Seg::index(index)]);
            for change in changes {
                apply_change(generation, &block_path, &change)?;
            }
        }
        Ok(())
    }

    /// 把失败提交取走的东西放回去，下一次提交先应用它们。
    pub fn restore(&mut self, commit: PartialCommit) {
        for (index, taken) in commit.tool_calls {
            if let Some(call) = self.tool_calls.get_mut(&index) {
                let mut changes = taken.changes;
                changes.append(&mut call.changes);
                call.changes = changes;
            }
        }
    }
}

/// 把一条参数改动应用到已提交的那块参数上。
///
/// 上游在这里直接 `parent[key] = …`（它拿得到那块参数的 `Draft`）；本库的 `Draft` 只能按绝对路径
/// 写，所以路径要重新拼一遍 —— 拼接**只在这里**做，`PathSegment → Seg` 的转换仍然只有
/// [`seg`] 一处。
fn apply_change(draft: &mut Draft, block: &[Seg], change: &JsonChange) -> Result<(), OpError> {
    let arguments = child(block, &Seg::key("arguments"));
    let Some((last, prefix)) = change.path().split_last() else {
        // 只有对象根会被跟踪（`observe()` 丢掉别的），而且只有 `Set` 会从根开始。
        if let JsonChange::Set { value, .. } = change {
            if value.is_object() {
                draft.set(&arguments, value.clone())?;
            }
        }
        return Ok(());
    };
    let mut parent_path = arguments;
    parent_path.extend(seg(prefix));
    let full_path = child(&parent_path, &seg(std::slice::from_ref(last))[0]);
    match change {
        JsonChange::Set { value, .. } => draft.set(&full_path, value.clone()),
        JsonChange::Append { text, .. } => {
            // 数组里的字符串整值赋值（上游：`appendText()` 只寻址对象成员），对象成员才追加。
            if matches!(value_at(draft.value(), &parent_path), Some(Value::Array(_))) {
                let old = value_at(draft.value(), &full_path)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                draft.set(&full_path, Value::String(old + text))
            } else {
                draft.append_text(&full_path, text)
            }
        }
    }
}
