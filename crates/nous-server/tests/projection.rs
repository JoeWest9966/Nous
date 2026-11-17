//! `publication → 帧` 的判据：entry / submission 的分派，以及「工具启动 ≤ 1 个事件循环」。
//!
//! 最后一条是跨票接缝（`#14` 与工具执行层）：两边各自正确，接缝处仍可能「攒到工具结束
//! 才发」。所以判据看**时间线**（发布之后最多让一个事件循环），不是「最终收到了」。

mod common;

use common::{live_record, FakeSource};
use nous_durable::session::publish::{CommitChange, CommitPublication, DocumentCommitChange};
use nous_durable::types::{EntryRecord, SubmissionRecord};
use nous_server::{project, AppState, Hub};
use nous_wire::RemoteEvent;
use serde_json::{json, Value};

fn entry(value: Value) -> CommitChange {
    CommitChange::Entry(serde_json::from_value::<EntryRecord>(value).expect("entry 形状"))
}

fn submission(value: Value) -> CommitChange {
    CommitChange::Submission(serde_json::from_value::<SubmissionRecord>(value).expect("submission 形状"))
}

fn publication(changes: Vec<CommitChange>) -> CommitPublication {
    CommitPublication::new(1, changes)
}

#[test]
fn an_assistant_entry_ends_the_bubble_and_other_entries_are_appended() {
    let assistant = entry(json!({
        "id": 12,
        "conversationId": 1,
        "kind": "pi.assistant",
        "model": [{"role": "assistant", "timestamp": 1, "content": [{"type": "text", "text": "hi"}]}]
    }));
    let events = project::of_change(&assistant).expect("投影");
    assert_eq!(events.len(), 1);
    let RemoteEvent::MessageEnd { entry: ended, .. } = &events[0] else {
        panic!("assistant entry 必须发 message_end，实测 {events:?}");
    };
    assert_eq!(ended.as_ref().unwrap()["kind"], json!("pi.assistant"));

    let user = entry(json!({
        "id": 11,
        "conversationId": 1,
        "kind": "pi.user",
        "model": [{"role": "user", "timestamp": 1, "content": "hi"}]
    }));
    let events = project::of_change(&user).expect("投影");
    assert!(matches!(events.as_slice(), [RemoteEvent::EntryAppended { .. }]));
}

#[test]
fn only_an_unanswered_submission_becomes_a_frame() {
    let unanswered = submission(json!({
        "id": 5,
        "conversationId": 1,
        "type": "input",
        "status": "unanswered",
        "reason": "model_error",
        "detail": {"provider": "x"}
    }));
    let events = project::of_change(&unanswered).expect("投影");
    let RemoteEvent::Submission { record: Some(record) } = &events[0] else {
        panic!("unanswered 必须发 submission，实测 {events:?}");
    };
    assert_eq!(record.status, "unanswered");
    assert_eq!(record.reason.as_deref(), Some("model_error"));
    // `detail` 在库里是 JSON，线上是字符串：不静默丢诊断载荷。
    assert_eq!(record.detail.as_deref(), Some("{\"provider\":\"x\"}"));

    let done = submission(json!({
        "id": 6,
        "conversationId": 1,
        "type": "input",
        "status": "done",
        "entry": 11,
        "answer": 12
    }));
    assert!(project::of_change(&done).expect("投影").is_empty());
}

#[test]
fn conversations_and_tasks_produce_no_frames() {
    // 任务状态是不透明 JSON（`storage.md` §12.5）：不猜 `task_failed`。
    let task = json!({
        "id": 7,
        "conversationId": 1,
        "kind": "pi.generation",
        "version": 1,
        "input": {},
        "background": false,
        "abortRequested": false,
        "state": {"status": "faulted", "error": "boom"}
    });
    let change = CommitChange::Task(serde_json::from_value(task).expect("task 形状"));
    assert!(project::of_change(&change).expect("投影").is_empty());
}

#[tokio::test]
async fn a_tool_start_reaches_the_subscriber_within_one_event_loop() {
    let state = AppState::new(FakeSource::two_sessions(), Hub::new());
    let mut subscriber = state.hub().subscribe(1);

    // 一次 `pi.live` 提交：工具槽 `tools[0]` 出现（启动那一拍）。`ops` 是线上形状，
    // 不是测试专用的旁路 —— `publish_publication` 就是 `CommitObserver::advance` 的路径。
    let slot = json!({"callId": "call-1", "name": "bash", "status": "running"});
    let change = CommitChange::Document(DocumentCommitChange::Change {
        record: live_record(100, 1),
        conversation_id: Some(1),
        version: 3,
        value: json!({"tools": [slot]}),
        ops: json!([["s", ["tools", 0], slot]]),
    });
    state
        .publish_publication(&publication(vec![change]))
        .expect("投影");

    // 允许、且只允许一个事件循环。攒到工具结束的实现会在这里超时（`try_recv` 空）。
    tokio::task::yield_now().await;
    let event = subscriber.try_recv().expect("工具启动必须在一拍内到订阅者");
    let RemoteEvent::ToolExecutionStart {
        tool_call_id,
        tool_name,
        ..
    } = event
    else {
        panic!("第一帧该是 tool_execution_start，实测 {event:?}");
    };
    assert_eq!(tool_call_id, "call-1");
    assert_eq!(tool_name, "bash");
}
