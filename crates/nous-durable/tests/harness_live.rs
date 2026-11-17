//! `pi.live` 的判据：**序列化后的字段名逐字照上游**（互通只靠字段名，不靠类型）+ 九个 draft
//! 变更函数与 `checkpoint_when` 的行为。
//!
//! 上游形状见 `packages/durable/src/harness/live.ts`。字段名那组测试的价值在于：上游是 TS 交叉
//! 类型，本库用 `flatten` 展开共用那截 —— 一旦 `flatten` 的顺序或 `rename` 写错，键名就漂了。

use nous_durable::harness::live::{
    add_compaction_status, checkpoint_when, compaction_status, end_run,
    fail_compaction_status, finish_slot, remove_compaction_status, remove_nested_slots, tool_slot,
    AfterRun, CompactionFailure, CompactionReason, CompactionStatus, Deferred, DiagnosticSeverity,
    LiveGeneration, LiveRun, LiveState, NestedToolSlot, NestedToolSummary, Retry, RunEnd, RunFailure,
    RunFailureReason, SlotProgress, ToolDiagnostic, ToolSlot, ToolStatus,
};
use nous_durable::types::SubmissionSettlement;
use serde_json::json;

/// 共用那截：每个可选字段都填上。
fn progress(call_id: &str, name: &str, status: ToolStatus) -> SlotProgress {
    SlotProgress {
        call_id: call_id.to_string(),
        name: name.to_string(),
        status,
        output: Some("line\n".to_string()),
        dropped_bytes: Some(12),
        dropped_lines: Some(1),
        details: Some(json!({"path": "/tmp/a"})),
        diagnostics: Some(vec![ToolDiagnostic {
            severity: DiagnosticSeverity::Warn,
            message: "slow".to_string(),
            code: Some("slow_tool".to_string()),
        }]),
    }
}

/// 一个「什么都在」的值：每个可选的键都出现，才能逐个比对字段名。
fn everything() -> LiveState {
    LiveState {
        run: Some(LiveRun {
            task_id: 3,
            inputs: vec![11, 12],
            turns: 2,
            first: Some(10),
            after_run: Some(AfterRun {
                tasks: Some(vec![13]),
            }),
        }),
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
            progress: progress("call_1", "read", ToolStatus::Running),
            task_id: Some(7),
            unoffered: true,
            entry: Some(9),
        }]),
        nested_tools: Some(vec![NestedToolSlot {
            progress: progress("call_1/key", "grep", ToolStatus::Running),
            parent_call_id: "call_1".to_string(),
            parent_task_id: 7,
            task_id: 8,
            arguments: json!({"pattern": "x"}).as_object().unwrap().clone(),
            summary: Some(NestedToolSummary {
                is_error: false,
                duration_ms: Some(42),
                usage: Some(json!({"input": 1, "output": 2})),
                error: None,
            }),
        }]),
        compactions: Some(vec![CompactionStatus {
            task_id: 14,
            trigger: CompactionReason::Threshold,
            blocking: true,
            attempt: 1,
            retry: Some(Retry {
                at: 1_700_000_002_000,
                error: "too long".to_string(),
            }),
        }]),
        compaction_failure: Some(CompactionFailure {
            task_id: 15,
            trigger: CompactionReason::Overflow,
            error: "context too long".to_string(),
        }),
        run_failure: Some(RunFailure {
            task_id: 16,
            inputs: vec![17],
            reason: RunFailureReason::ModelError,
            detail: Some("502".to_string()),
            first: Some(18),
        }),
    }
}

