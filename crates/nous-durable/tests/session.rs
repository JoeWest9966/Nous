//! #13 commit ③ 判据：`Session` 的公开面与三条关闭语义。
//!
//! 这张文件里最不寻常的是第一条测试：**扫源码数方法**。Rust 没有反射，而「公开面就是 A3 那张表，
//! 多一个都不行」是用户定的判据 —— 让它机器判，比写在文档里靠人记强。#22 的端点层要按这张表写，
//! 它漂了就等于让端点层猜。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nous_chord::context::Context;
use nous_chord::watch::WatchEnd;
use nous_durable::session::{
    CommitObserver, CommitOptions, CommitPersistence, CommitPublication, ObserverRegistry,
    Session, SessionEnd, SessionError,
};
use nous_durable::storage::commit::CommitResult;
use nous_durable::types::EntryRecord;
use nous_durable::{
    DocumentContent, Refusal, StorageWrite, StoredDocument, Storage,
};
use serde_json::{json, Value};

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-session-{tag}-{}-{:?}.sqlite",
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

fn user_entry(id: i64, conversation_id: i64) -> StorageWrite {
    StorageWrite::Entry(json!({
        "id": id,
        "conversationId": conversation_id,
        "kind": "pi.user",
        "model": [{ "role": "user", "content": "hi", "timestamp": 1 }],
    }))
}

fn session_document(id: i64, content: DocumentContent) -> StorageWrite {
    StorageWrite::DocumentCreate {
        id,
        record: json!({
            "id": id,
            "kind": "qrd3wh.sessions",
            "scope": { "kind": "session" },
        }),
        content,
    }
}

fn base(value: Value) -> DocumentContent {
    DocumentContent::Base { version: 1, value }
}

// ── 1. 公开面就是 A3 那张表 ───────────────────────────────────────────────────

/// A3 的五条：`commit` / `close` / `closed` / `snapshot` / `get_entry`。
const A3_METHODS: [&str; 5] = ["close", "closed", "commit", "get_entry", "snapshot"];

#[test]
fn session_public_face_is_exactly_the_a3_table() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session/mod.rs"),
    )
    .expect("src/session/mod.rs 读得出来");

    let mut async_methods: Vec<String> = Vec::new();
    let mut sync_methods: Vec<String> = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub ") else {
            continue;
        };
        let (is_async, rest) = match rest.strip_prefix("async fn ") {
            Some(rest) => (true, rest),
            None => match rest.strip_prefix("fn ") {
                Some(rest) => (false, rest),
                None => continue,
            },
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if is_async {
            async_methods.push(name);
        } else {
            sync_methods.push(name);
        }
    }
    async_methods.sort();
    sync_methods.sort();

    assert_eq!(
        async_methods,
        A3_METHODS,
        "`Session` 的 async 方法必须就是 A3 的五条；加方法先改 #13 票面与 docs/storage.md §10.5"
    );
    // 同步的只有构造器：公开面不含「再多一个同步入口」。
    assert_eq!(
        sync_methods,
        vec!["open", "with_observers"],
        "同步 `pub fn` 只许是构造器"
    );
}

// ── 2. 一次提交：先落库、再发布，publication 就是库里的样子 ──────────────────

#[derive(Clone, Default)]
struct Recorder {
    seen: Arc<Mutex<Vec<CommitPublication>>>,
    ends: Arc<Mutex<Vec<WatchEnd>>>,
}

impl CommitObserver for Recorder {
    fn advance(&self, publication: &CommitPublication, _context: &Context) {
        self.seen.lock().unwrap().push(publication.clone());
    }
    fn end(&self, end: &WatchEnd) {
        self.ends.lock().unwrap().push(end.clone());
    }
}

