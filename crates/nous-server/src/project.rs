//! `CommitPublication` → `nous_wire::RemoteEvent`。SSE 的**唯一**入口。
//!
//! 语义（`docs/harness.md` §2，`#14` 与 `#22` 共用）：
//!
//! - **不能假设每条提交都有 `message_update`。** 只有 `Document::Change` + `kind == "pi.live"`
//!   + op 触及 `generation.message` 才产出 `MessageChange`；其余格 0 帧。
//! - **也不能用 `changes.is_empty()` 判「安静」。** `Entry` 变化照样产出 `message_end` /
//!   `entry_appended`，`Submission` 变化产出 `submission` —— 它们是另一类帧。
//! - `Conversation` / `Task` 不产帧：`wire.md` 没有会话元数据帧，任务状态是**不透明 JSON**
//!   （`storage.md` §12.5 明确不解释），从它猜 `task_failed` 会把「restart」误报成故障。
//!
//! 消息增量与工具卡片的投影都在 `nous-durable`：前者是 `harness::project::message_changes`，
//! 后者是 `harness::project::tool_changes`（它必须解 `ops`，而本 crate 的契约面不含
//! `nous_chord`）。这一层只做「类型 → 事件」的搬运，不重算语义。

use nous_durable::harness::project::{message_changes, tool_changes, ToolChange};
use nous_durable::session::publish::{CommitChange, CommitPublication, DocumentCommitChange};
use nous_durable::types::{EntryRecord, SubmissionRecord};
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
        // 任务：状态是不透明 JSON，不猜 `task_failed`（见模块文档）。
        CommitChange::Conversation(_) | CommitChange::Task(_) => Ok(Vec::new()),
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

/// `Entry` 是「定稿」：assistant 收尾气泡，其余是「别的标签页追加的 entry」。
fn entry_events(entry: &EntryRecord) -> Vec<RemoteEvent> {
    let value = match serde_json::to_value(entry) {
        Ok(value) => value,
        // 记录刚从库里读出来，序列化不该失败；真失败时宁可不发，也不发一个空 entry。
        Err(_) => return Vec::new(),
    };
    if entry.kind == "pi.assistant" {
        vec![RemoteEvent::MessageEnd {
            entry: Some(value),
            message: None,
            message_id: None,
        }]
    } else {
        vec![RemoteEvent::EntryAppended { entry: value }]
    }
}

/// `Submission`：只有 `unanswered` 要弹「没答上来」。
fn submission_events(submission: &SubmissionRecord) -> Vec<RemoteEvent> {
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

/// `detail` 在库里是 JSON（`storage.md`：原样存原样取），`wire.md` 的 `record.detail` 是
/// 给用户看的字符串。是字符串就用它本身，其余序列化成 JSON —— 不静默丢诊断载荷。
fn detail_text(detail: &Value) -> String {
    match detail {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
