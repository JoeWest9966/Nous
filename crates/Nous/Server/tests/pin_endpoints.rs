//! #49：置顶端点的 HTTP 形状、幂等、排序与观察者广播。
//!
//! 接缝：真 Router + 临时 sqlite + 挂在同一个 `ObserverRegistry` 上的 Recorder。
//! 不读库文件断言行为：pin 是否落盘、是否排序，都通过 `GET /v1/sessions` 看。

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_durable::session::publish::{Context, WatchEnd};
use nous_durable::session::{
    CommitObserver, CommitOptions, CommitPublication, ObserverRegistry, Session,
};
use nous_durable::{Storage, StorageWrite};
use nous_server::{router, AppState, DurableCommands};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-issue49-pin-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

async fn seed(path: &PathBuf) {
    let session = Session::open(path).unwrap();
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 1 })),
                StorageWrite::Conversation(json!({ "id": 2 })),
                StorageWrite::Conversation(json!({ "id": 3 })),
                StorageWrite::Conversation(json!({ "id": 4 })),
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    session.close().await.unwrap();
}

fn app_with_recorder(path: &PathBuf) -> (Router, Arc<Recorder>) {
    let registry = ObserverRegistry::new();
    let session = Arc::new(Session::with_observers(
        Storage::open(path).unwrap(),
        registry.clone(),
    ));
    let commands = DurableCommands::with_session(session, registry.clone());
    let recorder = Arc::new(Recorder::default());
    let _ = registry.attach(recorder.clone());
    let state = AppState::durable_with_commands(
        nous_server::ImportedSource::open(path).unwrap(),
        commands,
    );
    (router(state), recorder)
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<usize>>);

impl CommitObserver for Recorder {
    fn advance(&self, _: &CommitPublication, _: &Context) {
        *self.0.lock().unwrap() += 1;
    }
    fn end(&self, _: &WatchEnd) {}
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(body) => Body::from(serde_json::to_vec(&body).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn session_order(body: &Value) -> Vec<(i64, bool)> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["id"].as_i64().unwrap(), item["pinned"].as_bool().unwrap()))
        .collect()
}