#[tokio::test]
async fn a_commit_is_stored_before_it_is_published() {
    let path = temp_db("store-then-publish");
    let registry = ObserverRegistry::new();
    let recorder = Recorder::default();
    let _observation = registry.attach(Arc::new(recorder.clone()));
    let session = Session::with_observers(Storage::open(&path).unwrap(), registry);

    let outcome = session
        .commit(
            vec![
                conversation(2),
                user_entry(9, 2),
                session_document(31, base(json!({ "a": 1 }))),
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();

    let publication = outcome.publication.clone().expect("落库了就有 publication");
    assert_eq!(outcome.stored, 1);
    assert_eq!(outcome.refused, None);
    assert_eq!(publication.changes.len(), 3);
    // 观察者拿到的就是返回的那份，而且只有一份。
    assert_eq!(*recorder.seen.lock().unwrap(), vec![publication.clone()]);

    // 「先落库」是可查的：文档的 `createdAt` 是提交盖的章，等于这一批的 seq。
    let json = serde_json::to_value(&publication).unwrap();
    assert_eq!(json["seq"], json!(publication.seq));
    let document = json["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["type"] == json!("document"))
        .expect("publication 里有文档");
    assert_eq!(
        document["record"]["createdAt"],
        json!(publication.seq),
        "`createdAt` 是落库时盖的章：拿提交时那份副本就会漏掉它"
    );
    assert_eq!(document["value"], json!({ "a": 1 }));
    assert_eq!(document["ops"], json!([]), "创建时 ops 是空数组");
    assert!(
        document.get("conversationId").is_none(),
        "session 作用域的文档：`conversationId` 键缺席"
    );

    // 库里也真有：snapshot 读得回来，entry 带同一个 seq。
    let snapshot: StoredDocument = session.snapshot(31).await.unwrap().expect("文档在世");
    assert_eq!(snapshot.value, json!({ "a": 1 }));
    let (entry, seq): (EntryRecord, i64) = session.get_entry(9).await.unwrap().expect("entry 在");
    assert_eq!(entry.kind, "pi.user");
    assert_eq!(seq, publication.seq);

    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_delta_publishes_the_stored_value_and_the_submitted_ops() {
    let path = temp_db("delta-ops");
    let session = Session::open(&path).unwrap();
    session
        .commit(
            vec![session_document(31, base(json!({ "title": "Untitled" })))],
            CommitOptions::default(),
        )
        .await
        .unwrap();

    // 提交一批「不最小」的 op：先设成原值，再设成新值。publication 里必须是这两条。
    let ops = json!([
        ["s", ["title"], "Untitled"],
        ["s", ["title"], "Hello"],
    ]);
    let outcome = session
        .commit(
            vec![StorageWrite::DocumentChange {
                id: 31,
                content: DocumentContent::Delta { version: 1, ops: ops.clone() },
            }],
            CommitOptions::default(),
        )
        .await
        .unwrap();

    let publication = outcome.publication.expect("落库了");
    let change = match &publication.changes[0] {
        nous_durable::session::CommitChange::Document(change) => change,
        other => panic!("不是文档变更：{other:?}"),
    };
    let json = serde_json::to_value(change).unwrap();
    assert_eq!(json["ops"], ops, "ops 是提交上来的那批，不是重算的 diff");
    assert_eq!(json["value"], json!({ "title": "Hello" }), "value 是重放后的值");
    assert_eq!(json["version"], json!(1));

    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_refused_commit_stores_nothing_and_publishes_nothing() {
    let path = temp_db("refused");
    let session = Session::open(&path).unwrap();
    session
        .commit(vec![conversation(2)], CommitOptions::default())
        .await
        .unwrap();

    let outcome = session
        .commit(vec![conversation(2)], CommitOptions::default())
        .await
        .unwrap();
    assert_eq!(outcome.stored, 0);
    assert_eq!(
        outcome.refused,
        Some(Refusal::IdAlreadyBelongs {
            id: 2,
            table: nous_durable::TableName::Conversation,
        })
    );
    assert!(outcome.publication.is_none(), "没落库就没有可发布的事实");

    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn deferred_persistence_is_refused_not_silently_stored() {
    let path = temp_db("deferred");
    let session = Session::open(&path).unwrap();
    let limited = Storage::open_read_only(&path).unwrap();

    let error = session
        .commit(
            vec![conversation(2)],
            CommitOptions {
                persistence: CommitPersistence::Deferred { within_ms: Some(10) },
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SessionError::DeferredUnsupported), "{error}");

    // 明确拒绝的意思：什么都没写。
    assert_eq!(
        limited.conversation(2).unwrap().map(|record| record["id"].clone()),
        None
    );

    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

// ── 3. 关闭的三条语义 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn commit_after_close_is_a_clear_error_not_a_panic_or_a_hang() {
    let path = temp_db("after-close");
    let session = Session::open(&path).unwrap();
    session.close().await.unwrap();

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        session.commit(vec![conversation(2)], CommitOptions::default()),
    )
    .await
    .expect("关掉之后的提交不许挂住")
    .unwrap_err();
    assert!(matches!(error, SessionError::Closed), "{error}");

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn close_is_idempotent() {
    let path = temp_db("close-twice");
    let session = Session::open(&path).unwrap();
    session.close().await.unwrap();
    session.close().await.expect("第二次 close() 返回 Ok");

    assert_eq!(session.closed().await, SessionEnd::Closed);
    let _ = std::fs::remove_file(&path);
}

#[derive(Clone, Default)]
struct Timeline {
    published_at: Arc<Mutex<Option<std::time::Instant>>>,
}

impl CommitObserver for Timeline {
    fn advance(&self, _publication: &CommitPublication, _context: &Context) {
        *self.published_at.lock().unwrap() = Some(std::time::Instant::now());
    }
    fn end(&self, _end: &WatchEnd) {}
}

/// 在飞的提交要**走完**，不是被 close 掐断。
///
/// 这条测试必须证明「close 确实等了」，而不是「两边恰好排上了队」。做法：一批足够大、大到
/// close() 被调用时它肯定还在跑；然后断言**发布时刻晚于 close 的调用时刻** —— 只有 close 等过，
/// 这个顺序才可能出现。批次太小会让这条测试变成白跑，所以断言里直接说清了。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_waits_for_the_commit_that_is_in_flight() {
    let path = temp_db("in-flight");
    let registry = ObserverRegistry::new();
    let timeline = Timeline::default();
    let _observation = registry.attach(Arc::new(timeline.clone()));
    let session = Arc::new(Session::with_observers(
        Storage::open(&path).unwrap(),
        registry,
    ));

    // 一批足够大的写，让提交在 close() 进来时还在跑。
    let mut batch = vec![conversation(2)];
    for id in 100..20_100 {
        batch.push(StorageWrite::Entry(json!({
            "id": id,
            "conversationId": 2,
            "kind": "pi.user",
            "model": [{ "role": "user", "content": "x", "timestamp": 1 }],
        })));
    }
    let committing = {
        let session = session.clone();
        tokio::spawn(async move { session.commit(batch, CommitOptions::default()).await })
    };
    // 让提交先跑起来（另一个 worker 线程上），再进 close。
    tokio::time::sleep(Duration::from_millis(5)).await;
    let close_called_at = std::time::Instant::now();
    let close_result = {
        let session = session.clone();
        tokio::spawn(async move { session.close().await })
    };

    let (commit_result, close_result) = tokio::time::timeout(Duration::from_secs(120), async {
        (committing.await.unwrap(), close_result.await.unwrap())
    })
    .await
    .expect("不许死锁");

    let outcome = commit_result.expect("在飞的提交必须走完 —— 被 close 掐断的话这里是 Err(Closed)");
    assert_eq!(outcome.stored, 1);
    close_result.expect("close 之后提交都收了场");
    assert_eq!(session.closed().await, SessionEnd::Closed);

    let published_at = timeline
        .published_at
        .lock()
        .unwrap()
        .expect("提交发布过，才算走完");
    assert!(
        published_at > close_called_at,
        "这条测试没验证到等待：提交在 close() 之前就发布完了。把批次调大，别删断言"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_storage_failure_closes_the_session_and_reports_it() {
    let path = temp_db("failed");
    let session = Session::open(&path).unwrap();

    // 缺 `id` 的记录是形状错误（不是拒绝）：存储层报错，会话照上游语义自己关掉。
    let error = session
        .commit(
            vec![StorageWrite::Entry(json!({ "conversationId": 2, "kind": "pi.user" }))],
            CommitOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SessionError::Storage(_)), "{error}");

    match session.closed().await {
        SessionEnd::Failed { error } => assert!(!error.is_empty()),
        other => panic!("失败之后 closed() 应当报 failed，实际 {other:?}"),
    }
    // 失败之后就是关了的会话。
    assert!(matches!(
        session.commit(vec![conversation(2)], CommitOptions::default()).await,
        Err(SessionError::Closed)
    ));
    // `close()` 仍然幂等地返回 Ok（存储已经关了）。
    session.close().await.expect("失败之后 close() 仍返回 Ok");

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn reads_after_close_are_errors() {
    let path = temp_db("read-after-close");
    let session = Session::open(&path).unwrap();
    session
        .commit(vec![conversation(2), user_entry(9, 2)], CommitOptions::default())
        .await
        .unwrap();
    session.close().await.unwrap();

    assert!(matches!(session.snapshot(9).await, Err(SessionError::Closed)));
    assert!(matches!(session.get_entry(9).await, Err(SessionError::Closed)));
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn observers_end_once_with_the_reason() {
    let path = temp_db("observe-end");
    let registry = ObserverRegistry::new();
    let recorder = Recorder::default();
    let _observation = registry.attach(Arc::new(recorder.clone()));
    let session = Session::with_observers(Storage::open(&path).unwrap(), registry);

    session.close().await.unwrap();
    session.close().await.unwrap();
    assert_eq!(*recorder.ends.lock().unwrap(), vec![WatchEnd::Ended]);

    let _ = std::fs::remove_file(&path);
}

// ── 4. 回执形状（`CommitResult` 多出来的那一栏） ──────────────────────────────

#[test]
fn stored_batches_describe_what_the_commit_stored() {
    let path = temp_db("stored-batches");
    let mut storage = Storage::open(&path).unwrap();
    let result: CommitResult = storage
        .commit(
            &[
                vec![conversation(2), user_entry(9, 2)],
                vec![session_document(31, base(json!({ "a": 1 })))],
            ],
            |_, _| {},
        )
        .unwrap();

    assert_eq!(result.stored, 2);
    assert_eq!(result.batches.len(), 2, "一批一条回执");
    assert!(result.batches[0].seq < result.batches[1].seq);
    assert_eq!(result.batches[0].changes.len(), 2);
    assert_eq!(result.batches[1].changes.len(), 1);
    drop(storage);
    let _ = std::fs::remove_file(&path);
}
