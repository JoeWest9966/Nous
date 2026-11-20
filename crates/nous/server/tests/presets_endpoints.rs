//! #60 presets：资源化端点 + 修订表的行为判据。
//!
//! 三处公共接缝，全部从真实实现取值：
//! 1. HTTP：真 Router（`tower::ServiceExt::oneshot`）；
//! 2. 盘：直接读 `<root>/<section>/<name>/preset.json` 的字节，独立算 sha256（`sha256sum`）；
//! 3. 库：另开只读 `rusqlite` 连接读 `nous-plugins.db` 的 `presets_revisions`。
//!
//! 不拿响应体当证据：响应的 `sha` 必须等于文件字节的哈希，修订必须真在库里。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_durable::storage::sessions::SessionItem;
use nous_server::{router, AppState, Result, ServerError, SessionSource};
use serde_json::{json, Value};
use tower::ServiceExt;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn temp_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-presets-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

struct Fixture {
    _guard: std::sync::MutexGuard<'static, ()>,
    dir: PathBuf,
    root: PathBuf,
    db: PathBuf,
    app: Router,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir(tag);
        let root = dir.join("templates");
        let db = dir.join("nous-plugins.db");
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("NOUS_PRESETS", &root);
        // 每个 case 用独立路径，AppState 构造时只读一次环境，之后 handler 自己按配置解析。
        let source: std::sync::Arc<dyn SessionSource> = std::sync::Arc::new(EmptySource);
        // 先建插件库（迁移出 presets_revisions / preset_write_receipts），再注入写者。
        let _ = nous_store::Store::open(&db).unwrap();
        let store = std::sync::Arc::new(nous_server::presets::PresetStore::new(root.clone(), db.clone()));
        let app = router(AppState::with_source(source).with_presets(store));
        Self { _guard: guard, dir, root, db, app }
    }

    fn file(&self, section: &str, name: &str) -> PathBuf {
        self.root.join(section).join(name).join("preset.json")
    }
}

struct EmptySource;
impl SessionSource for EmptySource {
    fn sessions(&self) -> Result<Vec<SessionItem>> {
        Ok(vec![])
    }
    fn entries(&self, _id: i64) -> Result<Vec<Value>> {
        Ok(vec![])
    }
    fn live_message(&self, _id: i64) -> Result<Option<Value>> {
        Ok(None)
    }
}

