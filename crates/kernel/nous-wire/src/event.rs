//! `wire.md` §「GET /v1/sessions/:id/events」的 Rust 投影。
//!
//! 规则（照抄 `wire.md` 的通用约定）：
//!
//! - **只有 `wire.md` 点名依赖的字段才具名**，其余一律 `serde_json::Value` 透传。
//! - 一律不 `deny_unknown_fields`：引擎加字段是常态，多一个字段不该让我们反序列化失败。
//! - `Option` 字段 `skip_serializing_if`，不往线上吐多余 `null`。
//! - 认不出来的 `type` 落 [`RemoteEvent::Unknown`] / [`MessageChange::Unknown`]：**不 panic、不断连接**。
//!
//! ## 一个必须知道的局限
//!
//! `Unknown` 是 `#[serde(other)]` 的单元变体，它**不记得原来的 `type` 字面量**。
//! 这对客户端是够的（不认识就忽略），但意味着本类型层**不能用来做保真重放**：
//! 想要「原样转发未知事件」，得在更外层保留原始 `serde_json::Value`。
//!
//! ## 与上游 remote 协议的差别（有意，不是漏抄）
//!
//! 上游 remote 协议的 `protocol.rs` 里的 `ui_dialog_request` / `ui_dialog_closed` /
//! `agent_start` / `agent_end` / `retry_scheduled` / `turn_start` / `turn_end` /
//! `tool_execution_end`、以及 `ready` / `response` / `command` 那套帧，本 crate 全都不抄：
//! `wire.md` 的事件表里没有它们，`nous-wire` 只认宪法。它们落到 `Unknown`。
//! `tool_execution_end` 是最容易误抄的一个 —— `wire.md` 只点名了 `start` / `update`。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Everything `/v1/sessions/:id/events` can push.
///
/// `#[serde(tag = "type")]`：线上是 `{"type":"message_update", ...}` 一类的扁平对象，
/// 不是 `{"MessageUpdate": {...}}`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum RemoteEvent {
    /// 重连第一帧。
    ///
    /// 我们只用 `generation?.message`（有正在生成的消息就续上）和
    /// `entries[].model[].timestamp`（**唯一的时间源**，`/v1/entries` 不给时间）。
    /// `entries` 故意留 `Value`：entry 的形状有几十个字段、且只属于引擎，
    /// 具名等于替引擎把它冻结下来。
    Snapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generation: Option<Generation>,
        #[serde(default)]
        entries: Vec<Value>,
    },

    /// user / system 也会发，只有 `message.role == "assistant"` 该开口一个气泡。
    MessageStart {
        #[serde(default)]
        message: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },

    /// 增量帧。**真正的载荷是 `changes[]`**，见 [`MessageChange`]。
    MessageUpdate {
        #[serde(default)]
        changes: Vec<MessageChange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        /// 上游有时在同一条 update 里带整条 message。`wire.md` 依赖的是
        /// `changes[]`；这里收着透传，不参与增量。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<Value>,
    },

    /// 收尾：`entry.kind == "pi.assistant"` 时定稿气泡。
    MessageEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
    },

    /// 别的标签页 / 客户端追加的 entry。按 `entry.kind` 决定补不补气泡。
    EntryAppended {
        #[serde(default)]
        entry: Value,
    },

    /// `record.status == "unanswered"` 时弹「没答上来」，原因在 `reason` / `detail`。
    Submission {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record: Option<SubmissionRecord>,
    },

    /// 任务挂了，`message` 直接给用户看。
    TaskFailed {
        #[serde(default)]
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },

    /// 开一张工具卡片。
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        #[serde(default)]
        args: Value,
    },

    /// 追加 / 覆盖工具卡片的内容。
    ///
    /// `output` 留 `Value`：`wire.md` 只写了 `output.append` / `output.set`
    /// 两种意图，没钉死外壳（可能是 `{append: …}`、也可能是 `{type: "append", …}`）。
    /// 钉错了比不钉更糟 —— 等真调用方来确定再具名。
    ToolExecutionUpdate {
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        #[serde(default)]
        output: Value,
    },

    /// 不认识的事件。**不 panic、不断连接**；`type` 字面量不保留（见模块文档）。
    #[serde(other)]
    Unknown,
}

