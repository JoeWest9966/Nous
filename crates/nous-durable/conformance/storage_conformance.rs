//! durable 存储的一致性套件：改写自 `packages/durable/src/testing/storage-conformance.ts`。
//!
//! ## 规矩
//!
//! - **案名逐字照抄上游**，一个都不改名 —— 这样对得上行、对得上票，也让「哪些抄了、哪些没抄」
//!   是一张能核对的表（票 #12 里那张）。
//! - 一份**真后端**（临时文件 SQLite）上跑，不造假后端：假后端通过什么也证明不了。
//! - 减法都要有理由：砍掉的案进票 #21，不无声消失。
//!
//! 上游 32 个 `createCase(`，这里 29 个；`FLOOR.tsv` 记的正是这个数字。
//!
//! ## 八个上游文件，为什么这里只有一份规范
//!
//! 上游 `packages/durable/src/testing/` 八个文件里**只有两个是规范**：`storage-conformance.ts`
//! （2,099 行 / 32 个 `createCase(`）与 `env-conformance.ts`（633 行 / 20 个）。其余六个的
//! `createCase(` 调用点是 0（benchmark / 断言 / 注册器 / 类型 / 出口 / 提交辅助），不设下限，
//! 也不为凑文件数造空文件。
//!
//! 于是 32 案 = **29 已落** + **3 不做**（对象别名两条 + 不种根会话；理由在票 #21 与
//! `docs/not-ported.md`）。**清单清零了**：`#21` 之前那 9 条「等真后端」随 `scan_tasks` /
//! `scan_submissions` / `scan_documents` / fork 链 entry 扫描 / `close()` 落地全部抄进来，案名
//! 一个都没改。
//! `env_conformance.rs` **不做**：本库 `nous-env` 只有 trait、没有一个实现，而 conformance 测的是
//! **真后端** —— 造个假后端让用例通过，什么也证明不了（票 #15）。逐文件的判定见 `FLOOR.tsv` 文件头。
//!
//! ## 这个文件不是恒绿：十条变异实测
//!
//! 「29 条全绿」本身不说明套件有牙齿 —— 得有人把被测实现改坏，它必须红。十条现改现跑，改的是
//! `crates/nous-durable/src/**`，跑完立刻还原（提交的 diff 里没有这些行，一个断言都没动过；红全部
//! 来自被测实现）：
//!
//! | 变异（改哪一行） | 红的案 |
//! |---|---|
//! | `storage.rs` 删掉 `next_seq += 1`（每批不推进 seq） | **8 条**，含 `commits_batches_in_order_each_with_its_seq_checked_against_the_earlier_ones`、`receives_a_durable_commit_once_before_resolving_and_never_a_refused_one`、`reconstructs_rewindable_documents_and_preserves_half_open_incarnations` |
//! | `writes.rs` 让 `IdWrittenTwice` 永不返回 | **2 条**：`receives_a_durable_commit_once_before_resolving_and_never_a_refused_one`、`stores_the_batches_before_the_first_one_refused_and_none_after_it` |
//! | `documents.rs` `entry.deltas_since_base + 1` → `entry.deltas_since_base` | **2 条**：`commits_batches_in_order_each_with_its_seq_checked_against_the_earlier_ones`、`streams_long_document_tails_across_root_replacement_deltas` |
//! | `cursor.rs` 让「游标方向与请求方向不一致」不再拒绝 | **1 条**：`scans_tables_in_either_id_order_and_continues_a_cursor_in_its_order` |
//! | `storage.rs` 让 `ensure_open` 永不返回 `Closed` | **1 条**：`rejects_every_operation_after_close` |
//! | `scans.rs` 让 `scan_tasks` 丢掉 `status` 过滤 | **3 条**：`replaces_complete_task_records_and_pages_filtered_task_scans`、`stores_owners_and_scans_waiting_and_completing_tasks_by_status`、`stores_and_scans_sleeping_tasks_by_status` |
//! | `scans.rs` `submission_by_request` 拿原文而不是编码体去比 `request_id` | **3 条**：`indexes_request_ids_…`、`stores_passive_write_submissions_…`、`stores_continue_submissions_…` |
//! | `scans.rs` `find_document` 的 WHERE 去掉 `key_value` | **1 条**：`indexes_logical_addresses_and_exact_scope_scans_independently` |
//! | `scans.rs` `find_document` 的 WHERE 去掉 `family` | **1 条**：同上 |
//! | `scans.rs` 降序链去掉 `upper = min(upper, parent.at)` | **1 条**：`scans_deep_fork_history_newest_first_through_every_ancestor_cap` |
//!
//! 两条**不红**的，如实记下来（它们不是失败，是「这条案测不到这个面」）：
//!
//! - `scans.rs` 的 `scan_documents` 去掉 `owner_id` 过滤 → 全绿。**原因**：夹具里每个 `scope_kind`
//!   只有一个 owner（`session` 恒为 0、`conversation` 只有 2、`task` 只有 11），少比一列不改变结果。
//!   要钉住它得再造一条同 kind 不同 owner 的作用域，那是上游这条案**没有**的形状 —— 记着，不编。
//! - 第一版 `indexes_logical_addresses_…` 只断言了 `__proto__` 一处 `find_document`，那时把
//!   `key_value` 从 WHERE 里拿掉**也是全绿**：两份文档是**同一次提交**建的，`created_at` 打平，
//!   `ORDER BY created_at DESC` 恰好还是先建的那份。补上 `constructor` 与 `key: ""` 两个地址之后才红
//!   —— 「绿」和「有牙齿」的差别就在这一处，是**变异测试**找出来的，不是读代码看出来的。

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

