//! `pi.live`：正在跑的那一轮生成 / 工具 / 压缩的可呈现状态。
//!
//! 上游：`packages/durable/src/harness/live.ts`（452 行）。这里落**类型面 + 九个 draft 变更函数 +
//! `checkpoint_when`**；上游那份里与本库无关的（`defineDoc` 的地址盖章、`renameReason` 迁移、
//! 任务类型判定那组）不落，理由写在各条下面。
//!
//! ## 这些函数不提交
//!
//! 九个函数都只改 `Draft<LiveState>` / `&mut LiveState`，**不碰 `Session`**（上游签名同样只拿
//! `Tx` + `Draft`）。所以 `pi.live` 的提交点在调用方 —— 工具执行层、调度层、`generation.rs`。
//! 这条不是随口说的：书签类变更**没有自家节拍**（`docs/storage.md` §6），必须**每次状态转换自带
//! 一次提交**，不许攒到下一次别的事件（一个跑 60 秒的 `bash` 期间没有别的事件，攒就等于客户端
//! 60 秒看不见这个工具）。
//!
//! ## 字段名
//!
//! 一律照上游 `camelCase`（`callId` / `droppedBytes` / `taskId` / `pollAt` / `parentTaskId` …），
//! `Option` 为 `None` 时整个键不出现；`unoffered` 只在 `true` 时出现（上游是字面量 `true`，不是
//! 布尔）。`ToolSlot` 与 `NestedToolSlot` 共用的那截写成 [`SlotProgress`] 并用 `flatten` 展开 ——
//! 上游是 TS 的交叉类型，Rust 没有交叉类型，而**两处各自抄一遍**会让 `finish_slot`/`clear_progress`
//! 需要两个几乎一样的实现，那才是真坑。
//!
//! ## 刻意不落的三件
//!
//! - **`LiveDoc.version = 3` 与它的 `migrate`**：本库的文档记录没有 per-document 版本号
//!   （`DocumentRecord` 只有 `kind`/`scope`/`history`/`fork`），而且我们从没写过 v1/v2，迁移无对象。
//!   真需要版本时按 `docs/storage.md` 的迁移链走，别在 `pi.live` 里单开一套。
//! - **`defineDoc` 自动写地址**：本库要显式建文档，地址在
//!   [`super::generation::live_record`] 一处生成，别在调用点手抄。
//! - **`RUN_TASK_KINDS` / `cutOffTooOften` / `salvageable`**：那是任务调度层的判定（要
//!   `settings.restartLimit`），随调度层落。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::types::SubmissionSettlement;
use crate::Id;

/// 一次工具调用对外公布的那截，模型发起与嵌套调用共用（上游 `SlotProgress`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotProgress {
    /// 这次调用的 ID（嵌套调用是 `<父 callId>/<key>`）。
    pub call_id: String,
    /// 工具名。
    pub name: String,
    /// 到哪一步了。
    pub status: ToolStatus,
    /// 保留的运行时输出；只有跑着的时候才写。
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
    /// 通过 `api.diagnostic()` 记下的诊断。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<ToolDiagnostic>>,
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

/// 一条工具诊断（上游 `ToolDiagnostic`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDiagnostic {
    /// 严重程度。
    pub severity: DiagnosticSeverity,
    /// 文本。
    pub message: String,
    /// 机器可读的码（可选）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// 诊断的严重程度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    /// 提示。
    Info,
    /// 警告。
    Warn,
    /// 错误。
    Error,
}

/// 当前工具轮里的一次模型发起的调用。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSlot {
    /// 共用那截（`callId` / `name` / `status` / `output` / `droppedBytes` / `droppedLines` /
    /// `details` / `diagnostics`），展开到本结构体的顶层。
    #[serde(flatten)]
    pub progress: SlotProgress,
    /// 还没开始的调用（顺序执行的轮次里）没有它。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<Id>,
    /// 请求里没提供这个工具：它的任务直接以 `tool_unavailable` 结束，什么都不跑。上游是字面量
    /// `true`，所以 `false` 时整个键不出现，而不是写成 `false`。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unoffered: bool,
    /// 做完之后的结果 entry；工具任务 faulted 或孤儿时没有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<Id>,
}

