//! `PartialChanges` 的判据。**核心三条是体积判据，不是值判据** —— 值在这条路径上永远相等，
//! 断言它等于什么都没说。
//!
//! 1. 连续 k 次 partial 提交，第 k 次的 op 只有一条 `Append`，且 op 的序列化体积里没有整段正文；
//! 2. `len(ops)` 不随正文变长而增长；
//! 3. 负向对照：把「逐叶写」换成整块 `set`，同一组断言必须红 —— 否则前两条测的只是「反正只有
//!    一条 op」这种空转。
//!
//! 另有三条语义判据：块的 `*_end` 事件丢掉增量、工具参数按改动逐条写、`restore` 把失败的提交
//! 放回去。

use std::sync::Arc;

use nous_ai::{Api, AssistantMessage, AssistantMessageEvent};
use nous_chord::delta::apply::{apply_immutable_trusted, TrustedOps};
use nous_chord::delta::codec::Encoder;
use nous_chord::delta::draft::Draft;
use nous_chord::delta::{Op, Revision, Seg};
use nous_durable::harness::partial::PartialChanges;
use serde_json::{json, Value};

fn empty_partial() -> Arc<AssistantMessage> {
    Arc::new(AssistantMessage::empty(Api::Faux, "faux", "faux", 0))
}

fn text_delta(index: usize, delta: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::TextDelta {
        content_index: index,
        delta: delta.to_string(),
        partial: empty_partial(),
    }
}

fn toolcall_delta(index: usize, delta: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::ToolCallDelta {
        content_index: index,
        delta: delta.to_string(),
        partial: empty_partial(),
    }
}

fn partial_message() -> Value {
    json!({
        "role": "assistant",
        "content": [{"type": "text", "text": ""}],
        "usage": {"input": 1, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 1},
        "stopReason": "stop",
        "timestamp": 0,
    })
}

fn toolcall_message() -> Value {
    json!({
        "role": "assistant",
        "content": [
            {"type": "text", "text": ""},
            {"type": "toolCall", "id": "t1", "name": "read", "arguments": {}},
        ],
        "usage": {"input": 1, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 1},
        "stopReason": "stop",
        "timestamp": 0,
    })
}

fn draft_of(base: &Value) -> Draft {
    Draft::new(base, Revision::ZERO)
}

fn ops_of(draft: Draft) -> Vec<Op> {
    draft.prepare(Revision::ZERO).expect("修订号没变").into_ops()
}

fn replayed(base: &Value, ops: &[Op]) -> Value {
    let trusted = TrustedOps::from_decoded(ops.to_vec()).expect("产出的 op 必须自己合法");
    apply_immutable_trusted(base, &trusted).expect("能应用")
}

/// 出帧体积：`Op` 本身不是 `Serialize`，线格式由 chord 的 encoder 决定 —— 判据要判的就是**发出去的
/// 那串 JSON**，不是 `Op` 的内存形状。
fn op_payload(ops: &[Op]) -> String {
    let mut encoder = Encoder::new();
    serde_json::to_string(&encoder.encode(ops)).expect("线格式能序列化")
}

fn op_bytes(ops: &[Op]) -> usize {
    op_payload(ops).len()
}

/// 一条 `Append` 之外的 op 就算违规：返回第一处违规的说明。
fn append_only_violations(per_commit: &[Vec<Op>], chunks: &[&str], text: &str) -> Vec<String> {
    let mut violations = Vec::new();
    for (round, ops) in per_commit.iter().enumerate().skip(1) {
        if ops.len() != 1 {
            violations.push(format!("第 {round} 次提交的 op 数是 {}", ops.len()));
        }
        match &ops[..] {
            [Op::Append(_, delta)] if delta == chunks[round] => {}
            [Op::Append(_, delta)] => violations.push(format!(
                "第 {round} 次提交追加的是 {delta:?}，不是这一片增量 {:?}",
                chunks[round]
            )),
            other => violations.push(format!("第 {round} 次提交不是一条追加：{other:?}")),
        }
        if op_payload(ops).contains(text) {
            violations.push(format!("第 {round} 次提交的载荷里带了整段正文"));
        }
    }
    violations
}

