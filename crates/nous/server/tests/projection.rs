//! `publication → 帧` 的判据：entry / submission 的分派，以及「工具启动 ≤ 1 个事件循环」。
//!
//! 最后一条是跨票接缝（`#14` 与工具执行层）：两边各自正确，接缝处仍可能「攒到工具结束
//! 才发」。所以判据看**时间线**（发布之后最多让一个事件循环），不是「最终收到了」。

mod common;

use common::{live_record, FakeSource};
use nous_durable::session::publish::{CommitChange, CommitPublication, DocumentCommitChange};
use nous_durable::types::{EntryRecord, SubmissionRecord};
use nous_server::{project, AppState};
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
    // 先 `message_start`（客户端靠它开气泡），再 `entry_appended`（给别的标签页）。
    assert_eq!(events.len(), 2, "实测 {events:?}");
    let RemoteEvent::MessageStart { message, .. } = &events[0] else {
        panic!("第一帧必须是 message_start，实测 {events:?}");
    };
    assert_eq!(message["role"], json!("user"));
    assert!(matches!(events[1], RemoteEvent::EntryAppended { .. }));
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

/// 「0 帧」不是注释：只改 `Conversation` / `Task` 的一次提交，SSE 上**一帧都没有**
/// （既没有 `message_update`，也没有别的）。`changes.is_empty()` 不能当「安静」的判据，
/// 反过来「有 changes 就发一帧」也不行 —— 这条钉住后者。
#[tokio::test]
async fn a_commit_that_only_touches_conversation_and_task_emits_no_frames_at_all() {
    let state = AppState::with_source(FakeSource::two_sessions());
    let mut subscriber = state.hub().subscribe(1);
    let task = json!({
        "id": 7,
        "conversationId": 1,
        "kind": "pi.generation",
        "version": 1,
        "input": {},
        "background": false,
        "abortRequested": false,
        "state": {"status": "running"}
    });
    let publication = publication(vec![
        CommitChange::Conversation(
            serde_json::from_value(json!({"id": 1})).expect("conversation 形状"),
        ),
        CommitChange::Task(serde_json::from_value(task).expect("task 形状")),
    ]);
    state
        .publish_publication(&publication)
        .expect("投影不该失败");
    tokio::task::yield_now().await;
    assert!(
        subscriber.try_recv().is_err(),
        "只改会话/任务的提交不许产出任何帧（包括 message_update）"
    );
}

#[tokio::test(start_paused = true)]
async fn a_tool_start_reaches_the_subscriber_within_one_event_loop() {
    let state = AppState::with_source(FakeSource::two_sessions());
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

    // 时间线：`pi.live` 那一拍（publication 到手）之后，**零虚拟时间**、只让一个事件循环 ——
    // 必须已经在订阅者手里。攒到工具结束的实现会在这里空手（`try_recv` 空），把提交挪进
    // `spawn` 且不醒的实现也会红。
    let before = tokio::time::Instant::now();
    tokio::task::yield_now().await;
    let event = subscriber.try_recv().expect("工具启动必须在一拍内到订阅者");
    assert_eq!(
        tokio::time::Instant::now(),
        before,
        "工具启动不许等任何定时器（等一个 tick 就是「攒到结束」的前兆）"
    );
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

#[test]
fn sse_frames_for_actor_stamped_entries_do_not_carry_actor() {
    let user = entry(json!({
        "id": 21,
        "conversationId": 1,
        "kind": "pi.user",
        "model": [{"role": "user", "timestamp": 1, "content": "hi"}],
        "actor": {"kind": "agent", "id": "test-agent"}
    }));
    let assistant = entry(json!({
        "id": 22,
        "conversationId": 1,
        "kind": "pi.assistant",
        "model": [{"role": "assistant", "timestamp": 2, "content": [{"type": "text", "text": "hi"}]}],
        "reason": "done",
        "actor": {"kind": "agent", "id": "test-agent"}
    }));
    let mut events = project::of_change(&user).expect("user 投影");
    events.extend(project::of_change(&assistant).expect("assistant 投影"));
    let tags: Vec<_> = events.iter().filter_map(|event| event.type_tag()).collect();
    assert_eq!(tags, ["message_start", "entry_appended", "message_end"]);
    for event in &events {
        let value = serde_json::to_value(event).expect("帧可序列化");
        let text = value.to_string();
        assert!(!text.contains("\"actor\""), "SSE 帧不许带 actor：{value}");
    }
}
