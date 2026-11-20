//! 写端点的判据：`messages` / `abort` / `fork`。全部打在真 sqlite 上（临时库，不进 git）。

mod common;

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_durable::session::{CommitOptions, Session};
use nous_durable::{Storage, StorageWrite};
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-server-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

async fn seed(path: &Path, task: Option<Value>) {
    let session = Session::open(path).unwrap();
    let mut writes = vec![StorageWrite::Conversation(json!({ "id": 1 }))];
    if let Some(task) = task {
        writes.push(StorageWrite::Task(task));
    }
    session
        .commit(writes, CommitOptions::default())
        .await
        .unwrap();
    session.close().await.unwrap();
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
async fn the_same_request_id_returns_the_same_submission_and_stores_one_row() {
    let path = temp_db("idempotent");
    seed(&path, None).await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    let first = call(
        &routes,
        "POST",
        "/v1/sessions/1/messages",
        Some(json!({ "content": "hello", "requestId": "req-1" })),
    )
    .await;
    assert_eq!(first.0, StatusCode::ACCEPTED, "实测 {:?}", first.1);
    assert_eq!(first.1["status"], json!("queued"));

    let second = call(
        &routes,
        "POST",
        "/v1/sessions/1/messages",
        Some(json!({ "content": "hello again", "requestId": "req-1" })),
    )
    .await;
    assert_eq!(second.0, StatusCode::ACCEPTED, "两次都 202");
    assert_eq!(
        second.1["submissionId"], first.1["submissionId"],
        "同一个 requestId 必须拿到同一条 submission"
    );

    // 「两次都 202」不算过：库里必须只多一条。
    drop(routes);
    drop(app);
    let storage = Storage::open_read_only(&path).unwrap();
    let submissions = storage.submissions(1).unwrap();
    assert_eq!(submissions.len(), 1, "幂等：同 requestId 只落一条");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn the_first_frame_after_posting_is_message_start_not_message_update() {
    let path = temp_db("message-start");
    seed(&path, None).await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    // 先连上 SSE（拿 snapshot），再发消息 —— 判据看的是「响应发出后」。
    let response = routes
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let snapshot = next_frame(&mut stream).await;
    assert_eq!(snapshot["type"], json!("snapshot"));

    let posted = call(
        &routes,
        "POST",
        "/v1/sessions/1/messages",
        Some(json!({ "content": "open a bubble", "requestId": "first" })),
    )
    .await;
    assert_eq!(posted.0, StatusCode::ACCEPTED);

    let first = next_frame(&mut stream).await;
    assert_eq!(
        first["type"],
        json!("message_start"),
        "客户端靠第一条 message_start 开气泡，实测 {first}"
    );
    assert_eq!(first["message"]["role"], json!("user"));

    // 同一个 entry 还带一条 entry_appended（给别的标签页），但它在 start 之后。
    let second = next_frame(&mut stream).await;
    assert_eq!(second["type"], json!("entry_appended"));
    drop(routes);
    drop(app);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn abort_is_200_when_running_and_409_when_idle() {
    let path = temp_db("abort");
    seed(
        &path,
        Some(json!({
            "id": 10,
            "conversationId": 1,
            "kind": "pi.generation",
            "version": 1,
            "input": {},
            "background": false,
            "abortRequested": false,
            "state": { "status": "running" }
        })),
    )
    .await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    let (status, body) = call(&routes, "POST", "/v1/sessions/1/abort", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], json!("aborted"));

    // 已经打过标记，但任务还是 running —— 再打一次仍然成功（幂等）。
    let (status, _) = call(&routes, "POST", "/v1/sessions/1/abort", None).await;
    assert_eq!(status, StatusCode::OK);

    drop(routes);
    drop(app);
    let storage = Storage::open_read_only(&path).unwrap();
    let tasks = storage.tasks(1).unwrap();
    assert_eq!(tasks[0]["abortRequested"], json!(true));
    let _ = std::fs::remove_file(&path);

    // 空会话中止 = 409（不是 200「假装停了」，也不是 400）。
    let idle_path = temp_db("abort-idle");
    seed(&idle_path, None).await;
    let app = AppState::durable(&idle_path).unwrap();
    let routes = router(app.clone());
    let (status, body) = call(&routes, "POST", "/v1/sessions/1/abort", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "实测 {body}");
    assert!(body["error"].is_string());
    drop(routes);
    drop(app);
    let _ = std::fs::remove_file(&idle_path);
}

#[tokio::test]
async fn fork_returns_201_and_only_names_when_a_name_was_given() {
    let path = temp_db("fork");
    seed(&path, None).await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    let (status, anonymous) = call(&routes, "POST", "/v1/sessions/1/fork", None).await;
    assert_eq!(status, StatusCode::CREATED, "实测 {anonymous:?}");
    assert!(anonymous["id"].is_i64());
    assert_eq!(anonymous["at"], json!(0), "没有 entry 时分叉点是 0");

    let (status, named) = call(
        &routes,
        "POST",
        "/v1/sessions/1/fork",
        Some(json!({ "name": "分支" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // 会话树是每次重读的：分出来的会话立刻可见，带 parent。
    let (status, body) = call(&routes, "GET", "/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    let anonymous_item = items
        .iter()
        .find(|item| item["id"] == anonymous["id"])
        .expect("新会话在树里");
    assert_eq!(anonymous_item["parent"]["id"], json!(1));
    assert_eq!(anonymous_item["parent"]["at"], json!(0));
    assert_eq!(anonymous_item["name"], json!(""), "不给名字就是空串");
    let named_item = items
        .iter()
        .find(|item| item["id"] == named["id"])
        .expect("带名字的新会话在树里");
    assert_eq!(named_item["name"], json!("分支"));

    drop(routes);
    drop(app);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn fork_while_busy_is_409_and_wait_ms_is_400() {
    let path = temp_db("fork-busy");
    seed(
        &path,
        Some(json!({
            "id": 10,
            "conversationId": 1,
            "kind": "pi.generation",
            "version": 1,
            "input": {},
            "background": false,
            "abortRequested": false,
            "state": { "status": "running" }
        })),
    )
    .await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    let (status, body) = call(&routes, "POST", "/v1/sessions/1/fork", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "正在跑不许分叉，实测 {body}");
    assert!(body["error"].is_string());

    let (status, body) = call(
        &routes,
        "POST",
        "/v1/sessions/1/messages",
        Some(json!({ "content": "hi", "waitMs": 500 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "waitMs 没实现就不许假装，实测 {body}");

    drop(routes);
    drop(app);
    let _ = std::fs::remove_file(&path);
}

async fn next_frame(stream: &mut axum::body::BodyDataStream) -> Value {
    use futures::StreamExt;
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("2 秒内该到一帧")
        .expect("流不该先关")
        .expect("块读得出来");
    let text = String::from_utf8(chunk.to_vec()).unwrap();
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("data 行");
    serde_json::from_str(data).expect("帧是 JSON")
}

/// abort 两次：都 200、响应字节相等、`entries` 只多一条终态提交。
#[tokio::test]
async fn abort_twice_is_byte_identical_and_adds_one_entry() {
    let path = temp_db("abort-idempotent-http");
    seed(
        &path,
        Some(json!({
            "id": 10,
            "conversationId": 1,
            "kind": "pi.generation",
            "version": 1,
            "input": {},
            "background": false,
            "abortRequested": false,
            "state": { "status": "running" }
        })),
    )
    .await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/v1/sessions/1/abort")
        .body(Body::empty())
        .unwrap();
    let first = routes.clone().oneshot(request).await.unwrap();
    let first_status = first.status();
    let first_bytes = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    let second = routes.clone().oneshot(request_clone()).await.unwrap();
    let second_status = second.status();
    let second_bytes = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(first_bytes, second_bytes, "两次响应字节相等");

    drop(routes);
    drop(app);
    let storage = Storage::open_read_only(&path).unwrap();
    let entries = storage
        .scan_entries(
            &nous_durable::EntryQuery {
                conversation_id: 1,
                min_entry_id: None,
                max_entry_id: None,
                order: None,
            },
            100,
            None,
        )
        .unwrap()
        .unwrap();
    let terminal: Vec<&Value> = entries
        .items
        .iter()
        .filter(|entry| entry.get("reason").is_some())
        .collect();
    assert_eq!(terminal.len(), 1, "abort 两次只多一条终态 entry");
    assert_eq!(
        terminal[0]["kind"],
        json!("pi.assistant"),
        "没有第二种 entry 类型"
    );
    assert_eq!(terminal[0]["reason"], json!("user_cancel"));
    let _ = std::fs::remove_file(&path);
}

fn request_clone() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/sessions/1/abort")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn unread_has_the_same_object_shape_in_read_only_and_durable_assemblies() {
    let read_only = router(AppState::read_only(common::FakeSource::two_sessions()));
    let (status, body) = call(&read_only, "GET", "/v1/unread", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.is_object(), "只读装配的 unread 必须是平铺 map，不是 {body}");
    assert!(body.as_object().unwrap().keys().all(|key| key.parse::<i64>().is_ok()), "key 必须是会话 id：{body}");
    assert!(body.get("1").is_some() && body.get("2").is_some(), "只读装配也要按会话 id 给条目：{body}");

    let path = temp_db("unread-shape");
    seed(&path, None).await;
    let durable = router(AppState::durable(&path).unwrap());
    let (status, body) = call(&durable, "GET", "/v1/unread", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.is_object(), "正常装配的 unread 也必须是平铺 map：{body}");
    assert!(body.get("1").is_some_and(|value| value.get("unread").is_some()), "id 是字符串键的会话条目：{body}");
    let _ = std::fs::remove_file(&path);
}
