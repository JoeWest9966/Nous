//! #13 commit ② 判据：提交发布面的形状，与上游 `types.ts` 逐字段相同。
//!
//! 三件事分开验，因为它们会分开坏：
//!
//! 1. **JSON 形状**：`type` / `kind` 两个判别键、`conversationId` 在 session 文档上**缺席**
//!    （不是 `null`）、`copy` 的 `conversationId` 必填、多余键报错。
//! 2. **一次提交里三种记录各出现一次**：publication 只带一个 `seq`，改动本身不带 `seq`
//!    —— 「seq 全等于 publication.seq」在形状上的意思就是「改动身上没有第二个 seq」。
//! 3. **`ops` 是提交上来的那批，不是重算的 diff**：故意给一批「多余但合法」的 op，publication
//!    必须原样带着它，不许折叠。

use std::sync::{Arc, Mutex};

use nous_chord::context::Context;
use nous_chord::watch::WatchEnd;
use nous_durable::session::entries::{TOOL_RESULT, USER};
use nous_durable::session::{
    CommitChange, CommitObservation, CommitObserver, CommitPublication, DocumentCommitChange,
    ObserverRegistry,
};
use nous_durable::storage::import::ImportedStore;
use nous_durable::types::{
    ContextEdit, DocumentFork, DocumentHistory, DocumentRecord, DocumentScope, EntryRecord,
};
use nous_durable::{CopySource, DocumentPoint, Id};
use nous_chord::sqlite::{SqliteRow, SqliteValue as Cell};
use serde_json::{json, Value};

// ── 造几份记录 ────────────────────────────────────────────────────────────────

fn conversation(id: Id) -> nous_durable::types::ConversationRecord {
    serde_json::from_value(json!({"id": id})).unwrap()
}

fn entry(id: Id, conversation_id: Id, kind: &str) -> EntryRecord {
    serde_json::from_value(json!({
        "id": id,
        "conversationId": conversation_id,
        "kind": kind,
        "model": [{"role": "user", "content": "hi", "timestamp": 1}],
    }))
    .unwrap()
}

fn session_document(id: Id) -> DocumentRecord {
    serde_json::from_value(json!({
        "id": id,
        "kind": "qrd3wh.sessions",
        "createdAt": 1,
        "scope": {"kind": "session"},
    }))
    .unwrap()
}

fn conversation_document(id: Id, conversation_id: Id) -> DocumentRecord {
    serde_json::from_value(json!({
        "id": id,
        "kind": "pi.live",
        "createdAt": 2,
        "scope": {"kind": "conversation", "conversationId": conversation_id},
        "history": "rewindable",
        "fork": "asOf",
    }))
    .unwrap()
}

// ── 1. 形状 ───────────────────────────────────────────────────────────────────

#[test]
fn publication_shape_matches_upstream_field_names() {
    let publication = CommitPublication::new(
        42,
        vec![
            CommitChange::Conversation(conversation(2)),
            CommitChange::Entry(entry(9, 2, "pi.user")),
            CommitChange::Document(DocumentCommitChange::Change {
                record: conversation_document(31, 2),
                conversation_id: Some(2),
                version: 3,
                value: json!({"text": "hi"}),
                ops: json!([["s", ["text"], "hi"]]),
            }),
        ],
    );

    let written = serde_json::to_value(&publication).unwrap();

    assert_eq!(
        written["seq"], json!(42),
        "publication 只有这一个 seq"
    );
    assert_eq!(
        written["changes"][0],
        json!({"type": "conversation", "value": {"id": 2}})
    );
    assert_eq!(
        written["changes"][1],
        json!({
            "type": "entry",
            "value": {
                "id": 9,
                "conversationId": 2,
                "kind": "pi.user",
                "model": [{"role": "user", "content": "hi", "timestamp": 1}],
            },
        })
    );
    assert_eq!(
        written["changes"][2],
        json!({
            "type": "document",
            "kind": "change",
            "record": {
                "id": 31,
                "kind": "pi.live",
                "createdAt": 2,
                "scope": {"kind": "conversation", "conversationId": 2},
                "history": "rewindable",
                "fork": "asOf",
            },
            "conversationId": 2,
            "version": 3,
            "value": {"text": "hi"},
            "ops": [["s", ["text"], "hi"]],
        })
    );

    // 读回来一字不差。
    let parsed: CommitPublication = serde_json::from_value(written).unwrap();
    assert_eq!(parsed, publication);
}

