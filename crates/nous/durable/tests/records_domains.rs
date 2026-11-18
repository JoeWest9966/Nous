//! #13 的第二条判据：**取值域逐格验**。
//!
//! fixture 只覆盖真库里出现过的值（`input/done`、`input/unanswered`、文档的两个组合），
//! 剩下那些格子必须用自造值钉住 —— 否则「矩阵写错一个格子」这类错误永远碰不到。
//!
//! 每条枚举两条测试：**合法值逐个往返** + **非法组合明确报错**。
//! 报错必须发生在**反序列化**，不是某个 `if` 分支。

use nous_durable::types::{
    ContextEdit, ConversationOwnership, DocumentFork, DocumentHistory, DocumentRecord,
    DocumentScope, EntryDraft, EntryHead, SubmissionCreate, SubmissionFields, SubmissionRecord,
    SubmissionSettlement, SubmissionStatus, SubmissionType,
};
use serde_json::json;

fn record(scope: DocumentScope, history: Option<DocumentHistory>, fork: Option<DocumentFork>) -> serde_json::Value {
    let mut value = json!({"id": 7, "kind": "pi.live", "createdAt": 3});
    value["scope"] = serde_json::to_value(scope).unwrap();
    if let Some(history) = history {
        value["history"] = serde_json::to_value(history).unwrap();
    }
    if let Some(fork) = fork {
        value["fork"] = serde_json::to_value(fork).unwrap();
    }
    value
}

// ── DocumentScope × DocumentHistory × DocumentFork ─────────────────────────────

#[test]
fn document_scope_literals_are_exactly_the_three_upstream_kinds() {
    assert_eq!(
        serde_json::to_value(DocumentScope::Session).unwrap(),
        json!({"kind": "session"})
    );
    assert_eq!(
        serde_json::to_value(DocumentScope::Conversation { conversation_id: 42 }).unwrap(),
        json!({"kind": "conversation", "conversationId": 42})
    );
    assert_eq!(
        serde_json::to_value(DocumentScope::Task { task_id: 9 }).unwrap(),
        json!({"kind": "task", "taskId": 9})
    );
    assert!(serde_json::from_value::<DocumentScope>(json!({"kind": "workspace"})).is_err());
}

#[test]
fn document_history_and_fork_literals_match_upstream() {
    assert_eq!(serde_json::to_value(DocumentHistory::Latest).unwrap(), json!("latest"));
    assert_eq!(
        serde_json::to_value(DocumentHistory::Rewindable).unwrap(),
        json!("rewindable")
    );
    assert_eq!(serde_json::to_value(DocumentFork::Current).unwrap(), json!("current"));
    assert_eq!(serde_json::to_value(DocumentFork::Initial).unwrap(), json!("initial"));
    assert_eq!(serde_json::to_value(DocumentFork::AsOf).unwrap(), json!("asOf"));
    assert!(serde_json::from_value::<DocumentFork>(json!("asof")).is_err());
}

#[test]
fn every_legal_scope_history_fork_combination_round_trips() {
    let cases = [
        (DocumentScope::Session, None, None),
        (DocumentScope::Task { task_id: 5 }, None, None),
        (DocumentScope::Conversation { conversation_id: 2 }, Some(DocumentHistory::Latest), Some(DocumentFork::Current)),
        (DocumentScope::Conversation { conversation_id: 2 }, Some(DocumentHistory::Latest), Some(DocumentFork::Initial)),
        (DocumentScope::Conversation { conversation_id: 2 }, Some(DocumentHistory::Rewindable), Some(DocumentFork::AsOf)),
        (DocumentScope::Conversation { conversation_id: 2 }, Some(DocumentHistory::Rewindable), Some(DocumentFork::Current)),
        (DocumentScope::Conversation { conversation_id: 2 }, Some(DocumentHistory::Rewindable), Some(DocumentFork::Initial)),
    ];
    for (scope, history, fork) in cases {
        let value = record(scope, history, fork);
        let parsed: DocumentRecord = serde_json::from_value(value.clone())
            .unwrap_or_else(|error| panic!("合法组合被拒：{value} —— {error}"));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }
}