// ───────────────────────────── 上游的 32 个 createCase( 里已抄的案 ─────────────────────────────

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

#[test]
fn rejects_every_operation_after_close() {
    // 上游 case 32（`rejects every operation after close`）：关掉之后，读、提交、取 id 全都明确失败。
    //
    // 上游的 `close()` 不消费自己，所以「关完之后还调它」是一条能写的测试；本库照抄这个形状 ——
    // `close()` 只置一个门闩（`StorageError::Closed`），不消费 `self`。消费掉 `self` 的写法根本
    // 测不到这条语义，也就不该声称支持它。
    let mut fixture = backend("closed");
    fixture.storage.commit(&[vec![conversation(2)]], |_, _| {}).unwrap();

    fixture.storage.close().unwrap();

    let error = fixture.storage.conversation(2).unwrap_err();
    assert!(matches!(error, StorageError::Closed), "{error}");
    assert!(error.to_string().contains("closed"), "{error}");

    let error = fixture
        .storage
        .commit(&[vec![conversation(3)]], |_, _| panic!("关掉的存储不该有回执"))
        .unwrap_err();
    assert!(matches!(error, StorageError::Closed), "{error}");

    let error = fixture.storage.mint_id().unwrap_err();
    assert!(matches!(error, StorageError::Closed), "{error}");

    // 关掉之后「再关一次」不是错误：已经关了就是已经关了。
    fixture.storage.close().unwrap();
}

/// 上游的 `pendingTask(id, conversationId)`：一条待跑的任务，形状逐字段照抄。
fn pending_task(id: i64, conversation_id: i64) -> Value {
    json!({
        "id": id,
        "conversationId": conversation_id,
        "kind": "test.task",
        "version": 1,
        "input": { "value": id },
        "state": { "status": "pending", "checkpoint": { "phase": "ready" } },
        "background": false,
        "abortRequested": false
    })
}

fn ids_of(page: &nous_durable::Page<Value>) -> Vec<i64> {
    ids_in(&page.items)
}

fn ids_in(items: &[Value]) -> Vec<i64> {
    items
        .iter()
        .map(|record| record["id"].as_i64().unwrap())
        .collect()
}

/// 按状态扫一页任务（上游那几条案里最常见的形状）。写成 `fn` 而不是闭包：闭包会借着
/// `fixture.storage`，而后面的 `commit` 要 `&mut` 它。
fn tasks_with_status(storage: &Storage, status: &str) -> Vec<Value> {
    let query = nous_durable::TaskQuery {
        status: Some(status.to_string()),
        ..Default::default()
    };
    tasks_of(storage, query, 10, None).items
}

fn tasks_of(
    storage: &Storage,
    query: nous_durable::TaskQuery,
    limit: usize,
    cursor: Option<&nous_durable::Cursor>,
) -> nous_durable::Page<Value> {
    storage.scan_tasks(&query, limit, cursor).unwrap().unwrap()
}

#[test]
fn replaces_complete_task_records_and_pages_filtered_task_scans() {
    // 上游 case：整条任务记录（含 `memos` / `background` / `abortRequested`）被后写的覆盖，不是合并；
    // 扫描按状态 / 后台 / 中止标记过滤，并且能翻页。
    let mut fixture = backend("task-replace");
    let (first_id, second_id, third_id) = (11, 12, 13);
    fixture
        .storage
        .commit(
            &[vec![
                conversation(2),
                StorageWrite::Task({
                    let mut record = pending_task(first_id, 2);
                    record["memos"] = json!({ "winner": "first" });
                    record
                }),
                StorageWrite::Task({
                    let mut record = pending_task(second_id, 2);
                    record["background"] = json!(true);
                    record
                }),
                StorageWrite::Task({
                    let mut record = pending_task(third_id, 2);
                    record["abortRequested"] = json!(true);
                    record
                }),
            ]],
            |_, _| {},
        )
        .unwrap();

    // 覆盖成 running：`state` 整块换掉，`abortRequested` 换成 true。
    let running = {
        let mut record = pending_task(first_id, 2);
        record["state"] = json!({ "status": "running", "checkpoint": { "phase": "effect", "attempt": 1 } });
        record["abortRequested"] = json!(true);
        record
    };
    fixture
        .storage
        .commit(&[vec![StorageWrite::Task(running.clone())]], |_, _| {})
        .unwrap();
    assert_eq!(fixture.storage.task(first_id).unwrap().unwrap(), running);

    // 再覆盖成 terminal：这一条是新记录，不是与上面合并。
    let terminal = json!({
        "id": first_id,
        "conversationId": 2,
        "kind": "test.task",
        "version": 1,
        "input": { "value": first_id },
        "state": { "status": "terminal", "outcome": { "status": "completed", "result": { "entryId": 99 } } },
        "background": false,
        "abortRequested": true
    });
    fixture
        .storage
        .commit(&[vec![StorageWrite::Task(terminal.clone())]], |_, _| {})
        .unwrap();
    assert_eq!(fixture.storage.task(first_id).unwrap().unwrap(), terminal);

    // 分页：pending 只剩 12 和 13，一页一条。
    let pending = nous_durable::TaskQuery {
        status: Some("pending".to_string()),
        ..Default::default()
    };
    let first_page = tasks_of(&fixture.storage, pending.clone(), 1, None);
    assert_eq!(ids_of(&first_page), vec![second_id]);
    let next = first_page.next.expect("还有第二条");
    let second_page = tasks_of(&fixture.storage, pending, 1, Some(&next));
    assert_eq!(ids_of(&second_page), vec![third_id]);
    assert!(second_page.next.is_none());

    // 状态 + 中止标记一起过滤：只剩 terminal 那条。
    let terminal_query = nous_durable::TaskQuery {
        status: Some("terminal".to_string()),
        abort_requested: Some(true),
        ..Default::default()
    };
    let page = tasks_of(&fixture.storage, terminal_query, 10, None);
    assert_eq!(page.items, vec![terminal]);

    // 后台标记是一个独立的索引列，不跟着状态走。
    let background_query = nous_durable::TaskQuery {
        background: Some(true),
        ..Default::default()
    };
    let page = tasks_of(&fixture.storage, background_query, 10, None);
    assert_eq!(ids_of(&page), vec![second_id]);
}

