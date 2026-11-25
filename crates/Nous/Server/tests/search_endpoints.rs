//! #85 搜索判据：会话正文、文档正文、使用反查。HTTP 公共接缝。
//!
//! 每条判据都打真 `Router` + 真 durable 库：种子经 `Session::commit` 落盘，
//! 断言从 HTTP 回包取 —— HTTP 进 → 库里那一行正文出来，不靠「函数存在」冒充行为。

mod common;

use std::path::{Path, PathBuf};
use std::time::Instant;

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

async fn call_with(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(body.map(|v| serde_json::to_vec(&v).unwrap()).unwrap_or_default()))
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

async fn call(app: &Router, uri: &str) -> (StatusCode, Value) {
    call_with(app, Method::GET, uri, None).await
}

fn entry(id: i64, conversation: i64, text: &str) -> Value {
    json!({
        "id": id, "conversationId": conversation, "kind": "pi.user",
        "head": null,
        "model": [{"role": "user", "timestamp": 1_700_000_000_000_i64,
                   "content": [{"type": "text", "text": text}]}],
    })
}

/// 两轮对话：id 1 正文有「茄子」及前后上下文，id 2 正文无关；
/// 扩展 / 技能挂在 id 1，文档正文另含「茄子」但标题不含。
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
                StorageWrite::Entry(entry(12, 2, "另一段完全无关的正文")),
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

/// 判据 1：两轮对话的正文里只有 id 1 有「茄子」；snippet 必须带前后各 40 字符。
#[tokio::test]
async fn search_finds_eggplant_in_entry_bodies_with_context() {
    let path = temp_db("eggplant");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/search?q=%E8%8C%84%E5%AD%90&kind=sessions").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    let items = body["items"].as_array().expect("items 是数组");
    assert_eq!(items.len(), 1, "两轮对话只有一轮正文含茄子：{body}");
    let hit = &items[0];
    assert_eq!(hit["kind"], json!("sessions"));
    assert_eq!(hit["id"], json!("1"));
    let snippet = hit["snippet"].as_str().expect("snippet 是字符串");
    assert!(snippet.contains("茄子"), "snippet 含命中词：{snippet}");
    assert!(snippet.contains(&"前".repeat(40)), "命中前有 40 字符上下文：{snippet}");
    assert!(snippet.contains("后文"), "命中后有上下文：{snippet}");
    assert!(snippet.chars().count() > 40, "snippet 不是正文截断成一截：{snippet}");
    let _ = std::fs::remove_file(&path);
}

/// 判据 2：`q=茄子` 与带尾空格的 `q=茄子 ` 都命中（trim 断言）。
#[tokio::test]
async fn trailing_space_does_not_hide_the_match() {
    let path = temp_db("trim");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    for (tag, uri) in [
        ("无空格", "/v1/search?q=%E8%8C%84%E5%AD%90&kind=sessions"),
        ("尾空格", "/v1/search?q=%E8%8C%84%E5%AD%90%20&kind=sessions"),
    ] {
        let (status, body) = call(&app, uri).await;
        assert_eq!(status, StatusCode::OK, "{tag} 实测 {:?}", body);
        assert!(
            body["items"].as_array().unwrap().iter().any(|item| item["id"] == json!("1")),
            "{tag} 该命中 id=1：{body}"
        );
    }
    let _ = std::fs::remove_file(&path);
}

/// 判据 3：`q=a` / 空串 / trim 后只剩一个字符 → 400，不许全表扫。
#[tokio::test]
async fn a_one_character_query_is_rejected_before_scanning() {
    let path = temp_db("short-query");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    for uri in ["/v1/search?q=a", "/v1/search?q=", "/v1/search?q=a%20%20"] {
        let (status, body) = call(&app, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} 实测 {:?}", body);
        assert_eq!(body["error"].as_str().is_some(), true, "错误形状是 {{error}}：{body}");
    }
    let _ = std::fs::remove_file(&path);
}

/// 判据 4：文档正文与名字都参与；正文命中时 snippet 取自正文而非名字。
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

    // 名字命中（正文并不含这个词）：kind/section/name 参与搜索。
    let (status, body) = call(&app, "/v1/search?q=eggplant-note&kind=documents").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    assert_eq!(body["total"], json!(1), "文档名参与搜索：{body}");
    let _ = std::fs::remove_file(&path);
}