/// 正文按块喂进去，每 `every` 片提交一次；返回「每次提交的 op」与「最后一次的正文」。
fn stream_text(chunks: &[&str]) -> (Vec<Vec<Op>>, String, usize) {
    let mut changes = PartialChanges::new();
    let mut message = partial_message();
    let mut committed = json!({});
    let mut per_commit = Vec::new();
    let mut ledger_rounds = 0;
    let mut text = String::new();
    for chunk in chunks {
        changes.observe(&text_delta(0, chunk));
        text.push_str(chunk);
        message["content"][0]["text"] = json!(text);
        let mut draft = draft_of(&committed);
        let taken = changes.take(&mut message);
        changes
            .commit(&mut draft, &mut message, taken)
            .expect("提交能写");
        if !draft.appends().is_empty() {
            ledger_rounds += 1;
        }
        let ops = ops_of(draft);
        committed = replayed(&committed, &ops);
        per_commit.push(ops);
    }
    assert_eq!(
        committed["message"]["content"][0]["text"],
        json!(text),
        "提交出来的正文必须等于喂进去的正文"
    );
    (per_commit, text, ledger_rounds)
}

/// ① 每次提交只有一条 `Append`，且它的载荷里没有整段正文；② `len(ops)` 不随正文变长而增长。
#[test]
fn partial_changes_appends_text_instead_of_reassigning() {
    let chunks: Vec<&str> = std::iter::repeat("0123456789").take(400).collect();
    let (per_commit, text, ledger_rounds) = stream_text(&chunks);
    assert!(text.len() >= 4_000, "这一段要长到能看出 O(n²)");

    // 第一次提交只能整块写（文档里还没有 message），从第二次起每次都必须只有一条追加。
    let violations = append_only_violations(&per_commit, &chunks, &text);
    assert!(violations.is_empty(), "违规：{violations:#?}");

    // 追加**账本**才是「计划真的被用上了」的唯一直接证据（见负向对照那条的解释）。
    assert!(
        ledger_rounds >= chunks.len() - 1,
        "每次提交都该在账本里留下追加记录，实际只有 {ledger_rounds} 次"
    );
    let tail: Vec<usize> = per_commit.iter().skip(1).map(Vec::len).collect();
    assert!(
        tail.iter().all(|&count| count == 1),
        "op 数随正文变长而增长了：{tail:?}"
    );
    let payloads: Vec<usize> = per_commit.iter().skip(1).map(|ops| op_bytes(ops)).collect();
    assert!(
        payloads.iter().all(|&bytes| bytes < 200),
        "每次提交的字节数应该只跟增量走，实际 {payloads:?}"
    );
}

