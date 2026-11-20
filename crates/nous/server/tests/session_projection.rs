//! #66：三处投影在对外接缝 `GET /v1/sessions` 上的实测。
//!
//! 真 durable 库 → 真 Router → 真 JSON。不靠假源 mock 掉投影。

mod common;

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_device::{CatalogDiagnostics, ModelItem, ModelRef, ModelsResponse};
use nous_durable::session::{CommitOptions, Session};
use nous_durable::{DocumentContent, StorageWrite};
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-server-projection-66-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn catalog() -> ModelsResponse {
    ModelsResponse {
        default: Some(ModelRef {
            provider: "p".into(),
            model_id: "m".into(),
        }),
        items: vec![ModelItem {
            provider: "p".into(),
            model_id: "m".into(),
            name: "Model M".into(),
            thinking_levels: vec!["low".into(), "high".into()],
            context_window: None,
            max_tokens: None,
        }],
        catalog: CatalogDiagnostics {
            state: "ok",
            source: None,
            path: PathBuf::from("/test/models.json"),
        },
    }
}

#[tokio::test]
async fn get_sessions_shows_the_catalog_levels_and_a_tombstone_unchanged() {
    let path = temp_db("http-projection");
    let session = Session::open(&path).unwrap();
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 1 })),
                StorageWrite::Entry(json!({
                    "id": 10, "conversationId": 1, "kind": "pi.user",
                    "model": [{ "timestamp": 1000 }]
                })),
                StorageWrite::DocumentCreate {
                    id: 100,
                    record: json!({
                        "id": 100, "kind": "pi.agent", "key": "agent-1",
                        "scope": { "kind": "conversation", "conversationId": 1 },
                        "history": "latest", "fork": "initial"
                    }),
                    content: DocumentContent::Base {
                        version: 1,
                        value: json!({
                            "model": { "provider": "p", "modelId": "m" },
                            "thinkingLevel": "off"
                        }),
                    },
                },
                StorageWrite::DocumentCreate {
                    id: 101,
                    record: json!({
                        "id": 101, "kind": "qrd3wh.sessions",
                        "scope": { "kind": "session" }
                    }),
                    content: DocumentContent::Base {
                        version: 1,
                        value: json!({
                            "archived": {
                                "1": { "at": 1794569866329i64, "gen": "20261113", "purged": false }
                            }
                        }),
                    },
                },
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    session.close().await.unwrap();

    let app = router(AppState::durable_with_commands(
        nous_server::ImportedSource::open(&path).unwrap(),
        nous_server::DurableCommands::open(&path).unwrap(),
    ).with_models(catalog()));
    let (status, body) = get(&app, "/v1/sessions").await;
    assert_eq!(status, StatusCode::OK, "实测 {body}");
    let item = &body["items"][0];
    assert_eq!(item["updatedAt"], json!(1000));
    assert_eq!(item["thinkingLevels"], json!(["low", "high"]));
    assert_eq!(
        item["archived"],
        json!({ "at": 1794569866329i64, "gen": "20261113", "purged": false })
    );
    let _ = std::fs::remove_file(&path);
}
