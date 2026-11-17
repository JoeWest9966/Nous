//! 一条事件一条手写 JSON。**JSON 是权威**（`wire.md` 的线上形状），Rust 只负责对上它。
//!
//! 这里刻意不写 helper 去"造"事件：如果某个形状写不出来，说明类型层写错了，
//! 而不是测试写错了。

use nous_wire::{entry_kind, message_role, MessageChange, RemoteEvent};
use serde_json::{json, Value};

fn event(raw: &str) -> RemoteEvent {
    serde_json::from_str(raw).unwrap_or_else(|error| panic!("{raw}\n→ {error}"))
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

#[test]
fn text_start_carries_the_whole_first_block() {
    let raw = r#"{"type":"message_update","messageId":"m1","eventType":"text_start",
        "changes":[{"type":"text_start","contentIndex":0,"block":{"type":"text","text":"Hello there"}}]}"#;
    match event(raw) {
        RemoteEvent::MessageUpdate {
            changes,
            message_id,
            ..
        } => {
            assert_eq!(message_id.as_deref(), Some("m1"));
            assert_eq!(changes.len(), 1);
            match &changes[0] {
                MessageChange::TextStart { content_index, block } => {
                    assert_eq!(*content_index, 0);
                    // 跳过 text_start 等于吞掉第一段 —— 这里断言它带着全文。
                    assert_eq!(block["text"], "Hello there");
                }
                other => panic!("expected text_start, got {other:?}"),
            }
        }
        other => panic!("expected message_update, got {other:?}"),
    }
}

#[test]
fn text_delta_is_indexed_and_additive() {
    let raw = r#"{"type":"message_update","changes":[
        {"type":"text_delta","contentIndex":1,"delta":" world"},
        {"type":"thinking_delta","contentIndex":2,"delta":"musing"}]}"#;
    match event(raw) {
        RemoteEvent::MessageUpdate { changes, .. } => {
            assert_eq!(changes.len(), 2);
            assert_eq!(
                changes[0],
                MessageChange::TextDelta {
                    content_index: 1,
                    delta: " world".into()
                }
            );
            assert_eq!(
                changes[1],
                MessageChange::ThinkingDelta {
                    content_index: 2,
                    delta: "musing".into()
                }
            );
        }
        other => panic!("expected message_update, got {other:?}"),
    }
}

#[test]
fn toolcall_delta_carries_a_path_not_a_patch() {
    let raw = r#"{"type":"message_update","changes":[
        {"type":"toolcall_delta","contentIndex":3,"path":"arguments.path","delta":"a.txt"}]}"#;
    match event(raw) {
        RemoteEvent::MessageUpdate { changes, .. } => match &changes[0] {
            MessageChange::ToolcallDelta {
                content_index,
                path,
                delta,
            } => {
                assert_eq!(*content_index, 3);
                assert_eq!(path, "arguments.path");
                assert_eq!(delta, "a.txt");
            }
            other => panic!("expected toolcall_delta, got {other:?}"),
        },
        other => panic!("expected message_update, got {other:?}"),
    }
    // 缺 path 不炸（上游偶尔只给 delta）。
    let bare = r#"{"type":"message_update","changes":[{"type":"toolcall_delta","contentIndex":0,"delta":"{}"}]}"#;
    assert!(matches!(
        event(bare),
        RemoteEvent::MessageUpdate { changes, .. } if matches!(changes[0], MessageChange::ToolcallDelta { .. })
    ));
}