/// ③ 负向对照 —— 结果和设计这套判据时的预期不一样，写清楚：
///
/// **把「逐叶写 + `Append` 计划」换成整块 `set`，体积判据不会红。** `Draft::prepare` 每次都先
/// `diff_revisions(base, value)` 兜底，diff 自己就能从「旧串是新串的前缀」重新推导出那条 `a`；
/// 而且 `Draft::splice`/`set` 只影响本次的写，不影响 diff 看到的值差。所以两条路径**发出去的
/// 字节完全一样**，差的是 `prepare` 里那条 O(长度) 的字符串比较，体积判据看不见。
///
/// 真正会红的是**追加账本**：`Draft::appends()` 里有没有那条记录。这正是
/// `partial_changes_appends_text_instead_of_reassigning` 里 `ledger_paths` 断言守的东西 ——
/// `take()` 不再产出 `Append` 计划、或 `assign_json` 不再用它，账本就空，那条红。
///
/// 体积这个**度量**本身也要证明不是空转：手搭一组「每次整块写」的 op，`append_only_violations`
/// 必须报违规。
#[test]
fn dropping_the_append_plan_is_caught_by_the_ledger_not_by_the_bytes() {
    let chunks: Vec<&str> = std::iter::repeat("0123456789").take(40).collect();

    // (a) 同样的流，提交时不用计划、整块写 → 字节数与有计划的路径相同。
    let mut message = partial_message();
    let mut committed = json!({});
    let mut naive: Vec<Vec<Op>> = Vec::new();
    let mut text = String::new();
    for chunk in &chunks {
        text.push_str(chunk);
        message["content"][0]["text"] = json!(text);
        let mut draft = draft_of(&committed);
        let taken = PartialChanges::new().take(&mut message); // 计划照生成，但下面不用它
        let _ = taken;
        draft
            .set(&[Seg::key("message")], message.clone())
            .expect("能写");
        let ops = ops_of(draft);
        committed = replayed(&committed, &ops);
        naive.push(ops);
    }
    assert!(
        append_only_violations(&naive, &chunks, &text).is_empty(),
        "整块写这条路，diff 兜底也会发 `a` —— 体积判据在这条上确实分辨不出来"
    );

    // (b) 手搭「每次提交带整段正文」，同一个判据必须报违规。
    let mut naive_ops: Vec<Vec<Op>> = Vec::new();
    let mut growing = String::new();
    for chunk in &chunks {
        growing.push_str(chunk);
        let mut whole = partial_message();
        whole["content"][0]["text"] = json!(growing);
        naive_ops.push(vec![Op::Set(vec![Seg::key("message")], whole)]);
    }
    assert!(
        !append_only_violations(&naive_ops, &chunks, &growing).is_empty(),
        "真·整块写必须被判据抓住，否则这套判据只是空转"
    );
}

/// 块的 `*_end` 事件丢掉攒的增量：provider 可能在块结束时替换整段文本。
#[test]
fn a_block_end_drops_the_pending_delta() {
    let mut changes = PartialChanges::new();
    let mut message = partial_message();
    changes.observe(&text_delta(0, "ab"));
    changes.observe(&AssistantMessageEvent::TextEnd {
        content_index: 0,
        content: "ab".to_string(),
        partial: empty_partial(),
    });
    let taken = changes.take(&mut message);
    assert!(
        taken.tool_calls.is_empty(),
        "`text_end` 之后不该还留着增量"
    );
    let mut draft = draft_of(&json!({"message": partial_message()}));
    changes.commit(&mut draft, &mut message, taken).unwrap();
    let ops = ops_of(draft);
    assert!(
        !ops.iter().any(|op| matches!(op, Op::Append(..))),
        "增量已经丢了，不该再发追加：{ops:?}"
    );
}

/// 工具参数：喂片段，提交后已提交的参数恰好等于解析到现在的文本。
#[test]
fn a_streamed_tool_call_commits_its_argument_changes() {
    let mut changes = PartialChanges::new();
    let mut message = toolcall_message();
    let mut committed = json!({});
    changes.observe(&AssistantMessageEvent::ToolCallStart {
        content_index: 1,
        partial: empty_partial(),
    });
    for piece in ["{\"path\":", "\"/tmp/a\"", ",\"depth\":", "3}"] {
        changes.observe(&toolcall_delta(1, piece));
        let mut draft = draft_of(&committed);
        let taken = changes.take(&mut message);
        assert_eq!(
            message["content"][1]["arguments"],
            json!({}),
            "take() 要清空被跟踪调用的参数：提交时按改动逐条写"
        );
        changes
            .commit(&mut draft, &mut message, taken)
            .expect("提交能写");
        let ops = ops_of(draft);
        committed = replayed(&committed, &ops);
    }
    assert_eq!(
        committed["message"]["content"][1]["arguments"],
        json!({"path": "/tmp/a", "depth": 3}),
        "已提交的参数必须等于解析到现在的文本"
    );
    let ops_payload = serde_json::to_string(&committed["message"]["content"][1]["arguments"]).unwrap();
    assert!(ops_payload.contains("\"path\""));
}

