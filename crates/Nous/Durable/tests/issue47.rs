//! #47：rename / read / unread 三条写命令与读投影。
//!
//! 判据：写走同一个 `Session`（`ObserverRegistry` 必须收到），水位只进不退，
//! unread 数来自独立种下的 entry id（不是拿实现重算一遍）。

use std::sync::{Arc, Mutex};

use nous_chord::context::Context;
use nous_chord::watch::WatchEnd;
use nous_durable::session::commands::{mark_read, rename_session, unread_map};
use nous_durable::session::{
    CommitObserver, CommitPublication, ObserverRegistry, Session,
};
use nous_durable::types::DocumentScope;
use nous_durable::{DocumentContent, Storage, StorageWrite};
use serde_json::{json, Value};

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-issue47-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<CommitPublication>>>);

impl CommitObserver for Recorder {
    fn advance(&self, publication: &CommitPublication, _: &Context) {
        self.0.lock().unwrap().push(publication.clone());
    }
    fn end(&self, _: &WatchEnd) {}
}

async fn seed(path: &std::path::Path) {
    let session = Session::open(path).unwrap();
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 2 })),
                StorageWrite::Conversation(json!({ "id": 1 })),
                StorageWrite::Entry(json!({
                    "id": 11, "conversationId": 1, "kind": "pi.user", "text": "one"
                })),
                StorageWrite::Entry(json!({
                    "id": 12, "conversationId": 1, "kind": "pi.user", "text": "two"
                })),
                StorageWrite::Entry(json!({
                    "id": 21, "conversationId": 2, "kind": "pi.user", "text": "other"
                })),
            ],
            nous_durable::session::CommitOptions::default(),
        )
        .await
        .unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn rename_writes_a_names_document_and_publishes_through_the_registry() {
    let path = temp_db("rename");
    seed(&path).await;
    let registry = ObserverRegistry::new();
    let recorder = Recorder::default();
    let _watch = registry.attach(Arc::new(recorder.clone()));
    let session = Session::with_observers(Storage::open(&path).unwrap(), registry);

    assert!(recorder.0.lock().unwrap().is_empty());
    let result = rename_session(&session, 1, "  改过的名字  ").await.unwrap();
    assert_eq!(result.id, 1);
    assert_eq!(result.name, "改过的名字");
    assert_eq!(recorder.0.lock().unwrap().len(), 1, "rename 必须广播");

    // 新文档就是 render_sessions 认的那张 names 表。
    let storage = Storage::open_read_only(&path).unwrap();
    let docs = storage
        .scan_documents(
            &nous_durable::DocumentQuery {
                scope: json!({ "kind": "session" }),
                kind: Some("qrd3wh.sessions".into()),
                at: nous_durable::DocumentPoint::Current,
            },
            100,
            None,
        )
        .unwrap()
        .unwrap();
    let names = docs
        .items
        .iter()
        .filter_map(|doc| {
            storage
                .materialize(doc["id"].as_i64()?, nous_durable::DocumentPoint::Current)
                .ok()
                .flatten()
        })
        .any(|doc| doc.value["names"]["1"] == json!("改过的名字"));
    assert!(names, "名字落在 qrd3wh.sessions 的 names 表里");

    let bad = rename_session(&session, 1, "   ").await;
    assert!(bad.is_err(), "trim 后为空必须拒绝");
    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn mark_read_advances_the_watermark_only_forward_and_publishes() {
    let path = temp_db("read");
    seed(&path).await;
    let registry = ObserverRegistry::new();
    let recorder = Recorder::default();
    let _watch = registry.attach(Arc::new(recorder.clone()));
    let session = Session::with_observers(Storage::open(&path).unwrap(), registry);

    let first = mark_read(&session, 1).await.unwrap();
    assert_eq!(first.id, 1);
    assert_eq!(first.unread, 0);
    assert!(first.read_at > 0, "readAt 是 epoch 毫秒");
    assert_eq!(recorder.0.lock().unwrap().len(), 1, "read 必须广播");
    assert_eq!(unread_map(&session).await.unwrap()["1"]["unread"], json!(0));

    // 新 entry 之后未读回 1；再打一次已读回到 0，水位只进不退。
    session
        .commit(
            vec![StorageWrite::Entry(json!({
                "id": 13, "conversationId": 1, "kind": "pi.user", "text": "three"
            }))],
            nous_durable::session::CommitOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(unread_map(&session).await.unwrap()["1"]["unread"], json!(1));
    let second = mark_read(&session, 1).await.unwrap();
    assert!(second.read_at >= first.read_at, "水位时间戳不倒退");
    assert_eq!(unread_map(&session).await.unwrap()["1"]["unread"], json!(0));

    let missing = mark_read(&session, 999).await;
    assert!(missing.is_err(), "不存在的会话该报错");
    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn unread_map_counts_entries_after_each_sessions_watermark() {
    let path = temp_db("unread");
    seed(&path).await;
    // 独立种下的水位：会话 1 读到 11（后面有 12 一条），会话 2 不种（默认 0）。
    let session = Session::open(&path).unwrap();
    // 种子用了 id 1..21；100 是测试自己订的文档 id，跟实现怎么分配无关。
    let doc_id = 100_i64;
    session
        .commit(
            vec![StorageWrite::DocumentCreate {
                id: doc_id,
                record: json!({
                    "id": doc_id, "kind": "qrd3wh.unread", "scope": DocumentScope::Session,
                }),
                content: DocumentContent::Base {
                    version: 1,
                    value: json!({ "marks": { "1": { "entryId": 11, "readAt": null } } }),
                },
            }],
            nous_durable::session::CommitOptions::default(),
        )
        .await
        .unwrap();
    let map = unread_map(&session).await.unwrap();
    assert_eq!(map["1"]["unread"], json!(1), "水位 11 之后只有 entry 12");
    assert_eq!(map["1"]["readAt"], Value::Null);
    assert_eq!(map["2"]["unread"], json!(0), "没有水位的会话给 0");
    assert_eq!(map["2"]["readAt"], Value::Null);
    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}
