//! #87 判据 2/3：盘面树的 HTTP 写 → sqlite 双向断言（真 Router + 临时库）。
//! 每一步都从库里重新读文档，不让「HTTP 200」冒充「落库」。

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use nous_durable::session::Session;
use nous_durable::{DocumentPoint, Storage};
use nous_server::{AppState, DurableCommands, SessionObserver};
use serde_json::{json, Value};
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-documents-tree-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

async fn setup(path: &Path) -> axum::Router {
    let registry = nous_durable::session::ObserverRegistry::new();
    let session = std::sync::Arc::new(Session::with_observers(
        Storage::open(path).unwrap(), registry.clone(),
    ));
    let commands = DurableCommands::with_session(session.clone(), registry);
    let state = AppState::durable_with_commands(
        nous_server::ImportedSource::open(path).unwrap(), commands.clone(),
    );
    commands.attach(SessionObserver::new(state.clone()));
    nous_server::router(state.with_documents_backend(path.to_path_buf(), session))
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

/// 独立只读连接：断言库里**真有**这一行，且字段是刚刚 HTTP 写进去的值。
fn stored_tree(path: &Path, board: &str) -> Value {
    let storage = Storage::open_read_only(path).unwrap();
    let all = storage.scan_documents(&nous_durable::DocumentQuery {
        scope: json!({ "kind": "session" }),
        kind: Some(format!("tree.{board}")),
        at: DocumentPoint::Current,
    }, 100, None).unwrap().unwrap();
    let found = all.items.iter()
        .find(|item| item["key"] == json!("board"))
        .unwrap_or_else(|| panic!("库里应有 tree.{board}/board 行：{}", serde_json::to_string(&all.items).unwrap()));
    let id = found["id"].as_i64().unwrap();
    storage.materialize(id, DocumentPoint::Current).unwrap().unwrap().value
}

fn folder(id: &str, parent: Option<&str>) -> Value {
    json!({ "id": id, "title": id, "kind": "folder", "parent": parent, "order": 0, "trashedAt": null })
}

#[tokio::test]
async fn tree_lifecycle_has_http_and_db_rows_on_every_step() {
    let path = temp_db("lifecycle");
    let app = setup(&path).await;
    let board = "desk";

    // 建节点（根 + 文件夹 + 文档）：整棵树一次 PUT 落库（HTTP 进 → 库里多一行）。
    let seed = json!({
        "etag": null,
        "nodes": [folder("root", None), folder("folder", Some("root")),
                  json!({"id":"doc","title":"笔记","kind":"doc","parent":"folder","order":0,"trashedAt":null})],
    });
    let (status, body) = call(&app, Method::PUT, &format!("/v1/documents/tree/{board}/board"), Some(seed)).await;
    assert_eq!(status, StatusCode::OK, "建节点 PUT: status={status} body={body}");
    let db = stored_tree(&path, board);
    assert_eq!(db["nodes"]["doc"]["title"], json!("笔记"), "HTTP 写进去的节点必须在库里");
    let (status, readback) = call(&app, Method::GET,
        &format!("/v1/documents/tree/{board}/board"), None).await;
    assert_eq!(status, StatusCode::OK, "GET 读回整棵树: {readback}");
    assert_eq!(readback["fields"]["nodes"]["doc"]["title"], json!("笔记"), "GET 必须读到刚写的节点");

    assert_eq!(db["nodes"]["root"]["children"], json!(["folder"]), "父子边必须落库");
    println!("命中行: 库里 tree.{board}/board 的 doc 节点 title={}", db["nodes"]["doc"]["title"]);

    let etag = body["etag"].as_str().unwrap().to_string();
    // 挂在父下（move）：一次写同时改源父、目标父和节点自身。
    let (status, moved) = call(&app, Method::POST,
        &format!("/v1/documents/tree/{board}/board/move"),
        Some(json!({"etag": etag, "id": "doc", "parent": "root", "order": 1}))).await;
    assert_eq!(status, StatusCode::OK, "挂到父下: {moved}");
    let db = stored_tree(&path, board);
    assert_eq!(db["nodes"]["doc"]["parent"], json!("root"), "移动必须落库");
    assert_eq!(db["nodes"]["folder"]["children"], json!([]), "旧父不能还挂着孩子");
    assert_eq!(db["nodes"]["root"]["children"], json!(["folder", "doc"]), "新父要按 order 挂上");
    println!("命中行: 库里 doc.parent={} root.children={}", db["nodes"]["doc"]["parent"], db["nodes"]["root"]["children"]);

    let etag = moved["etag"].as_str().unwrap().to_string();
    // 回收 → 恢复 → 清除：墓碑和清除都写回同一份文档。
    let (status, trashed) = call(&app, Method::POST,
        &format!("/v1/documents/tree/{board}/board/trash"),
        Some(json!({"etag": etag, "id": "doc"}))).await;
    assert_eq!(status, StatusCode::OK, "回收: {trashed}");
    let db = stored_tree(&path, board);
    assert!(db["nodes"]["doc"]["trashedAt"].is_i64(), "回收要落墓碑到库: {db}");
    assert_eq!(db["nodes"]["root"]["children"], json!(["folder"]), "回收要点掉父的孩子");

    let etag = trashed["etag"].as_str().unwrap().to_string();
    let (status, restored) = call(&app, Method::POST,
        &format!("/v1/documents/tree/{board}/board/restore"),
        Some(json!({"etag": etag, "id": "doc", "parent": "root"}))).await;
    assert_eq!(status, StatusCode::OK, "恢复: {restored}");
    let db = stored_tree(&path, board);
    assert!(db["nodes"]["doc"]["trashedAt"].is_null(), "恢复要清墓碑: {db}");
    println!("命中行: 库里 doc.trashedAt={}", db["nodes"]["doc"]["trashedAt"]);

    let etag = restored["etag"].as_str().unwrap().to_string();
    let (status, trashed2) = call(&app, Method::POST,
        &format!("/v1/documents/tree/{board}/board/trash"),
        Some(json!({"etag": etag, "id": "doc"}))).await;
    assert_eq!(status, StatusCode::OK);
    let etag = trashed2["etag"].as_str().unwrap().to_string();
    let (status, _) = call(&app, Method::POST,
        &format!("/v1/documents/tree/{board}/board/purge"),
        Some(json!({"etag": etag, "id": "doc"}))).await;
    assert_eq!(status, StatusCode::OK);
    let db = stored_tree(&path, board);
    assert!(db["nodes"].get("doc").is_none(), "清除必须从库里的文档删掉: {db}");

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn cycles_and_stale_etags_are_rejected_and_leave_the_db_untouched() {
    let path = temp_db("guards");
    let app = setup(&path).await;
    let board = "desk";
    let seed = json!({"etag": null, "nodes": [
        folder("root", None), folder("a", Some("root")), folder("b", Some("a"))]});
    let (status, created) = call(&app, Method::PUT, &format!("/v1/documents/tree/{board}/board"), Some(seed)).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let before = stored_tree(&path, board);

    let etag = created["etag"].as_str().unwrap().to_string();
    // 自引用 + 拖到后代：4xx，库里树原样不变。
    for (id, parent) in [("a", "a"), ("a", "b")] {
        let (status, body) = call(&app, Method::POST,
            &format!("/v1/documents/tree/{board}/board/move"),
            Some(json!({"etag": etag, "id": id, "parent": parent, "order": 1}))).await;
        assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT,
            "环要 4xx: {status} {body}");
        assert!(body["error"].as_str().unwrap_or_default().contains(parent),
            "error 要点出命中节点: {body}");
        assert_eq!(stored_tree(&path, board), before, "4xx 之后库里树不能变");
        println!("命中行: {id}->{parent} 回 {status}，error={}", body["error"]);
    }

    // 旧版本写同节点 → 409（不靠锁文件）。
    let (status, body) = call(&app, Method::POST,
        &format!("/v1/documents/tree/{board}/board/move"),
        Some(json!({"etag": "stale-etag", "id": "b", "parent": "root", "order": 2}))).await;
    assert_eq!(status, StatusCode::CONFLICT, "旧 etag 必须 409: {body}");
    assert_eq!(stored_tree(&path, board), before);

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn archived_tree_imports_into_the_same_document_store() {
    let path = temp_db("import");
    let app = setup(&path).await;
    let old = json!({
        "v": 1, "name": "desk", "version": 7,
        "root": {"id": "root", "name": "老盘", "children": [
            {"id": "g1", "name": "组一", "children": [{"id": "n1", "name": "文档一"}]}
        ]}
    });
    let (status, body) = call(&app, Method::POST,
        "/v1/documents/tree/desk/board/import",
        Some(json!({ "tree.json": old, "mapDirs": ["root", "g1", "n1"], "goneDirs": ["deleted"] }))).await;
    assert_eq!(status, StatusCode::OK, "迁移导入必须 200: {body}");
    assert_eq!(body["migration"]["nodes"], json!(3), "报告节点数: {body}");
    assert_eq!(body["migration"]["folders"], json!(2), "报告组数: {body}");

    // HTTP 进 → 库里真有一行，且内容来自旧归档（不是空树）。
    let db = stored_tree(&path, "desk");
    assert_eq!(db["nodes"]["n1"]["title"], json!("文档一"), "旧节点名必须落库");
    assert_eq!(db["nodes"]["n1"]["parent"], json!("g1"), "旧父子边必须落库");
    assert!(db["nodes"]["deleted"]["trashedAt"].is_i64(), ".gone 必须是库内墓碑");
    println!("命中行: 迁移后库里 n1.title={} deleted.trashedAt={}",
        db["nodes"]["n1"]["title"], db["nodes"]["deleted"]["trashedAt"]);
    let _ = std::fs::remove_file(&path);
}
