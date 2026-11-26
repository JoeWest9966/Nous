//! #67 会话归档：archive / 归档库 / restore 的 HTTP 判据。
//!
//! 全部打在临时真库 + 真 router 上：判据看的是 `/v1/sessions`、`/v1/archive`、
//! `/v1/sessions/:id/entries` 的状态码与 JSON 形状，不是内部函数。
//! 核心：归档 → purge（真删）→ entries 404 → restore{gen} → 逐条 id 一致。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_durable::session::Session;
use nous_durable::{Storage, StorageWrite};
use nous_server::{router, AppState};
use serde_json::{json, Value};
use std::path::PathBuf;
use tower::ServiceExt;

fn temp_db(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nous-archive-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("nous.sqlite")
}

async fn seed(path: &PathBuf, id: i64, entries: &[i64]) {
    let session = Session::open(path).unwrap();
    let mut writes = vec![StorageWrite::Conversation(json!({ "id": id, "name": "归档测试" }))];
    for entry_id in entries {
        writes.push(StorageWrite::Entry(json!({
            "id": entry_id,
            "conversationId": id,
            "kind": "pi.user",
            "model": [{ "role": "user", "content": format!("第 {entry_id} 条") }],
        })));
    }
    // review 7：票面要求的闭包成员全部种上（tasks / submissions / documents /
    // document_revisions / record_ids 由 mint 分配，会话与 entries 显式写）。
    writes.push(StorageWrite::Task(json!({
        "id": id + 1_000,
        "conversationId": id,
        "kind": "pi.generation",
        "version": 1,
        "input": {},
        "background": false,
        "abortRequested": false,
        "state": { "status": "terminal" },
    })));
    writes.push(StorageWrite::Submission(json!({
        "id": id + 2_000,
        "conversationId": id,
        "type": "input",
        "status": "queued",
        "content": "归档测试提交",
        "requestId": "archive-seed-1",
    })));
    let outcome = session.commit(writes, Default::default()).await.unwrap();
    assert!(outcome.refused.is_none(), "{:?}", outcome.refused);
    // 文档 + 修订（一次 Base，一次 Delta 各写一条 revision）。
    let doc = StorageWrite::DocumentCreate {
        id: id + 3_000,
        record: json!({
            "id": id + 3_000,
            "kind": "qrd3wh.test",
            "scope": { "kind": "conversation", "conversationId": id },
            "history": "latest",
            "fork": "current",
        }),
        content: nous_durable::DocumentContent::Base {
            version: 1,
            value: json!({ "text": "归档文档" }),
        },
    };
    let outcome = session.commit(vec![doc], Default::default()).await.unwrap();
    assert!(outcome.refused.is_none(), "{:?}", outcome.refused);
    session.close().await.unwrap();
}

/// 从独立只读连接数各表行数——不同源（rusqlite 独立连接，不经 durable 的 API）。
fn table_counts(path: &PathBuf) -> std::collections::BTreeMap<String, i64> {
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for table in ["conversations", "entries", "tasks", "submissions", "documents", "document_revisions", "record_ids"] {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap();
        out.insert(table.to_string(), n);
    }
    out
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
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

fn item<'a>(body: &'a Value, id: i64) -> Option<&'a Value> {
    body["items"].as_array()?.iter().find(|item| item["id"] == json!(id))
}

