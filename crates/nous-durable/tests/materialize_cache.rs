//! `#25` 的判据：`materialize(Current)` 的结果可以复用，**但库永远是真相**。
//!
//! 顺序是这张票的硬约束：这四条先写、先跑绿（红线之前就是这么写的），再动缓存。理由在票面上：
//! 加缓存就必然有失效问题，先优化后补测试 = 给自己装一个「读到旧值」的静默 bug，而它在流式 UI
//! 上只表现为「最后一段文本没出现」，没人会怀疑存储层。
//!
//! 四条各守一个方向：
//!
//! 1. [`externally_written_document_invalidates_the_cached_value`] —— **另一个连接**（另一个进程
//!    的等价物）改了同一份文档，本连接的 publication `value` 必须跟着变。只信进程内缓存就会红。
//! 2. [`an_externally_retired_document_does_not_come_back_from_the_cache`] —— 退役不写新 revision，
//!    光比「最新 revision 的 seq」会漏掉它。
//! 3. [`the_cache_does_not_leak_between_documents`] —— 同一 store 里两份文档交替读，值各归各的。
//! 4. [`a_cached_value_equals_a_fresh_read_only_read`] —— 缓存的值与「另开一条只读连接直接读库」
//!    逐字段相等。`#13` 那条「publication `value` 等于直接读回」不因为加了缓存而放松。

use std::path::PathBuf;

use serde_json::{json, Value};

use nous_durable::{
    CommitResult, DocumentContent, DocumentPoint, Storage, StorageWrite, StoredChange,
    StoredDocumentChange,
};

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-cache-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

/// 一份文档的记录。`history: "latest"` 是 `pi.live` 的真形状；`"rewindable"` 才留历史。
fn document(id: i64, conversation: i64, history: &str) -> Value {
    // `key` 必须带上：地址是 (kind, scope, family, key) 的元组，两份没有 key 的同 kind 文档
    // 会撞同一个地址，第二份被拒绝（拒绝不是错误，只看 `refused` 会看不见）。
    json!({
        "id": id,
        "kind": "pi.live",
        "key": format!("live-{id}"),
        "scope": { "kind": "conversation", "conversationId": conversation },
        "history": history,
        "fork": "initial"
    })
}

fn create(id: i64, conversation: i64, history: &str, text: &str) -> StorageWrite {
    StorageWrite::DocumentCreate {
        id,
        record: document(id, conversation, history),
        content: DocumentContent::Base {
            version: 1,
            value: json!({ "text": text }),
        },
    }
}

/// 一次「往 `text` 尾巴追加」的 delta 提交（harness 的 partial 就是这个形状）。
fn append(id: i64, version: i64, chunk: &str) -> StorageWrite {
    StorageWrite::DocumentChange {
        id,
        content: DocumentContent::Delta {
            version,
            ops: json!([["a", ["text"], chunk]]),
        },
    }
}

/// 一条 publication 里那份文档的 `value`。
fn published_value(result: &CommitResult) -> Value {
    for batch in &result.batches {
        for change in &batch.changes {
            if let StoredChange::Document(StoredDocumentChange::Change { value, .. }) = change {
                return value.clone();
            }
        }
    }
    panic!("这批没有文档内容变更：{:?}", result.batches);
}

fn text_of(value: &Value) -> &str {
    value.get("text").and_then(Value::as_str).expect("text")
}

fn read(storage: &Storage, id: i64) -> Value {
    storage
        .materialize(id, DocumentPoint::Current)
        .expect("materialize")
        .expect("文档在")
        .value
}

#[test]
fn externally_written_document_invalidates_the_cached_value() {
    let path = temp_db("external");
    let mut first = Storage::open(&path).expect("打开");
    first
        .commit(
            &[vec![
                StorageWrite::Conversation(json!({ "id": 2 })),
                create(3, 2, "latest", ""),
            ]],
            |_, _| {},
        )
        .expect("建文档");
    first
        .commit(&[vec![append(3, 1, "aaa")]], |_, _| {})
        .expect("第一次 delta");

    // 读一次：如果实现了缓存，这条把本连接的值焐热。
    assert_eq!(text_of(&read(&first, 3)), "aaa");

    // 另一个写连接 = 另一个进程的等价物（同一个库文件，各自的连接与事务）。
    let mut second = Storage::open(&path).expect("第二个写连接");
    second
        .commit(&[vec![append(3, 1, "bbb")]], |_, _| {})
        .expect("外部 delta");

    // ① 本连接直接读也必须看得见外部那次。
    assert_eq!(
        text_of(&read(&first, 3)),
        "aaabbb",
        "外部连接写了同一份文档，本连接还读到旧值 —— 缓存没按「库里的最新 revision」失效"
    );

    // ② 本连接的下一次 delta 提交，publication 的 `value` 必须是「外部那次 + 本次」。
    let outcome = first
        .commit(&[vec![append(3, 1, "ccc")]], |_, _| {})
        .expect("第二次 delta");
    assert_eq!(
        text_of(&published_value(&outcome)),
        "aaabbbccc",
        "publication 的 value 少了外部那一段：客户端会看到少字，而每一帧都合法"
    );
    assert_eq!(text_of(&read(&first, 3)), "aaabbbccc");
}

