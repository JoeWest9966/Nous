//! 观察收尾的信号与实例 —— `packages/chord/src/watch.ts` 的 Rust 翻译。
//!
//! 本模块是纯库：`Context` 的取消/值沿链传递，异步由上层驱动（没有 tokio 运行时、
//! 没有 `Promise`）。`watchEvents` 视角**不提货**（`docs/spec-from-pi-durable.md` 已定），
//! 事件出口用本模块自己的 [`RemoteEvent`] 形状（`nous-wire` 的投影同源，但 `nous-chord`
//! 无内部依赖，不能反向依赖 `nous-wire`）。
//!
//! 窗口语义照上游 `Conversation.watch()`（`packages/durable/src/harness/view.ts`）：
//! [`ConversationWindow::DEFAULT_MIN`] = 50，max 默认 2 × min；不是「给全部 entries」。
//! 写入超出 max 时一次 splice 砍回 min，并把被砍掉的最后一条 id 记进 `before`。

pub mod call;
mod end;
pub mod handle;
pub mod window;

pub use call::{NativeWatchCall, WatchCall};
pub use end::WatchEnd;
pub use handle::{
    Listener, NativeWatch, PollOutcome, Watch, WatchHandle, WatchStartError,
    MAX_PENDING_NATIVE_VALUES,
};
pub use window::{ConversationView, ConversationWindow, WindowBounds, WindowError};

use std::sync::Arc;

use serde_json::Value;

/// 事件出口。形状对齐 `nous-wire::RemoteEvent`（`wire.md` 的字段名），但本 crate
/// 不依赖 `nous-wire`：这里只定义类型，真正上线时由服务层把它转成线上的事件。
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteEvent {
    Snapshot {
        session: Option<Value>,
        generation: Option<Value>,
        entries: Vec<Value>,
    },
    MessageStart {
        message: Value,
        message_id: Option<String>,
    },
    MessageUpdate {
        changes: Vec<Value>,
        message_id: Option<String>,
        message: Option<Value>,
    },
    MessageEnd {
        entry: Option<Value>,
        message: Option<Value>,
        message_id: Option<String>,
    },
    EntryAppended { entry: Value },
    Submission { record: Option<Value> },
    TaskFailed {
        message: String,
        task_id: Option<String>,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: Option<String>,
        output: Value,
    },
    Unknown,
}

impl RemoteEvent {
    /// `wire.md` 的事件名；`Unknown` 没有名字。
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
}

/// 观察器的事件出口：由上层实现成 SSE 发送 / 转 `nous-wire` 类型。
pub trait EventSink: Send + Sync {
    fn send(&self, event: RemoteEvent);
}

impl EventSink for Box<dyn EventSink> {
    fn send(&self, event: RemoteEvent) {
        (**self).send(event);
    }
}

impl EventSink for Arc<dyn EventSink> {
    fn send(&self, event: RemoteEvent) {
        (**self).send(event);
    }
}