impl RemoteEvent {
    /// `wire.md` 里的事件名。`Unknown` 没有名字（原字面量已丢）。
    pub fn type_tag(&self) -> Option<&'static str> {
        match self {
            RemoteEvent::Snapshot { .. } => Some("snapshot"),
            RemoteEvent::MessageStart { .. } => Some("message_start"),
            RemoteEvent::MessageUpdate { .. } => Some("message_update"),
            RemoteEvent::MessageEnd { .. } => Some("message_end"),
            RemoteEvent::EntryAppended { .. } => Some("entry_appended"),
            RemoteEvent::Submission { .. } => Some("submission"),
            RemoteEvent::TaskFailed { .. } => Some("task_failed"),
            RemoteEvent::ToolExecutionStart { .. } => Some("tool_execution_start"),
            RemoteEvent::ToolExecutionUpdate { .. } => Some("tool_execution_update"),
            RemoteEvent::Unknown => None,
        }
    }

    /// 是不是认识的事件。`Unknown` 之外都算认识。
    pub fn is_known(&self) -> bool {
        !matches!(self, RemoteEvent::Unknown)
    }
}

/// `snapshot` 里「正在生成的那条」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Generation {
    /// 有值 = 重连后接着收这条消息的增量。其余字段（`generationId` / 计时…）
    /// `wire.md` 不依赖，不具名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
}

/// `submission` 里的 `record`。三个字段都是 `wire.md` 点名依赖的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRecord {
    /// `"unanswered"` 是「没答上来」的那一种；其余值客户端不特殊处理。
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `message_update.changes[]` —— 全集来自 pi-durable 的 `MessageChange`
/// （`wire.md` 抄的就是它），不是猜的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum MessageChange {
    /// 块的开头。`block.text` 已经是全文 —— **跳过它就等于把第一段吞掉**。
    TextStart { content_index: u64, block: Value },
    /// 思考块的开头。思考不是正文，别往气泡里塞。
    ThinkingStart { content_index: u64, block: Value },
    /// 工具调用块的开头。
    ToolcallStart { content_index: u64, block: Value },
    /// 追加文本。**只改内存里的 `Map<contentIndex, String>`**，别一个 token 一次
    /// notify（`wire.md` 的合帧铁律：16ms 定时器读一次）。
    TextDelta { content_index: u64, delta: String },
    ThinkingDelta { content_index: u64, delta: String },
    /// `path` 是 JSON 指针式子路径（`arguments.foo` 这类），不是 JSON-Patch。
    ToolcallDelta {
        content_index: u64,
        #[serde(default)]
        path: String,
        #[serde(default)]
        delta: String,
    },
    /// **整块替换**：该块后续变化都在这一批里，以它为准。
    Block { content_index: u64, block: Value },
    /// **整条替换**：以 `message` 为准。
    Message { #[serde(default)] message: Value },
    /// 不认识的 change 类型。同样不 panic、不断连接。
    #[serde(other)]
    Unknown,
}

impl MessageChange {
    /// 认识的 change 名字；`Unknown` 为 `None`。
    pub fn type_tag(&self) -> Option<&'static str> {
        match self {
            MessageChange::TextStart { .. } => Some("text_start"),
            MessageChange::ThinkingStart { .. } => Some("thinking_start"),
            MessageChange::ToolcallStart { .. } => Some("toolcall_start"),
            MessageChange::TextDelta { .. } => Some("text_delta"),
            MessageChange::ThinkingDelta { .. } => Some("thinking_delta"),
            MessageChange::ToolcallDelta { .. } => Some("toolcall_delta"),
            MessageChange::Block { .. } => Some("block"),
            MessageChange::Message { .. } => Some("message"),
            MessageChange::Unknown => None,
        }
    }
}

/// 从 entry / message 的 `Value` 里读 `role`（`message_start` 用）。
pub fn message_role(message: &Value) -> Option<&str> {
    message.get("role").and_then(Value::as_str)
}

/// 从 entry 的 `Value` 里读 `kind`（`message_end` / `entry_appended` 用）。
pub fn entry_kind(entry: &Value) -> Option<&str> {
    entry.get("kind").and_then(Value::as_str)
}
