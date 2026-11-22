//! #62 判据 7 + 端点公共接缝：真 Router 上三条 stats 端点、capabilities、openapi。
//! 用临时真库装配 StatsRuntime，不用假数据替端点自证。

mod common;

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nous_durable::{Storage, StorageWrite};
use nous_server::{router, AppState, stats::StatsRuntime};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-stats62-http-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    let mut storage = Storage::open(&path).unwrap();
    storage.commit(
        &[vec![StorageWrite::Conversation(json!({"id": 1}))]],
        |_, _| {},
    ).unwrap();
    storage.close().unwrap();
    path
}

fn app(db: &PathBuf) -> axum::Router {
    let source = nous_server::ImportedSource::open(db).unwrap();
    let state = AppState::with_source(source).with_stats(StatsRuntime {
        durable_db: db.clone(),
        plugins_db: db.with_file_name("nous-stats62-http-never.sqlite"),
    });
    router(state)
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app.clone().oneshot(
        Request::builder().uri(uri).body(Body::empty()).unwrap(),
    ).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn all_three_stats_routes_and_openapi_are_declared_and_live() {
    let db = temp_db("declare");
    let app = app(&db);

    // 八条路径都是 200（空库也 200，判据 6 的 HTTP 面）。conversation/:id 要一个存在的会话。
    for path in [
        "/v1/stats/usage",
        "/v1/stats/tasks",
        "/v1/stats/db",
        "/v1/stats/overview",
        "/v1/stats/conversations",
        "/v1/stats/conversation/1",
        "/v1/stats/cost",
        "/v1/stats/health",
    ] {
        let (status, body) = get(&app, path).await;
        assert_eq!(status, StatusCode::OK, "GET {path} 应 200：{body}");
    }
    let (_, openapi) = get(&app, "/v1/openapi.json").await;
    let paths = openapi["paths"].as_object().unwrap();
    for path in [
        "/v1/stats/usage",
        "/v1/stats/tasks",
        "/v1/stats/db",
        "/v1/stats/overview",
        "/v1/stats/conversations",
        "/v1/stats/conversation/:id",
        "/v1/stats/cost",
        "/v1/stats/health",
    ] {
        assert!(paths.contains_key(path), "openapi.json 缺 {path}");
        assert!(paths[path]["get"].is_object());
    }

    let (_, caps) = get(&app, "/v1/capabilities").await;
    let stats = caps["modules"].as_array().unwrap()
        .iter().find(|m| m["name"] == json!("stats"))
        .expect("capabilities 必须声明 stats 模块");
    assert_eq!(stats["enabled"], json!(true));
    let endpoints: Vec<&str> = stats["endpoints"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    for expected in ["GET /v1/stats/usage", "GET /v1/stats/tasks", "GET /v1/stats/db"] {
        assert!(endpoints.contains(&expected), "stats.endpoints 缺 {expected}");
    }
    // openapi 的 paths 与 _routes 同源：三条声明在两边都在。
    let (_, routes) = get(&app, "/v1/_routes").await;
    let route_paths: Vec<&str> = routes["routes"].as_array().unwrap()
        .iter().filter(|r| r["module"] == json!("stats"))
        .map(|r| r["path"].as_str().unwrap()).collect();
    assert_eq!(route_paths.len(), 8);
    std::fs::remove_file(&db).ok();
}

#[tokio::test]
async fn usage_endpoint_honours_half_open_window_over_http() {
    let db = temp_db("http-window");
    // 直接往库里插两条 assistant entry（判据 2/3 的 lib 级已对账，这里验 HTTP 参数解析）。
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        for (id, at) in [(10_i64, 1_700_000_000_000_i64), (11, 1_700_000_010_000)] {
            let record = json!({
                "id": id, "conversationId": 1, "kind": "pi.assistant",
                "model": [{ "provider": "p", "modelId": "m", "timestamp": at,
                             "usage": { "totalTokens": 5, "cost": { "total": 1 } } }],
            });
            conn.execute(
                "INSERT INTO entries (id, conversation_id, head, commit_seq, record) VALUES (?1, 1, NULL, ?2, ?3)",
                rusqlite::params![id, id, record.to_string()],
            ).unwrap();
        }
    }
    let app = app(&db);
    let (status, full) = get(&app, "/v1/stats/usage").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(full["totals"]["requests"], json!(2));
    let (status, sliced) = get(
        &app,
        "/v1/stats/usage?from=1700000000000&to=1700000000001",
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sliced["totals"]["requests"], json!(1), "闭开区间：只命中 from 那条");
    let (status, bad) = get(&app, "/v1/stats/usage?group_by=bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法 group_by 400，不是静默退化");
    assert!(bad["error"].is_string());
    std::fs::remove_file(&db).ok();
}