/// 跑着的工具通过 `executeTool()` 发起的一次嵌套调用。
///
/// 上游按任务 id 关联：`taskId` 是这次调用的工具任务，`parentTaskId` 是发起它的那个。
/// `callId` 是 `<父 callId>/<key>`，因为 key 里没有 `/`，拼出来唯一。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolSlot {
    /// 共用那截，展开到顶层（见 [`SlotProgress`]）。
    #[serde(flatten)]
    pub progress: SlotProgress,
    /// 发起这次调用的那个调用的 `callId`。
    pub parent_call_id: String,
    /// 发起它的那个调用的任务。
    pub parent_task_id: Id,
    /// 这次调用自己的工具任务。
    pub task_id: Id,
    /// 这次调用拿到的参数；开始执行之后换成修复 / `beforeTool` / 强转之后的实参。模型发起的调用
    /// 的参数在 assistant entry 里，不在这里。
    pub arguments: Map<String, Value>,
    /// 做完之后怎么写状态行；结果本身在调用方的嵌套调用文档里。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<NestedToolSummary>,
}

/// 嵌套调用怎么结束的：错误标志、耗时、花费、最多 500 字的错误文本。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolSummary {
    /// 是不是失败结束。
    pub is_error: bool,
    /// 跑了多久（毫秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// 这次调用自己的花费。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    /// 错误文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 一次正在跑的压缩任务（规范 §8.7）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionStatus {
    /// 压缩任务。
    pub task_id: Id,
    /// 谁起的：手动 / 过阈值 / 上下文溢出。
    pub trigger: CompactionReason,
    /// 有没有一次生成在等它（这次压缩归那次生成所有）。
    pub blocking: bool,
    /// 第几次尝试。
    pub attempt: u64,
    /// 下一次摘要尝试前的退避。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
}

/// 什么起了这次压缩。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionReason {
    /// 用户 / 宿主显式要求。
    Manual,
    /// 过了上下文阈值。
    Threshold,
    /// 上下文溢出触发的压缩。
    Overflow,
}

/// 自上次压缩产出摘要以来，最近一次失败的压缩（规范 §8.7）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionFailure {
    /// 失败的那个压缩任务。
    pub task_id: Id,
    /// 是什么起的这次压缩。
    pub trigger: CompactionReason,
    /// 失败 / faulted 结果的错误文本，或者调度器为什么把它当孤儿。
    pub error: String,
}

/// 一轮运行是怎么结束的：答上了，还是没答上（并说明为什么）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum RunEnd {
    /// 答上了，`answer` 是那条 entry。
    Done {
        /// 回答 entry。
        answer: Id,
    },
    /// 没答上：把它这一轮的输入按 `unanswered` 结算，并记下原因。
    #[serde(rename_all = "camelCase")]
    Unanswered {
        /// 为什么没答上。
        reason: RunFailureReason,
        /// 补充说明。
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// 输入怎么结算；不给就按 `unanswered` + `reason` + `detail`。
        #[serde(skip_serializing_if = "Option::is_none")]
        settlement: Option<SubmissionSettlement>,
    },
}

/// 一轮运行为什么没答上（上游 `RunFailureReason`，十种取值全抄）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFailureReason {
    /// 模型请求失败（重试用尽之后），或者请求需要的压缩失败了。
    ModelError,
    /// 没配模型，或配的那个不可用。
    NoModel,
    /// 这一轮被中止（`Conversation.abort()` / `abortTask()`）；`detail` 说它的 abort 处理被切断
    /// `settings.restartLimit` 次还没做完。
    Aborted,
    /// 排在队里的 reset 在工具之后、回答之前的边界上另起了新上下文（工具的 handoff 会接着跑）。
    Reset,
    /// 这一轮的任务在无法恢复的地方抛了，或者存储拒绝了它那次提交。
    Faulted,
    /// 这一轮的任务被中止时，没有任何已注册的定义能接它。
    Orphaned,
    /// 宿主在模型回答时停了，次数超过 `settings.retry` 的重试上限。
    Interrupted,
    /// 这一轮的生成被连续切断 `settings.restartLimit` 次；被切断的请求或轮询先算一次失败尝试。
    RestartLimit,
    /// 上下文没以用户消息或工具结果结尾（空的，或者以一条回答结尾），所以没发请求。
    NothingToAnswer,
    /// 一轮工具达到了会话的 `maxTurns`；continue 会接着它的结果跑。
    TurnLimit,
}