#[tokio::test]
async fn archive_purge_restore_round_trips_every_entry_id() {
    let path = temp_db("roundtrip");
    let ids = [101, 102, 103];
    seed(&path, 500, &ids).await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    // 归档：回包带 gen / ids / rows，会话仍在列表里，墓碑 purged:false。
    let before_counts = table_counts(&path);
    assert!(before_counts["tasks"] >= 1 && before_counts["submissions"] >= 1
        && before_counts["documents"] >= 1 && before_counts["document_revisions"] >= 1,
        "seed 必须种全票面闭包成员：{before_counts:?}");
    let (status, body) = call(&routes, "POST", "/v1/sessions/500/archive", None).await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    let gen = body["gen"].as_str().expect("回包有 gen").to_string();
    assert_eq!(gen.len(), 8);
    assert_eq!(body["ids"], json!([500]));

    // review 7：generation 文件必须含票面要求的全成员（不只是 conversations/entries）。
    let gen_file = path.parent().unwrap().join(format!("archives/gen-{gen}.db"));
    assert!(gen_file.exists());
    let gen_counts = table_counts(&gen_file);
    for table in ["conversations", "entries", "tasks", "submissions", "documents", "document_revisions", "record_ids"] {
        assert!(gen_counts[table] > 0, "generation 缺成员 {table}：{gen_counts:?}");
    }

    let (_, sessions) = call(&routes, "GET", "/v1/sessions", None).await;
    let entry = item(&sessions, 500).expect("归档后会话仍在 /v1/sessions");
    assert_eq!(entry["archived"]["purged"], json!(false));
    assert_eq!(entry["archived"]["gen"], json!(gen));

    let (_, archive) = call(&routes, "GET", "/v1/archive", None).await;
    let gens = archive["gens"].as_array().unwrap();
    assert!(gens.iter().any(|g| g["gen"] == json!(gen)), "归档库能列出这一代");
    let archived_item = gens.iter().flat_map(|g| g["items"].as_array().unwrap().iter())
        .find(|i| i["id"] == json!(500)).expect("归档库列出该会话");
    assert_eq!(archived_item["live"], json!(true));
    assert_eq!(archived_item["archived"]["purged"], json!(false));

    // purge 真删 → entries 404。
    let (status, _) = call(&routes, "POST", "/v1/sessions/500/purge", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&routes, "GET", "/v1/sessions/500/entries", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "撕掉之后 entries 必须 404");

    let (_, archive) = call(&routes, "GET", "/v1/archive", None).await;
    let destroyed = archive["gens"].as_array().unwrap().iter()
        .flat_map(|g| g["items"].as_array().unwrap().iter())
        .find(|i| i["id"] == json!(500)).unwrap();
    assert_eq!(destroyed["live"], json!(false));
    assert_eq!(destroyed["archived"]["purged"], json!(true));

    // restore：逐条 id 比对（不是只比条数）。
    let (status, body) = call(
        &routes,
        "POST",
        "/v1/archive/500/restore",
        Some(json!({ "gen": gen })),
    ).await;
    assert_eq!(status, StatusCode::OK, "实测 {:?}", body);
    let (status, entries) = call(&routes, "GET", "/v1/sessions/500/entries", None).await;
    assert_eq!(status, StatusCode::OK);
    let mut after: Vec<i64> = entries["items"].as_array().unwrap().iter()
        .map(|entry| entry["id"].as_i64().unwrap()).collect();
    after.sort_unstable();
    let mut expected = ids.to_vec();
    expected.sort_unstable();
    assert_eq!(after, expected, "捞回来的 entry id 必须逐条一致");

    let restored_counts = table_counts(&path);
    for table in ["conversations", "entries", "tasks", "submissions", "documents", "document_revisions", "record_ids"] {
        assert!(restored_counts[table] >= before_counts[table],
            "restore 后 {table} 应至少回到归档前：{restored_counts:?} vs {before_counts:?}");
    }
    let (_, archive) = call(&routes, "GET", "/v1/archive", None).await;
    let restored = archive["gens"].as_array().unwrap().iter()
        .flat_map(|g| g["items"].as_array().unwrap().iter())
        .find(|i| i["id"] == json!(500)).unwrap();
    assert_eq!(restored["live"], json!(true));
    assert_eq!(restored["archived"], json!(null), "捞回后墓碑抹掉");

    // 墓碑抹掉之后还能再归档。
    let (status, body) = call(&routes, "POST", "/v1/sessions/500/archive", None).await;
    assert_eq!(status, StatusCode::OK, "捞回后可再次归档: {:?}", body);
}