/// 每个字段的键名与上游一致（`camelCase`，`Option` 为 `None` 时整个键不出现）。
#[test]
fn live_state_serializes_the_field_names_upstream_uses() {
    let value = serde_json::to_value(everything()).expect("能序列化");
    assert_eq!(
        value,
        json!({
            "run": {
                "taskId": 3,
                "inputs": [11, 12],
                "turns": 2,
                "first": 10,
                "afterRun": {"tasks": [13]},
            },
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
                "diagnostics": [{"severity": "warn", "message": "slow", "code": "slow_tool"}],
                "taskId": 7,
                "unoffered": true,
                "entry": 9,
            }],
            "nestedTools": [{
                "callId": "call_1/key",
                "name": "grep",
                "status": "running",
                "output": "line\n",
                "droppedBytes": 12,
                "droppedLines": 1,
                "details": {"path": "/tmp/a"},
                "diagnostics": [{"severity": "warn", "message": "slow", "code": "slow_tool"}],
                "parentCallId": "call_1",
                "parentTaskId": 7,
                "taskId": 8,
                "arguments": {"pattern": "x"},
                "summary": {"isError": false, "durationMs": 42, "usage": {"input": 1, "output": 2}},
            }],
            "compactions": [{
                "taskId": 14,
                "trigger": "threshold",
                "blocking": true,
                "attempt": 1,
                "retry": {"at": 1_700_000_002_000_i64, "error": "too long"},
            }],
            "compactionFailure": {"taskId": 15, "trigger": "overflow", "error": "context too long"},
            "runFailure": {"taskId": 16, "inputs": [17], "reason": "model_error", "detail": "502", "first": 18},
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
        progress: SlotProgress {
            call_id: "c".to_string(),
            name: "n".to_string(),
            status: ToolStatus::Pending,
            output: None,
            dropped_bytes: None,
            dropped_lines: None,
            details: None,
            diagnostics: None,
        },
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

/// 十种「为什么没答上」全抄上游，一个不许改名（少一个，客户端就认不出那个原因）。
#[test]
fn run_failure_reason_spellings_match_upstream() {
    for (reason, spelling) in [
        (RunFailureReason::ModelError, "model_error"),
        (RunFailureReason::NoModel, "no_model"),
        (RunFailureReason::Aborted, "aborted"),
        (RunFailureReason::Reset, "reset"),
        (RunFailureReason::Faulted, "faulted"),
        (RunFailureReason::Orphaned, "orphaned"),
        (RunFailureReason::Interrupted, "interrupted"),
        (RunFailureReason::RestartLimit, "restart_limit"),
        (RunFailureReason::NothingToAnswer, "nothing_to_answer"),
        (RunFailureReason::TurnLimit, "turn_limit"),
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), json!(spelling));
        // 结算里那个 `reason` 字段也是同一个字符串，两处不许漂。
        assert_eq!(reason.as_str(), spelling);
    }
}

/// 压缩的三个触发原因、诊断的三档严重程度，同样逐字照上游。
#[test]
fn compaction_and_diagnostic_spellings_match_upstream() {
    for (reason, spelling) in [
        (CompactionReason::Manual, "manual"),
        (CompactionReason::Threshold, "threshold"),
        (CompactionReason::Overflow, "overflow"),
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), json!(spelling));
    }
    for (severity, spelling) in [
        (DiagnosticSeverity::Info, "info"),
        (DiagnosticSeverity::Warn, "warn"),
        (DiagnosticSeverity::Error, "error"),
    ] {
        assert_eq!(serde_json::to_value(severity).unwrap(), json!(spelling));
    }
}

/// 一个跑着的模型发起的槽。
fn model_slot(call_id: &str, task_id: Option<i64>, status: ToolStatus) -> ToolSlot {
    ToolSlot {
        progress: progress(call_id, "read", status),
        task_id,
        unoffered: false,
        entry: None,
    }
}

/// 一个跑着的嵌套槽。
fn nested_slot(task_id: i64, parent_task_id: i64) -> NestedToolSlot {
    NestedToolSlot {
        progress: progress("call_1/key", "grep", ToolStatus::Running),
        parent_call_id: "call_1".to_string(),
        parent_task_id,
        task_id,
        arguments: json!({}).as_object().unwrap().clone(),
        summary: None,
    }
}

/// `checkpoint_when`：**恰恰在没什么在跑的时候**才是完整 base。
///
/// 这条是 delta 链长度的唯一规则（上游注释里的 REMINDER），所以两边的假都必须能红：
/// 有生成在跑 → 不是 base；有工具槽跑着 → 不是 base。
#[test]
fn checkpoint_when_holds_exactly_when_nothing_runs() {
    // 空闲：是。
    assert!(checkpoint_when(&LiveState::default()));

    // 有生成在跑：不是。
    let mut live = LiveState::default();
    live.generation = Some(LiveGeneration {
        attempt: 1,
        ..LiveGeneration::default()
    });
    assert!(!checkpoint_when(&live));

    // 工具跑着：不是。
    live.generation = None;
    live.tools = Some(vec![model_slot("c1", Some(7), ToolStatus::Running)]);
    assert!(!checkpoint_when(&live));

    // 排着队或做完了：是（槽只在跑着的时候带输出）。
    live.tools = Some(vec![
        model_slot("c1", Some(7), ToolStatus::Done),
        model_slot("c2", None, ToolStatus::Pending),
    ]);
    assert!(checkpoint_when(&live));

    // 嵌套调用跑着：不是。
    live.nested_tools = Some(vec![nested_slot(8, 7)]);
    assert!(!checkpoint_when(&live));
}