#[test]
fn block_and_message_are_whole_replacements() {
    let raw = r#"{"type":"message_update","changes":[
        {"type":"block","contentIndex":0,"block":{"type":"text","text":"superseded"}},
        {"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"full"}]}}]}"#;
    match event(raw) {
        RemoteEvent::MessageUpdate { changes, .. } => {
            assert!(matches!(
                &changes[0],
                MessageChange::Block { content_index: 0, block } if block["text"] == "superseded"
            ));
            assert!(matches!(
                &changes[1],
                MessageChange::Message { message } if message_role(message) == Some("assistant")
            ));
        }
        other => panic!("expected message_update, got {other:?}"),
    }
}

#[test]
fn message_end_identifies_the_entry_kind() {
    let raw = r#"{"type":"message_end","entry":{"id":"e1","kind":"pi.assistant","text":"done"}}"#;
    match event(raw) {
        RemoteEvent::MessageEnd { entry, .. } => {
            let entry = entry.expect("entry");
            assert_eq!(entry_kind(&entry), Some("pi.assistant"));
        }
        other => panic!("expected message_end, got {other:?}"),
    }
}

#[test]
fn submission_unanswered_carries_reason_and_detail() {
    let raw = r#"{"type":"submission","record":{"status":"unanswered","reason":"timeout","detail":"no provider"}}"#;
    match event(raw) {
        RemoteEvent::Submission { record } => {
            let record = record.expect("record");
            assert_eq!(record.status, "unanswered");
            assert_eq!(record.reason.as_deref(), Some("timeout"));
            assert_eq!(record.detail.as_deref(), Some("no provider"));
        }
        other => panic!("expected submission, got {other:?}"),
    }
}

#[test]
fn unknown_event_type_does_not_panic_and_keeps_the_stream() {
    for raw in [
        r#"{"type":"tool_execution_end","toolCallId":"c1","toolName":"read","isError":false,"result":{}}"#,
        r#"{"type":"agent_start"}"#,
        r#"{"type":"something_in_2030","payload":{"deep":true}}"#,
    ] {
        let parsed = event(raw);
        assert_eq!(parsed, RemoteEvent::Unknown, "{raw}");
        assert!(!parsed.is_known());
        assert_eq!(parsed.type_tag(), None);
    }
}

#[test]
fn unknown_change_type_does_not_panic() {
    let raw = r#"{"type":"message_update","changes":[{"type":"brand_new_change","contentIndex":0}]}"#;
    match event(raw) {
        RemoteEvent::MessageUpdate { changes, .. } => {
            assert_eq!(changes, vec![MessageChange::Unknown]);
            assert_eq!(changes[0].type_tag(), None);
        }
        other => panic!("expected message_update, got {other:?}"),
    }
}

#[test]
fn empty_changes_array_is_fine() {
    let parsed = event(r#"{"type":"message_update","changes":[]}"#);
    match &parsed {
        RemoteEvent::MessageUpdate { changes, .. } => assert!(changes.is_empty()),
        other => panic!("expected message_update, got {other:?}"),
    }
    // 而且回写出来还是空数组，不是 null。
    assert_eq!(to_value(&parsed)["changes"], json!([]));
    // changes 整个缺席也不炸。
    assert!(matches!(
        event(r#"{"type":"message_update"}"#),
        RemoteEvent::MessageUpdate { changes, .. } if changes.is_empty()
    ));
}

#[test]
fn snapshot_exposes_generation_and_keeps_entries_opaque() {
    let raw = r#"{"type":"snapshot","session":{"id":189,"name":""},
        "generation":{"id":"g1","message":{"role":"assistant","content":[]}},
        "entries":[{"id":"e1","kind":"pi.user","model":[{"timestamp":1700000000000}]}]}"#;
    match event(raw) {
        RemoteEvent::Snapshot {
            session,
            generation,
            entries,
        } => {
            assert_eq!(session.unwrap()["id"], 189);
            // generation.message 有值 = 有正在生成的消息，续上。
            let generation = generation.unwrap();
            assert_eq!(
                message_role(generation.message.as_ref().unwrap()),
                Some("assistant")
            );
            assert_eq!(entries.len(), 1);
            // entries 是唯一的时间源 —— 但整条 entry 留 Value，不替引擎冻结。
            assert_eq!(entries[0]["model"][0]["timestamp"], 1_700_000_000_000i64);
        }
        other => panic!("expected snapshot, got {other:?}"),
    }
    // 没在生成时的 snapshot：generation 缺席。
    assert!(matches!(
        event(r#"{"type":"snapshot","entries":[]}"#),
        RemoteEvent::Snapshot { generation: None, .. }
    ));
}

#[test]
fn tool_execution_events_name_the_call_and_pass_the_output_through() {
    match event(r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"read","args":{"path":"a"}}"#) {
        RemoteEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            assert_eq!(tool_call_id, "c1");
            assert_eq!(tool_name, "read");
            assert_eq!(args["path"], "a");
        }
        other => panic!("expected tool_execution_start, got {other:?}"),
    }
    // output 的具体外壳没钉死，两种都透传。
    for output in [json!({"append": "line\n"}), json!({"type":"set","content":"all"})] {
        let raw = json!({"type":"tool_execution_update","toolCallId":"c1","output":output});
        match serde_json::from_value(raw).unwrap() {
            RemoteEvent::ToolExecutionUpdate {
                tool_call_id,
                output,
                ..
            } => {
                assert_eq!(tool_call_id, "c1");
                assert!(output.get("append").is_some() || output.get("type").is_some());
            }
            other => panic!("expected tool_execution_update, got {other:?}"),
        }
    }
}

#[test]
fn entry_appended_and_task_failed() {
    match event(r#"{"type":"entry_appended","entry":{"id":"e9","kind":"pi.user","text":"hi"}}"#) {
        RemoteEvent::EntryAppended { entry } => assert_eq!(entry_kind(&entry), Some("pi.user")),
        other => panic!("expected entry_appended, got {other:?}"),
    }
    match event(r#"{"type":"task_failed","message":"boom","taskId":"t1"}"#) {
        RemoteEvent::TaskFailed { message, task_id } => {
            assert_eq!(message, "boom");
            assert_eq!(task_id.as_deref(), Some("t1"));
        }
        other => panic!("expected task_failed, got {other:?}"),
    }
}

#[test]
fn fields_go_out_camel_case_and_absent_options_stay_absent() {
    let event = RemoteEvent::ToolExecutionUpdate {
        tool_call_id: "c1".into(),
        tool_name: None,
        output: json!({"append": "x"}),
    };
    let value = to_value(&event);
    assert_eq!(value["type"], "tool_execution_update");
    assert_eq!(value["toolCallId"], "c1");
    assert!(
        value.get("toolName").is_none(),
        "Option 字段不吐 null：{value}"
    );
    assert!(value.get("tool_name").is_none(), "不许漏 snake_case");

    // 事件名也是宪法里的写法。
    assert_eq!(to_value(&RemoteEvent::Unknown)["type"], "unknown");
    assert_eq!(
        to_value(&MessageChange::ToolcallStart {
            content_index: 2,
            block: json!({"type":"toolCall"})
        })["type"],
        "toolcall_start"
    );
}

#[test]
fn unknown_fields_are_ignored_not_rejected() {
    // 引擎加字段是常态：多一个字段不该让我们反序列化失败。
    let raw = r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"read",
        "args":{},"startedAt":1700000000000,"nestedNewThing":{"a":1}}"#;
    assert!(matches!(
        event(raw),
        RemoteEvent::ToolExecutionStart { .. }
    ));
    // 也一路忽略到 change 里。
    let raw = r#"{"type":"message_update","futureField":1,
        "changes":[{"type":"text_delta","contentIndex":0,"delta":"x","lineage":"new"}]}"#;
    assert!(matches!(
        event(raw),
        RemoteEvent::MessageUpdate { changes, .. } if changes.len() == 1
    ));
}