/// 已提交消息里那块丢了（别的写者重置了它）时，整块写快照并停止跟踪。
#[test]
fn a_tracked_tool_call_falls_back_to_its_snapshot_when_its_block_is_gone() {
    let mut changes = PartialChanges::new();
    let mut message = toolcall_message();
    changes.observe(&AssistantMessageEvent::ToolCallStart {
        content_index: 1,
        partial: empty_partial(),
    });
    changes.observe(&toolcall_delta(1, "{\"path\":"));
    changes.observe(&toolcall_delta(1, "\"/tmp/a\"}"));
    // 第一次提交：文档里还没有 message，块是新的，参数从改动写起。
    let mut first = draft_of(&json!({}));
    let taken = changes.take(&mut message);
    changes.commit(&mut first, &mut message, taken).unwrap();
    let committed = replayed(&json!({}), &ops_of(first));
    assert_eq!(
        committed["message"]["content"][1]["arguments"],
        json!({"path": "/tmp/a"})
    );

    // 另一个写者把 message 重置成「没有这一块」，而这次调用已经提交过改动。
    let reset = json!({"message": {"content": []}});
    message["content"][1]["arguments"] = json!({"path": "/tmp/a", "depth": 3});
    changes.observe(&toolcall_delta(1, ""));
    let mut second = draft_of(&reset);
    let taken = changes.take(&mut message);
    assert!(taken.tool_calls.contains_key(&1), "它还在跟踪");
    changes.commit(&mut second, &mut message, taken).unwrap();
    let after = replayed(&reset, &ops_of(second));
    assert_eq!(
        after["message"]["content"][1]["arguments"],
        json!({"path": "/tmp/a", "depth": 3}),
        "块是新的时候整块写快照"
    );

    // 跟踪已经停掉：再喂片段也不会被记下来。
    changes.observe(&toolcall_delta(1, ",\"x\":1}"));
    let taken = changes.take(&mut message);
    assert!(
        taken.tool_calls.is_empty(),
        "快照兜底之后要停止跟踪这次调用"
    );
}

/// 参数根不是对象：这一片不算参数，停止跟踪（否则会把一整个数组写成 arguments）。
#[test]
fn arguments_that_are_not_an_object_stop_the_tracking() {
    let mut changes = PartialChanges::new();
    let mut message = toolcall_message();
    changes.observe(&AssistantMessageEvent::ToolCallStart {
        content_index: 1,
        partial: empty_partial(),
    });
    changes.observe(&toolcall_delta(1, "[1,2]"));
    let taken = changes.take(&mut message);
    assert!(
        taken.tool_calls.is_empty(),
        "数组根不该被当成参数改动记下来"
    );
}

/// 提交失败时把取走的改动放回去，下一次提交先应用它们。
#[test]
fn partial_changes_restores_what_a_failed_commit_took() {
    let mut changes = PartialChanges::new();
    let mut message = toolcall_message();
    changes.observe(&AssistantMessageEvent::ToolCallStart {
        content_index: 1,
        partial: empty_partial(),
    });
    changes.observe(&toolcall_delta(1, "{\"path\":"));
    changes.observe(&toolcall_delta(1, "\"/tmp/a\""));

    // 第一次提交失败（模拟：取走之后没有提交）。
    let failed = changes.take(&mut message);
    assert_eq!(failed.tool_calls[&1].changes.len(), 2, "两片 → 两条改动");
    changes.restore(failed);

    // 再喂一片，下一次提交必须把失败那批也带上，且顺序不变。
    changes.observe(&toolcall_delta(1, ",\"depth\":3}"));
    let mut draft = draft_of(&json!({}));
    let taken = changes.take(&mut message);
    assert_eq!(
        taken.tool_calls[&1].changes.len(),
        3,
        "恢复之后改动要接在前面，实际是 {:?}",
        taken.tool_calls[&1].changes
    );
    changes.commit(&mut draft, &mut message, taken).unwrap();
    let committed = replayed(&json!({}), &ops_of(draft));
    assert_eq!(
        committed["message"]["content"][1]["arguments"],
        json!({"path": "/tmp/a", "depth": 3}),
        "失败那批改动没有丢"
    );
}