/// 结掉这一轮：输入的结算按 `end` 走、`run` 与这一轮的呈现全删。
#[test]
fn end_run_settles_the_inputs_and_clears_the_round() {
    let mut live = LiveState::default();
    live.run = Some(LiveRun {
        task_id: 3,
        inputs: vec![11, 12],
        turns: 1,
        first: Some(10),
        after_run: None,
    });
    live.generation = Some(LiveGeneration::default());
    live.tools = Some(vec![model_slot("c1", Some(7), ToolStatus::Done)]);
    live.nested_tools = Some(vec![nested_slot(8, 7)]);

    let settlements = end_run(&mut live, 3, &RunEnd::Done { answer: 21 });

    assert_eq!(
        settlements,
        vec![
            (11, SubmissionSettlement::Done { answer: 21 }),
            (12, SubmissionSettlement::Done { answer: 21 }),
        ]
    );
    assert_eq!(live, LiveState::default(), "这一轮的呈现必须全删");
    assert_eq!(live.run_failure, None, "答上了就不记失败");
}

/// 没答上：结算成 `unanswered`（带原因与说明），并记下 `runFailure`。
///
/// 还有一层上游语义：**答完之后只跑 `afterRun` 任务的那一轮，不管怎么结束都不记失败** —— 它
/// 的输入早就结算过了，`run` 里只剩 `afterRun`。
#[test]
fn end_run_records_a_failure_only_when_no_answer_arrived() {
    let mut live = LiveState::default();
    live.run = Some(LiveRun {
        task_id: 3,
        inputs: vec![11],
        turns: 2,
        first: Some(10),
        after_run: None,
    });

    let settlements = end_run(
        &mut live,
        3,
        &RunEnd::Unanswered {
            reason: RunFailureReason::Interrupted,
            detail: Some("cut off 3 times".to_string()),
            settlement: None,
        },
    );

    assert_eq!(
        settlements,
        vec![(
            11,
            SubmissionSettlement::Unanswered {
                reason: "interrupted".to_string(),
                detail: Some(json!("cut off 3 times")),
            }
        )]
    );
    assert_eq!(
        live.run_failure,
        Some(RunFailure {
            task_id: 3,
            inputs: vec![11],
            reason: RunFailureReason::Interrupted,
            detail: Some("cut off 3 times".to_string()),
            first: Some(10),
        })
    );
    assert!(live.run.is_none());

    // 答完只跑 `afterRun` 的那一轮：不结算、不记失败，但 `run` 照样删。
    let mut answered = LiveState::default();
    answered.run = Some(LiveRun {
        task_id: 4,
        inputs: vec![],
        turns: 1,
        first: Some(10),
        after_run: Some(AfterRun {
            tasks: Some(vec![13]),
        }),
    });
    let settlements = end_run(
        &mut answered,
        4,
        &RunEnd::Unanswered {
            reason: RunFailureReason::Faulted,
            detail: None,
            settlement: None,
        },
    );
    assert!(settlements.is_empty());
    assert_eq!(answered.run_failure, None);
    assert!(answered.run.is_none());
}

/// 调用的不是这一轮的任务：`run` 不动（上游那个 `if` 的判断），但这一轮的呈现照样删 ——
/// 上游 `delete generation/tools/nestedTools` 写在这个 `if` **外面**。
#[test]
fn end_run_of_another_task_leaves_the_run_and_still_clears_the_round() {
    let mut live = LiveState::default();
    live.run = Some(LiveRun {
        task_id: 3,
        inputs: vec![11],
        turns: 0,
        first: None,
        after_run: None,
    });
    live.generation = Some(LiveGeneration::default());

    let settlements = end_run(&mut live, 99, &RunEnd::Done { answer: 21 });

    assert!(settlements.is_empty());
    assert_eq!(live.run.map(|run| run.task_id), Some(3));
    assert!(live.generation.is_none());
}

