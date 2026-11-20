//! #64：sessions 写入面（config / purge / selection）的公共接缝。
//!
//! 接缝：真 Router + 临时 durable 库；不查私有函数、不读库文件断言行为 ——
//! 配置是否逐键、purge 是否删干净、selection 是否落盘，全从 HTTP 看得见。

mod common;

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_durable::session::{CommitOptions, Session};
use nous_durable::{DocumentContent, StorageWrite};
use nous_server::{router, AppState, DurableCommands, ImportedSource};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-issue64-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    path
}

/// 三条会话，各自带 entry / task / submission / document；会话 1 还被置顶、有未读水位。
async fn seed(path: &PathBuf) {
    let session = Session::open(path).unwrap();
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": 1 })),
                StorageWrite::Conversation(json!({ "id": 2 })),
                StorageWrite::Conversation(json!({ "id": 3 })),
                StorageWrite::Entry(json!({
                    "id": 11, "conversationId": 1, "kind": "pi.user", "text": "purge me"
                })),
                StorageWrite::Entry(json!({
                    "id": 21, "conversationId": 2, "kind": "pi.user", "text": "keep me"
                })),
                StorageWrite::Task(json!({
                    "id": 12, "conversationId": 1, "kind": "pi.generation",
                    "version": 1, "input": {}, "state": { "status": "terminal" },
                    "abortRequested": false, "background": false
                })),
                StorageWrite::Submission(json!({
                    "id": 13, "conversationId": 1, "type": "input", "status": "done",
                    "requestId": "seed-1", "entry": 11, "answer": 14
                })),
                StorageWrite::DocumentCreate {
                    id: 14,
                    record: json!({
                        "id": 14, "kind": "pi.agent",
                        "scope": { "kind": "conversation", "conversationId": 1 },
                        "history": "latest", "fork": "current"
                    }),
                    content: DocumentContent::Base {
                        version: 1,
                        value: json!({
                            "model": { "provider": "p", "modelId": "many" },
                            "thinkingLevel": "high",
                            "thinkingLevels": ["off", "medium", "high", "max"],
                            "readOnly": false,
                            "skills": ["a", "b", "c"]
                        }),
                    },
                },
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    session.close().await.unwrap();
}