#[allow(dead_code)]
fn _error_shape_is_available(error: ServerError) -> StatusCode {
    error.status()
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    if_match: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(value) = if_match {
        builder = builder.header("if-match", value);
    }
    let request = builder
        .header("content-type", "application/json")
        .body(match body {
            Some(value) => Body::from(serde_json::to_vec(&value).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body, etag)
}

/// 独立实现：用系统 `sha256sum` 算文件字节，不看产品代码里的哈希函数。
fn sha256_file(path: &Path) -> String {
    let output = Command::new("sha256sum").arg(path).output().unwrap();
    assert!(output.status.success(), "sha256sum 失败：{path:?}");
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

/// 另开连接读插件库：HTTP 写完之后改动的真凭据在库里，不在响应体里。
fn revision_rows(db: &Path, section: &str, name: &str) -> Vec<(i64, String, String, String)> {
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut stmt = conn
        .prepare("SELECT n, sha, body, request_id FROM presets_revisions WHERE section=?1 AND name=?2 ORDER BY n")
        .unwrap();
    stmt.query_map(rusqlite::params![section, name], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })
    .unwrap()
    .map(|row| row.unwrap())
    .collect()
}

#[tokio::test]
async fn capabilities_enable_presets_without_a_reason() {
    let fixture = Fixture::new("capabilities");
    let (status, body, _) = call(&fixture.app, "GET", "/v1/capabilities", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let module = body["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == json!("presets"))
        .expect("capabilities 必须声明 presets 模块");
    assert_eq!(module["enabled"], json!(true), "presets 必须由清单打开：{module}");
    assert!(module.get("reason").is_none(), "enabled:true 不该带 reason：{module}");
    assert!(
        module["endpoints"]
            .as_array()
            .is_some_and(|list| list.iter().any(|e| e.as_str() == Some("GET /v1/presets/{section}"))),
        "presets 的端点必须从注册表声明出来：{module}"
    );
}

fn preset_body(name: &str, summary: &str) -> Value {
    json!({
        "name": name,
        "summary": summary,
        "description": "一段说明",
        "category": ["写作", "周报"],
        "spec": { "v": 1, "session": "default" },
        "requestId": "same-req",
    })
}

#[tokio::test]
async fn put_writes_real_file_and_roster_sha_matches_file_bytes() {
    let fixture = Fixture::new("disk-sha");
    let body = preset_body("foo", "第一版");
    let (status, response, _) = call(
        &fixture.app,
        "PUT",
        "/v1/presets/Session-Template/foo",
        Some(body.clone()),
        None,
    ).await;
    assert_eq!(status, StatusCode::CREATED, "{response}");
    // 不信响应体：直接读盘。
    let path = fixture.file("Session-Template", "foo");
    assert!(path.is_file(), "PUT 之后磁盘上必须真的有 {path:?}");
    let on_disk = std::fs::read(&path).unwrap();
    let expected_sha = sha256_file(&path);
    assert_eq!(
        serde_json::from_slice::<Value>(&on_disk).unwrap()["summary"],
        json!("第一版"),
        "文件内容必须是刚写进去的那份"
    );

    let (status, roster, _) = call(&fixture.app, "GET", "/v1/presets/Session-Template", None, None).await;
    assert_eq!(status, StatusCode::OK, "{roster}");
    let items = roster["items"].as_array().expect("roster.items 是数组");
    let item = items.iter().find(|item| item["name"] == json!("foo"))
        .unwrap_or_else(|| panic!("roster 里没有 foo：{roster}"));
    assert_eq!(item["sha"], json!(expected_sha), "roster.sha 必须等于文件字节的 sha256：{item}；文件={:?}", std::fs::read(&path).unwrap());
    assert_eq!(item["version"], response["version"], "version 必须与 PUT 回执一致");
}

#[tokio::test]
async fn revisions_keep_every_version_and_rollback_restores_exact_bytes() {
    let fixture = Fixture::new("revisions");
    let first = preset_body("rev", "甲");
    let (status, _, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/rev", Some(first.clone()), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let mut second = preset_body("rev", "乙");
    second["requestId"] = json!("second-req");
    let (status, _, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/rev", Some(second), None).await;
    assert_eq!(status, StatusCode::CREATED);

    // 库里必须真有两行，且原文不同 —— 不是内存里回个回执。
    let rows = revision_rows(&fixture.db, "Session-Template", "rev");
    assert_eq!(rows.len(), 2, "revisions 表应有两条：{rows:?}");
    assert_ne!(rows[0].3, rows[1].3, "两次 PUT 的 request_id 必须不同：{rows:?}");
    assert_ne!(rows[0].2, rows[1].2, "两版正文必须不同：{rows:?}");

    let (status, list, _) = call(&fixture.app, "GET", "/v1/presets/Session-Template/rev/revisions", None, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"].as_array().unwrap().len(), 2);

    let (status, old, _) = call(&fixture.app, "GET", "/v1/presets/Session-Template/rev/revisions/1", None, None).await;
    assert_eq!(status, StatusCode::OK, "{old}");
    let first_body = old["body"].as_str().unwrap();

    let rollback = json!({ "n": 1, "requestId": "rollback-1" });
    let (status, receipt, _) = call(&fixture.app, "POST", "/v1/presets/Session-Template/rev/rollback", Some(rollback), None).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");

    let (status, current, _) = call(&fixture.app, "GET", "/v1/presets/Session-Template/rev", None, None).await;
    assert_eq!(status, StatusCode::OK, "{current}");
    assert_eq!(
        current["preset"]["summary"], json!("甲"),
        "rollback 后正文逐字等于第 1 版：{current}"
    );
    let disk = std::fs::read_to_string(fixture.file("Session-Template", "rev")).unwrap();
    assert_eq!(disk.trim_end(), first_body.trim_end(), "盘上的字节必须是第 1 版原文");
    // rollback 自己也是一次修订：表从 2 行变 3 行。
    assert_eq!(revision_rows(&fixture.db, "Session-Template", "rev").len(), 3);
}