impl RunFailureReason {
    /// 线格式上的那个值。给「结算的 `reason` 写什么」用，不用再 `serde_json` 绕一圈。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelError => "model_error",
            Self::NoModel => "no_model",
            Self::Aborted => "aborted",
            Self::Reset => "reset",
            Self::Faulted => "faulted",
            Self::Orphaned => "orphaned",
            Self::Interrupted => "interrupted",
            Self::RestartLimit => "restart_limit",
            Self::NothingToAnswer => "nothing_to_answer",
            Self::TurnLimit => "turn_limit",
        }
    }
}

/// 最近一次没答上就结束的运行（规范 §8.2）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFailure {
    /// 结束这一轮的那个任务：它是这一轮结束时的任务。
    pub task_id: Id,
    /// 这一轮的输入，按 `unanswered` 结算的那些。
    pub inputs: Vec<Id>,
    /// 为什么没答上。
    pub reason: RunFailureReason,
    /// 补充说明（各个原因各自的意思见上游 `RunFailureReason` 的注释）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 那一轮的 `run.first`，好让 continue 的 `afterRun` 钩子也看到它的历史。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<Id>,
}

/// 一轮运行的控制面（规范 §8.2）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveRun {
    /// 把这一轮输入跑完的那个任务：它的生成，从一次交到下一次。
    pub task_id: Id,
    /// 这一轮摆进来的输入，结束时结算；答完之后只跑 `afterRun` 任务时是 `[]`。
    pub inputs: Vec<Id>,
    /// 到目前为止几轮工具（`AgentState.maxTurns` 约束它；`onYield` 续跑从 0 重数）。
    pub turns: u64,
    /// 这一轮的第一条 entry。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<Id>,
    /// 答完之后还在等 `afterRun` 钩子要的任务时才有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_run: Option<AfterRun>,
}

/// 答完之后还在等的那些任务。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AfterRun {
    /// 已经建出来的任务；排队中的在终态之前一直排队，这一轮随后按「没有失败」结束。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<Id>>,
}

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

/// `pi.live` 的文档值：运行控制面 + 这一轮生成 / 工具 / 压缩的呈现。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveState {
    /// 运行控制面：**只在忙的时候出现**，包括答完但还在跑 `afterRun` 任务的那段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<LiveRun>,
    /// 当前这次生成尝试。**它定义了 `checkpoint_when` 的一半**（有它在跑就说明不是完整 base）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<LiveGeneration>,
    /// 当前这一轮的工具调用，按调用顺序：从那条发起工具的 assistant 消息开始，到生成的 `tools` 阶段
    /// 结束它为止。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSlot>>,
    /// 跑着的工具发起的嵌套调用，按创建顺序（父在子前，任务 id 递增）。调用结算时它的子槽一起删。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nested_tools: Option<Vec<NestedToolSlot>>,
    /// 正在跑的压缩任务，按任务 id 排序；没有时整个键不出现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compactions: Option<Vec<CompactionStatus>>,
    /// 最近一次失败的压缩，好让客户端在任务结束之后还能显示。产出摘要的压缩会清掉它；被中止的、
    /// 或发现没东西可压的留着它。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_failure: Option<CompactionFailure>,
    /// 最近一次没答上就结束的运行为什么结束。在这一轮结束的那次提交里写；之后开始新一轮时删掉
    /// （同一次提交里由 `afterFailedRun: "continue"` 或 reset 起的新一轮保留它）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_failure: Option<RunFailure>,
}