#[test]
fn illegal_scope_history_fork_combinations_fail_at_deserialization() {
    let conversation = DocumentScope::Conversation { conversation_id: 2 };
    let cases = [
        // asOf 只在 rewindable 下有意义
        (record(conversation, Some(DocumentHistory::Latest), Some(DocumentFork::AsOf)), "asOf"),
        // 会话作用域必须带 history / fork
        (record(conversation, None, None), "history"),
        (record(conversation, Some(DocumentHistory::Latest), None), "fork"),
        // session / task 作用域不许带
        (record(DocumentScope::Session, Some(DocumentHistory::Latest), None), "history"),
        (record(DocumentScope::Task { task_id: 5 }, None, Some(DocumentFork::AsOf)), "task"),
    ];
    for (value, expected) in cases {
        let error = serde_json::from_value::<DocumentRecord>(value.clone())
            .expect_err("非法组合必须报错");
        assert!(
            error.to_string().to_lowercase().contains(&expected.to_lowercase()),
            "报错信息该提到 {expected}，实际：{error}（值 {value}）"
        );
    }
}

// ── SubmissionRecord 的 type × status 矩阵 ─────────────────────────────────────

fn submission(fields: serde_json::Value) -> serde_json::Value {
    let mut value = json!({"id": 100});
    for (key, item) in fields.as_object().unwrap() {
        value[key] = item.clone();
    }
    value
}

#[test]
fn submission_type_and_status_literals_match_upstream() {
    for (kind, text) in [
        (SubmissionType::Input, "input"),
        (SubmissionType::Write, "write"),
        (SubmissionType::Continue, "continue"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(text));
    }
    for (status, text) in [
        (SubmissionStatus::Queued, "queued"),
        (SubmissionStatus::Placed, "placed"),
        (SubmissionStatus::Done, "done"),
        (SubmissionStatus::Unanswered, "unanswered"),
    ] {
        assert_eq!(serde_json::to_value(status).unwrap(), json!(text));
    }
    assert!(serde_json::from_value::<SubmissionStatus>(json!("answered")).is_err());
}

#[test]
fn every_legal_submission_combination_round_trips() {
    let cases = [
        json!({"conversationId": 2, "type": "input", "status": "queued"}),
        json!({"conversationId": 2, "type": "input", "status": "placed", "entry": 9}),
        json!({"conversationId": 2, "type": "input", "status": "done", "entry": 9, "answer": 13}),
        json!({"conversationId": 2, "type": "input", "status": "unanswered", "entry": 9, "reason": "model_error"}),
        json!({"conversationId": 2, "type": "input", "status": "unanswered", "reason": "model_error", "detail": "400"}),
        json!({"conversationId": 2, "type": "write", "status": "queued"}),
        json!({"conversationId": 2, "type": "write", "status": "done", "entry": 9}),
        json!({"conversationId": 2, "type": "write", "status": "unanswered", "reason": "quota"}),
        json!({"conversationId": 2, "type": "continue", "status": "placed"}),
        json!({"conversationId": 2, "type": "continue", "status": "done", "answer": 13}),
        json!({"conversationId": 2, "type": "continue", "status": "unanswered", "reason": "aborted"}),
        // requestId 与 trace 在任意组合下都可选
        json!({"conversationId": 2, "type": "input", "status": "queued", "requestId": "r-1", "trace": {"traceId": "abc"}}),
    ];
    for fields in cases {
        let value = submission(fields.clone());
        let parsed: SubmissionRecord = serde_json::from_value(value.clone())
            .unwrap_or_else(|error| panic!("合法组合被拒：{value} —— {error}"));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }
}