/// `tool_slot` 两种槽都找得到（嵌套是二分），找不到就是 `None`。
#[test]
fn tool_slot_finds_model_and_nested_slots() {
    let mut live = LiveState::default();
    live.tools = Some(vec![model_slot("c1", Some(7), ToolStatus::Running)]);
    live.nested_tools = Some(vec![nested_slot(20, 7), nested_slot(21, 7), nested_slot(30, 21)]);

    assert_eq!(tool_slot(&mut live, 7).and_then(|slot| slot.task_id()), Some(7));
    assert!(matches!(
        tool_slot(&mut live, 20),
        Some(nous_durable::harness::live::SlotRef::Nested(_))
    ));
    assert_eq!(tool_slot(&mut live, 30).and_then(|slot| slot.task_id()), Some(30));
    assert!(tool_slot(&mut live, 999).is_none());

    // 写进去的东西要落在对应的槽上。
    let mut slot = tool_slot(&mut live, 21).expect("嵌套槽在");
    slot.progress_mut().output = Some("tail\n".to_string());
    assert_eq!(
        live.nested_tools.as_ref().unwrap()[1].progress.output.as_deref(),
        Some("tail\n")
    );
}

/// `finish_slot`：标成 done 并清掉跑着时公布的东西，结果字段由调用方设。
#[test]
fn finish_slot_marks_done_and_clears_running_output() {
    let mut live = LiveState::default();
    live.tools = Some(vec![model_slot("c1", Some(7), ToolStatus::Running)]);

    finish_slot(tool_slot(&mut live, 7).expect("槽在"));

    let slot = &live.tools.as_ref().unwrap()[0];
    assert_eq!(slot.progress.status, ToolStatus::Done);
    clear_assertions(&slot.progress);
    assert_eq!(slot.entry, None, "结果 entry 是调用方的事");
}

/// `clear_progress` 只删跑着时公布的那五样。
fn clear_assertions(progress: &SlotProgress) {
    assert_eq!(progress.output, None);
    assert_eq!(progress.dropped_bytes, None);
    assert_eq!(progress.dropped_lines, None);
    assert_eq!(progress.details, None);
    assert_eq!(progress.diagnostics, None);
    // 身份字段不许被清。
    assert_eq!(progress.call_id, "c1");
    assert_eq!(progress.name, "read");
}

/// 压缩状态：按任务 id 排、删空了就把列表删掉、失败时先记 `compactionFailure` 再删状态。
#[test]
fn compaction_statuses_sort_remove_and_record_failure() {
    let mut live = LiveState::default();
    for task_id in [30, 10, 20] {
        add_compaction_status(
            &mut live,
            CompactionStatus {
                task_id,
                trigger: CompactionReason::Manual,
                blocking: false,
                attempt: 1,
                retry: None,
            },
        );
    }
    let order: Vec<i64> = live
        .compactions
        .as_ref()
        .unwrap()
        .iter()
        .map(|status| status.task_id)
        .collect();
    assert_eq!(order, vec![10, 20, 30]);
    assert_eq!(compaction_status(&live, 20).map(|s| s.task_id), Some(20));

    fail_compaction_status(&mut live, 20, "summarizer failed".to_string());
    assert_eq!(
        live.compaction_failure,
        Some(CompactionFailure {
            task_id: 20,
            trigger: CompactionReason::Manual,
            error: "summarizer failed".to_string(),
        })
    );
    assert!(compaction_status(&live, 20).is_none());
    assert_eq!(live.compactions.as_ref().unwrap().len(), 2);

    remove_compaction_status(&mut live, 10);
    remove_compaction_status(&mut live, 30);
    assert_eq!(live.compactions, None, "空了就把列表删掉");
    // 已经不存在的任务：什么都不做（不许 panic，也不许凭空造失败记录）。
    fail_compaction_status(&mut live, 404, "nope".to_string());
    assert_eq!(
        live.compaction_failure.map(|failure| failure.task_id),
        Some(20),
        "找不到状态时不许覆盖上一条失败"
    );
}

/// 删父的嵌套槽要连带子（列表里父在子前面）。
#[test]
fn remove_nested_slots_takes_the_children_with_the_parent() {
    let mut live = LiveState::default();
    live.nested_tools = Some(vec![
        nested_slot(20, 7),
        nested_slot(21, 20),
        nested_slot(22, 21),
        nested_slot(30, 7),
    ]);

    remove_nested_slots(&mut live, 20);

    // 20 自己是**任务**的槽，不是它自己的子（上游删的是「父是 taskId」的那些），所以它留下，
    // 21 与 22（21 的子）跟着走。
    let remaining: Vec<i64> = live
        .nested_tools
        .as_ref()
        .unwrap()
        .iter()
        .map(|slot| slot.task_id)
        .collect();
    assert_eq!(remaining, vec![20, 30]);

    // 7 是这一轮的工具任务，20 与 30 都是它发的 ⇒ 一起走，列表空了就删掉。
    remove_nested_slots(&mut live, 7);
    assert_eq!(live.nested_tools, None);
}
