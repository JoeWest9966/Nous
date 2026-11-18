//! documents 写路径判据：create → get → put → history 全经同一个 `Session`，
//! 观察者在每次写后收到一次 publication（不是第二条 Storage 绕过广播）。

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use nous_durable::session::publish::{Context, WatchEnd};
use nous_durable::session::{CommitObserver, CommitPublication, ObserverRegistry};
use nous_server::{AppState, DurableCommands, SessionObserver};
use serde_json::{json, Value};
use tower::ServiceExt;

mod common;

#[derive(Default)]
struct Counter(Mutex<usize>);
impl CommitObserver for Counter {
    fn advance(&self, _: &CommitPublication, _: &Context) { *self.0.lock().unwrap() += 1 }
    fn end(&self, _: &WatchEnd) {}
}

fn temp_db() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("nous-documents-e2e-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

async fn call(app: &axum::Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() { builder = builder.header("content-type", "application/json"); }
    let response = app.clone().oneshot(builder.body(Body::from(
        body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default(),
    )).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, value)
}

#[tokio::test]
async fn create_get_put_history_go_through_session_and_broadcast() {
    let path = temp_db();
    let registry = ObserverRegistry::new();
    let counter = Arc::new(Counter::default());
    registry.attach(counter.clone());
    let session = Arc::new(nous_durable::session::Session::with_observers(
        nous_durable::Storage::open(&path).unwrap(), registry.clone(),
    ));
    let commands = DurableCommands::with_session(session.clone(), registry);
    let state = AppState::durable_with_commands(
        nous_server::ImportedSource::open(&path).unwrap(), commands.clone(),
    );
    commands.attach(SessionObserver::new(state.clone()));
    let state = state.with_documents_backend(path.clone(), session);
    let app = nous_server::router(state);

    let preset = json!({
        "name": "desk", "summary": "一句话", "description": "说明",
        "category": ["写作"], "spec": null, "body": null
    });
    let (status, created) = call(&app, Method::POST, "/v1/documents/presets/Session-Template", Some(preset.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let (status, got) = call(&app, Method::GET, "/v1/documents/presets/Session-Template/desk", None).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["fields"]["summary"], json!("一句话"));

    let mut changed = preset.clone();
    changed["summary"] = json!("改过了");
    let (status, _) = call(&app, Method::PUT, "/v1/documents/presets/Session-Template/desk", Some(changed)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, history) = call(&app, Method::GET, "/v1/documents/presets/Session-Template/desk/history", None).await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert!(history["commits"].as_array().unwrap().len() >= 2, "{history}");
    assert!(*counter.0.lock().unwrap() >= 2, "create 和 put 两次写都要广播（get 不写）");

    let (status, diff) = call(&app, Method::GET, "/v1/documents/presets/Session-Template/desk/pending/diff", None).await;
    assert_eq!(status, StatusCode::OK, "pending/diff 在 session 作用域文档上必须可用: {diff}");
    assert_eq!(diff["after"]["summary"], json!("改过了"));

    let _ = std::fs::remove_file(&path);
}