#[test]
fn illegal_submission_combinations_fail_at_deserialization() {
    let cases = [
        // write 没有 placed 这一格
        json!({"conversationId": 2, "type": "write", "status": "placed"}),
        // continue 没有 queued 这一格
        json!({"conversationId": 2, "type": "continue", "status": "queued"}),
        // input.placed 必须带 entry
        json!({"conversationId": 2, "type": "input", "status": "placed"}),
        // input.done 必须带 answer
        json!({"conversationId": 2, "type": "input", "status": "done", "entry": 9}),
        // write/continue 的终态不许带 entry
        json!({"conversationId": 2, "type": "write", "status": "unanswered", "entry": 9, "reason": "x"}),
        json!({"conversationId": 2, "type": "continue", "status": "done", "entry": 9, "answer": 13}),
        // reason 只属于 unanswered
        json!({"conversationId": 2, "type": "input", "status": "queued", "reason": "x"}),
        // unanswered 必须给 reason
        json!({"conversationId": 2, "type": "input", "status": "unanswered", "entry": 9}),
        // detail 必须跟着 reason
        json!({"conversationId": 2, "type": "input", "status": "unanswered", "detail": "x"}),
        // 未知 type / status
        json!({"conversationId": 2, "type": "passive", "status": "queued"}),
        json!({"conversationId": 2, "type": "input", "status": "settled"}),
    ];
    for fields in cases {
        let value = submission(fields.clone());
        assert!(
            serde_json::from_value::<SubmissionRecord>(value.clone()).is_err(),
            "非法组合被接受了：{value}"
        );
        // 同一个形状走 SubmissionCreate（没有 id）也必须一样被拒。
        let mut without_id = value.clone();
        without_id.as_object_mut().unwrap().remove("id");
        assert!(
            serde_json::from_value::<SubmissionCreate>(without_id.clone()).is_err(),
            "SubmissionCreate 接受了非法组合：{without_id}"
        );
    }
}

#[test]
fn submission_create_carries_no_id() {
    let value = json!({"conversationId": 2, "type": "input", "status": "queued"});
    let create: SubmissionCreate = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&create).unwrap(), value);
}

// ── 其余枚举 ───────────────────────────────────────────────────────────────────

#[test]
fn context_edit_actions_are_omit_and_replace_only() {
    let omit: ContextEdit = serde_json::from_value(json!({"action": "omit", "target": 9})).unwrap();
    assert_eq!(omit.target(), 9);
    assert_eq!(serde_json::to_value(&omit).unwrap(), json!({"action": "omit", "target": 9}));

    let replace: ContextEdit =
        serde_json::from_value(json!({"action": "replace", "target": 9, "messages": []})).unwrap();
    assert_eq!(replace.target(), 9);
    assert_eq!(
        serde_json::to_value(&replace).unwrap(),
        json!({"action": "replace", "target": 9, "messages": []})
    );

    // omit 不许带 messages；replace 必须带 messages。
    assert!(serde_json::from_value::<ContextEdit>(json!({"action": "omit", "target": 9, "messages": []})).is_err());
    assert!(serde_json::from_value::<ContextEdit>(json!({"action": "replace", "target": 9})).is_err());
    assert!(serde_json::from_value::<ContextEdit>(json!({"action": "drop", "target": 9})).is_err());
}

#[test]
fn conversation_ownership_has_exactly_two_kinds() {
    let ownerless: ConversationOwnership =
        serde_json::from_value(json!({"kind": "ownerless"})).unwrap();
    assert_eq!(serde_json::to_value(ownerless).unwrap(), json!({"kind": "ownerless"}));

    let owned: ConversationOwnership =
        serde_json::from_value(json!({"kind": "task", "taskId": 11})).unwrap();
    assert_eq!(
        serde_json::to_value(owned).unwrap(),
        json!({"kind": "task", "taskId": 11})
    );
    assert!(serde_json::from_value::<ConversationOwnership>(json!({"kind": "user"})).is_err());
    assert!(serde_json::from_value::<ConversationOwnership>(json!({"kind": "task"})).is_err());
}

#[test]
fn entry_draft_head_accepts_an_id_or_the_literal_self() {
    let draft: EntryDraft = serde_json::from_value(json!({"kind": "pi.user", "head": "self"})).unwrap();
    assert_eq!(draft.head, Some(EntryHead::SelfRef));
    assert_eq!(
        serde_json::to_value(&draft).unwrap(),
        json!({"kind": "pi.user", "head": "self"})
    );

    let draft: EntryDraft = serde_json::from_value(json!({"kind": "pi.user", "head": 9})).unwrap();
    assert_eq!(draft.head, Some(EntryHead::Id(9)));
    assert_eq!(
        serde_json::to_value(&draft).unwrap(),
        json!({"kind": "pi.user", "head": 9})
    );

    assert!(serde_json::from_value::<EntryDraft>(json!({"kind": "pi.user", "head": "me"})).is_err());
}

