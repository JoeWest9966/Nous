//! `pi.live` 类型面的判据：**序列化后的字段名逐字照上游**（互通只靠字段名，不靠类型）。
//!
//! 上游形状见 `packages/durable/src/harness/live.ts`（`LiveState` / `generation` / `ToolSlot`）。
//! 这里只判本库今天写的那些字段；缺的字段（`run` / `nestedTools` / `compactions` /
//! `compactionFailure` / `runFailure`、`ToolSlot.diagnostics`）随写它们的功能一起落，见
//! `crates/nous-durable/src/harness/live.rs` 模块文档。

use nous_durable::harness::live::{Deferred, LiveGeneration, LiveState, Retry, ToolSlot, ToolStatus};
use serde_json::json;

/// 一个「什么都在」的值：每个可选的键都出现，才能逐个比对字段名。
fn everything() -> LiveState {
    LiveState {
        generation: Some(LiveGeneration {
            attempt: 2,
            message: Some(json!({"role": "assistant", "content": []})),
            retry: Some(Retry {
                at: 1_700_000_000_000,
                error: "rate limited".to_string(),
            }),
            deferred: Some(Deferred {
                poll_at: 1_700_000_001_000,
            }),
        }),
        tools: Some(vec![ToolSlot {
            call_id: "call_1".to_string(),
            name: "read".to_string(),
            status: ToolStatus::Running,
            output: Some("line\n".to_string()),
            dropped_bytes: Some(12),
            dropped_lines: Some(1),
            details: Some(json!({"path": "/tmp/a"})),
            task_id: Some(7),
            unoffered: true,
            entry: Some(9),
        }]),
    }
}

/// 每个字段的键名与上游一致（`camelCase`，`Option` 为 `None` 时整个键不出现）。
#[test]
fn live_state_serializes_the_field_names_upstream_uses() {
    let value = serde_json::to_value(everything()).expect("能序列化");
    assert_eq!(
        value,
        json!({
            "generation": {
                "attempt": 2,
                "message": {"role": "assistant", "content": []},
                "retry": {"at": 1_700_000_000_000_i64, "error": "rate limited"},
                "deferred": {"pollAt": 1_700_000_001_000_i64},
            },
            "tools": [{
                "callId": "call_1",
                "name": "read",
                "status": "running",
                "output": "line\n",
                "droppedBytes": 12,
                "droppedLines": 1,
                "details": {"path": "/tmp/a"},
                "taskId": 7,
                "unoffered": true,
                "entry": 9,
            }],
        })
    );
}

/// 空的 `pi.live` 是 `{}`：`initial: () => ({})`，没有一个键。
#[test]
fn an_empty_live_state_serializes_to_an_empty_object() {
    assert_eq!(serde_json::to_value(LiveState::default()).unwrap(), json!({}));
}

/// `unoffered` 上游是字面量 `true`：假的时候**整个键不出现**，不许写成 `false`。
#[test]
fn unoffered_is_absent_rather_than_false() {
    let slot = ToolSlot {
        call_id: "c".to_string(),
        name: "n".to_string(),
        status: ToolStatus::Pending,
        output: None,
        dropped_bytes: None,
        dropped_lines: None,
        details: None,
        task_id: None,
        unoffered: false,
        entry: None,
    };
    assert_eq!(
        serde_json::to_value(slot).unwrap(),
        json!({"callId": "c", "name": "n", "status": "pending"})
    );
}

/// 反序列化也照同样的键名回来：写出去再读回来不许变。
#[test]
fn a_live_state_round_trips() {
    let value = everything();
    let encoded = serde_json::to_value(&value).unwrap();
    let decoded: LiveState = serde_json::from_value(encoded).expect("能反序列化");
    assert_eq!(decoded, value);
}

/// 三种状态拼写与上游一致。
#[test]
fn tool_status_spellings_match_upstream() {
    for (status, spelling) in [
        (ToolStatus::Pending, "pending"),
        (ToolStatus::Running, "running"),
        (ToolStatus::Done, "done"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), json!(spelling));
    }
}