#[test]
fn session_documents_have_no_conversation_id_key_at_all() {
    let change = DocumentCommitChange::Change {
        record: session_document(31),
        conversation_id: None,
        version: 1,
        value: json!({}),
        ops: json!([]),
    };
    let written = serde_json::to_value(&change).unwrap();
    assert!(
        written.get("conversationId").is_none(),
        "session 作用域的文档：键要缺席，不是 null —— 实际 {written}"
    );
    assert_eq!(
        written,
        json!({
            "type": "document",
            "kind": "change",
            "record": {"id": 31, "kind": "qrd3wh.sessions", "createdAt": 1, "scope": {"kind": "session"}},
            "version": 1,
            "value": {},
            "ops": [],
        })
    );

    let retire = DocumentCommitChange::Retire {
        record: session_document(31),
        conversation_id: None,
    };
    assert!(serde_json::to_value(&retire).unwrap().get("conversationId").is_none());
}

#[test]
fn retire_and_copy_have_their_own_shapes() {
    let retire = DocumentCommitChange::Retire {
        record: conversation_document(31, 2),
        conversation_id: Some(2),
    };
    let written = serde_json::to_value(&retire).unwrap();
    assert_eq!(written["type"], json!("document"));
    assert_eq!(written["kind"], json!("retire"));
    assert_eq!(written["conversationId"], json!(2));
    assert!(written.get("version").is_none(), "退役不带内容");
    assert!(written.get("ops").is_none());

    let copy = DocumentCommitChange::Copy {
        record: session_document(44),
        conversation_id: 2,
        source: CopySource {
            id: 31,
            at: DocumentPoint::At(7),
        },
    };
    let written = serde_json::to_value(&copy).unwrap();
    assert_eq!(
        written,
        json!({
            "type": "document.copy",
            "record": {"id": 44, "kind": "qrd3wh.sessions", "createdAt": 1, "scope": {"kind": "session"}},
            "conversationId": 2,
            "source": {"id": 31, "at": 7},
        })
    );
    // `at` 是 `Seq | "current"`，不是 `{"kind": "current"}`。
    let current = DocumentCommitChange::Copy {
        record: session_document(44),
        conversation_id: 2,
        source: CopySource {
            id: 31,
            at: DocumentPoint::Current,
        },
    };
    assert_eq!(
        serde_json::to_value(&current).unwrap()["source"],
        json!({"id": 31, "at": "current"})
    );
}

#[test]
fn extra_and_missing_keys_are_rejected() {
    let cases = [
        // 表记录只许 {type, value}
        json!({"type": "conversation", "value": {"id": 2}, "extra": 1}),
        json!({"type": "conversation"}),
        // 文档 change 缺 ops
        json!({
            "type": "document", "kind": "change",
            "record": {"id": 31, "kind": "k", "createdAt": 1, "scope": {"kind": "session"}},
            "version": 1, "value": {}
        }),
        // copy 的 conversationId 必填
        json!({
            "type": "document.copy",
            "record": {"id": 44, "kind": "k", "createdAt": 1, "scope": {"kind": "session"}},
            "source": {"id": 31, "at": 7}
        }),
        // `null` 不是「缺席」
        json!({
            "type": "document", "kind": "retire",
            "record": {"id": 31, "kind": "k", "createdAt": 1, "scope": {"kind": "session"}},
            "conversationId": null
        }),
        // 判别键不认识
        json!({"type": "blob", "value": {}}),
        json!({
            "type": "document", "kind": "rewind",
            "record": {"id": 31, "kind": "k", "createdAt": 1, "scope": {"kind": "session"}}
        }),
        // 作用域与 history/fork 的配对规则在 publication 里同样生效
        json!({
            "type": "document", "kind": "retire",
            "record": {
                "id": 31, "kind": "k", "createdAt": 1,
                "scope": {"kind": "conversation", "conversationId": 2},
                "history": "latest", "fork": "asOf"
            },
            "conversationId": 2
        }),
    ];
    for value in cases {
        assert!(
            serde_json::from_value::<CommitChange>(value.clone()).is_err(),
            "该被拒：{value}"
        );
    }
}

// ── 2. 一次提交里三种记录各一次、只有一个 seq ─────────────────────────────────