#[tokio::test]
async fn same_day_archive_is_idempotent_by_id() {
    let path = temp_db("same-day");
    seed(&path, 501, &[201]).await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    let (status, first) = call(&routes, "POST", "/v1/sessions/501/archive", None).await;
    assert_eq!(status, StatusCode::OK);
    let gen = first["gen"].as_str().unwrap().to_string();
    let (status, second) = call(&routes, "POST", "/v1/sessions/501/archive", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["gen"], json!(gen));
    assert_eq!(second["rows"], first["rows"]);

    let (_, archive) = call(&routes, "GET", "/v1/archive", None).await;
    let gens = archive["gens"].as_array().unwrap();
    assert_eq!(gens.iter().filter(|g| g["gen"] == json!(gen)).count(), 1);
    let rows = gens.iter().find(|g| g["gen"] == json!(gen)).unwrap()["items"]
        .as_array().unwrap().iter().filter(|i| i["id"] == json!(501)).count();
    assert_eq!(rows, 1, "同日二次归档不产生重复行");
}

#[tokio::test]
async fn busy_session_and_live_descendant_are_both_409() {
    let path = temp_db("conflicts");
    let session = Session::open(&path).unwrap();
    let writes = vec![
        StorageWrite::Conversation(json!({ "id": 600 })),
        StorageWrite::Task(json!({
            "id": 700,
            "conversationId": 600,
            "kind": "pi.generation",
            "version": 1,
            "input": {},
            "background": false,
            "abortRequested": false,
            "state": { "status": "running" },
        })),
        StorageWrite::Conversation(json!({ "id": 601 })),
        StorageWrite::Conversation(json!({ "id": 602, "parent": { "conversationId": 601, "at": 1 } })),
    ];
    let outcome = session.commit(writes, Default::default()).await.unwrap();
    assert!(outcome.refused.is_none(), "{:?}", outcome.refused);
    session.close().await.unwrap();
    let app = AppState::durable(&path).unwrap();
    let routes = router(app.clone());

    let (status, body) = call(&routes, "POST", "/v1/sessions/600/archive", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "忙会话归档 409: {:?}", body);
    let (status, body) = call(&routes, "POST", "/v1/sessions/601/archive", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "活后代归档 409: {:?}", body);
}

#[tokio::test]
async fn capabilities_declares_archive_enabled_without_reason() {
    let path = temp_db("capabilities");
    let _storage = Storage::open(&path).unwrap();
    let app = AppState::durable(&path).unwrap();
    let routes = router(app);
    let (_, body) = call(&routes, "GET", "/v1/capabilities", None).await;
    let module = body["modules"].as_array().unwrap().iter()
        .find(|m| m["name"] == json!("Data_Archive")).expect("Data_Archive 模块仍在清单里");
    assert_eq!(module["enabled"], json!(true), "实测 {:?}", module);
    assert!(module.get("reason").is_none(), "enabled 时 reason 消失");
}

#[tokio::test]
async fn archiving_does_not_touch_durable_migrations_or_backup_check() {
    let path = temp_db("check");
    seed(&path, 700, &[301]).await;
    let app = AppState::durable(&path).unwrap();
    let routes = router(app);
    let (status, body) = call(&routes, "POST", "/v1/sessions/700/archive", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // gen 文件存在，但 durable 的 --check 只认 NOUS_DB 那一份：schema 仍是 2。
    let gen = body["gen"].as_str().unwrap();
    let gen_file = path.parent().unwrap().join(format!("archives/gen-{gen}.db"));
    assert!(gen_file.exists(), "generation 文件落在 archives/ 下");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_nous-app"))
        .arg("--check")
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.lines().next(), Some("quick_check=ok"), "{text}");
    assert_eq!(text.lines().nth(1), Some("schema=2"), "{text}");
}
