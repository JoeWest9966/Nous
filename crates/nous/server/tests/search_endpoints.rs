//! #85 搜索判据：会话正文、文档正文、使用反查。HTTP 公共接缝。

mod common;

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use nous_durable::session::{CommitOptions, Session};
use nous_durable::StorageWrite;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-server-search-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn call(app: &Router, uri: &str) -> (StatusCode, Value) {
    call_with(app, Method::GET, uri, None).await
}

async fn call_with(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(builder
            .body(Body::from(body.map(|v| serde_json::to_vec(&v).unwrap()).unwrap_or_default())).unwrap())
        .await
        .unwrap();
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

fn entry(id: i64, conversation: i64, text: &str) -> Value {
    json!({
        "id": id, "conversationId": conversation, "kind": "pi.user",
        "head": null,
        "model": [{"role": "user", "timestamp": 1_700_000_000_000_i64,
                   "content": [{"type": "text", "text": text}]}],
    })
}

/// 两轮对话：id 1 名字与正文都含「茄子」，id 2 正文有上下文。
async fn seed_conversations(path: &Path) {
    let session = Session::open(path).unwrap();
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({
                    "id": 1, "extensions": ["ext-x"], "skills": ["skill-a"]
                })),
                StorageWrite::Conversation(json!({ "id": 2 })),
                StorageWrite::Entry(entry(11, 1, &("前".repeat(40) + "茄子今天很新鲜，后文"))),
                StorageWrite::Entry(entry(12, 2, "另一段与茄子无关的正文")),
                StorageWrite::DocumentCreate {
                    id: 30,
                    record: json!({
                        "id": 30, "kind": "presets.Session-Template",
                        "scope": { "kind": "session" }, "key": "eggplant-note"
                    }),
                    content: nous_durable::DocumentContent::Base {
                        version: 1,
                        value: json!({ "summary": "茄子文档标题很普通", "description": "正文里有茄子，snippet 要来自这里" }),
                    },
                },
            ],
            CommitOptions::default(),
        )
        .await
        .unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn search_finds_eggplant_in_entry_bodies_with_context() {
    let path = temp_db("eggplant");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/search?q=%E8%8C%84%E5%AD%90&kind=sessions").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    let items = body["items"].as_array().expect("items 是数组");
    assert!(!items.is_empty(), "正文命中不能空：{body}");
    let hit = items
        .iter()
        .find(|item| item["id"] == json!("1"))
        .unwrap_or_else(|| panic!("含茄子的会话 id=1 必须命中：{body}"));
    assert_eq!(hit["kind"], json!("sessions"));
    let snippet = hit["snippet"].as_str().expect("snippet 是字符串");
    assert!(snippet.contains("茄子"), "snippet 含命中词：{snippet}");
    assert!(snippet.contains("前"), "snippet 含命中前上下文：{snippet}");
    assert!(snippet.contains("后文"), "snippet 含命中后上下文：{snippet}");
    assert!(snippet.len() > "茄子".len(), "snippet 不是光秃秃的命中词");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_one_character_query_is_rejected_before_scanning() {
    let path = temp_db("short-query");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/search?q=a").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实测 {:?}", body);
    assert!(body["error"].is_string(), "错误形状是 {{error}}");
    let _ = std::fs::remove_file(&path);
}


#[tokio::test]
async fn documents_search_snippets_come_from_the_body() {
    let path = temp_db("documents");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/search?q=%E8%8C%84%E5%AD%90&kind=documents").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    let items = body["items"].as_array().expect("items 是数组");
    assert_eq!(items.len(), 1, "文档正文命中一条：{body}");
    let hit = &items[0];
    assert_eq!(hit["kind"], json!("documents"));
    assert_eq!(hit["title"], json!("eggplant-note"));
    let snippet = hit["snippet"].as_str().unwrap();
    assert!(snippet.contains("正文里有茄子"), "snippet 取自正文：{snippet}");
    assert!(!snippet.contains("茄子文档标题很普通"), "不该用标题代替命中正文：{snippet}");
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn trailing_space_does_not_hide_the_match() {
    let path = temp_db("trim");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/search?q=%E8%8C%84%E5%AD%90%20&kind=sessions").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    assert!(body["items"].as_array().unwrap().iter().any(|item| item["id"] == json!("1")), "{body}");
    let _ = std::fs::remove_file(&path);
}


#[tokio::test]
async fn usage_lists_the_sessions_that_use_a_name_and_returns_empty_when_nobody_does() {
    let path = temp_db("usage");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/extensions?usage=1").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/extensions?usage=1 实测 status={status:?} body={body}");
    assert_eq!(body["ext-x"], json!([1]), "用了扩展 X 的会话 id 要出现：{body}");

    let (status, body) = call(&app, "/v1/skills?usage=1").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    assert_eq!(body["skill-a"], json!([1]), "用了技能 A 的会话 id 要出现：{body}");

    let empty_app = router(AppState::durable(&temp_db("usage-empty")).unwrap());
    let (status, body) = call(&empty_app, "/v1/extensions?usage=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({}), "没人用时返回空映射（没有条目 = 空数组语义），不是 404：{body}");
    let _ = std::fs::remove_file(&path);
}