#[tokio::test]
async fn pin_replays_by_request_id_and_orders_pinned_sessions_first() {
    let path = temp_db("shape-and-replay");
    seed(&path).await;
    let (app, recorder) = app_with_recorder(&path);

    // 没给 requestId 是 400（幂等锚是必填的）。
    let (status, error) = call(&app, "POST", "/v1/sessions/2/pin", Some(json!({ "pinned": true }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "缺 requestId 400：{error}");
    assert!(error["error"].is_string());
    assert_eq!(*recorder.0.lock().unwrap(), 0, "400 不该提交");

    let (status, first) = call(
        &app,
        "POST",
        "/v1/sessions/2/pin",
        Some(json!({ "pinned": true, "requestId": "pin-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pin 200：{first}");
    assert_eq!(first["id"], json!(2));
    assert_eq!(first["pinned"], json!(true));
    assert_eq!(first["changed"], json!(true));
    assert_eq!(first["requestId"], json!("pin-1"));
    assert_eq!(first["version"], json!(1));
    assert_eq!(*recorder.0.lock().unwrap(), 1, "pin 必须经 Session 广播（#42 的坑）");

    // 同一锚重放：回包逐字一致，不涨版本、不新增广播。
    let (status, replay) = call(
        &app,
        "POST",
        "/v1/sessions/2/pin",
        Some(json!({ "pinned": true, "requestId": "pin-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, first, "同一个 requestId 回放第一次的回包");
    assert_eq!(*recorder.0.lock().unwrap(), 1, "重放不落新提交");

    // 再置顶 4：新置顶排最前；版本号从已有锚 +1。
    let (_, second) = call(
        &app,
        "POST",
        "/v1/sessions/4/pin",
        Some(json!({ "pinned": true, "requestId": "pin-2" })),
    )
    .await;
    assert_eq!(second["id"], json!(4));
    assert_eq!(second["version"], json!(2));
    let (_, sessions) = call(&app, "GET", "/v1/sessions", None).await;
    assert_eq!(
        session_order(&sessions),
        vec![(4, true), (2, true), (3, false), (1, false)],
        "置顶在前，其余保持原顺序"
    );

    // 取消置顶 2：从 ids 移除，回 changed:true。
    let (_, unpinned) = call(
        &app,
        "POST",
        "/v1/sessions/2/pin",
        Some(json!({ "pinned": false, "requestId": "pin-3" })),
    )
    .await;
    assert_eq!(unpinned["pinned"], json!(false));
    assert_eq!(unpinned["changed"], json!(true));
    assert_eq!(unpinned["version"], json!(3));
    let (_, sessions) = call(&app, "GET", "/v1/sessions", None).await;
    assert_eq!(session_order(&sessions)[0], (4, true));
    assert!(session_order(&sessions)[1..].iter().all(|(_, pinned)| !pinned));

    // 重复置顶同一个会话：回 changed:false，但仍是一次提交（锚要记下来）。
    let (_, same) = call(
        &app,
        "POST",
        "/v1/sessions/4/pin",
        Some(json!({ "pinned": true, "requestId": "pin-4" })),
    )
    .await;
    assert_eq!(same["changed"], json!(false));
    assert_eq!(same["version"], json!(4));
    assert_eq!(*recorder.0.lock().unwrap(), 4);

    let path = path.clone();
    drop(app);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn pin_keeps_only_the_newest_fifty_idempotency_anchors() {
    let path = temp_db("fifty-anchors");
    seed(&path).await;
    let (app, recorder) = app_with_recorder(&path);

    for version in 1..=55 {
        let (status, body) = call(
            &app,
            "POST",
            "/v1/sessions/1/pin",
            Some(json!({ "pinned": version % 2 == 0, "requestId": format!("anchor-{version}") })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["version"], json!(version));
    }
    // 旧锚被淘汰：重放 anchor-1 就是一次全新的提交（version 56）。
    let (_, body) = call(
        &app,
        "POST",
        "/v1/sessions/1/pin",
        Some(json!({ "pinned": true, "requestId": "anchor-1" })),
    )
    .await;
    assert_eq!(body["version"], json!(56), "淘汰的锚不再幂等");
    assert_eq!(body["requestId"], json!("anchor-1"));
    assert_eq!(*recorder.0.lock().unwrap(), 56);

    // 最近的锚还在：重放 anchor-55 不新增提交。
    let (_, body) = call(
        &app,
        "POST",
        "/v1/sessions/1/pin",
        Some(json!({ "pinned": false, "requestId": "anchor-55" })),
    )
    .await;
    assert_eq!(body["version"], json!(55), "最近 50 个锚仍在，回放旧回包");
    assert_eq!(*recorder.0.lock().unwrap(), 56);

    let path = path.clone();
    drop(app);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn concurrent_pins_do_not_lose_an_idempotency_anchor() {
    let path = temp_db("concurrent-anchors");
    seed(&path).await;
    let (app, _) = app_with_recorder(&path);

    let first = call(
        &app,
        "POST",
        "/v1/sessions/2/pin",
        Some(json!({ "pinned": true, "requestId": "race-1" })),
    );
    let second = call(
        &app,
        "POST",
        "/v1/sessions/2/pin",
        Some(json!({ "pinned": true, "requestId": "race-2" })),
    );
    let (a, b) = tokio::join!(first, second);
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    let versions = [a.1["version"].as_i64().unwrap(), b.1["version"].as_i64().unwrap()];
    assert_eq!(versions, [1, 2], "并发两次提交必须拿到不同 version");

    // 两个锚都要能回放，且回包逐字等于当时那次。
    let (_, replay_a) = call(
        &app,
        "POST",
        "/v1/sessions/2/pin",
        Some(json!({ "pinned": false, "requestId": "race-1" })),
    )
    .await;
    assert_eq!(replay_a, a.1, "race-1 不能被并发覆盖");
    let (_, replay_b) = call(
        &app,
        "POST",
        "/v1/sessions/2/pin",
        Some(json!({ "pinned": false, "requestId": "race-2" })),
    )
    .await;
    assert_eq!(replay_b, b.1, "race-2 不能被并发覆盖");
}

#[tokio::test]
async fn pin_on_an_unknown_session_is_404() {
    let path = temp_db("missing-session");
    seed(&path).await;
    let (app, recorder) = app_with_recorder(&path);
    let (status, body) = call(
        &app,
        "POST",
        "/v1/sessions/999/pin",
        Some(json!({ "pinned": true, "requestId": "pin-missing" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"].is_string());
    assert_eq!(*recorder.0.lock().unwrap(), 0);
    let path = path.clone();
    drop(app);
    let _ = std::fs::remove_file(path);
}
