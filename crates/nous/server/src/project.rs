//! `CommitPublication` → `nous_wire::RemoteEvent`。SSE 的**唯一**入口。
//!
//! 语义（`docs/harness.md` §2，`#14` 与 `#22` 共用）：
//!
//! - **不能假设每条提交都有 `message_update`。** 只有 `Document::Change` + `kind == "pi.live"`
//!   + op 触及 `generation.message` 才产出 `MessageChange`；其余格 0 帧。
//! - **也不能用 `changes.is_empty()` 判「安静」。** `Entry` 变化照样产出 `message_end` /
//!   `entry_appended`，`Submission` 变化产出 `submission` —— 它们是另一类帧。
//! - `Conversation` 不产帧：`wire.md` 没有会话元数据帧。
//! - `Task` 只产一种帧：**终态且 outcome.failed** 才发 `task_failed`。中途状态与
//!   `completed` / `restart` 都不发（不解释状态机本身，只认终态里的失败归因）。
//!
//! 消息增量与工具卡片的投影都在 `nous-durable`：前者是 `harness::project::message_changes`，
//! 后者是 `harness::project::tool_changes`（它必须解 `ops`，而本 crate 的契约面不含
//! `nous_chord`）。这一层只做「类型 → 事件」的搬运，不重算语义。

use nous_durable::harness::project::{message_changes, tool_changes, ToolChange};
use nous_durable::session::publish::{CommitChange, CommitPublication, DocumentCommitChange};
use nous_durable::types::{EntryRecord, SubmissionRecord, TaskRecord};
use nous_wire::{RemoteEvent, SubmissionRecord as WireSubmission};
use serde_json::{json, Value};

use crate::error::{Result, ServerError};

/// 一次提交 → 帧，按 publication 里的顺序展开。
pub fn events(publication: &CommitPublication) -> Result<Vec<RemoteEvent>> {
    let mut events = Vec::new();
    for change in &publication.changes {
        events.extend(of_change(change)?);
    }
    Ok(events)
}

/// 一条改动 → 帧。0..N 条。
pub fn of_change(change: &CommitChange) -> Result<Vec<RemoteEvent>> {
    match change {
        CommitChange::Document(document) => document_events(document),
        CommitChange::Entry(entry) => Ok(entry_events(entry)),
        CommitChange::Submission(submission) => Ok(submission_events(submission)),
        // 会话元数据：`wire.md` 没有帧。`GET /v1/sessions` 是快照，不是事件。
        CommitChange::Conversation(_) => Ok(Vec::new()),
        CommitChange::Task(task) => Ok(task_events(task)),
    }
}

fn document_events(document: &DocumentCommitChange) -> Result<Vec<RemoteEvent>> {
    let DocumentCommitChange::Change { .. } = document else {
        // `Retire` / `Copy`：`wire.md` 没有对应帧。硬发一条空消息会让气泡凭空清空。
        return Ok(Vec::new());
    };

    let mut events = Vec::new();
    let changes = message_changes(&CommitChange::Document(document.clone()))
        .map_err(|error| ServerError::Internal(format!("message projection: {error}")))?;
    if !changes.is_empty() {
        events.push(RemoteEvent::MessageUpdate {
            changes,
            message_id: None,
            message: None,
        });
    }
    let tools = tool_changes(&CommitChange::Document(document.clone()))
        .map_err(|error| ServerError::Internal(format!("tool projection: {error}")))?;
    for tool in tools {
        events.push(match tool {
            ToolChange::Start {
                tool_call_id,
                tool_name,
            } => RemoteEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                // 卡片先开出来；`wire.md` 只依赖 `toolCallId` / `toolName`。参数留在 assistant
                // entry 里（工具槽没有参数），这里不编一份。
                args: json!({}),
            },
            ToolChange::Update {
                tool_call_id,
                append,
                set,
            } => RemoteEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name: None,
                output: match (append, set) {
                    (Some(text), _) => json!({ "append": text }),
                    (None, Some(text)) => json!({ "set": text }),
                    (None, None) => Value::Null,
                },
            },
        });
    }
    Ok(events)
}

