//! #66：会话列表三处投影的公共接缝判据。
//!
//! 判据打在 `render_sessions` 的输出上：它是 durable 交给 `/v1/sessions` 的投影，
//! 测试不碰 SQL、不碰进程内时钟，只看「写进去什么 → 会话列表看见什么」。

use std::collections::BTreeMap;

use nous_durable::storage::import::ImportedStore;
use nous_durable::storage::sessions::{render_sessions, render_sessions_with_levels};
use nous_durable::session::{CommitOptions, Session};
use nous_durable::{DocumentContent, StorageWrite};
use serde_json::json;

fn temp_db(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-projection-66-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

fn sessions(path: &std::path::Path) -> Vec<nous_durable::storage::sessions::SessionItem> {
    render_sessions(&ImportedStore::read(path).unwrap()).unwrap()
}

fn store_with_agent_model(model_id: &str) -> ImportedStore {
    let path = temp_db("levels-fixture");
    let _store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let session = Session::open(&path).unwrap();
            session
                .commit(
                    vec![
                        StorageWrite::Conversation(json!({ "id": 1 })),
                        StorageWrite::DocumentCreate {
                            id: 100,
                            record: json!({
                                "id": 100,
                                "kind": "pi.agent",
                                "key": "agent-1",
                                "scope": { "kind": "conversation", "conversationId": 1 },
                                "history": "latest",
                                "fork": "initial"
                            }),
                            content: DocumentContent::Base {
                                version: 1,
                                value: json!({
                                    "model": { "provider": "p", "modelId": model_id },
                                    "thinkingLevel": "off"
                                }),
                            },
                        },
                    ],
                    CommitOptions::default(),
                )
                .await
                .unwrap();
            session.close().await.unwrap();
        });
    let store = ImportedStore::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    store
}

#[tokio::test]
async fn updated_at_is_the_newest_entry_timestamp_and_stays_null_without_entries() {
    let path = temp_db("updated-at");
    let session = Session::open(&path).unwrap();
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 1 })),
                StorageWrite::Conversation(json!({ "id": 2 })),
                StorageWrite::Entry(json!({
                    "id": 10, "conversationId": 1, "kind": "pi.user",
                    "model": [{ "timestamp": 1000 }]
                })),
                StorageWrite::Entry(json!({
                    "id": 11, "conversationId": 1, "kind": "pi.assistant",
                    "model": [{ "timestamp": 2000 }]
                })),
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    let items = sessions(&path);
    let first = items.iter().find(|item| item.id == 1).unwrap();
    let empty = items.iter().find(|item| item.id == 2).unwrap();
    assert_eq!(first.updated_at, Some(2000), "最新 entry 的时间戳（不是 1000）");
    assert_eq!(empty.updated_at, None, "没有 entry 的会话必须是 null，不是 0 / 当前时间");

    session
        .commit(
            vec![StorageWrite::Entry(json!({
                "id": 12, "conversationId": 1, "kind": "pi.user",
                "model": [{ "timestamp": 3000 }]
            }))],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    let items = sessions(&path);
    let after = items.iter().find(|item| item.id == 1).unwrap();
    assert_eq!(after.updated_at, Some(3000));
    assert!(after.updated_at > first.updated_at, "追加 entry 后必须严格递增");

    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn archived_tombstones_are_read_from_the_session_document() {
    let path = temp_db("archived");
    let session = Session::open(&path).unwrap();
    // 两个会话共用一份 qrd3wh.sessions 单例文档（坑：每个会话一次 Create 会撞地址）。
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 1 })),
                StorageWrite::Conversation(json!({ "id": 2 })),
                StorageWrite::DocumentCreate {
                    id: 100,
                    record: json!({
                        "id": 100,
                        "kind": "qrd3wh.sessions",
                        "scope": { "kind": "session" }
                    }),
                    content: DocumentContent::Base {
                        version: 1,
                        value: json!({
                            "archived": {
                                "1": { "at": 1794569866329i64, "gen": "20261113", "purged": false },
                                "2": { "at": 1794826245070i64, "gen": "20261116", "purged": true }
                            }
                        }),
                    },
                },
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    let items = sessions(&path);
    let first = items.iter().find(|item| item.id == 1).unwrap();
    let second = items.iter().find(|item| item.id == 2).unwrap();
    assert_eq!(
        serde_json::to_value(first.archived.as_ref().unwrap()).unwrap(),
        json!({ "at": 1794569866329i64, "gen": "20261113", "purged": false }),
        "归档墓碑逐字来自 qrd3wh.sessions.archived"
    );
    assert_eq!(
        serde_json::to_value(second.archived.as_ref().unwrap()).unwrap(),
        json!({ "at": 1794826245070i64, "gen": "20261116", "purged": true }),
        "purge 后 purged=true"
    );

    session.close().await.unwrap();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn thinking_levels_come_from_the_catalog_and_are_empty_when_it_is_absent() {
    let store = store_with_agent_model("m");
    let empty = render_sessions(&store).unwrap();
    assert_eq!(empty[0].thinking_levels, Vec::<String>::new(), "编目为空就 []，不编造档位");

    let catalog = BTreeMap::from([("p/m".to_string(), vec!["low".into(), "high".into()])]);
    let items = render_sessions_with_levels(&store, &catalog).unwrap();
    assert_eq!(
        items[0].thinking_levels,
        vec!["low", "high"],
        "档位表来自 device 编目里该会话当前模型"
    );
}