#[test]
fn submission_settlement_has_exactly_two_terminal_states() {
    let done: SubmissionSettlement =
        serde_json::from_value(json!({"status": "done", "answer": 13})).unwrap();
    assert_eq!(
        serde_json::to_value(done).unwrap(),
        json!({"status": "done", "answer": 13})
    );

    let unanswered: SubmissionSettlement = serde_json::from_value(
        json!({"status": "unanswered", "reason": "model_error", "detail": {"code": 400}}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(unanswered).unwrap(),
        json!({"status": "unanswered", "reason": "model_error", "detail": {"code": 400}})
    );

    assert!(serde_json::from_value::<SubmissionSettlement>(json!({"status": "queued"})).is_err());
    assert!(serde_json::from_value::<SubmissionSettlement>(json!({"status": "done"})).is_err());
    assert!(serde_json::from_value::<SubmissionSettlement>(json!({"status": "unanswered"})).is_err());
}

#[test]
fn task_abandon_on_restart_is_only_ever_true() {
    let task: nous_durable::types::TaskRecord = serde_json::from_value(json!({
        "id": 11,
        "conversationId": 2,
        "kind": "pi.generation",
        "version": 1,
        "input": {},
        "background": false,
        "abortRequested": false,
        "abandonOnRestart": true,
        "state": {"status": "terminal"},
    }))
    .unwrap();
    assert_eq!(task.abandon_on_restart, Some(true));
    assert_eq!(
        serde_json::to_value(&task).unwrap()["abandonOnRestart"],
        json!(true)
    );

    let error = serde_json::from_value::<nous_durable::types::TaskRecord>(json!({
        "id": 11,
        "conversationId": 2,
        "kind": "pi.generation",
        "version": 1,
        "input": {},
        "background": false,
        "abortRequested": false,
        "abandonOnRestart": false,
        "state": {"status": "terminal"},
    }))
    .expect_err("abandonOnRestart: false 不是上游的取值");
    assert!(error.to_string().contains("abandonOnRestart"), "{error}");
}

#[test]
fn submission_fields_alone_validate_the_same_matrix() {
    // 合法：只缺 id。
    let fields: SubmissionFields = serde_json::from_value(json!({
        "conversationId": 2, "type": "input", "status": "done", "entry": 9, "answer": 13
    }))
    .unwrap();
    assert_eq!(fields.kind, SubmissionType::Input);
    assert_eq!(fields.status, SubmissionStatus::Done);
    assert!(fields.validate().is_ok());

    // 非法：同一份校验。
    assert!(serde_json::from_value::<SubmissionFields>(json!({
        "conversationId": 2, "type": "continue", "status": "queued"
    }))
    .is_err());
}

// ── 终态五值（issue #38 / spec ch5）────────────────────────────────────────────

#[test]
fn terminal_reason_has_exactly_five_literals() {
    use nous_durable::types::TerminalReason;

    let cases = [
        (TerminalReason::Done, "done"),
        (TerminalReason::UserCancel, "user_cancel"),
        (TerminalReason::Timeout, "timeout"),
        (TerminalReason::ExtensionCrash, "extension_crash"),
        (TerminalReason::Abandoned, "abandoned"),
    ];
    for (value, literal) in cases {
        assert_eq!(serde_json::to_value(value).unwrap(), json!(literal));
        assert_eq!(
            serde_json::from_value::<TerminalReason>(json!(literal)).unwrap(),
            value
        );
    }
    for bad in ["cancelled", "crash", "abandon", "broken"] {
        assert!(
            serde_json::from_value::<TerminalReason>(json!(bad)).is_err(),
            "{bad} 不是终态值"
        );
    }
}

#[test]
fn terminal_reason_lives_on_an_entry_record_not_a_second_entry_kind() {
    use nous_durable::types::{EntryRecord, TerminalReason};

    let source = json!({
        "id": 12,
        "conversationId": 1,
        "kind": "pi.assistant",
        "model": [{ "role": "assistant", "content": [] }],
        "reason": "extension_crash",
    });
    let entry: EntryRecord = serde_json::from_value(source.clone()).unwrap();
    assert_eq!(entry.reason, Some(TerminalReason::ExtensionCrash));
    assert_eq!(serde_json::to_value(entry).unwrap(), source, "终态不改 entry 的形状");

    let live = json!({
        "id": 13,
        "conversationId": 1,
        "kind": "pi.assistant",
        "model": [{ "role": "assistant", "content": [] }],
    });
    let entry: EntryRecord = serde_json::from_value(live.clone()).unwrap();
    assert_eq!(entry.reason, None, "没有终态的 entry 不许凭空带 reason");
    assert_eq!(serde_json::to_value(entry).unwrap(), live);
}
