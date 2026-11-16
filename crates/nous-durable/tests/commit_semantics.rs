//! 12b 的判据：提交语义（seq / 回执 / 分配器 / 拒绝 / 游标），含**三条负例**。
//!
//! 三条负例是票面点名的，每条都必须「明确失败」而不是悄悄换个写法：
//!
//! 1. **半写回滚**：一次调用里前面的批通过了、后面的批撞数据库约束 → 整次调用不可见
//!    （对比：只是「被拒绝」时前面的批照常提交 —— 上行 `commit.ts` 没有 savepoint）。
//! 2. **seq 回退**：把 `next_seq` 拨回去，提交必须拒绝，绝不复用旧号。
//! 3. **别人的库**：`writer != 'nous'` 写模式必拒，只读照读。

use std::path::PathBuf;

use serde_json::{json, Value};

use nous_chord::sqlite::SqliteExecutor;
use nous_durable::storage::commit::ConversationQuery;
use nous_durable::{
    Cursor, DocumentContent, Refusal, ScanOrder, Storage, StorageError, StorageWrite,
};

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-commit-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

fn conversation(id: i64) -> StorageWrite {
    StorageWrite::Conversation(json!({ "id": id }))
}

fn entry(id: i64, conversation_id: i64) -> StorageWrite {
    StorageWrite::Entry(json!({ "id": id, "conversationId": conversation_id, "kind": "pi.user" }))
}

fn session_document(id: i64, content: DocumentContent) -> StorageWrite {
    StorageWrite::DocumentCreate {
        id,
        record: json!({
            "id": id, "kind": "pi.agent", "scope": { "kind": "session" },
            "history": "latest", "fork": "current"
        }),
        content,
    }
}

