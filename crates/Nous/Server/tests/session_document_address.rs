//! 建会话的重复地址回归：同一份库连续建两条会话，第二次绝不能再 DocumentCreate
//! 一份 session 作用域的 `qrd3wh.sessions` —— 那个地址只允许一个在世化身。
//!
//! 这里打真 Router + 临时 sqlite，和 pin/rename 两个端点一样从 `GET /v1/sessions` 看结果。

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
use nous_durable::Storage;
use nous_server::{router, AppState, DurableCommands};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-issue-session-address-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
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

/// 两条会话必须都 201，名字都在 `GET /v1/sessions` 上，且每次写都广播。
#[tokio::test]
async fn creating_a_second_conversation_reuses_the_session_scope_names_document() {
    let path = temp_db("two-creates");
    let session = Session::open(&path).unwrap();
    session
        .commit(vec![], CommitOptions::default())
        .await
        .unwrap();
    session.close().await.unwrap();
    let (app, recorder) = app_with_recorder(&path);

    let (first_status, first) = call(&app, "POST", "/v1/sessions", Some(json!({ "name": "甲" }))).await;
    assert_eq!(first_status, StatusCode::CREATED, "第一次建会话：{first}");
    let (second_status, second) = call(&app, "POST", "/v1/sessions", Some(json!({ "name": "乙" }))).await;
    assert_eq!(second_status, StatusCode::CREATED, "第二次建会话不该撞文档地址：{second}");

    let first_id = first["id"].as_i64().unwrap();
    let second_id = second["id"].as_i64().unwrap();
    assert_ne!(first_id, second_id, "两条会话的 id 必须不同");

    let (status, list) = call(&app, "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "两条会话都要在树里");
    for (id, name) in [(first_id, "甲"), (second_id, "乙")] {
        let item = items
            .iter()
            .find(|item| item["id"] == json!(id))
            .unwrap_or_else(|| panic!("会话 {id} 不在 GET /v1/sessions：{list}"));
        assert_eq!(item["name"], json!(name), "会话 {id} 的名字丢了");
    }
    assert_eq!(
        *recorder.0.lock().unwrap(),
        2,
        "两次 create 都广播一次"
    );
    let _ = std::fs::remove_file(&path);
}

/// 置顶 + 后续 rename / create 都必须保住 `pins`（同一份文档只改 names）。
#[tokio::test]
async fn rename_and_create_keep_the_pinned_state_on_the_shared_document() {
    let path = temp_db("pin-survives");
    let session = Session::open(&path).unwrap();
    session
        .commit(vec![], CommitOptions::default())
        .await
        .unwrap();
    session.close().await.unwrap();
    let (app, _recorder) = app_with_recorder(&path);

    let (status, first) = call(&app, "POST", "/v1/sessions", Some(json!({ "name": "甲" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let id = first["id"].as_i64().unwrap();

    let (status, pinned) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{id}/pin"),
        Some(json!({ "pinned": true, "requestId": "pin-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pinned}");

    // 改名在当前代码里会把 pins 整份丢掉；修好后必须还在。
    let (status, renamed) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{id}/rename"),
        Some(json!({ "name": "甲改" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");

    // 再建一条：新的 names 写进同一份文档，也不能把 pins 抹掉。
    let (status, second) = call(&app, "POST", "/v1/sessions", Some(json!({ "name": "乙" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");

    let (_, list) = call(&app, "GET", "/v1/sessions", None).await;
    let items = list["items"].as_array().unwrap();
    let item = items.iter().find(|item| item["id"] == json!(id)).unwrap();
    assert_eq!(item["name"], json!("甲改"));
    assert_eq!(item["pinned"], json!(true), "pins 被 rename/create 抹掉了：{list}");
    let _ = std::fs::remove_file(&path);
}

/// 观察者广播在第二条会话创建时也不能少（与已有 rename/pin 广播判据同一条线）。
#[tokio::test]
async fn observer_receives_every_create_broadcast() {
    let path = temp_db("broadcast-each-create");
    let (app, recorder) = app_with_recorder(&path);
    for name in ["一", "二", "三"] {
        let (status, created) = call(&app, "POST", "/v1/sessions", Some(json!({ "name": name }))).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
    }
    assert_eq!(*recorder.0.lock().unwrap(), 3, "三条 create 三次广播");
    let _ = std::fs::remove_file(&path);
}

/// fork 带名字也要复用同一份 session 作用域文档：先有会话意味着单例文档已经存在，
/// 第二次 DocumentCreate 会撞地址。同时验证 fork 只合 names，不抹掉 pins/其他键。
#[tokio::test]
async fn named_fork_reuses_the_session_scope_document_and_preserves_other_keys() {
    let path = temp_db("named-fork");
    let session = Session::open(&path).unwrap();
    session
        .commit(vec![], CommitOptions::default())
        .await
        .unwrap();
    session.close().await.unwrap();
    let (app, _recorder) = app_with_recorder(&path);

    let (status, first) =
        call(&app, "POST", "/v1/sessions", Some(json!({ "name": "甲" }))).await;
    assert_eq!(status, StatusCode::CREATED, "第一次建会话：{first}");
    let first_id = first["id"].as_i64().unwrap();

    let (status, pinned) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{first_id}/pin"),
        Some(json!({ "pinned": true, "requestId": "pin-fork-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "置顶：{pinned}");

    let (status, forked) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{first_id}/fork"),
        Some(json!({ "name": "分支" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "fork 带名字撞了单例文档地址：{forked}"
    );
    let fork_id = forked["id"].as_i64().unwrap();
    assert_ne!(fork_id, first_id, "fork 必须铸新会话 id");

    let (status, list) = call(&app, "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    for (id, name) in [(first_id, "甲"), (fork_id, "分支")] {
        let item = items
            .iter()
            .find(|item| item["id"] == json!(id))
            .unwrap_or_else(|| panic!("会话 {id} 不在 GET /v1/sessions：{list}"));
        assert_eq!(item["name"], json!(name), "会话 {id} 的名字丢了");
    }
    let original = items
        .iter()
        .find(|item| item["id"] == json!(first_id))
        .unwrap();
    assert_eq!(original["pinned"], json!(true), "fork 抹掉了 pins 投影：{list}");

    // 其他键（这里看 pins.requests 的幂等锚）必须还在整份文档值里。
    let raw = Storage::open(&path).unwrap();
    let record = raw
        .find_document(
            &json!({ "kind": "qrd3wh.sessions", "scope": { "kind": "session" } }),
            nous_durable::DocumentPoint::Current,
        )
        .unwrap()
        .expect("单例文档应该在");
    let doc_id = record["id"].as_i64().unwrap();
    let value = raw
        .materialize(doc_id, nous_durable::DocumentPoint::Current)
        .unwrap()
        .expect("单例文档值应该在")
        .value;
    assert!(
        value["pins"]["requests"].as_object().is_some_and(|requests| {
            requests.contains_key("pin-fork-1")
        }),
        "fork 抹掉了 pins.requests：{value}"
    );
    drop(raw);
    let _ = std::fs::remove_file(&path);
}
