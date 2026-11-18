//! #48 第二片：durable 侧只加**只读** `revisions(id)`，以及经 `Session` 的文档写命令。
//!
//! 接缝：① `Storage::revisions` 的公开查询；② `session::commands::write_document`
//! 与既有 submit/fork 同一把锁 / 同一事务 / 同一 ObserverRegistry。

use std::sync::{Arc, Mutex};

use nous_durable::session::publish::{Context, WatchEnd};
use nous_durable::session::{CommitObserver, ObserverRegistry, Session};
use nous_durable::{DocumentContent, Storage, StorageWrite, session::commands::DocumentMutation};
use serde_json::json;

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-documents-durable-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

#[derive(Default)]
struct CountObserver(Mutex<usize>);
impl CommitObserver for CountObserver {
    fn advance(&self, _: &nous_durable::session::CommitPublication, _: &Context) {
        *self.0.lock().unwrap() += 1;
    }
    fn end(&self, _: &WatchEnd) {}
}

#[test]
fn revisions_are_readable_in_seq_order_and_write_broadcasts_once() {
    let path = temp_db("rev");
    let registry = ObserverRegistry::new();
    let observer = Arc::new(CountObserver::default());
    registry.attach(observer.clone());
    let session = Session::with_observers(Storage::open(&path).unwrap(), registry);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let record = json!({
        "id": 1, "kind": "presets.Session-Template", "key": "desk",
        "scope": { "kind": "conversation", "conversationId": 1 },
        "history": "rewindable", "fork": "asOf"
    });
    runtime.block_on(async {
        let outcome = nous_durable::session::commands::write_document(
            &session,
            vec![DocumentMutation::Create {
                kind: "presets.Session-Template".into(),
                key: "desk".into(),
                value: json!({ "name": "desk" }),
            }],
        )
        .await
        .unwrap();
        let _ = (record, StorageWrite::DocumentCreate { id: 1, record: json!({}), content: DocumentContent::Base { version: 1, value: json!({}) } });
        assert_eq!(outcome.changes.len(), 1);
    });
    assert_eq!(*observer.0.lock().unwrap(), 1, "一次写 = 一次广播");

    // 第二条：base + delta，revisions 必须给出两条，seq 升序。
    let probe = Storage::open(&path).unwrap();
    let doc_id = probe.find_document(&json!({
        "kind": "presets.Session-Template", "scope": { "kind": "session" }, "key": "desk"
    }), nous_durable::DocumentPoint::Current).unwrap().unwrap()
        .get("id").and_then(|v| v.as_i64()).unwrap();
    runtime.block_on(async {
        nous_durable::session::commands::write_document(
            &session,
            vec![DocumentMutation::Delta { id: doc_id, version: 1, ops: json!([["s", ["name"], "desk-2"]]) }],
        )
        .await
        .unwrap();
    });

    let storage = Storage::open(&path).unwrap();
    let revisions = storage.revisions(doc_id).unwrap();
    assert_eq!(revisions.len(), 2, "base + 一条 delta：{revisions:?}");
    assert!(revisions[0].0 < revisions[1].0);
    assert_eq!(revisions[0].1, "base");
    assert_eq!(revisions[1].1, "delta");
    let _ = std::fs::remove_file(&path);
}
