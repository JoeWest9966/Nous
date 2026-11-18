//! #47：写端点的 HTTP 形状 + ObserverRegistry 广播回归。
//!
//! 打真 Router + 临时 sqlite；`Recorder` 挂在 `DurableCommands` 的 registry 上，
//! 照 durable 的 `create_conversation_publishes_through_the_observer_registry` 判「有没有广播」。

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

/// 快照目录跟源码走，不依赖 cwd。
fn snapshot(test: &str, value: &Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{test}.json"));
    let actual = serde_json::to_string_pretty(value).unwrap() + "\n";
    match std::fs::read_to_string(&path) {
        Ok(expected) if expected == actual => {}
        Ok(expected) => panic!(
            "字段形状快照漂移：{}\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
            path.display()
        ),
        Err(_) => panic!(
            "快照不存在：{}（按 actual 建立文件）\n--- actual ---\n{actual}",
            path.display()
        ),
    }
}

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-issue47-http-{tag}-{}-{:?}.sqlite",
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
                StorageWrite::Entry(json!({
                    "id": 11, "conversationId": 1, "kind": "pi.user", "text": "one"
                })),
                StorageWrite::Entry(json!({
                    "id": 12, "conversationId": 1, "kind": "pi.user", "text": "two"
                })),
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    session.close().await.unwrap();
}

/// 带观察者的真装配：与 `main` 同构（一个 Session、一个 registry、一个 hub）。
fn app_with_recorder(path: &PathBuf) -> (Router, Arc<Recorder>, Arc<ObserverRegistry>) {
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
    (router(state), recorder, registry)
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

#[tokio::test]
async fn unread_is_a_flat_map_with_every_session_and_null_read_at() {
    let path = temp_db("unread-shape");
    seed(&path).await;
    let (app, _, _registry) = app_with_recorder(&path);
    let (status, body) = call(&app, "GET", "/v1/unread", None).await;
    assert_eq!(status, StatusCode::OK, "GET /v1/unread 200：{body}");
    let map = body.as_object().expect("unread 是平铺 map，没有 items 包一层");
    assert_eq!(map.len(), 2, "覆盖所有会话");
    for id in ["1", "2"] {
        assert!(map[id]["unread"].is_u64(), "{id}.unread 是数");
        assert!(map[id]["readAt"].is_null(), "{id}.readAt 还没标过，null");
    }
    assert_eq!(map["1"]["unread"], json!(0));
    snapshot("unread_is_a_flat_map_with_every_session_and_null_read_at", &body);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn read_receipt_pins_fields_and_publishes_through_the_observer_registry() {
    let path = temp_db("read-shape");
    seed(&path).await;
    let (app, recorder, _registry) = app_with_recorder(&path);
    let (status, receipt) = call(&app, "POST", "/v1/read/1", None).await;
    assert_eq!(status, StatusCode::OK, "POST /v1/read/:id 200：{receipt}");
    assert_eq!(receipt["id"], json!(1));
    assert!(receipt["readAt"].is_i64(), "readAt 是 epoch 毫秒");
    assert_eq!(receipt["unread"], json!(0));
    assert_eq!(
        *recorder.0.lock().unwrap(),
        1,
        "read 必须经 Session 广播（#42 的坑）"
    );

    // 水位真推进了：再拉 unread，会话 1 已读到 0。
    let (_, unread) = call(&app, "GET", "/v1/unread", None).await;
    assert_eq!(unread["1"]["unread"], json!(0));
    assert!(unread["1"]["readAt"].is_i64());

    let (status, missing) = call(&app, "POST", "/v1/read/999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不存在 404：{missing}");
    assert!(missing["error"].is_string());
    let (status, wrong_method) = call(&app, "GET", "/v1/read/1", None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "只许 POST：{wrong_method}");
    // 回执形状快照：readAt 每次不同，归一后钉 id / unread 与字段集合。
    let mut stable = receipt;
    stable["readAt"] = json!(0);
    snapshot("read_receipt_pins_fields_and_publishes_through_the_observer_registry", &stable);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn rename_trims_rejects_empty_and_publishes_through_the_observer_registry() {
    let path = temp_db("rename-shape");
    seed(&path).await;
    let (app, recorder, _registry) = app_with_recorder(&path);
    let (status, renamed) = call(
        &app,
        "POST",
        "/v1/sessions/1/rename",
        Some(json!({ "name": "  改过的名字  " })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rename 200：{renamed}");
    assert_eq!(renamed["id"], json!(1));
    assert_eq!(renamed["name"], json!("改过的名字"), "回包是 trim 后的生效值");
    assert_eq!(
        *recorder.0.lock().unwrap(),
        1,
        "rename 必须经 Session 广播（#42 的坑）"
    );

    // 名字真的进了会话树。
    let (_, sessions) = call(&app, "GET", "/v1/sessions", None).await;
    let one = sessions["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == json!(1))
        .unwrap();
    assert_eq!(one["name"], json!("改过的名字"));

    let (status, empty) = call(
        &app,
        "POST",
        "/v1/sessions/1/rename",
        Some(json!({ "name": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空名字 400：{empty}");
    assert!(empty["error"].is_string());

    let long = "名字".repeat(80);
    let (status, truncated) = call(
        &app,
        "POST",
        "/v1/sessions/1/rename",
        Some(json!({ "name": long })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(truncated["name"].as_str().unwrap().chars().count(), 120);
    snapshot("rename_trims_rejects_empty_and_publishes_through_the_observer_registry", &renamed);
    let _ = std::fs::remove_file(&path);
}

/// 观察者注册表是共享的 `Arc` —— 即使 handler 内部另开 guard 也不该绕过广播。
#[tokio::test]
async fn a_second_rename_reaches_the_same_observer_registry() {
    let path = temp_db("rename-twice");
    seed(&path).await;
    let (app, recorder, registry) = app_with_recorder(&path);
    call(
        &app,
        "POST",
        "/v1/sessions/1/rename",
        Some(json!({ "name": "one" })),
    )
    .await;
    call(
        &app,
        "POST",
        "/v1/sessions/2/rename",
        Some(json!({ "name": "two" })),
    )
    .await;
    assert_eq!(*recorder.0.lock().unwrap(), 2, "两次 rename 两次广播");
    drop(registry);
    let _ = std::fs::remove_file(&path);
}