#[test]
fn one_commit_publishes_each_record_variant_once_under_a_single_seq() {
    let publication = CommitPublication::new(
        77,
        vec![
            CommitChange::Conversation(conversation(2)),
            CommitChange::Entry(entry(9, 2, "pi.user")),
            CommitChange::Document(DocumentCommitChange::Change {
                record: conversation_document(31, 2),
                conversation_id: Some(2),
                version: 1,
                value: json!({}),
                ops: json!([]),
            }),
        ],
    );

    let written = serde_json::to_value(&publication).unwrap();
    let changes = written["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3);

    let mut tags: Vec<&str> = changes
        .iter()
        .map(|change| change["type"].as_str().unwrap())
        .collect();
    tags.sort_unstable();
    assert_eq!(tags, vec!["conversation", "document", "entry"]);

    // 「seq 全等于 publication.seq」的形状说法：改动身上不许有第二个 seq。
    for change in changes {
        assert!(
            change.get("seq").is_none(),
            "改动不携带自己的 seq，它属于 publication.seq —— 实际 {change}"
        );
    }
    assert_eq!(written["seq"], json!(77));
}

// ── 3. ops 原样透传，不是重算的 diff ──────────────────────────────────────────

#[test]
fn document_ops_are_the_submitted_batch_not_a_recomputed_diff() {
    // 这批 op 故意「不最小」：先设成跟原来一样的值，再改真正要改的那一项。
    // 重算 diff 的实现会把它折成一条（甚至 0 条）；publication 必须原样保留两条。
    let submitted = json!([
        ["s", ["title"], "Untitled"],
        ["s", ["title"], "Hello"],
    ]);

    let change = DocumentCommitChange::Change {
        record: conversation_document(31, 2),
        conversation_id: Some(2),
        version: 2,
        value: json!({"title": "Hello"}),
        ops: submitted.clone(),
    };

    let written = serde_json::to_value(&change).unwrap();
    assert_eq!(
        written["ops"], submitted,
        "ops 必须是提交上来的那一批，不是重算的 diff"
    );

    // 再读回来也不许被折叠。
    let parsed: CommitChange = serde_json::from_value(written).unwrap();
    let CommitChange::Document(DocumentCommitChange::Change { ops, value, version, .. }) = parsed
    else {
        panic!("读回来不是文档 change");
    };
    assert_eq!(ops, submitted);
    assert_eq!(value, json!({"title": "Hello"}));
    assert_eq!(version, 2);
}

// ── 4. 真 fixture 的文档记录能进 publication ──────────────────────────────────

#[test]
fn publication_carries_fixture_document_records_unchanged() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agent.sqlite");
    if !path.exists() {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    }
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    let mut checked = 0;
    for row in store.documents.iter().take(50) {
        let text = match row.get("record") {
            Some(Cell::Text(text)) => text.clone(),
            other => panic!("record 列不是文本：{other:?}"),
        };
        let source: Value = serde_json::from_str(&text).unwrap();
        let record: DocumentRecord = serde_json::from_str(&text).unwrap();
        let conversation_id = match record.scope {
            DocumentScope::Session => None,
            DocumentScope::Conversation { conversation_id } => Some(conversation_id),
            DocumentScope::Task { .. } => continue,
        };
        // 上游要求 `history: latest` / `fork: initial` 那一档在 publication 里也是同一个形状。
        if matches!(record.history, Some(DocumentHistory::Latest))
            && !matches!(record.fork, Some(DocumentFork::Initial) | Some(DocumentFork::Current))
        {
            continue;
        }
        let change = DocumentCommitChange::Change {
            record,
            conversation_id,
            version: 1,
            value: source.clone(),
            ops: json!([]),
        };
        let written = serde_json::to_value(&change).unwrap();
        assert_eq!(written["record"], source, "记录本身不许被 publication 改写");
        let back: DocumentCommitChange = serde_json::from_value(written).unwrap();
        assert_eq!(back, change);
        checked += 1;
    }
    assert!(checked > 0, "fixture 里应当至少有几十份能进 publication 的文档");
}

// ── 5. 观察者名单 ─────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Recorder {
    seen: Arc<Mutex<Vec<i64>>>,
    ends: Arc<Mutex<Vec<WatchEnd>>>,
}

impl CommitObserver for Recorder {
    fn advance(&self, publication: &CommitPublication, _context: &Context) {
        self.seen.lock().unwrap().push(publication.seq);
    }

    fn end(&self, end: &WatchEnd) {
        self.ends.lock().unwrap().push(end.clone());
    }
}

#[test]
fn observers_see_every_publication_once_until_they_detach() {
    let registry = ObserverRegistry::new();
    let first = Recorder::default();
    let second = Recorder::default();
    let first_observation: CommitObservation = registry.attach(Arc::new(first.clone()));
    let _second_observation = registry.attach(Arc::new(second.clone()));
    assert_eq!(registry.len(), 2);

    let context = Context::new();
    registry.publish(&CommitPublication::new(1, vec![]), &context);
    registry.publish(&CommitPublication::new(2, vec![]), &context);
    assert_eq!(*first.seen.lock().unwrap(), vec![1, 2]);
    assert_eq!(*second.seen.lock().unwrap(), vec![1, 2]);

    // 摘掉第一个：之后只有第二个收到。`detach()` 幂等。
    first_observation.detach();
    first_observation.detach();
    assert!(!first_observation.is_attached());
    assert_eq!(registry.len(), 1);
    registry.publish(&CommitPublication::new(3, vec![]), &context);
    assert_eq!(*first.seen.lock().unwrap(), vec![1, 2]);
    assert_eq!(*second.seen.lock().unwrap(), vec![1, 2, 3]);

    // 收尾：每个还挂着的各收一次，然后名单清空、不会收第二次。
    registry.end_all(&WatchEnd::failed("session closed"));
    registry.end_all(&WatchEnd::ENDED);
    assert!(registry.is_empty());
    assert_eq!(
        *second.ends.lock().unwrap(),
        vec![WatchEnd::failed("session closed")]
    );
    assert!(first.ends.lock().unwrap().is_empty(), "摘掉的收不到 end");
}