/// 判据 5：一个会话用了扩展 X / 技能 A → 返回该会话 id；没人用 → 空映射（不是 404）。
#[tokio::test]
async fn usage_lists_the_sessions_that_use_a_name_and_returns_empty_when_nobody_does() {
    let path = temp_db("usage");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/extensions?usage=1").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/extensions?usage=1 实测 status={status:?} body={body}");
    assert_eq!(body["ext-x"], json!([1]), "用了扩展 X 的会话 id 要出现：{body}");
    assert_eq!(body.as_object().unwrap().get("skill-a"), None, "扩展反查不串到技能：{body}");

    let (status, body) = call(&app, "/v1/skills?usage=1").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    assert_eq!(body["skill-a"], json!([1]), "用了技能 A 的会话 id 要出现：{body}");

    let empty_path = temp_db("usage-empty");
    let empty_app = router(AppState::durable(&empty_path).unwrap());
    let (status, body) = call(&empty_app, "/v1/extensions?usage=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({}), "没人用时返回空映射，不是 404：{body}");
    let _ = std::fs::remove_file(&empty_path);

    // 不带 usage=1 时仍走原有清单形状，不被可选参数带跑偏。
    let (status, body) = call(&app, "/v1/extensions?usage=0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "items": [] }), "usage=0 不是反查：{body}");
    let _ = std::fs::remove_file(&path);
}

/// 判据 6：1000 条 entry 的库里单次搜索 < 200ms。种子一次批量提交，计时只包 HTTP 搜索。
#[tokio::test]
async fn a_search_over_one_thousand_entries_finishes_under_two_hundred_millis() {
    let path = temp_db("perf-1000");
    let session = Session::open(&path).unwrap();
    let mut writes = vec![StorageWrite::Conversation(json!({ "id": 1 }))];
    for id in 1001..=2000_i64 {
        let text = if id == 1500 {
            format!("{} needle-hit {}", "填".repeat(60), "尾")
        } else {
            format!("普通正文 {id}")
        };
        writes.push(StorageWrite::Entry(entry(id, 1, &text)));
    }
    session.commit(writes, CommitOptions::default()).await.unwrap();
    session.close().await.unwrap();
    let app = router(AppState::durable(&path).unwrap());

    let started = Instant::now();
    let (status, body) = call(&app, "/v1/search?q=needle-hit&kind=sessions").await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    assert_eq!(body["total"], json!(1), "1000 条里只命中一条：{body}");
    assert!(
        elapsed.as_millis() < 200,
        "1000 条 entry 单次搜索应 < 200ms，实测 {}ms",
        elapsed.as_millis()
    );
    let _ = std::fs::remove_file(&path);
}

/// `kind=all` 默认合并两种命中；`total` 是命中总数，`truncated` 明确，`limit` 上限 100。
#[tokio::test]
async fn search_reports_total_and_truncation_independently_of_the_page_limit() {
    let path = temp_db("truncated");
    let session = Session::open(&path).unwrap();
    // 搜索结果按会话去重：造 105 个会话各一条命中，才能验总数与截断。
    let mut writes = Vec::new();
    for id in 1..=105_i64 {
        writes.push(StorageWrite::Conversation(json!({ "id": id })));
        writes.push(StorageWrite::Entry(entry(1000 + id, id, "命中词在正文里")));
    }
    session.commit(writes, CommitOptions::default()).await.unwrap();
    session.close().await.unwrap();
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/search?q=%E5%91%BD%E4%B8%AD%E8%AF%8D&limit=2").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
    assert_eq!(body["total"], json!(105), "total 是命中总数，不是本页条数：{body}");
    assert_eq!(body["truncated"], json!(true));

    // limit 超过 100 夹到 100：total 105、items 100、truncated 仍 true。
    let (_, body) = call(&app, "/v1/search?q=%E5%91%BD%E4%B8%AD%E8%AF%8D&limit=999").await;
    assert_eq!(body["items"].as_array().unwrap().len(), 100);
    assert_eq!(body["total"], json!(105));
    assert_eq!(body["truncated"], json!(true));

    // kind 非法 → 400，不让拼错的参数退化成全库扫描。
    let (status, body) = call(&app, "/v1/search?q=%E5%91%BD%E4%B8%AD%E8%AF%8D&kind=oops").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "实测 {:?}", body);
    let _ = std::fs::remove_file(&path);
}

/// 默认 kind=all：会话正文 + 文档正文/名字的命中合并进同一个 items。
#[tokio::test]
async fn default_kind_merges_session_and_document_hits() {
    let path = temp_db("all");
    seed_conversations(&path).await;
    let app = router(AppState::durable(&path).unwrap());

    let (status, body) = call(&app, "/v1/search?q=%E8%8C%84%E5%AD%90").await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    let kinds: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["sessions", "documents"], "all 合并两类命中：{body}");
    let _ = std::fs::remove_file(&path);
}

/// `/v1/search` 必须在路由注册表里声明；_routes 与 capabilities 三处同源。
#[tokio::test]
async fn search_is_declared_in_the_route_registry() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, routes) = call(&app, "/v1/_routes").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        routes["routes"].as_array().unwrap().iter().any(|route| {
            route["method"] == json!("GET") && route["path"] == json!("/v1/search")
        }),
        "_routes 必须声明 GET /v1/search：{routes}"
    );
}
