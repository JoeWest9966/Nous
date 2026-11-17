//! `pi.live` 的类型面：正在跑的那一轮生成/工具的可呈现状态。
//!
//! 上游：`packages/durable/src/harness/live.ts`（452 行）。这里只落 `PartialChanges` 需要的那三件
//! —— `LiveState`、`LiveGeneration`、`ToolSlot`。`LiveRun` / `NestedToolSlot` / `CompactionStatus` /
//! `CompactionFailure` / `RunFailure` 以及那 7 个函数、`LIVE_DOC`、`checkpoint_when` 都不在这个
//! 提交里（见 `docs/harness.md` §②"公开面"）。
//!
//! ## 字段名
//!
//! 一律照上游 `camelCase`（`callId` / `droppedBytes` / `taskId` / `pollAt` …），`Option` 为 `None`
//! 时整个键不出现。互通只靠序列化后的字段名，所以下面每条都有对应的键名测试
//! （`tests/harness_live.rs`）。
//!
//! ## 有意保留的缺口
//!
//! - `ToolSlot.diagnostics`（上游 `ToolDiagnostic[]`）还没落：`harness/types.ts` 那张诊断类型还没提货，
//!   而本库今天没有写它的地方。加的时候照上游键名 `diagnostics`。
//! - `LiveState` 只写了本库今天会写的字段；其余字段随写它们的功能一起落，**不是忘了**。上游
//!   `LiveState` 的 `run` / `nestedTools` / `compactions` / `compactionFailure` / `runFailure` 都还缺。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Id;

/// 一轮生成尝试的呈现：第几次、已提交的节流 partial、重试与延后轮询的时间点。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveGeneration {
    /// 第几次尝试（从 1 起）。
    pub attempt: u64,
    /// 已提交的节流 partial：**整条 assistant 消息的 JSON 形态**，不是 `AssistantMessage` 结构体
    /// （文档里存的是 JSON）。上游类型是 `JsonRepresentation<AssistantMessage>`，同一个意思。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
    /// 下一次尝试前的退避：什么时候（毫秒时间戳）试、上次为什么失败。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// provider 侧延后响应，等它到点再轮询。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred: Option<Deferred>,
}

/// 重试退避。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Retry {
    /// 什么时候（毫秒时间戳）可以重试。
    pub at: u64,
    /// 上一次失败的错误文本。
    pub error: String,
}

/// 延后响应的轮询时间点。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deferred {
    /// 什么时候（毫秒时间戳）轮询。
    pub poll_at: u64,
}

/// 一次工具调用的呈现。模型发起与嵌套调用共用这套字段。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSlot {
    /// 这次调用的 ID（嵌套调用是 `<父 callId>/<key>`）。
    pub call_id: String,
    /// 工具名。
    pub name: String,
    /// 到哪一步了。
    pub status: ToolStatus,
    /// 保留的运行时输出，以及超出上限被丢掉的部分。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// 被丢掉多少字节。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_bytes: Option<u64>,
    /// 被丢掉多少行。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_lines: Option<u64>,
    /// 上一次 `details()` 的值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// 还没开始的调用（顺序执行的轮次里）没有它。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<Id>,
    /// 请求里没提供这个工具：它的任务直接以 `tool_unavailable` 结束，什么都不跑。上游是字面量 `true`，
    /// 所以这里 `false` 时整个键不出现，而不是写成 `false`。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unoffered: bool,
    /// 做完之后的结果 entry；工具任务 faulted 或孤儿时没有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<Id>,
}

/// 工具槽的三步。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    /// 还没开始（顺序轮次里排着）。
    Pending,
    /// 正在跑。
    Running,
    /// 做完了。
    Done,
}

/// `pi.live` 的文档值：这一轮生成与工具轮的呈现。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveState {
    /// 当前这次生成尝试。**它定义了 `checkpoint_when` 的一半**（有它在跑就说明不是完整 base）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<LiveGeneration>,
    /// 这一轮的工具调用，按调用顺序。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSlot>>,
}