/// 查找结果：上游的 `Draft<ToolSlot | NestedToolSlot>`（两处共用那截 [`SlotProgress`]）。
pub enum SlotRef<'a> {
    /// 模型发起的一轮里的槽。
    Model(&'a mut ToolSlot),
    /// 嵌套调用。
    Nested(&'a mut NestedToolSlot),
}

impl<'a> SlotRef<'a> {
    /// 共用那截，可变。
    pub fn progress_mut(&mut self) -> &mut SlotProgress {
        match self {
            Self::Model(slot) => &mut slot.progress,
            Self::Nested(slot) => &mut slot.progress,
        }
    }

    /// 这个槽的工具任务 id。
    pub fn task_id(&self) -> Option<Id> {
        match self {
            Self::Model(slot) => slot.task_id,
            Self::Nested(slot) => Some(slot.task_id),
        }
    }

    /// 消费自己，只留共用那截（`finish_slot` / `clear_progress` 用）。
    pub fn into_progress(self) -> &'a mut SlotProgress {
        match self {
            Self::Model(slot) => &mut slot.progress,
            Self::Nested(slot) => &mut slot.progress,
        }
    }
}

/// 结掉 `task_id` 名下的那一轮：结算它的输入、删 `run`，没答上时记 `runFailure`。
///
/// **总是**删 `generation` / `tools` / `nestedTools` —— 它们是那一轮的呈现，跟着结束的轮走。
/// 返回要结算的输入（`(submissionId, settlement)`）：上游在 `Tx` 里直接 `settleSubmission`，
/// 本库没有 `Tx`（`#13` 的提交是一批写），所以**把结算拿出来交给调用方**，和它那次提交一起落。
pub fn end_run(live: &mut LiveState, task_id: Id, end: &RunEnd) -> Vec<(Id, SubmissionSettlement)> {
    let mut settlements = Vec::new();
    if live.run.as_ref().is_some_and(|run| run.task_id == task_id) {
        // 先拷出来：下面要改 `live`，而 `run` 只是按值读。
        let (inputs, first, has_after_run) = live
            .run
            .as_ref()
            .map(|run| (run.inputs.clone(), run.first, run.after_run.is_some()))
            .unwrap_or_default();
        match end {
            RunEnd::Done { answer } => {
                for id in &inputs {
                    settlements.push((
                        *id,
                        SubmissionSettlement::Done {
                            answer: *answer,
                        },
                    ));
                }
            }
            // 答完之后只跑 `afterRun` 任务的那一轮，不管怎么结束都不记失败（上游同一条判断）。
            RunEnd::Unanswered {
                reason,
                detail,
                settlement,
            } if !has_after_run => {
                let fallback = SubmissionSettlement::Unanswered {
                    reason: reason.as_str().to_string(),
                    detail: detail.clone().map(Value::String),
                };
                for id in &inputs {
                    settlements.push((*id, settlement.clone().unwrap_or_else(|| fallback.clone())));
                }
                live.run_failure = Some(RunFailure {
                    task_id,
                    inputs: inputs.clone(),
                    reason: *reason,
                    detail: detail.clone(),
                    first,
                });
            }
            RunEnd::Unanswered { .. } => {}
        }
        live.run = None;
    }
    live.generation = None;
    live.tools = None;
    live.nested_tools = None;
    settlements
}

/// 记下一个在这次提交里建出来的压缩任务的状态；状态按任务 id 排序。
pub fn add_compaction_status(live: &mut LiveState, status: CompactionStatus) {
    let statuses = live.compactions.get_or_insert_with(Vec::new);
    statuses.push(status);
    // 上游依赖「任务 id 按建任务的提交顺序递增」，同一批里按 id 再排一次，不花什么钱。
    statuses.sort_by_key(|each| each.task_id);
}

/// `task_id` 那个压缩任务的状态，如果在列表里。
pub fn compaction_status(live: &LiveState, task_id: Id) -> Option<&CompactionStatus> {
    live.compactions
        .as_ref()
        .and_then(|statuses| statuses.iter().find(|each| each.task_id == task_id))
}