#[test]
fn stores_owners_and_scans_waiting_and_completing_tasks_by_status() {
    // 上游 case：`owner` 边与两种中间状态（`waiting` / `completing`）能存能扫；
    // 状态一变，它就从这个状态的扫描里消失、出现在新状态的扫描里。
    let mut fixture = backend("task-owner");
    let (owner_id, waiting_id, completing_id) = (11, 12, 13);
    let owner = pending_task(owner_id, 2);
    let waiting = json!({
        "id": waiting_id,
        "conversationId": 2,
        "kind": "test.task",
        "version": 1,
        "input": { "value": waiting_id },
        "owner": owner_id,
        "state": {
            "status": "waiting",
            "checkpoint": { "phase": "next" },
            "on": [owner_id],
            "policy": "allSettled"
        },
        "memos": { "kept": true },
        "background": false,
        "abortRequested": false
    });
    let completing = json!({
        "id": completing_id,
        "conversationId": 2,
        "kind": "test.task",
        "version": 1,
        "input": { "value": completing_id },
        "owner": owner_id,
        "state": {
            "status": "completing",
            "outcome": { "status": "failed", "error": { "message": "held" } }
        },
        "background": false,
        "abortRequested": false
    });
    fixture
        .storage
        .commit(
            &[vec![
                conversation(2),
                StorageWrite::Task(owner.clone()),
                StorageWrite::Task(waiting.clone()),
                StorageWrite::Task(completing.clone()),
            ]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(fixture.storage.task(waiting_id).unwrap().unwrap(), waiting);
    assert_eq!(fixture.storage.task(completing_id).unwrap().unwrap(), completing);

    assert_eq!(tasks_with_status(&fixture.storage, "waiting"), vec![waiting.clone()]);
    assert_eq!(tasks_with_status(&fixture.storage, "completing"), vec![completing.clone()]);
    assert_eq!(ids_in(&tasks_with_status(&fixture.storage, "pending")), vec![owner_id]);

    // `completing` → `terminal`：`outcome` 整块搬过去，状态扫描立刻换边。
    let terminal = {
        let mut record = completing.clone();
        record["state"] = json!({
            "status": "terminal",
            "outcome": { "status": "failed", "error": { "message": "held" } }
        });
        record
    };
    fixture
        .storage
        .commit(&[vec![StorageWrite::Task(terminal.clone())]], |_, _| {})
        .unwrap();
    assert_eq!(tasks_with_status(&fixture.storage, "completing"), Vec::<Value>::new());
    assert_eq!(tasks_with_status(&fixture.storage, "terminal"), vec![terminal]);
}

#[test]
fn stores_and_scans_sleeping_tasks_by_status() {
    // 上游 case：`sleeping`（v2 才加的状态）能存、能扫，并且和 `running` 之间换边。
    let mut fixture = backend("task-sleeping");
    let sleeping_id = 11;
    let sleeping = json!({
        "id": sleeping_id,
        "conversationId": 2,
        "kind": "test.task",
        "version": 1,
        "input": { "value": sleeping_id },
        "state": { "status": "sleeping", "checkpoint": { "phase": "next" }, "until": 61_000 },
        "memos": { "kept": true },
        "background": false,
        "abortRequested": false
    });
    fixture
        .storage
        .commit(
            &[vec![conversation(2), StorageWrite::Task(sleeping.clone())]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(fixture.storage.task(sleeping_id).unwrap().unwrap(), sleeping);

    assert_eq!(tasks_with_status(&fixture.storage, "sleeping"), vec![sleeping.clone()]);

    let running = {
        let mut record = sleeping.clone();
        record["state"] = json!({ "status": "running", "checkpoint": { "phase": "next" } });
        record
    };
    fixture
        .storage
        .commit(&[vec![StorageWrite::Task(running.clone())]], |_, _| {})
        .unwrap();
    assert_eq!(tasks_with_status(&fixture.storage, "sleeping"), Vec::<Value>::new());
    assert_eq!(tasks_with_status(&fixture.storage, "running"), vec![running]);
}

fn submissions_of(
    storage: &Storage,
    query: nous_durable::SubmissionQuery,
    limit: usize,
    cursor: Option<&nous_durable::Cursor>,
) -> nous_durable::Page<Value> {
    storage
        .scan_submissions(&query, limit, cursor)
        .unwrap()
        .unwrap()
}

/// 一条一条翻完整个筛选结果（上游那几条案里的 `ids(query)`）。
fn submission_ids(storage: &Storage, query: nous_durable::SubmissionQuery) -> Vec<i64> {
    let mut found = Vec::new();
    let mut cursor = None;
    loop {
        let page = submissions_of(storage, query.clone(), 1, cursor.as_ref());
        found.extend(ids_of(&page));
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    found
}

#[test]
fn indexes_request_ids_per_conversation_and_replaces_complete_submission_records() {
    // 上游 case：`requestId` 的幂等键**按会话**索引（两个会话可以有同一个 requestId），整条提交记录
    // 被后写的覆盖，状态一换就从旧的扫描里消失。
    let mut fixture = backend("submission-request");
    let (first_id, second_id, other_conversation_id) = (11, 12, 13);
    let first = json!({
        "id": first_id, "conversationId": 2, "requestId": "same", "type": "input", "status": "queued"
    });
    let second = json!({
        "id": second_id, "conversationId": 2, "requestId": "other", "type": "input", "status": "queued"
    });
    let other = json!({
        "id": other_conversation_id, "conversationId": 3, "requestId": "same", "type": "input", "status": "queued"
    });
    fixture
        .storage
        .commit(
            &[vec![
                conversation(2),
                conversation(3),
                StorageWrite::Submission(first.clone()),
                StorageWrite::Submission(second.clone()),
                StorageWrite::Submission(other.clone()),
            ]],
            |_, _| {},
        )
        .unwrap();

    // 同一个 requestId 在两个会话里各有一条：幂等键是 (conversationId, requestId)。
    assert_eq!(fixture.storage.submission_by_request(2, "same").unwrap().unwrap(), first);
    assert_eq!(fixture.storage.submission_by_request(3, "same").unwrap().unwrap(), other);

    // 覆盖成 placed：`entry` 是这一条新拿到的 entry id。
    let placed_second = json!({
        "id": second_id, "conversationId": 2, "requestId": "other", "type": "input",
        "status": "placed", "entry": 14
    });
    fixture
        .storage
        .commit(&[vec![StorageWrite::Submission(placed_second.clone())]], |_, _| {})
        .unwrap();
    assert_eq!(fixture.storage.submission(second_id).unwrap().unwrap(), placed_second);
    assert_eq!(
        fixture.storage.submission_by_request(2, "other").unwrap().unwrap(),
        placed_second
    );

    let every = nous_durable::SubmissionQuery::default();
    assert_eq!(
        submission_ids(&fixture.storage, every.clone()),
        vec![first_id, second_id, other_conversation_id]
    );
    let in_root = nous_durable::SubmissionQuery {
        conversation_id: Some(2),
        ..Default::default()
    };
    assert_eq!(submission_ids(&fixture.storage, in_root), vec![first_id, second_id]);

    // 状态一变就换边。
    let queued = nous_durable::SubmissionQuery {
        status: Some("queued".to_string()),
        ..Default::default()
    };
    assert_eq!(
        submission_ids(&fixture.storage, queued.clone()),
        vec![first_id, other_conversation_id]
    );
    let placed = nous_durable::SubmissionQuery {
        status: Some("placed".to_string()),
        ..Default::default()
    };
    assert_eq!(submission_ids(&fixture.storage, placed.clone()), vec![second_id]);

    let other_queued = nous_durable::SubmissionQuery {
        conversation_id: Some(3),
        status: Some("queued".to_string()),
        ..Default::default()
    };
    assert_eq!(submission_ids(&fixture.storage, other_queued), vec![other_conversation_id]);
    let other_placed = nous_durable::SubmissionQuery {
        conversation_id: Some(3),
        status: Some("placed".to_string()),
        ..Default::default()
    };
    assert!(
        submission_ids(&fixture.storage, other_placed).is_empty(),
        "会话 3 里没有 placed"
    );

    // 扫出来的整条记录与按 id 读出来的一模一样（不是「挑几个字段」）。
    let page = submissions_of(&fixture.storage, placed, 10, None);
    assert_eq!(page.items, vec![placed_second]);
}

#[test]
fn stores_passive_write_submissions_without_input_only_lifecycle_states() {
    // 上游 case：`write` 型提交不走 `input` 那条生命周期 —— 它可以从 `queued` 直接到 `done` 或
    // `unanswered`（带着 `reason` 与 `detail`），中间没有 `placed`。
    let mut fixture = backend("submission-passive");
    let (done_id, failed_id) = (11, 12);
    fixture
        .storage
        .commit(
            &[vec![
                conversation(2),
                StorageWrite::Submission(json!({
                    "id": done_id, "conversationId": 2, "requestId": "passive-done",
                    "type": "write", "status": "queued"
                })),
                StorageWrite::Submission(json!({
                    "id": failed_id, "conversationId": 2, "requestId": "passive-failed",
                    "type": "write", "status": "queued"
                })),
            ]],
            |_, _| {},
        )
        .unwrap();

    let done = json!({
        "id": done_id, "conversationId": 2, "requestId": "passive-done",
        "type": "write", "status": "done", "entry": 13
    });
    let unanswered = json!({
        "id": failed_id, "conversationId": 2, "requestId": "passive-failed",
        "type": "write", "status": "unanswered", "reason": "closed", "detail": { "retryable": false }
    });
    fixture
        .storage
        .commit(
            &[vec![
                StorageWrite::Submission(done.clone()),
                StorageWrite::Submission(unanswered.clone()),
            ]],
            |_, _| {},
        )
        .unwrap();

    assert_eq!(fixture.storage.submission(done_id).unwrap().unwrap(), done);
    assert_eq!(
        fixture.storage.submission_by_request(2, "passive-done").unwrap().unwrap(),
        done
    );
    assert_eq!(fixture.storage.submission(failed_id).unwrap().unwrap(), unanswered);
    assert_eq!(
        fixture
            .storage
            .submission_by_request(2, "passive-failed")
            .unwrap()
            .unwrap(),
        unanswered
    );
}

#[test]
fn stores_continue_submissions_which_have_no_entry_and_are_never_queued() {
    // 上游 case：`continue` 型提交起点就是 `placed`（它从不排队），成功时用 `answer` 而不是 `entry`。
    let mut fixture = backend("submission-continue");
    let (done_id, failed_id) = (11, 12);
    let placed_done = json!({
        "id": done_id, "conversationId": 2, "requestId": "continue-done",
        "type": "continue", "status": "placed"
    });
    let placed_failed = json!({
        "id": failed_id, "conversationId": 2, "requestId": "continue-failed",
        "type": "continue", "status": "placed"
    });
    fixture
        .storage
        .commit(
            &[vec![
                conversation(2),
                StorageWrite::Submission(placed_done.clone()),
                StorageWrite::Submission(placed_failed.clone()),
            ]],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(fixture.storage.submission(done_id).unwrap().unwrap(), placed_done);
    let placed = nous_durable::SubmissionQuery {
        status: Some("placed".to_string()),
        ..Default::default()
    };
    assert_eq!(
        submissions_of(&fixture.storage, placed, 10, None).items,
        vec![placed_done.clone(), placed_failed.clone()]
    );

    let done = json!({
        "id": done_id, "conversationId": 2, "requestId": "continue-done",
        "type": "continue", "status": "done", "answer": 13
    });
    let unanswered = json!({
        "id": failed_id, "conversationId": 2, "requestId": "continue-failed",
        "type": "continue", "status": "unanswered", "reason": "nothing_to_answer"
    });
    fixture
        .storage
        .commit(
            &[vec![
                StorageWrite::Submission(done.clone()),
                StorageWrite::Submission(unanswered.clone()),
            ]],
            |_, _| {},
        )
        .unwrap();

    assert_eq!(fixture.storage.submission(done_id).unwrap().unwrap(), done);
    assert_eq!(
        fixture.storage.submission_by_request(2, "continue-done").unwrap().unwrap(),
        done
    );
    assert_eq!(fixture.storage.submission(failed_id).unwrap().unwrap(), unanswered);
    assert_eq!(
        fixture
            .storage
            .submission_by_request(2, "continue-failed")
            .unwrap()
            .unwrap(),
        unanswered
    );
}

fn documents_of(
    storage: &Storage,
    query: nous_durable::DocumentQuery,
    limit: usize,
    cursor: Option<&nous_durable::Cursor>,
) -> nous_durable::Page<Value> {
    storage
        .scan_documents(&query, limit, cursor)
        .unwrap()
        .unwrap()
}

#[test]
fn indexes_logical_addresses_and_exact_scope_scans_independently() {
    // 上游 case：**按地址找**（`kind` + `scope` + `key`，五个索引列全要相等）与**按作用域扫**
    // （`kind` 可选、`key` 不参与）是两条独立的路。`__proto__` / `constructor` 这种 key 在这里只是
    // 普通字符串（`serde_json::Value` 是树，没有原型链）。
    let mut fixture = backend("document-address");
    let (first_id, second_id, conversation_document_id) = (20, 21, 22);
    let (singleton_id, family_id, other_kind_id) = (23, 24, 25);
    let created_at = fixture
        .storage
        .commit(
            &[vec![
                conversation(2),
                StorageWrite::Task(pending_task(11, 2)),
                StorageWrite::DocumentCreate {
                    id: first_id,
                    record: json!({
                        "id": first_id, "kind": "cache", "scope": { "kind": "session" }, "key": "__proto__"
                    }),
                    content: DocumentContent::Base { version: 1, value: json!({ "owner": "first" }) },
                },
                StorageWrite::DocumentCreate {
                    id: second_id,
                    record: json!({
                        "id": second_id, "kind": "cache", "scope": { "kind": "session" }, "key": "constructor"
                    }),
                    content: DocumentContent::Base { version: 1, value: json!({ "owner": "second" }) },
                },
                StorageWrite::DocumentCreate {
                    id: conversation_document_id,
                    record: json!({
                        "id": conversation_document_id, "kind": "cache",
                        "scope": { "kind": "conversation", "conversationId": 2 },
                        "history": "latest", "fork": "current", "key": "__proto__"
                    }),
                    content: DocumentContent::Base { version: 1, value: json!({ "owner": "conversation" }) },
                },
                StorageWrite::DocumentCreate {
                    id: singleton_id,
                    record: json!({
                        "id": singleton_id, "kind": "task.cache", "scope": { "kind": "task", "taskId": 11 }
                    }),
                    content: DocumentContent::Base { version: 1, value: json!({ "owner": "singleton" }) },
                },
                StorageWrite::DocumentCreate {
                    id: family_id,
                    record: json!({
                        "id": family_id, "kind": "task.cache", "scope": { "kind": "task", "taskId": 11 },
                        "key": "member"
                    }),
                    content: DocumentContent::Base { version: 1, value: json!({ "owner": "family" }) },
                },
                StorageWrite::DocumentCreate {
                    id: other_kind_id,
                    record: json!({
                        "id": other_kind_id, "kind": "task.other", "scope": { "kind": "task", "taskId": 11 }
                    }),
                    content: DocumentContent::Base { version: 1, value: json!({ "owner": "other" }) },
                },
            ]],
            |_, _| {},
        )
        .unwrap()
        .batches
        .first()
        .map(|batch| batch.seq)
        .unwrap();

    let session = json!({ "kind": "session" });

    // 按地址找：`__proto__` 与 `constructor` 是两个不同的地址，各回各的。
    //
    // 两次都断言（上游只断言了前者）：只断言一个的话，把 `key` 从 WHERE 里拿掉也不会红 ——
    // 两份文档是**同一次提交**建的，`created_at` 打平，`ORDER BY created_at DESC` 恰好还是先建的那份。
    let found = fixture
        .storage
        .find_document(
            &json!({ "kind": "cache", "scope": session, "key": "__proto__" }),
            DocumentPoint::Current,
        )
        .unwrap()
        .expect("session 作用域里有一份 cache/__proto__");
    assert_eq!(found["id"], json!(first_id));
    let other_key = fixture
        .storage
        .find_document(
            &json!({ "kind": "cache", "scope": session, "key": "constructor" }),
            DocumentPoint::Current,
        )
        .unwrap()
        .expect("session 作用域里有一份 cache/constructor");
    assert_eq!(other_key["id"], json!(second_id));

    // 按作用域扫：一页一条，两页拼起来是 id 序。
    let scope_query = || nous_durable::DocumentQuery {
        scope: json!({ "kind": "session" }),
        kind: None,
        at: DocumentPoint::Current,
    };
    let first_page = documents_of(&fixture.storage, scope_query(), 1, None);
    assert_eq!(first_page.items.len(), 1);
    let next = first_page.next.expect("session 作用域里有两份");
    let second_page = documents_of(&fixture.storage, scope_query(), 1, Some(&next));
    let mut seen = ids_of(&first_page);
    seen.extend(ids_of(&second_page));
    assert_eq!(seen, vec![first_id, second_id]);

    // 作用域带 owner：只看得见这个会话的那一份。
    let conversation_query = nous_durable::DocumentQuery {
        scope: json!({ "kind": "conversation", "conversationId": 2 }),
        kind: None,
        at: DocumentPoint::Current,
    };
    assert_eq!(
        ids_of(&documents_of(&fixture.storage, conversation_query, 10, None)),
        vec![conversation_document_id]
    );

    // 单例（没有 `key` 这个字段）与「`key` 是空串」的家族成员是**两个地址**：索引列里 `family`
    // 把它们分开（`0` / `1`），`key_value` 两边都是 `""` 的编码体。这一份故意晚一次提交建 ——
    // 否则两者 `created_at` 打平，去掉 `family` 也不会红。
    let empty_key_id = 26;
    fixture
        .storage
        .commit(
            &[vec![StorageWrite::DocumentCreate {
                id: empty_key_id,
                record: json!({
                    "id": empty_key_id, "kind": "task.cache",
                    "scope": { "kind": "task", "taskId": 11 }, "key": ""
                }),
                content: DocumentContent::Base { version: 1, value: json!({ "owner": "empty" }) },
            }]],
            |_, _| {},
        )
        .unwrap();

    // 单例（无 key）与家族成员（有 key）是两个地址：单例按「三个字段的地址」找得到。
    let singleton = fixture
        .storage
        .find_document(
            &json!({ "kind": "task.cache", "scope": { "kind": "task", "taskId": 11 } }),
            DocumentPoint::Current,
        )
        .unwrap()
        .expect("task 作用域里有一个 task.cache 单例");
    assert_eq!(singleton["id"], json!(singleton_id));
    let family = fixture
        .storage
        .find_document(
            &json!({ "kind": "task.cache", "scope": { "kind": "task", "taskId": 11 }, "key": "member" }),
            DocumentPoint::Current,
        )
        .unwrap()
        .expect("task 作用域里有一个 task.cache/member");
    assert_eq!(family["id"], json!(family_id));
    // `key: ""` 是第三个地址，不是那个单例。
    let empty_key = fixture
        .storage
        .find_document(
            &json!({ "kind": "task.cache", "scope": { "kind": "task", "taskId": 11 }, "key": "" }),
            DocumentPoint::Current,
        )
        .unwrap()
        .expect("task 作用域里有一个 task.cache/\"\"");
    assert_eq!(empty_key["id"], json!(empty_key_id));

    // 作用域 + kind：`task.other` 不跟着 `task.cache` 出来。
    let task_query = nous_durable::DocumentQuery {
        scope: json!({ "kind": "task", "taskId": 11 }),
        kind: Some("task.cache".to_string()),
        at: DocumentPoint::Current,
    };
    assert_eq!(
        ids_of(&documents_of(&fixture.storage, task_query, 10, None)),
        vec![singleton_id, family_id, empty_key_id]
    );

    // 只留最新态的文档（session 作用域）没有历史可读：在那个 seq 上读它是伪造。
    let error = fixture
        .storage
        .materialize(singleton_id, DocumentPoint::At(created_at))
        .unwrap_err();
    assert!(
        matches!(error, StorageError::HistoryNotRetained { document_id } if document_id == singleton_id),
        "{error}"
    );
    assert!(error.to_string().contains("does not retain historical content"), "{error}");
}

fn entries_of(
    storage: &Storage,
    query: nous_durable::EntryQuery,
    limit: usize,
    cursor: Option<&nous_durable::Cursor>,
) -> nous_durable::Page<Value> {
    storage.scan_entries(&query, limit, cursor).unwrap().unwrap()
}

/// 上游 `entry(id, conversationId, kind, extra)`：一条 entry 记录。
fn entry_with(id: i64, conversation_id: i64, kind: &str, extra: Value) -> StorageWrite {
    let mut record = json!({ "id": id, "conversationId": conversation_id, "kind": kind });
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            record[key] = value.clone();
        }
    }
    StorageWrite::Entry(record)
}

#[test]
fn scans_deep_fork_history_newest_first_through_every_ancestor_cap() {
    // 上游 case：fork 链上「一条会话的历史」= 自己那一段 + 每一代祖先到各自的 fork 点为止。
    //
    // ```text
    // 根(2)  3(first) 4(fork 点, head: 3) 5(fork 点之后，同一次提交里写的)
    //          └── 子(6, parent {2, at: 4})   7(fork 点)  8(fork 点之后)
    //                └── 孙(10, parent {6, at: 7})  11(head 标记, head: 11)  12
    // ```
    //
    // 于是「孙的历史」读出来必须是：12、11、7、4、3 —— 5 / 8 / 9 / 13 全在某个 fork 点之后，
    // 属于别的分支，一条都不许出现。
    let mut fixture = backend("fork-history");
    let (root, child, grandchild) = (2, 6, 10);
    let (root_first, root_fork_point, root_excluded_same_commit) = (3, 4, 5);
    let (child_fork_point, child_excluded) = (7, 8);
    let root_excluded_later = 9;
    let (grandchild_head, grandchild_tail) = (11, 12);
    let child_excluded_later = 13;

    let root_entries_seq = fixture
        .storage
        .commit(
            &[vec![
                conversation(root),
                entry_with(root_first, root, "message", Value::Null),
                entry_with(
                    root_fork_point,
                    root,
                    "marker",
                    json!({ "head": root_first }),
                ),
                entry_with(root_excluded_same_commit, root, "message", Value::Null),
            ]],
            |_, _| {},
        )
        .unwrap()
        .batches
        .first()
        .map(|batch| batch.seq)
        .unwrap();
    fixture
        .storage
        .commit(
            &[vec![StorageWrite::Conversation(json!({
                "id": child, "parent": { "conversationId": root, "at": root_fork_point }
            }))]],
            |_, _| {},
        )
        .unwrap();
    fixture
        .storage
        .commit(
            &[vec![
                entry_with(child_fork_point, child, "note", Value::Null),
                entry_with(child_excluded, child, "message", Value::Null),
            ]],
            |_, _| {},
        )
        .unwrap();
    fixture
        .storage
        .commit(
            &[vec![entry_with(root_excluded_later, root, "message", Value::Null)]],
            |_, _| {},
        )
        .unwrap();
    fixture
        .storage
        .commit(
            &[vec![StorageWrite::Conversation(json!({
                "id": grandchild, "parent": { "conversationId": child, "at": child_fork_point }
            }))]],
            |_, _| {},
        )
        .unwrap();
    let grandchild_entries_seq = fixture
        .storage
        .commit(
            &[vec![
                entry_with(
                    grandchild_head,
                    grandchild,
                    "marker",
                    json!({ "head": grandchild_head }),
                ),
                entry_with(grandchild_tail, grandchild, "message", Value::Null),
            ]],
            |_, _| {},
        )
        .unwrap()
        .batches
        .first()
        .map(|batch| batch.seq)
        .unwrap();
    fixture
        .storage
        .commit(
            &[vec![entry_with(child_excluded_later, child, "message", Value::Null)]],
            |_, _| {},
        )
        .unwrap();

    let descending = nous_durable::EntryQuery {
        conversation_id: grandchild,
        min_entry_id: None,
        max_entry_id: None,
        order: None,
    };
    let first = entries_of(&fixture.storage, descending.clone(), 2, None);
    assert_eq!(ids_of(&first), vec![grandchild_tail, grandchild_head]);
    let second = entries_of(&fixture.storage, descending.clone(), 2, first.next.as_ref());
    assert_eq!(ids_of(&second), vec![child_fork_point, root_fork_point]);
    let third = entries_of(&fixture.storage, descending, 2, second.next.as_ref());
    assert_eq!(ids_of(&third), vec![root_first]);
    assert!(third.next.is_none());

    // 最老的先：根那一段在前，每一代到下一个 fork 点为止。
    let ascending = nous_durable::EntryQuery {
        conversation_id: grandchild,
        min_entry_id: None,
        max_entry_id: None,
        order: Some(ScanOrder::Ascending),
    };
    let first_up = entries_of(&fixture.storage, ascending.clone(), 2, None);
    assert_eq!(ids_of(&first_up), vec![root_first, root_fork_point]);
    let second_up = entries_of(&fixture.storage, ascending.clone(), 2, first_up.next.as_ref());
    assert_eq!(ids_of(&second_up), vec![child_fork_point, grandchild_head]);
    let third_up = entries_of(&fixture.storage, descending.clone(), 2, second_up.next.as_ref());
    assert_eq!(ids_of(&third_up), vec![grandchild_tail]);
    assert!(third_up.next.is_none());

    // 上下界一起给：只有 fork 点这一对落在区间里。
    let bounded = nous_durable::EntryQuery {
        conversation_id: grandchild,
        min_entry_id: Some(root_fork_point),
        max_entry_id: Some(child_fork_point),
        order: Some(ScanOrder::Ascending),
    };
    assert_eq!(
        ids_of(&entries_of(&fixture.storage, bounded, 10, None)),
        vec![root_fork_point, child_fork_point]
    );

    // 游标接的是升序，查询却**明说**要降序：请求错误，不静默改向。
    let explicit_descending = nous_durable::EntryQuery {
        order: Some(ScanOrder::Descending),
        ..descending.clone()
    };
    let refusal = fixture
        .storage
        .scan_entries(&explicit_descending, 2, first_up.next.as_ref())
        .unwrap()
        .unwrap_err();
    assert!(matches!(refusal, Refusal::CursorOrderMismatch), "{refusal}");

    // `head` 标记替调用方给出真实下界：孙自己那条、以及「截至子 fork 点」那一条。
    let current_marker = fixture
        .storage
        .find_latest_head_marker(grandchild, None)
        .unwrap()
        .expect("孙自己有一条 head 标记");
    assert_eq!(current_marker["id"], json!(grandchild_head));
    assert_eq!(current_marker["head"], json!(grandchild_head));
    let historical_marker = fixture
        .storage
        .find_latest_head_marker(grandchild, Some(child_fork_point))
        .unwrap()
        .expect("截至子 fork 点，根那条 fork 点就是标记");
    assert_eq!(historical_marker["id"], json!(root_fork_point));
    assert_eq!(historical_marker["head"], json!(root_first));
    assert!(fixture
        .storage
        .find_latest_head_marker(grandchild, Some(root_first))
        .unwrap()
        .is_none());

    // 从 `head` 起算的「活跃历史」：翻完只有 12、11。
    let active = nous_durable::EntryQuery {
        conversation_id: grandchild,
        min_entry_id: Some(grandchild_head),
        max_entry_id: None,
        order: None,
    };
    let active_first = entries_of(&fixture.storage, active.clone(), 1, None);
    assert_eq!(ids_of(&active_first), vec![grandchild_tail]);
    assert!(active_first.next.is_some());
    let active_second = entries_of(&fixture.storage, active, 1, active_first.next.as_ref());
    assert_eq!(ids_of(&active_second), vec![grandchild_head]);
    assert!(active_second.next.is_none());

    // 从历史标记的 `head` 起算、到子 fork 点为止：三段拼起来。
    let historical = nous_durable::EntryQuery {
        conversation_id: grandchild,
        min_entry_id: Some(root_first),
        max_entry_id: Some(child_fork_point),
        order: None,
    };
    assert_eq!(
        ids_of(&entries_of(&fixture.storage, historical, 10, None)),
        vec![child_fork_point, root_fork_point, root_first]
    );

    // 全局按 id 读 entry：`commitSeq` 是写下它的那次提交。
    let (record, seq) = fixture.storage.entry(root_first).unwrap().unwrap();
    assert_eq!(record["id"], json!(root_first));
    assert_eq!(seq, root_entries_seq);
    assert_eq!(fixture.storage.entry(root_fork_point).unwrap().unwrap().1, root_entries_seq);
    assert_eq!(fixture.storage.entry(grandchild_head).unwrap().unwrap().1, grandchild_entries_seq);
    assert_eq!(fixture.storage.entry(grandchild_tail).unwrap().unwrap().1, grandchild_entries_seq);
    assert!(fixture.storage.entry(999_999).unwrap().is_none());

    // 从孙会话看：链上的都看得见，fork 点之后的全看不见。
    let visible = |id: i64| fixture.storage.entry_in_conversation(grandchild, id).unwrap();
    assert_eq!(visible(root_first).unwrap().1, root_entries_seq);
    assert_eq!(visible(child_fork_point).unwrap().0["conversationId"], json!(child));
    assert_eq!(visible(grandchild_tail).unwrap().1, grandchild_entries_seq);
    for hidden in [
        root_excluded_same_commit,
        root_excluded_later,
        child_excluded,
        child_excluded_later,
        999_999,
    ] {
        assert!(visible(hidden).is_none(), "entry {hidden} 不该从孙会话看得见");
    }
    // 反过来：根会话看不见孙的 entry。
    assert!(fixture
        .storage
        .entry_in_conversation(root, grandchild_head)
        .unwrap()
        .is_none());

    // 会话 id 写错是明确失败，不是空页。
    let error = fixture
        .storage
        .entry_in_conversation(999_999, root_first)
        .unwrap_err();
    assert!(matches!(error, StorageError::UnknownConversation { id } if id == 999_999), "{error}");
    assert!(error.to_string().contains("Unknown conversation"), "{error}");
    let error = fixture
        .storage
        .scan_entries(
            &nous_durable::EntryQuery {
                conversation_id: 999_999,
                min_entry_id: None,
                max_entry_id: None,
                order: None,
            },
            10,
            None,
        )
        .unwrap_err();
    assert!(matches!(error, StorageError::UnknownConversation { id } if id == 999_999), "{error}");
}