#[test]
fn an_observer_may_detach_itself_while_being_notified() {
    struct SelfDetaching {
        seen: Arc<Mutex<usize>>,
        handle: Mutex<Option<CommitObservation>>,
    }
    impl CommitObserver for SelfDetaching {
        fn advance(&self, _publication: &CommitPublication, _context: &Context) {
            *self.seen.lock().unwrap() += 1;
            if let Some(handle) = self.handle.lock().unwrap().take() {
                handle.detach();
            }
        }
        fn end(&self, _end: &WatchEnd) {}
    }

    let registry = ObserverRegistry::new();
    let observer = Arc::new(SelfDetaching {
        seen: Arc::new(Mutex::new(0)),
        handle: Mutex::new(None),
    });
    let handle = registry.attach(observer.clone());
    *observer.handle.lock().unwrap() = Some(handle);

    let context = Context::new();
    for seq in 0..3 {
        registry.publish(&CommitPublication::new(seq, vec![]), &context);
    }
    assert_eq!(*observer.seen.lock().unwrap(), 1, "摘掉自己后不再收到");
    assert!(registry.is_empty());
}

// ── 6. entry 判别键 ───────────────────────────────────────────────────────────

#[test]
fn entry_kinds_match_the_upstream_literals() {
    let names: Vec<&str> = nous_durable::session::entries::ALL
        .iter()
        .map(|kind| kind.kind())
        .collect();
    assert_eq!(
        names,
        vec![
            "pi.user",
            "pi.assistant",
            "pi.system",
            "pi.tool-result",
            "pi.reset",
            "pi.compaction",
        ]
    );
    assert!(USER.is(&entry(9, 2, "pi.user")));
    assert!(!USER.is(&entry(9, 2, "pi.assistant")));
}

#[test]
fn entry_kind_data_reads_fixture_tool_result_diagnostics() {
    #[derive(serde::Deserialize, serde::Serialize, Debug, PartialEq)]
    struct Diagnostics {
        diagnostics: Vec<Value>,
    }

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agent.sqlite");
    if !path.exists() {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    }
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    let mut seen = 0;
    for row in &store.entries {
        let record = entry_from(row);
        if !TOOL_RESULT.is(&record) {
            continue;
        }
        let diagnostics: Diagnostics = TOOL_RESULT
            .require_data(&record)
            .expect("pi.tool-result 的 data 带 diagnostics");
        assert_eq!(
            serde_json::to_value(&diagnostics).unwrap(),
            record.data.clone().unwrap()
        );
        seen += 1;
    }
    assert_eq!(seen, 36, "fixture 里 36 条 pi.tool-result");
}

fn entry_from(row: &SqliteRow) -> EntryRecord {
    match row.get("record") {
        Some(Cell::Text(text)) => serde_json::from_str(text).unwrap(),
        other => panic!("record 列不是文本：{other:?}"),
    }
}

#[test]
fn context_edits_survive_a_fixture_shaped_entry() {
    // `edits` 在 fixture 里没出现（旧引擎不写），所以这里用上游形状的值钉住：
    // 加进 `EntryRecord` 之后往返仍要对得上。
    let record: EntryRecord = serde_json::from_value(json!({
        "id": 9,
        "conversationId": 2,
        "kind": "pi.user",
        "edits": [
            {"action": "omit", "target": 5},
            {"action": "replace", "target": 6, "messages": [{"role": "user", "content": "x"}]},
        ],
    }))
    .unwrap();
    assert_eq!(record.edits.as_ref().unwrap().len(), 2);
    assert_eq!(
        record.edits.as_ref().unwrap()[0],
        ContextEdit::Omit { target: 5 }
    );
    assert_eq!(
        serde_json::to_value(&record).unwrap()["edits"],
        json!([
            {"action": "omit", "target": 5},
            {"action": "replace", "target": 6, "messages": [{"role": "user", "content": "x"}]},
        ])
    );
}