#[test]
fn a_commit_stamps_one_seq_per_batch_and_receipts_arrive_after_the_commit() {
    let path = temp_db("seqs");
    let mut storage = Storage::open(&path).unwrap();
    assert_eq!(storage.next_id(), 2, "迁移默认 next_id = 2（id 1 留给根会话）");

    let batches = vec![
        vec![conversation(2)],
        vec![entry(3, 2)],
        vec![entry(4, 2)],
    ];
    let mut receipts: Vec<(usize, i64)> = Vec::new();
    let mut visible_in_receipts = Vec::new();
    let result = storage
        .commit(&batches, |index, seq| {
            // 回执的语义是「已落盘」：回调里另开一个只读连接，必须看得见这一批。
            let reader = Storage::open_read_only(&path).unwrap();
            let seen = match index {
                0 => reader.conversation(2).unwrap().is_some(),
                _ => reader.entry(3).unwrap().is_some() || reader.entry(4).unwrap().is_some(),
            };
            visible_in_receipts.push(seen);
            receipts.push((index, seq));
        })
        .unwrap();

    assert_eq!(result.stored, 3);
    assert_eq!(result.refused, None);
    // 每批一个 seq，按顺序。
    assert_eq!(receipts, vec![(0, 1), (1, 2), (2, 3)]);
    assert!(visible_in_receipts.iter().all(|seen| *seen), "回执必须晚于 COMMIT");

    // entry 的 `commit_seq` 就是它那一批的 seq。
    assert_eq!(storage.entry(3).unwrap().unwrap().1, 2);
    assert_eq!(storage.entry(4).unwrap().unwrap().1, 3);

    // 计数器落盘：next_seq 推到了 4。
    let reader = Storage::open_read_only(&path).unwrap();
    let (_next_id, next_seq) = reader.store().metadata().unwrap();
    assert_eq!(next_seq, 4);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_refused_batch_stops_the_call_but_the_batches_before_it_stay() {
    let path = temp_db("refused");
    let mut storage = Storage::open(&path).unwrap();
    let batches = vec![
        vec![conversation(2)],
        // 同一批里同一个 id 写两遍：拒绝，不是数据库失败。
        vec![conversation(3), conversation(3)],
        vec![conversation(4)],
    ];
    let result = storage.commit(&batches, |_, _| {}).unwrap();
    assert_eq!(result.stored, 1);
    assert_eq!(result.refused, Some(Refusal::IdWrittenTwice { id: 3 }));

    // 第 1 批在，第 2、3 批不在（拒绝之后不再往下走）。
    assert!(storage.conversation(2).unwrap().is_some());
    assert!(storage.conversation(3).unwrap().is_none());
    assert!(storage.conversation(4).unwrap().is_none());

    // seq 没被浪费：下一次提交接着 2。
    let mut seqs = Vec::new();
    storage
        .commit(&[vec![conversation(4)]], |_, seq| seqs.push(seq))
        .unwrap();
    assert_eq!(seqs, vec![2]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_half_written_call_rolls_back_whole_and_is_invisible() {
    let path = temp_db("half-write");
    let mut storage = Storage::open(&path).unwrap();
    let before = storage.store().metadata().unwrap();

    // 第 1 批合法，第 2 批撞 `tasks.status` 的 CHECK：数据库失败 = 整次调用回滚。
    let bad_task = StorageWrite::Task(json!({
        "id": 9, "conversationId": 2, "kind": "pi.generation",
        "state": { "status": "nonsense" }
    }));
    let error = storage
        .commit(&[vec![conversation(2)], vec![bad_task]], |_, _| {
            panic!("整次调用失败时不该有回执")
        })
        .unwrap_err();
    assert!(
        matches!(&error, StorageError::Sqlite(sqlite) if sqlite.message.contains("CHECK")),
        "{error}"
    );

    // 半写不可见：会话、台账、seq 一样都没动。
    assert!(storage.conversation(2).unwrap().is_none());
    let record_ids = storage
        .store()
        .db()
        .get("SELECT COUNT(*) AS n FROM record_ids", &[])
        .unwrap()
        .unwrap();
    assert_eq!(record_ids.get("n").unwrap().as_i64(), Some(0));
    assert_eq!(storage.store().metadata().unwrap(), before);
    assert!(storage.entry(9).unwrap().is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_seq_that_went_backwards_is_refused_instead_of_reused() {
    let path = temp_db("seq-regression");
    let mut storage = Storage::open(&path).unwrap();
    // 一批里带一条 entry，seq 才真的被用掉（会话不盖 seq）。
    storage
        .commit(&[vec![conversation(2), entry(3, 2)]], |_, _| {})
        .unwrap();
    assert_eq!(storage.store().metadata().unwrap().1, 2);

    // 把计数器拨回去（模拟「拷贝了旧库 / 重放了一半」）。
    storage
        .store()
        .db()
        .run("UPDATE durable_metadata SET next_seq = 1 WHERE singleton = 1", &[])
        .unwrap();

    let error = storage
        .commit(&[vec![conversation(3)]], |_, _| panic!("不该有回执"))
        .unwrap_err();
    assert_eq!(
        error,
        StorageError::SeqRegression {
            next_seq: 1,
            floor: 1
        },
        "seq 1 已经被会话 2 用掉了"
    );
    assert!(storage.conversation(3).unwrap().is_none());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_database_written_by_someone_else_is_refused_in_write_mode_but_reads_read_only() {
    let path = temp_db("foreign-writer");
    {
        let mut storage = Storage::open(&path).unwrap();
        storage.commit(&[vec![conversation(2)]], |_, _| {}).unwrap();
    }
    // 换个 writer：这份库就不再是我们的了。
    {
        let raw = nous_chord::sqlite::LocalDatabase::open(&path).unwrap();
        raw.run("UPDATE durable_schema SET writer = 'qrd3wh' WHERE singleton = 1", &[])
            .unwrap();
    }
    match Storage::open(&path) {
        Err(StorageError::WriterMismatch { found }) => assert_eq!(found.as_deref(), Some("qrd3wh")),
        other => panic!("写模式必须拒绝：{:?}", other.err()),
    }
    // 只读：照读，提交不了（没有 write 入口）。
    let reader = Storage::open_read_only(&path).unwrap();
    assert!(reader.conversation(2).unwrap().is_some());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_id_allocator_never_reuses_an_id_even_after_the_record_is_gone() {
    let path = temp_db("allocator");
    let mut storage = Storage::open(&path).unwrap();

    // 分 5 个 id 给 5 条 entry（id 2..6），一次提交写出去。
    let ids: Vec<i64> = (0..5).map(|_| storage.mint_id().unwrap()).collect();
    assert_eq!(ids, vec![2, 3, 4, 5, 6]);
    let batches = vec![
        ids.iter()
            .map(|id| entry(*id, 1))
            .collect::<Vec<StorageWrite>>(),
    ];
    storage.commit(&batches, |_, _| {}).unwrap();
    // 分配出去的 id 立刻落盘成 next_id = 7。
    assert_eq!(storage.store().metadata().unwrap().0, "7");

    // 删掉三条记录（模拟 compaction 撕掉历史）。
    storage
        .store()
        .db()
        .run("DELETE FROM entries WHERE id IN (3, 4, 5)", &[])
        .unwrap();

    // 重开：next_id 不回退，mint 出来的是 7 —— 不是空出来的 3/4/5。
    let mut storage = Storage::open(&path).unwrap();
    assert_eq!(storage.next_id(), 7);
    assert_eq!(storage.mint_id().unwrap(), 7);
    // 被删掉的 id 仍在台账里（`record_ids`），所以谁都不会再分配它们。
    let kept = storage
        .store()
        .db()
        .get("SELECT COUNT(*) AS n FROM record_ids WHERE record_type = 'entry'", &[])
        .unwrap()
        .unwrap();
    assert_eq!(kept.get("n").unwrap().as_i64(), Some(5));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_opaque_cursor_pages_conversations_in_id_order() {
    let path = temp_db("cursor");
    let mut storage = Storage::open(&path).unwrap();
    let batches = vec![(2..7).map(conversation).collect::<Vec<StorageWrite>>()];
    storage.commit(&batches, |_, _| {}).unwrap();

    let query = ConversationQuery::default();
    let first = storage.scan_conversations(&query, 2, None).unwrap().unwrap();
    let ids = |page: &nous_durable::Page<Value>| -> Vec<i64> {
        page.items
            .iter()
            .map(|record| record.get("id").unwrap().as_i64().unwrap())
            .collect()
    };
    assert_eq!(ids(&first), vec![2, 3]);

    // 游标是不透明文本：只有方向 + id。
    let cursor = first.next.unwrap();
    let text = cursor.encode();
    assert!(text.chars().all(|c| c.is_ascii_alphanumeric()), "{text}");
    assert_eq!(Cursor::decode(&text).unwrap(), cursor);

    let second = storage
        .scan_conversations(&query, 2, Some(&cursor))
        .unwrap()
        .unwrap();
    assert_eq!(ids(&second), vec![4, 5]);
    let third = storage
        .scan_conversations(&query, 2, Some(&second.next.unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(ids(&third), vec![6]);
    assert_eq!(third.next, None, "最后一页不给空游标");

    // 降序：从最新往回翻。
    let descending = ConversationQuery {
        order: Some(ScanOrder::Descending),
        ..ConversationQuery::default()
    };
    let page = storage.scan_conversations(&descending, 2, None).unwrap().unwrap();
    assert_eq!(ids(&page), vec![6, 5]);

    // 拿升序的游标去问降序：明确失败，不猜方向。
    assert_eq!(
        storage.scan_conversations(&descending, 2, Some(&cursor)).unwrap(),
        Err(Refusal::CursorOrderMismatch)
    );
    // 编造的游标（不是 encode 出来的）在 decode 那一步就被拒。
    assert_eq!(
        Cursor::decode("a{}"),
        Err(Refusal::InvalidCursor),
        "调用方只能原样回传游标"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn owner_edges_are_written_and_filterable() {
    let path = temp_db("owner-edges");
    let mut storage = Storage::open(&path).unwrap();
    // 子树中止 / 子树 idle 靠 owner 的这两列：会话 3 是从会话 2、task 7 分出来的。
    let child = StorageWrite::Conversation(json!({
        "id": 3,
        "parent": { "conversationId": 2, "at": 5 },
        "owner": { "conversationId": 2, "taskId": 7 }
    }));
    storage
        .commit(&[vec![conversation(2), child]], |_, _| {})
        .unwrap();

    let stored = storage.conversation(3).unwrap().unwrap();
    // 记录原样（`parent` 与 `owner` 都在 JSON 里，一个字节不改）。
    assert_eq!(stored["owner"]["taskId"], json!(7));
    assert_eq!(stored["parent"]["at"], json!(5));

    let by_conversation = ConversationQuery {
        owner_conversation_id: Some(2),
        ..ConversationQuery::default()
    };
    let page = storage
        .scan_conversations(&by_conversation, 10, None)
        .unwrap()
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0]["id"], json!(3));

    let by_task = ConversationQuery {
        owner_task_id: Some(7),
        ..ConversationQuery::default()
    };
    assert_eq!(storage.scan_conversations(&by_task, 10, None).unwrap().unwrap().items.len(), 1);
    let none = ConversationQuery {
        owner_task_id: Some(8),
        ..ConversationQuery::default()
    };
    assert!(storage.scan_conversations(&none, 10, None).unwrap().unwrap().items.is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_delta_must_continue_the_version_the_last_revision_left() {
    let path = temp_db("delta-version");
    let mut storage = Storage::open(&path).unwrap();
    let base = DocumentContent::Base {
        version: 1,
        value: json!({ "a": 1 }),
    };
    storage
        .commit(&[vec![session_document(4, base)]], |_, _| {})
        .unwrap();

    // 接得上：delta 的 `version` 是**它接着的那个 base 的版本**（1），不是 2 ——
    // 版本号只在换 base 时前进（上游 `version transition requires a base` 就是这条）。
    let delta = DocumentContent::Delta {
        version: 1,
        ops: json!([]),
    };
    let result = storage
        .commit(&[vec![StorageWrite::DocumentChange { id: 4, content: delta }]], |_, _| {})
        .unwrap();
    assert_eq!(result.stored, 1);

    // 接不上：拒绝（不悄悄按库里的版本用）。
    let wrong = DocumentContent::Delta {
        version: 9,
        ops: json!([]),
    };
    let result = storage
        .commit(&[vec![StorageWrite::DocumentChange { id: 4, content: wrong }]], |_, _| {})
        .unwrap();
    assert_eq!(result.stored, 0);
    assert_eq!(
        result.refused,
        Some(Refusal::DeltaVersionMismatch {
            document_id: 4,
            previous: 1,
            wanted: 9
        })
    );
    let _ = std::fs::remove_file(&path);
}