fn app(path: &PathBuf) -> Router {
    let commands = DurableCommands::open(path).unwrap();
    let state = AppState::durable_with_commands(
        ImportedSource::open(path).unwrap(),
        commands,
    )
    .with_models(nous_server::models::ModelsResponse {
        default: None,
        items: vec![
            nous_server::models::ModelItem {
                provider: "p".into(),
                model_id: "many".into(),
                name: "Many levels".into(),
                thinking_levels: vec!["off".into(), "medium".into(), "high".into(), "max".into()],
                context_window: None,
                max_tokens: None,
            },
            nous_server::models::ModelItem {
                provider: "p".into(),
                model_id: "two-levels".into(),
                name: "Two levels".into(),
                thinking_levels: vec!["off".into(), "low".into()],
                context_window: None,
                max_tokens: None,
            },
        ],
    });
    router(state)
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
async fn selection_survives_a_restart_and_a_missing_session_reads_back_null() {
    let path = temp_db("selection");
    seed(&path).await;
    let router = app(&path);

    let (status, body) = call(&router, "GET", "/v1/selection", None).await;
    assert_eq!(status, StatusCode::OK, "GET /v1/selection 200：{body}");
    assert_eq!(body, json!({ "id": null }));

    let (status, written) = call(&router, "POST", "/v1/selection", Some(json!({ "id": 2 }))).await;
    assert_eq!(status, StatusCode::OK, "POST /v1/selection 200：{written}");
    assert_eq!(written, json!({ "id": 2 }));
    let (_, read) = call(&router, "GET", "/v1/selection", None).await;
    assert_eq!(read, json!({ "id": 2 }), "写了就立刻读得回来");

    // 重启：状态必须还在，不是内存里的一个变量。
    let restarted = app(&path);
    let (status, after_restart) = call(&restarted, "GET", "/v1/selection", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after_restart, json!({ "id": 2 }), "selection 落盘，不在内存里");

    let (status, cleared) = call(&restarted, "POST", "/v1/selection", Some(json!({ "id": null }))).await;
    assert_eq!(status, StatusCode::OK, "POST id=null 200：{cleared}");
    assert_eq!(cleared, json!({ "id": null }));
    let (_, read) = call(&restarted, "GET", "/v1/selection", None).await;
    assert_eq!(read, json!({ "id": null }), "清空后读回 null");

    // 指向已删会话：GET 回 null，并顺手清掉死指针（重启后再读仍是 null）。
    let (_, _) = call(&restarted, "POST", "/v1/selection", Some(json!({ "id": 3 }))).await;
    let (status, _) = call(&restarted, "POST", "/v1/sessions/3/purge", None).await;
    assert_eq!(status, StatusCode::OK, "会话 3 撕掉，selection 指向死指针");
    let (_, dead) = call(&restarted, "GET", "/v1/selection", None).await;
    assert_eq!(dead, json!({ "id": null }), "指向已删会话时回 null");
    let again = app(&path);
    let (_, dead) = call(&again, "GET", "/v1/selection", None).await;
    assert_eq!(dead, json!({ "id": null }), "死指针已被清掉，重启后仍是 null");

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn config_only_changes_the_keys_that_were_sent() {
    let path = temp_db("config-keys");
    seed(&path).await;
    let router = app(&path);

    let (status, first) = call(
        &router,
        "POST",
        "/v1/sessions/1/config",
        Some(json!({ "readOnly": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "config 200：{first}");
    assert_eq!(first["readOnly"], json!(true));

    // 第二次只发 model；readOnly 必须还是 true（整份覆盖会把它冲成 false）。
    let (status, second) = call(
        &router,
        "POST",
        "/v1/sessions/1/config",
        Some(json!({ "model": { "provider": "p", "modelId": "many" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "config 200：{second}");
    assert_eq!(second["readOnly"], json!(true), "逐键语义：没传的键不能动");
    assert_eq!(second["model"], json!({ "provider": "p", "modelId": "many" }));

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn changing_to_a_model_with_fewer_levels_clamps_thinking_level() {
    let path = temp_db("config-clamp");
    seed(&path).await;
    let router = app(&path);

    // 新模型只有 off / low 两档：原 high 必须被夹到合法值 low。
    let (status, body) = call(
        &router,
        "POST",
        "/v1/sessions/1/config",
        Some(json!({
            "model": { "provider": "p", "modelId": "two-levels" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "config 200：{body}");
    assert_eq!(body["thinkingLevel"], json!("low"), "夹到新模型的最大合法档");
    assert_eq!(body["model"]["modelId"], json!("two-levels"));

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn purge_removes_every_view_and_the_second_attempt_is_409() {
    let path = temp_db("purge");
    seed(&path).await;
    let router = app(&path);

    let (status, receipt) = call(&router, "POST", "/v1/sessions/1/purge", None).await;
    assert_eq!(status, StatusCode::OK, "purge 200：{receipt}");
    assert_eq!(receipt["id"], json!(1));
    assert!(receipt["purged"].as_bool().unwrap_or(false), "墓碑写进去了");
    assert!(receipt["at"].is_i64(), "墓碑有时刻");

    // 三条视图一起断言：列表无此 id、entries 404、unread 无此 id。
    let (_, sessions) = call(&router, "GET", "/v1/sessions", None).await;
    let ids: Vec<i64> = sessions["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_i64().unwrap())
        .collect();
    assert!(!ids.contains(&1), "GET /v1/sessions 不含被 purged 的 id");
    assert!(ids.contains(&2), "别的会话不受影响");

    let (status, error) = call(&router, "GET", "/v1/sessions/1/entries", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "entries 404：{error}");

    let (_, unread) = call(&router, "GET", "/v1/unread", None).await;
    assert!(unread.get("1").is_none(), "unread 里也没有该 id");

    // 公共 durable 读口：真删必须连 entry 行一起没，不能只藏会话。
    let storage = nous_durable::Storage::open(&path).unwrap();
    assert!(storage.entry(11).unwrap().is_none(), "entries 表不能残留被 purge 的行");
    assert!(storage.entry(21).unwrap().is_some(), "别的会话的 entry 还在");

    // 幂等口径：第二次 409，不是 200 / 500。
    let (status, error) = call(&router, "POST", "/v1/sessions/1/purge", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "第二次 409：{error}");
    assert!(error["error"].is_string());

    // 不存在的会话首次 purge → 404。
    let (status, error) = call(&router, "POST", "/v1/sessions/999/purge", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不存在 404：{error}");

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn capabilities_drops_selection_and_routes_attribute_it_to_sessions() {
    let path = temp_db("capabilities");
    seed(&path).await;
    let router = app(&path);
    let (_, capabilities) = call(&router, "GET", "/v1/capabilities", None).await;
    let modules = capabilities["modules"].as_array().unwrap();
    assert!(
        modules.iter().all(|m| m["name"].as_str() != Some("selection")),
        "capabilities 不含 selection 模块"
    );
    let sessions = modules
        .iter()
        .find(|m| m["name"] == json!("sessions"))
        .unwrap();
    let endpoints = sessions["endpoints"].as_array().unwrap();
    for route in [
        "GET /v1/selection",
        "POST /v1/selection",
        "POST /v1/sessions/:id/config",
        "POST /v1/sessions/:id/purge",
    ] {
        assert!(
            endpoints.iter().any(|e| e.as_str() == Some(route)),
            "sessions 声明了 {route}"
        );
    }

    let (_, routes) = call(&router, "GET", "/v1/_routes", None).await;
    for route in routes["routes"].as_array().unwrap() {
        if route["path"].as_str() == Some("/v1/selection") {
            assert_eq!(route["module"], json!("sessions"), "_routes 归属 sessions");
        }
        if route["path"].as_str() == Some("/v1/sessions/:id/purge") {
            assert_eq!(route["module"], json!("sessions"));
        }
    }

    let _ = std::fs::remove_file(&path);
}
