//! durable 存储的一致性套件：改写自 `packages/durable/src/testing/storage-conformance.ts`。
//!
//! ## 规矩
//!
//! - **案名逐字照抄上游**，一个都不改名 —— 这样对得上行、对得上票，也让「哪些抄了、哪些没抄」
//!   是一张能核对的表（票 #12 里那张）。
//! - 一份**真后端**（临时文件 SQLite）上跑，不造假后端：假后端通过什么也证明不了。
//! - 减法都要有理由：砍掉的案进票 #21，不无声消失。
//!
//! 上游 32 个 `createCase(`，这里 20 个；`FLOOR.tsv` 记的正是这个数字。
//!
//! ## 八个上游文件，为什么这里只有一份规范
//!
//! 上游 `packages/durable/src/testing/` 八个文件里**只有两个是规范**：`storage-conformance.ts`
//! （2,099 行 / 32 个 `createCase(`）与 `env-conformance.ts`（633 行 / 20 个）。其余六个的
//! `createCase(` 调用点是 0（benchmark / 断言 / 注册器 / 类型 / 出口 / 提交辅助），不设下限，
//! 也不为凑文件数造空文件。
//!
//! 于是 32 案 = **20 已落** + **9 等真后端**（`scan_tasks` / `scan_submissions` / `scan_documents` /
//! `close()`，票 #21）+ **3 不做**（对象别名两条 + 不种根会话；理由在票 #21 与 `docs/not-ported.md`）。
//! `env_conformance.rs` **不做**：本库 `nous-env` 只有 trait、没有一个实现，而 conformance 测的是
//! **真后端** —— 造个假后端让用例通过，什么也证明不了（票 #15）。逐文件的判定见 `FLOOR.tsv` 文件头。
//!
//! ## 这个文件不是恒绿：四条变异实测
//!
//! 「20 条全绿」本身不说明套件有牙齿 —— 得有人把被测实现改坏，它必须红。四条现改现跑，改的是
//! `crates/nous-durable/src/storage/**`，跑完立刻 `git checkout` 还原（提交的 diff 里没有这些行，
//! 一个断言都没动过；红全部来自被测实现）：
//!
//! | 变异（改哪一行） | 红的案 |
//! |---|---|
//! | `storage.rs:160` 删掉 `next_seq += 1`（每批不推进 seq） | **8 条**，含 `commits_batches_in_order_each_with_its_seq_checked_against_the_earlier_ones`、`receives_a_durable_commit_once_before_resolving_and_never_a_refused_one`、`reconstructs_rewindable_documents_and_preserves_half_open_incarnations` |
//! | `writes.rs:301-303` 让 `IdWrittenTwice` 永不返回 | **2 条**：`receives_a_durable_commit_once_before_resolving_and_never_a_refused_one`、`stores_the_batches_before_the_first_one_refused_and_none_after_it` |
//! | `documents.rs:427` `entry.deltas_since_base + 1` → `entry.deltas_since_base` | **2 条**：`commits_batches_in_order_each_with_its_seq_checked_against_the_earlier_ones`、`streams_long_document_tails_across_root_replacement_deltas` |
//! | `cursor.rs:89` 让「游标方向与请求方向不一致」不再拒绝 | **1 条**：`scans_tables_in_either_id_order_and_continues_a_cursor_in_its_order` |
//!
//! 行号对的是 `origin/master`（`64dc4de`）上那些行。第一条有 8 条红不是「判据糊」：seq 是每批的
//! 身份，掉了它半张套件都该红；后三条各自只红该红的那一两条，说明它们钉的是具体行为。

use std::path::PathBuf;

use serde_json::{json, Value};

use nous_chord::delta::codec::Encoder;
use nous_chord::delta::diff::diff_revisions;
use nous_chord::sqlite::SqliteExecutor;
use nous_durable::{
    CopySource, DocumentContent, DocumentPoint, Refusal, ScanOrder, Storage, StorageError,
    StorageWrite,
};

// ───────────────────────────── 夹具 ─────────────────────────────