/// `Entry` 是「定稿」：assistant 收尾气泡；user 先开一条 `message_start` 再报
/// `entry_appended`；其余只报 `entry_appended`。
///
/// 为什么 user 要两条：`wire.md` 明说 `message_start` **user / system 也会发**，而
/// `POST /messages` 之后客户端要靠第一条 `message_start` 开气泡（issue #22 的判据）。
/// 同一个 entry 再发 `entry_appended` 是给**别的标签页**的（它们没有本次提交的上下文），
/// 客户端按 `id` 去重。顺序必须是 start 在前 —— 反过来就等于没开气泡先收到内容。
pub fn entry_events(entry: &EntryRecord) -> Vec<RemoteEvent> {
    let value = match serde_json::to_value(entry) {
        Ok(value) => value,
        // 记录刚从库里读出来，序列化不该失败；真失败时宁可不发，也不发一个空 entry。
        Err(_) => return Vec::new(),
    };
    match entry.kind.as_str() {
        "pi.assistant" => vec![RemoteEvent::MessageEnd {
            entry: Some(value),
            message: None,
            message_id: None,
        }],
        "pi.user" => {
            let message = entry
                .model
                .as_ref()
                .and_then(|messages| messages.last())
                .cloned()
                .unwrap_or_else(|| json!({ "role": "user" }));
            vec![
                RemoteEvent::MessageStart {
                    message,
                    message_id: Some(entry.id.to_string()),
                },
                RemoteEvent::EntryAppended { entry: value },
            ]
        }
        _ => vec![RemoteEvent::EntryAppended { entry: value }],
    }
}

/// `Submission`：只有 `unanswered` 要弹「没答上来」。
pub fn submission_events(submission: &SubmissionRecord) -> Vec<RemoteEvent> {
    if !matches!(
        submission.fields.status,
        nous_durable::types::SubmissionStatus::Unanswered
    ) {
        return Vec::new();
    }
    vec![RemoteEvent::Submission {
        record: Some(WireSubmission {
            status: "unanswered".to_string(),
            reason: submission.fields.reason.clone(),
            detail: submission.fields.detail.as_ref().map(detail_text),
        }),
    }]
}

/// 失败终态 → `task_failed`。`TaskRecord.state` 本身是不透明 JSON，这里只认一个形状：
/// `status == "terminal"` + `outcome.status == "failed"`。其他状态（含 `restart` 中止）都不发 ——
/// 猜中途状态会把一次正常重启报成故障。
pub fn task_events(task: &TaskRecord) -> Vec<RemoteEvent> {
    const TERMINAL: &str = "terminal";
    const FAILED: &str = "failed";
    if task.state.get("status").and_then(Value::as_str) != Some(TERMINAL)
        || task
            .state
            .get("outcome")
            .and_then(|outcome| outcome.get("status"))
            .and_then(Value::as_str)
            != Some(FAILED)
    {
        return Vec::new();
    }
    let outcome = &task.state["outcome"];
    let reason = outcome
        .get("error")
        .and_then(|error| {
            error
                .as_str()
                .map(str::to_string)
                .or_else(|| error.get("message").and_then(Value::as_str).map(str::to_string))
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "任务失败".to_string());
    vec![RemoteEvent::TaskFailed {
        message: format!("任务 {} 失败：{reason}", task.id),
        task_id: Some(task.id.to_string()),
    }]
}

/// `detail` 在库里是 JSON（`storage.md`：原样存原样取），`wire.md` 的 `record.detail` 是
/// 给用户看的字符串。是字符串就用它本身，其余序列化成 JSON —— 不静默丢诊断载荷。
fn detail_text(detail: &Value) -> String {
    match detail {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