#[test]
fn an_externally_retired_document_does_not_come_back_from_the_cache() {
    let path = temp_db("retire");
    let mut first = Storage::open(&path).expect("打开");
    first
        .commit(
            &[vec![
                StorageWrite::Conversation(json!({ "id": 2 })),
                // `rewindable` 不是随手写的：只留最新态的文档退役时会把 revision 一起删掉，
                // 于是 `MAX(seq)` 变 NULL，光靠「比 seq」也能挡住 —— 那条路验不到 `retired_at`。
                // 这里要的是「revision 全在、只有 `retiredAt` 变了」的那种退役。
                create(3, 2, "rewindable", "alive"),
            ]],
            |_, _| {},
        )
        .expect("建文档");
    assert_eq!(text_of(&read(&first, 3)), "alive");

    // 退役**不写新 revision**：只比「最新 revision 的 seq」的缓存会在这里骗人。
    let mut second = Storage::open(&path).expect("第二个写连接");
    second
        .commit(&[vec![StorageWrite::DocumentRetire { id: 3 }]], |_, _| {})
        .expect("外部退役");

    assert!(
        first
            .materialize(3, DocumentPoint::Current)
            .expect("materialize")
            .is_none(),
        "外部退役之后本连接还看得见这份文档 —— 缓存把已退役的文档复活了"
    );
}

#[test]
fn the_cache_does_not_leak_between_documents() {
    let path = temp_db("leak");
    let mut storage = Storage::open(&path).expect("打开");
    storage
        .commit(
            &[vec![
                StorageWrite::Conversation(json!({ "id": 2 })),
                create(10, 2, "rewindable", "a0"),
                create(11, 2, "rewindable", "b0"),
            ]],
            |_, _| {},
        )
        .expect("建两份文档");

    // 交替：改一份、读另一份。按 id 存的缓存绝不会串，按「最近一份」存的会。
    for round in 0..5 {
        storage
            .commit(&[vec![append(10, 1, "a")]], |_, _| {})
            .expect("改 10");
        assert_eq!(
            text_of(&read(&storage, 11)),
            format!("b0{}", "b".repeat(round)),
            "第 {round} 轮：读 11 读到了 10 的值（或者少了一段）"
        );
        storage
            .commit(&[vec![append(11, 1, "b")]], |_, _| {})
            .expect("改 11");
        assert_eq!(
            text_of(&read(&storage, 10)),
            format!("a0{}", "a".repeat(round + 1)),
            "第 {round} 轮：读 10 读到了 11 的值（或者少了一段）"
        );
    }
}

#[test]
fn a_cached_value_equals_a_fresh_read_only_read() {
    let path = temp_db("equals");
    let mut storage = Storage::open(&path).expect("打开");
    storage
        .commit(
            &[vec![
                StorageWrite::Conversation(json!({ "id": 2 })),
                create(3, 2, "rewindable", ""),
            ]],
            |_, _| {},
        )
        .expect("建文档");
    for round in 0..20 {
        storage
            .commit(&[vec![append(3, 1, &format!("{round},"))]], |_, _| {})
            .expect("delta");
    }

    // 另开一条只读连接，直接从库里读（不经任何缓存）。
    let reader = Storage::open_read_only(&path).expect("只读连接");
    let cached = storage
        .materialize(3, DocumentPoint::Current)
        .expect("materialize")
        .expect("文档在");
    let fresh = reader
        .materialize(3, DocumentPoint::Current)
        .expect("materialize")
        .expect("文档在");

    // 逐字段：任何一项不同都说明缓存与库不一致。
    assert_eq!(cached.record, fresh.record, "record 不一致");
    assert_eq!(cached.version, fresh.version, "version 不一致");
    assert_eq!(
        cached.value, fresh.value,
        "value 不一致 —— #13 的「publication value 等于直接读回」被缓存放松了"
    );
    assert_eq!(
        cached.deltas_since_base, fresh.deltas_since_base,
        "deltas_since_base 不一致 —— 缓存没有把这个计数跟着推进（它决定该不该打新 base）"
    );
    assert_eq!(fresh.deltas_since_base, 20);
    assert_eq!(
        text_of(&cached.value),
        (0..20).map(|round| format!("{round},")).collect::<String>()
    );
}