struct Backend {
    path: PathBuf,
    storage: Storage,
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(self.path.with_extension("sqlite-shm"));
    }
}

/// 一份干净的真后端（临时文件库）。
fn backend(tag: &str) -> Backend {
    let path = std::env::temp_dir().join(format!(
        "nous-conformance-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    let storage = Storage::open(&path).expect("open a fresh database");
    Backend { path, storage }
}

fn conversation(id: i64) -> StorageWrite {
    StorageWrite::Conversation(json!({ "id": id }))
}

fn entry(id: i64, conversation_id: i64) -> StorageWrite {
    StorageWrite::Entry(json!({
        "id": id, "conversationId": conversation_id, "kind": "pi.user",
        "content": { "role": "user", "text": format!("entry {id}") }
    }))
}

fn task(id: i64, status: &str) -> StorageWrite {
    StorageWrite::Task(json!({
        "id": id, "conversationId": 2, "kind": "pi.generation",
        "state": { "status": status }
    }))
}

/// 一份 conversation 作用域的文档（只有这个作用域 + `rewindable` 才留历史）。
fn document(id: i64, conversation: i64, kind: &str, key: &str) -> Value {
    json!({
        "id": id, "kind": kind, "key": key,
        "scope": { "kind": "conversation", "conversationId": conversation },
        "history": "rewindable", "fork": "asOf"
    })
}

fn create_document(id: i64, conversation: i64, kind: &str, key: &str, value: Value) -> StorageWrite {
    StorageWrite::DocumentCreate {
        id,
        record: document(id, conversation, kind, key),
        content: DocumentContent::Base { version: 1, value },
    }
}

fn delta(id: i64, version: i64, before: &Value, after: &Value) -> StorageWrite {
    let trusted = diff_revisions(before, after);
    StorageWrite::DocumentChange {
        id,
        content: DocumentContent::Delta {
            version,
            ops: Value::Array(Encoder::new().encode(trusted.as_ops())),
        },
    }
}

fn revision_count(storage: &Storage, id: i64) -> i64 {
    storage
        .store()
        .db()
        .get(
            "SELECT COUNT(*) AS n FROM document_revisions WHERE document_id = ?",
            &[id.into()],
        )
        .unwrap()
        .unwrap()
        .get("n")
        .unwrap()
        .as_i64()
        .unwrap()
}

fn count(storage: &Storage, sql: &str, params: &[nous_chord::sqlite::SqliteValue]) -> i64 {
    storage
        .store()
        .db()
        .get(sql, params)
        .unwrap()
        .unwrap()
        .get("n")
        .unwrap()
        .as_i64()
        .unwrap()
}

// ───────────────────────────── 上游的 32 个 createCase( 里已抄的 20 个 ─────────────────────────────

#[test]
fn receives_a_durable_commit_once_before_resolving_and_never_a_refused_one() {
    // 上游 case 2：回执每批一次、只在提交成功时来，被拒的批没有回执。
    let mut fixture = backend("receipts");
    let mut receipts: Vec<(usize, i64)> = Vec::new();
    let result = fixture
        .storage
        .commit(
            &[
                vec![conversation(2)],
                vec![conversation(3)],
                vec![conversation(4), conversation(4)],
            ],
            |index, seq| receipts.push((index, seq)),
        )
        .unwrap();
    assert_eq!(result.stored, 2);
    assert!(result.refused.is_some());
    // 正好两批、按顺序、每批一个 seq；被拒的那批没有回执。
    assert_eq!(receipts, vec![(0, 1), (1, 2)]);
}

#[test]
fn commits_mixed_table_writes_atomically_and_rolls_all_of_them_back_on_failure() {
    // 上游 case 3：一个事务里多张表一起写、一起回滚。
    let mut fixture = backend("atomic");
    let before = fixture.storage.store().metadata().unwrap();
    let error = fixture
        .storage
        .commit(
            &[
                vec![
                    conversation(2),
                    entry(3, 2),
                    create_document(4, 2, "pi.agent", "live", json!({ "a": 1 })),
                ],
                // 第二批撞 `tasks.status` 的 CHECK：数据库失败 = 整次回滚。
                vec![task(5, "not-a-status")],
            ],
            |_, _| panic!("整次失败时不该有回执"),
        )
        .unwrap_err();
    assert!(matches!(error, StorageError::Sqlite(_)), "{error}");
    assert!(fixture.storage.conversation(2).unwrap().is_none());
    assert!(fixture.storage.entry(3).unwrap().is_none());
    assert!(fixture.storage.document(4).unwrap().is_none());
    assert_eq!(revision_count(&fixture.storage, 4), 0);
    assert_eq!(count(&fixture.storage, "SELECT COUNT(*) AS n FROM record_ids", &[]), 0);
    assert_eq!(fixture.storage.store().metadata().unwrap(), before);
}

#[test]
fn indexes_entries_committed_out_of_id_order() {
    // 上游 case 6：一批里乱序提交，读出来必须是 id 序。
    let mut fixture = backend("out-of-order");
    fixture
        .storage
        .commit(&[vec![conversation(2), entry(5, 2), entry(3, 2), entry(4, 2)]], |_, _| {})
        .unwrap();
    let page = fixture
        .storage
        .scan_entries(
            &nous_durable::EntryQuery {
                conversation_id: 2,
                min_entry_id: None,
                max_entry_id: None,
                order: Some(ScanOrder::Ascending),
            },
            10,
            None,
        )
        .unwrap()
        .unwrap();
    let ids: Vec<i64> = page.items.iter().map(|item| item["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![3, 4, 5]);
}

#[test]
fn continues_an_entry_cursor_below_its_last_item_after_a_newer_commit() {
    // 上游 case 7：翻页途中来了新提交，旧游标接着往下走、不跳不重。
    let mut fixture = backend("cursor-entry");
    fixture
        .storage
        .commit(&[vec![conversation(2), entry(3, 2), entry(4, 2), entry(5, 2)]], |_, _| {})
        .unwrap();
    let query = || nous_durable::EntryQuery {
        conversation_id: 2,
        min_entry_id: None,
        max_entry_id: None,
        order: None, // 默认降序：从最新的往回翻
    };
    let first = fixture.storage.scan_entries(&query(), 2, None).unwrap().unwrap();
    let first_ids: Vec<i64> = first.items.iter().map(|item| item["id"].as_i64().unwrap()).collect();
    assert_eq!(first_ids, vec![5, 4]);

    // 新提交插进来（id 更大 = 更新）。
    fixture
        .storage
        .commit(&[vec![entry(6, 2)]], |_, _| {})
        .unwrap();

    // 旧游标接着往下：不重复 5/4，也不把 6 塞进来。
    let second = fixture
        .storage
        .scan_entries(&query(), 2, Some(&first.next.unwrap()))
        .unwrap()
        .unwrap();
    let second_ids: Vec<i64> = second.items.iter().map(|item| item["id"].as_i64().unwrap()).collect();
    assert_eq!(second_ids, vec![3]);
}

#[test]
fn paginates_conversations_by_opaque_cursor_in_ascending_id_order() {
    // 上游 case 8：升序翻页，游标是不透明文本，末页不给游标。
    let mut fixture = backend("cursor-conversation");
    let batches = vec![(2..8).map(conversation).collect::<Vec<StorageWrite>>()];
    fixture.storage.commit(&batches, |_, _| {}).unwrap();

    let mut seen: Vec<i64> = Vec::new();
    let mut cursor = None;
    loop {
        let page = fixture
            .storage
            .scan_conversations(&nous_durable::ConversationQuery::default(), 2, cursor.as_ref())
            .unwrap()
            .unwrap();
        seen.extend(page.items.iter().map(|item| item["id"].as_i64().unwrap()));
        match page.next {
            Some(next) => {
                // 不透明：只有方向 + base36 id，别的什么都没有。
                let text = next.encode();
                assert!(text.chars().all(|c| c.is_ascii_alphanumeric()), "{text}");
                cursor = Some(next);
            }
            None => break,
        }
    }
    assert_eq!(seen, vec![2, 3, 4, 5, 6, 7]);
}

#[test]
fn filters_and_pages_conversations_by_durable_owner_edges() {
    // 上游 case 9：owner 两列既是过滤条件，也参与翻页。
    let mut fixture = backend("owner-edges");
    let owned = |id: i64, task: i64| {
        StorageWrite::Conversation(json!({
            "id": id, "owner": { "conversationId": 2, "taskId": task }
        }))
    };
    let batches = vec![vec![
        conversation(2),
        owned(10, 7),
        owned(11, 7),
        owned(12, 8),
        StorageWrite::Conversation(json!({ "id": 13 })),
    ]];
    fixture.storage.commit(&batches, |_, _| {}).unwrap();

    let by_task = nous_durable::ConversationQuery {
        owner_task_id: Some(7),
        ..nous_durable::ConversationQuery::default()
    };
    let first = fixture.storage.scan_conversations(&by_task, 1, None).unwrap().unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0]["id"], json!(10));
    let second = fixture
        .storage
        .scan_conversations(&by_task, 1, first.next.as_ref())
        .unwrap()
        .unwrap();
    assert_eq!(second.items[0]["id"], json!(11));
    assert_eq!(second.next, None, "只有两个，翻完就该没有游标");

    let by_conversation = nous_durable::ConversationQuery {
        owner_conversation_id: Some(2),
        ..nous_durable::ConversationQuery::default()
    };
    assert_eq!(
        fixture
            .storage
            .scan_conversations(&by_conversation, 10, None)
            .unwrap()
            .unwrap()
            .items
            .len(),
        3
    );
}

#[test]
fn scans_tables_in_either_id_order_and_continues_a_cursor_in_its_order() {
    // 上游 case 11：两个方向都能扫，游标记着自己属于哪个方向。
    let mut fixture = backend("both-orders");
    fixture
        .storage
        .commit(&[(2..6).map(conversation).collect::<Vec<StorageWrite>>()], |_, _| {})
        .unwrap();

    let ascending = nous_durable::ConversationQuery::default();
    let descending = nous_durable::ConversationQuery {
        order: Some(ScanOrder::Descending),
        ..nous_durable::ConversationQuery::default()
    };
    let up = fixture.storage.scan_conversations(&ascending, 2, None).unwrap().unwrap();
    assert_eq!(up.items[0]["id"], json!(2));
    let down = fixture.storage.scan_conversations(&descending, 2, None).unwrap().unwrap();
    assert_eq!(down.items[0]["id"], json!(5));

    // 上游说：方向与游标不符 = 明说，不猜。
    assert_eq!(
        fixture
            .storage
            .scan_conversations(&descending, 2, up.next.as_ref())
            .unwrap(),
        Err(Refusal::CursorOrderMismatch)
    );
}

#[test]
fn reconstructs_rewindable_documents_and_preserves_half_open_incarnations() {
    // 上游 case 18：base + delta 重放，在世区间左闭右开。
    let mut fixture = backend("rewindable");
    let base = json!({ "text": "第一版" });
    fixture
        .storage
        .commit(&[vec![create_document(10, 2, "pi.agent", "live", base.clone())]], |_, _| {})
        .unwrap();
    let second = json!({ "text": "第二版" });
    fixture
        .storage
        .commit(&[vec![delta(10, 1, &base, &second)]], |_, _| {})
        .unwrap();
    fixture
        .storage
        .commit(&[vec![StorageWrite::DocumentRetire { id: 10 }]], |_, _| {})
        .unwrap();

    assert!(fixture.storage.materialize(10, DocumentPoint::Current).unwrap().is_none());
    // seq 1 = base，seq 2 = delta，seq 3 = 退役（那一格已经不在世）。
    assert_eq!(
        fixture.storage.materialize(10, DocumentPoint::At(1)).unwrap().unwrap().value,
        base
    );
    assert_eq!(
        fixture.storage.materialize(10, DocumentPoint::At(2)).unwrap().unwrap().value,
        second
    );
    assert!(fixture.storage.materialize(10, DocumentPoint::At(3)).unwrap().is_none());
}

#[test]
fn streams_long_document_tails_across_root_replacement_deltas() {
    // 上游 case 19：长尾 + 中途一条整根替换（恢复点）之后，后面的 delta 仍然自给自足。
    let mut fixture = backend("long-tails");
    let mut value = json!({ "text": "a".repeat(4096) });
    fixture
        .storage
        .commit(&[vec![create_document(20, 2, "pi.agent", "live", value.clone())]], |_, _| {})
        .unwrap();

    // 500 条 delta：每条追加一段，中途一条整根替换。
    for step in 0..500 {
        let next = if step == 250 {
            json!({ "text": "replaced".to_string() })
        } else {
            let text = value["text"].as_str().unwrap();
            json!({ "text": format!("{text}{step:03}") })
        };
        fixture
            .storage
            .commit(&[vec![delta(20, 1, &value, &next)]], |_, _| {})
            .unwrap();
        value = next;
    }
    let read = fixture.storage.materialize(20, DocumentPoint::Current).unwrap().unwrap();
    assert_eq!(read.value, value, "重放必须和一路上写下的最后一个值相等");
    assert_eq!(read.deltas_since_base, 500);
    assert_eq!(read.version, 1, "delta 不换版本");
}

#[test]
fn copies_stored_document_bases_independently_and_rejects_ambiguous_sources() {
    // 上游 case 20：复制是**独立的一份 base**，且「同一批里改源」= 含糊，拒绝。
    let mut fixture = backend("copies");
    let base = json!({ "text": "源" });
    fixture
        .storage
        .commit(&[vec![create_document(30, 2, "pi.agent", "live", base.clone())]], |_, _| {})
        .unwrap();
    let changed = json!({ "text": "源改" });
    fixture
        .storage
        .commit(&[vec![delta(30, 1, &base, &changed)]], |_, _| {})
        .unwrap();

    fixture
        .storage
        .commit(
            &[vec![StorageWrite::DocumentCopy {
                id: 31,
                record: document(31, 3, "pi.agent", "live"),
                source: CopySource { id: 30, at: DocumentPoint::Current },
            }]],
            |_, _| {},
        )
        .unwrap();
    let copy = fixture.storage.materialize(31, DocumentPoint::Current).unwrap().unwrap();
    assert_eq!(copy.value, changed);
    assert_eq!(copy.deltas_since_base, 0, "复制出来的是一条 base，不共享 delta 链");
    assert_eq!(revision_count(&fixture.storage, 31), 1);

    // 复制之后再改源：副本不动。
    let source_later = json!({ "text": "源又改" });
    fixture
        .storage
        .commit(&[vec![delta(30, 1, &changed, &source_later)]], |_, _| {})
        .unwrap();
    assert_eq!(
        fixture.storage.materialize(31, DocumentPoint::Current).unwrap().unwrap().value,
        changed
    );

    // 含糊的源：同一批里改它。
    let result = fixture
        .storage
        .commit(
            &[vec![
                delta(30, 1, &source_later, &json!({ "text": "同批" })),
                StorageWrite::DocumentCopy {
                    id: 32,
                    record: document(32, 3, "pi.agent", "live"),
                    source: CopySource { id: 30, at: DocumentPoint::Current },
                },
            ]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 0);
    assert_eq!(result.refused, Some(Refusal::CopySourceChanged { document_id: 32 }));
}

#[test]
fn uses_bases_for_version_transitions_and_rejects_historical_reads_of_current_only_documents() {
    // 上游 case 21：换版本必须先来一条 base；只留最新态的文档读过去 = 拒绝。
    let mut fixture = backend("versions");
    let v1 = json!({ "text": "v1" });
    fixture
        .storage
        .commit(&[vec![create_document(40, 2, "pi.agent", "live", v1.clone())]], |_, _| {})
        .unwrap();

    // 不换 base 就想把版本从 1 推到 2：拒绝。
    let result = fixture
        .storage
        .commit(&[vec![delta(40, 2, &v1, &json!({ "text": "v2" }))]], |_, _| {})
        .unwrap();
    assert!(matches!(
        result.refused,
        Some(Refusal::DeltaVersionMismatch { document_id: 40, previous: 1, wanted: 2 })
    ));

    // 先来 base v2，再在 2 上叠 delta：放行。
    let v2 = json!({ "text": "v2" });
    fixture
        .storage
        .commit(
            &[vec![StorageWrite::DocumentChange {
                id: 40,
                content: DocumentContent::Base { version: 2, value: v2.clone() },
            }]],
            |_, _| {},
        )
        .unwrap();
    let result = fixture
        .storage
        .commit(&[vec![delta(40, 2, &v2, &json!({ "text": "v2+" }))]], |_, _| {})
        .unwrap();
    assert_eq!(result.stored, 1, "{:?}", result.refused);
    assert_eq!(
        fixture.storage.materialize(40, DocumentPoint::Current).unwrap().unwrap().version,
        2
    );

    // 只留最新态（session 作用域）的文档：读过去明确失败。
    let record = json!({
        "id": 41, "kind": "pi.agent", "scope": { "kind": "session" },
        "history": "latest", "fork": "current"
    });
    fixture
        .storage
        .commit(
            &[vec![StorageWrite::DocumentCreate {
                id: 41,
                record,
                content: DocumentContent::Base { version: 1, value: json!({ "x": 1 }) },
            }]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(
        fixture.storage.materialize(41, DocumentPoint::At(1)).unwrap_err(),
        StorageError::HistoryNotRetained { document_id: 41 }
    );
}

#[test]
fn keeps_document_lifecycle_failures_atomic_and_gives_create_plus_retire_an_empty_lifetime() {
    // 上游 case 23：创建 + 退役在同一批 = 空生命期；生命周期里的失败整批原子。
    let mut fixture = backend("lifecycle");
    let result = fixture
        .storage
        .commit(
            &[vec![
                create_document(50, 2, "pi.agent", "live", json!({ "x": 1 })),
                StorageWrite::DocumentRetire { id: 50 },
            ]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 1, "{:?}", result.refused);
    let record = fixture.storage.document(50).unwrap().unwrap();
    assert_eq!(record["createdAt"], json!(1));
    assert_eq!(record["retiredAt"], json!(1), "空生命期：创建和退役同一个 seq");
    assert!(fixture.storage.materialize(50, DocumentPoint::Current).unwrap().is_none());
    assert!(fixture.storage.materialize(50, DocumentPoint::At(1)).unwrap().is_none());

    // 生命周期里的数据库失败：前面那个文档（连同内容）一起回滚。
    let error = fixture
        .storage
        .commit(
            &[
                vec![create_document(51, 2, "pi.agent", "other", json!({ "x": 2 }))],
                vec![StorageWrite::DocumentRetire { id: 51 }, task(52, "bogus")],
            ],
            |_, _| panic!("整次失败时不该有回执"),
        )
        .unwrap_err();
    assert!(matches!(error, StorageError::Sqlite(_)), "{error}");
    assert!(fixture.storage.document(51).unwrap().is_none());
    assert_eq!(revision_count(&fixture.storage, 51), 0);
    assert!(fixture.storage.document(50).unwrap().is_some(), "上一轮的不受影响");
}

#[test]
fn refuses_a_creation_and_a_change_of_one_document_in_one_commit_in_either_order() {
    // 上游 case 24：两种顺序都不许。
    let mut fixture = backend("create-plus-change");
    let value = json!({ "x": 1 });
    for (label, writes) in [
        (
            "create then change",
            vec![
                create_document(60, 2, "pi.agent", "live", value.clone()),
                StorageWrite::DocumentChange {
                    id: 60,
                    content: DocumentContent::Base { version: 1, value: json!({ "x": 2 }) },
                },
            ],
        ),
        (
            "change then create",
            vec![
                StorageWrite::DocumentChange {
                    id: 60,
                    content: DocumentContent::Base { version: 1, value: json!({ "x": 2 }) },
                },
                create_document(60, 2, "pi.agent", "live", value.clone()),
            ],
        ),
    ] {
        let result = fixture.storage.commit(&[writes], |_, _| {}).unwrap();
        assert_eq!(result.stored, 0, "{label}");
        assert_eq!(
            result.refused,
            Some(Refusal::DuplicateContent { document_id: 60 }),
            "{label}"
        );
        assert!(fixture.storage.document(60).unwrap().is_none(), "{label}");
    }
}

#[test]
fn rolls_back_record_tables_and_secondary_indexes_when_a_document_command_fails() {
    // 上游 case 25：文档这条命令失败时，记录表与二级索引（`record_ids`）一起回滚。
    let mut fixture = backend("rollback-indexes");
    let error = fixture
        .storage
        .commit(
            &[
                vec![conversation(2), create_document(61, 2, "pi.agent", "live", json!({ "x": 1 }))],
                vec![create_document(62, 2, "pi.agent", "second", json!({ "x": 2 })), task(63, "bogus")],
            ],
            |_, _| panic!("整次失败时不该有回执"),
        )
        .unwrap_err();
    assert!(matches!(error, StorageError::Sqlite(_)), "{error}");
    assert!(fixture.storage.conversation(2).unwrap().is_none());
    assert!(fixture.storage.document(61).unwrap().is_none());
    assert!(fixture.storage.document(62).unwrap().is_none());
    assert_eq!(revision_count(&fixture.storage, 61), 0);
    assert_eq!(count(&fixture.storage, "SELECT COUNT(*) AS n FROM record_ids", &[]), 0);
}

#[test]
fn keeps_indexed_string_identities_lossless() {
    // 上游 case 26：索引列里的字符串身份不能撞 —— `"a.b"` 与 `["a","b"]` 是两样东西。
    let mut fixture = backend("string-identity");
    let batches = vec![vec![
        create_document(70, 2, "pi.agent", "a.b", json!({ "x": 1 })),
        create_document(71, 2, "pi.agent", "a\u{0}.b", json!({ "x": 2 })),
    ]];
    fixture.storage.commit(&batches, |_, _| {}).unwrap();

    let keys: Vec<(String, String)> = fixture
        .storage
        .store()
        .db()
        .all("SELECT id, kind || '|' || key_value AS k FROM documents ORDER BY id", &[])
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.get("id").unwrap().as_i64().unwrap().to_string(),
                row.get("k").unwrap().as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(keys.len(), 2);
    // 两条都在世（地址没撞），索引里也是两个不同的键。
    assert_ne!(keys[0].1, keys[1].1, "{keys:?}");
    assert!(keys[0].1.contains("\"a.b\""), "{keys:?}");
}

#[test]
fn keeps_one_global_record_id_namespace_and_rejects_exhausted_id_minting() {
    // 上游 case 27：id 命名空间全局唯一；id 空间用尽要明说。
    let mut fixture = backend("id-namespace");
    fixture.storage.commit(&[vec![conversation(2)]], |_, _| {}).unwrap();
    let result = fixture.storage.commit(&[vec![entry(2, 2)]], |_, _| {}).unwrap();
    assert_eq!(
        result.refused,
        Some(Refusal::IdAlreadyBelongs { id: 2, table: nous_durable::TableName::Conversation })
    );

    // 把计数器拨到 i64::MAX：下一次分配必须失败，不许回绕成负数。
    fixture
        .storage
        .store()
        .db()
        .run(
            &format!("UPDATE durable_metadata SET next_id = '{}' WHERE singleton = 1", i64::MAX),
            &[],
        )
        .unwrap();
    let mut reopened = Storage::open(&fixture.path).unwrap();
    assert_eq!(reopened.next_id(), i64::MAX);
    assert!(matches!(reopened.mint_id(), Err(StorageError::Shape(_))));
}

#[test]
fn commits_batches_in_order_each_with_its_seq_checked_against_the_earlier_ones() {
    // 上游 case 28：一批一个 seq，后一批看得到前一批的效果（这里靠 delta 的 base 版本）。
    let mut fixture = backend("ordered-batches");
    let v1 = json!({ "text": "v1" });
    let v2 = json!({ "text": "v2" });
    let v3 = json!({ "text": "v3" });
    let result = fixture
        .storage
        .commit(
            &[
                vec![create_document(80, 2, "pi.agent", "live", v1.clone())],
                vec![delta(80, 1, &v1, &v2)],
                vec![delta(80, 1, &v2, &v3)],
            ],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 3);
    let read = fixture.storage.materialize(80, DocumentPoint::Current).unwrap().unwrap();
    assert_eq!(read.value, v3);
    assert_eq!(read.deltas_since_base, 2);
    // 每一批的 seq 就是它落在 revision 上的 seq。
    let seqs: Vec<i64> = fixture
        .storage
        .store()
        .db()
        .all("SELECT seq FROM document_revisions WHERE document_id = 80 ORDER BY seq", &[])
        .unwrap()
        .iter()
        .map(|row| row.get("seq").unwrap().as_i64().unwrap())
        .collect();
    assert_eq!(seqs, vec![1, 2, 3]);
}

#[test]
fn stores_the_batches_before_the_first_one_refused_and_none_after_it() {
    // 上游 case 29：第 N 批被拒 → 前 N−1 批留下，之后的批一个都不碰。
    let mut fixture = backend("refused-stop");
    let result = fixture
        .storage
        .commit(
            &[
                vec![conversation(2)],
                vec![conversation(3)],
                vec![conversation(4), conversation(4)],
                vec![conversation(5)],
                vec![conversation(6)],
            ],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 2);
    assert_eq!(result.refused, Some(Refusal::IdWrittenTwice { id: 4 }));
    assert!(fixture.storage.conversation(2).unwrap().is_some());
    assert!(fixture.storage.conversation(3).unwrap().is_some());
    for id in [4, 5, 6] {
        assert!(fixture.storage.conversation(id).unwrap().is_none(), "{id}");
    }
}

#[test]
fn refuses_a_later_batch_its_own_malformed_actions_fail_keeping_the_batches_before_it() {
    // 上游 case 30：第二批自己的动作不合法（两条内容命令）→ 只拒它，第一批留下。
    let mut fixture = backend("malformed-later");
    let result = fixture
        .storage
        .commit(
            &[
                vec![create_document(90, 2, "pi.agent", "live", json!({ "x": 1 }))],
                vec![
                    StorageWrite::DocumentChange {
                        id: 90,
                        content: DocumentContent::Base { version: 2, value: json!({ "x": 2 }) },
                    },
                    StorageWrite::DocumentChange {
                        id: 90,
                        content: DocumentContent::Base { version: 3, value: json!({ "x": 3 }) },
                    },
                ],
            ],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 1);
    assert_eq!(result.refused, Some(Refusal::DuplicateContent { document_id: 90 }));
    let read = fixture.storage.materialize(90, DocumentPoint::Current).unwrap().unwrap();
    assert_eq!(read.value, json!({ "x": 1 }), "第一批的东西原样在");
    assert_eq!(read.version, 1);
}

#[test]
fn resolves_a_batched_copy_against_the_state_the_earlier_batches_left() {
    // 上游 case 31：同一批里的复制读的是**前面批已经落下的**状态。
    let mut fixture = backend("copy-against-earlier");
    let v1 = json!({ "text": "v1" });
    let v2 = json!({ "text": "v2" });
    fixture
        .storage
        .commit(&[vec![create_document(100, 2, "pi.agent", "live", v1.clone())]], |_, _| {})
        .unwrap();
    let result = fixture
        .storage
        .commit(
            &[
                vec![delta(100, 1, &v1, &v2)],
                vec![StorageWrite::DocumentCopy {
                    id: 101,
                    record: document(101, 3, "pi.agent", "live"),
                    source: CopySource { id: 100, at: DocumentPoint::Current },
                }],
            ],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(result.stored, 2, "{:?}", result.refused);
    // 复制看到的是第一批之后的 v2，不是提交之前的 v1。
    assert_eq!(
        fixture.storage.materialize(101, DocumentPoint::Current).unwrap().unwrap().value,
        v2
    );
}