/// 删掉 `task_id` 那个压缩任务的状态，列表空了就把列表也删掉。
pub fn remove_compaction_status(live: &mut LiveState, task_id: Id) {
    let Some(statuses) = live.compactions.as_mut() else {
        return;
    };
    if let Some(index) = statuses.iter().position(|each| each.task_id == task_id) {
        statuses.remove(index);
    }
    if statuses.is_empty() {
        live.compactions = None;
    }
}

/// 删掉 `task_id` 那个压缩任务的状态，并记下它是因为 `error` 失败的。
pub fn fail_compaction_status(live: &mut LiveState, task_id: Id, error: String) {
    if let Some(status) = compaction_status(live, task_id) {
        live.compaction_failure = Some(CompactionFailure {
            task_id: status.task_id,
            trigger: status.trigger,
            error,
        });
    }
    remove_compaction_status(live, task_id);
}

/// 工具任务 `task_id` 的槽（模型发起的或嵌套的），如果 `pi.live` 还列着它。
///
/// 嵌套槽用二分查找：一个工具可能发起几千次嵌套调用，每次都要在好几个提交里查自己的槽，而二分
/// 每读一个元素都要在 draft 的变更跟踪里记一个节点。它们的任务 id 递增（每个槽在建它那个任务的
/// 提交里追加，任务 id 按提交顺序递增，删除又保持顺序）。
pub fn tool_slot<'a>(live: &'a mut LiveState, task_id: Id) -> Option<SlotRef<'a>> {
    if let Some(tools) = live.tools.as_mut() {
        if let Some(slot) = tools.iter_mut().find(|each| each.task_id == Some(task_id)) {
            return Some(SlotRef::Model(slot));
        }
    }
    let nested = live.nested_tools.as_mut()?;
    let found = nested
        .binary_search_by_key(&task_id, |slot| slot.task_id)
        .ok()?;
    Some(SlotRef::Nested(&mut nested[found]))
}

/// 把槽标成做完并清掉它跑着时公布的东西；调用方自己设模型发起的调用的结果 `entry`，或嵌套调用的
/// `summary`。任务 faulted 或成孤儿时这两个都不设。
pub fn finish_slot(slot: SlotRef<'_>) {
    let progress = slot.into_progress();
    progress.status = ToolStatus::Done;
    clear_progress(progress);
}

/// 删掉工具任务 `task_id` 底下的嵌套槽，连带传递：列表里父在子前面。
pub fn remove_nested_slots(live: &mut LiveState, task_id: Id) {
    let Some(nested) = live.nested_tools.as_mut() else {
        return;
    };
    let mut removed = vec![task_id];
    for slot in nested.iter() {
        if removed.contains(&slot.parent_task_id) {
            removed.push(slot.task_id);
        }
    }
    nested.retain(|slot| !removed.contains(&slot.parent_task_id));
    if nested.is_empty() {
        live.nested_tools = None;
    }
}

/// 删掉工具跑着时公布的东西；它的结果 entry（或重跑）会替换它。
pub fn clear_progress(progress: &mut SlotProgress) {
    progress.output = None;
    progress.dropped_bytes = None;
    progress.dropped_lines = None;
    progress.details = None;
    progress.diagnostics = None;
}

/// 什么时候 `pi.live` 算一份**完整 base**（规范 §8.2）：没有生成在跑，也没有跑着的工具槽。
///
/// 上游注释里那句 REMINDER 是判据不是注释：空闲时、把生成交给工具轮的那次提交、两次工具之间，
/// 都成立，所以 delta 链最多跨一次生成或一轮工具的重叠执行。槽只在跑着的时候带输出，所以每份
/// base 都很小；嵌套调用的结果住在调用方的任务文档里，不在这里。
///
/// **不要**给它加「delta 条数上限」：那是另一条规则，工具输出基准看的是这一条。
pub fn checkpoint_when(live: &LiveState) -> bool {
    live.generation.is_none()
        && !live
            .tools
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|slot| slot.progress.status == ToolStatus::Running)
        && !live
            .nested_tools
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|slot| slot.progress.status == ToolStatus::Running)
}
