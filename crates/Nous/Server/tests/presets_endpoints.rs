//! #60 presets：资源化端点 + 修订表的行为判据。
//!
//! 三处公共接缝，全部从真实实现取值：
//! 1. HTTP：真 Router（`tower::ServiceExt::oneshot`）；
//! 2. 盘：直接读 `<root>/<section>/<name>/<manifest>` 的字节，独立算 sha256（`sha256sum`）；
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
use nous_presets_admin::ModelLevels;
use nous_presets_admin::routes::Sources;
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

        // 解析要问的外部根：全部落在临时目录里，判据不读开发机的真实私库。
        let skills = dir.join("skills");
        let prompts = dir.join("prompts");
        let extensions = dir.join("extend");
        let triggers = dir.join("triggers");
        std::fs::create_dir_all(skills.join("deep-dive")).unwrap();
        std::fs::write(skills.join("deep-dive/SKILL.md"), "---\nname: deep-dive\n---\n正文").unwrap();
        std::fs::create_dir_all(prompts.join("smart-ass")).unwrap();
        std::fs::write(prompts.join("smart-ass/nous.json"), r#"{"description":"d","category":[]}"#).unwrap();
        std::fs::write(prompts.join("smart-ass/PROMPT.md"), "正文").unwrap();
        std::fs::create_dir_all(extensions.join("probe")).unwrap();
        std::fs::create_dir_all(triggers.join("single-shot")).unwrap();
        std::fs::write(
            triggers.join("single-shot/nous.json"),
            r#"{"name":"single-shot","summary":"一次","version":1,"params":[{"name":"epoch","type":"epoch","required":true}]}"#,
        )
        .unwrap();
        let sources = Sources {
            root: root.clone(),
            db: db.clone(),
            skills,
            prompts,
            extensions,
            triggers,
            models: Some(vec![ModelLevels {
                provider: "volc-ark".into(),
                model_id: "flash".into(),
                thinking_levels: vec!["off".into(), "medium".into()],
            }]),
            device_model: Some(("volc-ark".into(), "flash".into())),
            catalog_env: Default::default(),
        };

        let source: std::sync::Arc<dyn SessionSource> = std::sync::Arc::new(EmptySource);
        // 先建插件库（迁移出 presets_revisions / preset_write_receipts），再注入写者。
        let _ = nous_store::Store::open(&db).unwrap();
        // 搬家后：宿主只交来源（`Sources`），写者与 handler 都在插件里。
        let app = router(AppState::with_source(source).with_presets(sources));
        Self { _guard: guard, dir, root, db, app }
    }

    fn file(&self, section: &str, name: &str) -> PathBuf {
        self.root.join(section).join(name).join("preset.json")
    }

    fn ultimate(&self, name: &str) -> PathBuf {
        self.root.join("Ultimate-Template").join(name).join("ultimate.json")
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
    let admin = body["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == json!("presets-admin"))
        .expect("capabilities 必须声明 presets-admin 模块");
    assert_eq!(admin["enabled"], json!(true), "presets-admin 的端点已挂上：{admin}");
}

/// 新契约（TS 内核同形）：`preset.json` 是**扁平**配置，`spec` 就是整份 body。
fn preset_body(name: &str, summary: &str) -> Value {
    json!({
        "v": 1,
        "name": name,
        "summary": summary,
        "description": "一段说明",
        "category": ["写作", "周报"],
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
    assert!(
        serde_json::from_slice::<Value>(&on_disk).unwrap().get("spec").is_none(),
        "新契约不许把整份 body 再包一层 spec"
    );

    let (status, roster, _) = call(&fixture.app, "GET", "/v1/presets/Session-Template", None, None).await;
    assert_eq!(status, StatusCode::OK, "{roster}");
    let items = roster["presets"].as_array().expect("roster.presets 是数组");
    let item = items.iter().find(|item| item["name"] == json!("foo"))
        .unwrap_or_else(|| panic!("roster 里没有 foo：{roster}"));
    assert_eq!(item["sha"], json!(expected_sha), "roster.sha 必须等于文件字节的 sha256：{item}");
    assert_eq!(item["version"], response["version"], "version 必须与 PUT 回执一致");
}

#[tokio::test]
async fn roster_carries_flat_spec_and_resolved_states() {
    let fixture = Fixture::new("resolved");
    let body = json!({
        "v": 1,
        "name": "deep-dive",
        "summary": "挖到底",
        "category": ["工程"],
        "model": { "provider": "volc-ark", "modelId": "flash" },
        "thinkingLevel": "medium",
        "skills": ["deep-dive", "gone"],
        "extensions": ["probe", "prompt:x"],
        "template": "smart-ass",
        "requestId": "resolved-1",
    });
    let (status, put, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/deep-dive", Some(body), None).await;
    assert_eq!(status, StatusCode::CREATED, "{put}");
    let (status, roster, _) = call(&fixture.app, "GET", "/v1/presets/Session-Template", None, None).await;
    assert_eq!(status, StatusCode::OK, "{roster}");
    let item = roster["presets"].as_array().unwrap().iter()
        .find(|item| item["name"] == json!("deep-dive")).unwrap().clone();
    // `spec` 就是整份 body —— 图的数据源。
    assert_eq!(item["spec"]["model"]["modelId"], json!("flash"));
    assert!(item["specError"].is_null(), "形状对的条目 specError 必须空：{item}");
    // `resolved` 的七维状态：引用坏了报 invalid，没坏报 ok。
    assert_eq!(item["resolved"]["model"]["state"], json!("ok"));
    assert_eq!(item["resolved"]["thinking"]["state"], json!("ok"));
    assert_eq!(item["resolved"]["skills"][0]["state"], json!("ok"));
    assert_eq!(item["resolved"]["skills"][1]["state"], json!("invalid"));
    assert_eq!(item["resolved"]["extensions"][1]["state"], json!("invalid"), "名牌不是扩展");
    assert_eq!(item["resolved"]["template"]["state"], json!("ok"));
    // 没有 git 仓：tracked=false，但 dir 是真的绝对路径。
    assert_eq!(item["tracked"], json!(false));
    assert!(item["dir"].as_str().unwrap().ends_with("Session-Template/deep-dive"));
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

#[tokio::test]
async fn idempotent_replays_keep_version_and_disk_unchanged() {
    let fixture = Fixture::new("idempotent");
    let body = preset_body("idem", "首版");
    let (_, first, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/idem", Some(body.clone()), None).await;
    let version = first["version"].as_i64().unwrap();
    let (_, replay, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/idem", Some(body), None).await;
    assert_eq!(replay["version"], json!(version), "重放 PUT 的 version 不许涨：{replay}");
    assert_eq!(revision_rows(&fixture.db, "Session-Template", "idem").len(), 1, "重放不许多一条修订");

    let copy = json!({ "name": "copied", "requestId": "copy-1" });
    let (status, _, _) = call(&fixture.app, "POST", "/v1/presets/Session-Template/idem/copy", Some(copy.clone()), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, again, _) = call(&fixture.app, "POST", "/v1/presets/Session-Template/idem/copy", Some(copy), None).await;
    let copy_version = again["version"].as_i64().unwrap();
    assert_eq!(revision_rows(&fixture.db, "Session-Template", "copied").len(), 1);
    assert!(copy_version >= 1, "copy 回执必须有 version");
    // 复制出来的清单里 `name` 必须换成新名字（目录名 = 主键）。
    let copied: Value = serde_json::from_slice(&std::fs::read(fixture.file("Session-Template", "copied")).unwrap()).unwrap();
    assert_eq!(copied["name"], json!("copied"));
}

#[tokio::test]
async fn wrong_if_match_refuses_and_leaves_the_file_bytes_untouched() {
    let fixture = Fixture::new("if-match");
    let body = preset_body("locked", "原文");
    let (_, first, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/locked", Some(body.clone()), None).await;
    let version = first["version"].as_i64().unwrap();
    let before = std::fs::read(fixture.file("Session-Template", "locked")).unwrap();
    let mut changed = preset_body("locked", "不该写进去");
    changed["requestId"] = json!("other-req");
    let (status, error, _) = call(
        &fixture.app,
        "PUT",
        "/v1/presets/Session-Template/locked",
        Some(changed),
        Some(&format!("{}", version + 99)),
    ).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED, "{error}");
    // 不信状态码：读文件逐字比对。
    assert_eq!(std::fs::read(fixture.file("Session-Template", "locked")).unwrap(), before);
    assert_eq!(revision_rows(&fixture.db, "Session-Template", "locked").len(), 1);
}

#[tokio::test]
async fn dry_run_validates_without_writing_but_real_put_rejects_the_same_file() {
    let fixture = Fixture::new("dry-run");
    let bad = json!({ "v": 1, "name": "bad", "summary": "x", "category": [], "content": "正文不归 preset 管", "requestId": "dry-1" });
    let (status, body, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/bad?dry_run=1", Some(bad.clone()), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "dry_run 也走同一份校验：{body}");
    assert!(!fixture.file("Session-Template", "bad").exists(), "dry_run 不许建目录");
    let (status, body, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/bad", Some(bad), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "真 PUT 必须同样 400：{body}");
    assert!(!fixture.file("Session-Template", "bad").exists());

    // 合法的 dry_run 也不许写盘 —— 否则「dry_run 照写盘」的哨兵打不红这条判据。
    let good = preset_body("dry-ok", "不该落盘");
    let (status, body, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/dry-ok?dry_run=1", Some(good), None).await;
    assert_eq!(status, StatusCode::OK, "合法的 dry_run 应 200：{body}");
    assert!(!fixture.file("Session-Template", "dry-ok").exists(), "dry_run=1 不许建目录/写文件");
    assert!(revision_rows(&fixture.db, "Session-Template", "dry-ok").is_empty(), "dry_run 不许多一条修订");
}

#[tokio::test]
async fn get_preset_returns_etag_equal_to_its_version() {
    let fixture = Fixture::new("etag");
    let body = preset_body("tag", "有 ETag");
    let (_, put, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/tag", Some(body), None).await;
    let (status, _, etag) = call(&fixture.app, "GET", "/v1/presets/Session-Template/tag", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(etag.as_deref(), put["version"].as_i64().map(|v| v.to_string()).as_deref());
}

#[tokio::test]
async fn writes_to_one_section_never_touch_the_other() {
    let fixture = Fixture::new("isolation");
    let body = preset_body("only", "只在 Session-Template");
    let (status, _, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/only", Some(body), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let patch = json!({ "archived": true, "requestId": "patch-1" });
    let (status, _, _) = call(&fixture.app, "PATCH", "/v1/presets/Session-Template/only", Some(patch), None).await;
    assert_eq!(status, StatusCode::OK);
    let rollback = json!({ "n": 1, "requestId": "rb-1" });
    let (status, _, _) = call(&fixture.app, "POST", "/v1/presets/Session-Template/only/rollback", Some(rollback), None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, other, _) = call(&fixture.app, "GET", "/v1/presets/Ultimate-Template", None, None).await;
    assert_eq!(status, StatusCode::OK, "{other}");
    assert_eq!(other["presets"].as_array().unwrap().len(), 0, "另一段 roster 不许多一条：{other}");
    assert!(revision_rows(&fixture.db, "Ultimate-Template", "only").is_empty(), "另一段 revisions 不许多一条");
    assert!(!fixture.ultimate("only").exists());
}

/// 新契约：未知键必须报错（拼错的键等于那条设置静默没生效）。
#[tokio::test]
async fn unknown_keys_are_rejected_not_silently_dropped() {
    let fixture = Fixture::new("unknown-key");
    let mut body = preset_body("legacy", "老条目");
    body["spec"] = json!({ "v": 1 });
    let (status, error, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/legacy", Some(body), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert!(
        error["error"].as_str().unwrap().contains("不认识的键「spec」"),
        "必须点名那个键：{error}"
    );
    assert!(!fixture.file("Session-Template", "legacy").exists());
}

#[tokio::test]
async fn archive_moves_dir_into_dot_archive_and_roster_lists_it() {
    let fixture = Fixture::new("archive");
    let body = preset_body("gone", "要归档的");
    let (status, _, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/gone", Some(body), None).await;
    assert_eq!(status, StatusCode::CREATED);

    let patch = json!({ "archived": true, "requestId": "archive-1" });
    let (status, response, _) = call(&fixture.app, "PATCH", "/v1/presets/Session-Template/gone", Some(patch), None).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    // 盘上：原目录没了，`.archive/gone/` 在。
    assert!(!fixture.file("Session-Template", "gone").exists());
    assert!(fixture.root.join("Session-Template/.archive/gone/preset.json").is_file());

    let (status, roster, _) = call(&fixture.app, "GET", "/v1/presets/Session-Template", None, None).await;
    assert_eq!(status, StatusCode::OK, "{roster}");
    assert!(roster["presets"].as_array().unwrap().is_empty(), "归档的不许留在名册里：{roster}");
    let archived = roster["archived"].as_array().unwrap();
    assert_eq!(archived.len(), 1, "归档区要列出来：{roster}");
    assert_eq!(archived[0]["name"], json!("gone"));
    assert_eq!(archived[0]["summary"], json!("要归档的"));

    // 还原回原处。
    let (status, _, _) = call(&fixture.app, "POST", "/v1/presets/Session-Template/restore", Some(json!({ "name": "gone" })), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(fixture.file("Session-Template", "gone").is_file());
}

#[tokio::test]
async fn validate_reports_missing_skill_and_unknown_key() {
    let fixture = Fixture::new("validate");
    let bad = json!({
        "v": 1, "name": "x", "summary": "s", "category": [],
        "skills": ["gone"], "thinking": "medium"
    });
    let (status, body, _) = call(&fixture.app, "POST", "/v1/presets/Session-Template/validate", Some(json!({ "spec": bad })), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(false));
    assert!(
        body["errors"].as_array().unwrap().iter().any(|e| e.as_str().unwrap().contains("不认识的键「thinking」")),
        "{body}"
    );

    let good = json!({
        "v": 1, "name": "x", "summary": "s", "category": [],
        "skills": ["gone"], "template": "smart-ass"
    });
    let (status, body, _) = call(&fixture.app, "POST", "/v1/presets/Session-Template/validate", Some(json!({ "spec": good })), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(false));
    assert!(
        body["errors"].as_array().unwrap().iter().any(|e| e.as_str().unwrap().contains("技能「gone」")),
        "{body}"
    );
    assert_eq!(body["resolved"]["template"]["state"], json!("ok"));
}

#[tokio::test]
async fn categories_rename_rewrites_preset_mounts() {
    let fixture = Fixture::new("categories");
    let mut body = preset_body("weekly", "周报");
    body["category"] = json!(["写作", "写作/周报"]);
    let (status, _, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/weekly", Some(body), None).await;
    assert_eq!(status, StatusCode::CREATED);

    let rename = json!({ "op": "rename", "name": "写作", "to": "写作2" });
    let (status, response, _) = call(&fixture.app, "POST", "/v1/presets/Session-Template/categories", Some(rename), None).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let categories = response["categories"].as_array().unwrap();
    assert!(categories.iter().any(|c| c == &json!("写作2")), "{response}");
    assert!(categories.iter().any(|c| c == &json!("写作2/周报")), "{response}");

    let on_disk: Value = serde_json::from_slice(&std::fs::read(fixture.file("Session-Template", "weekly")).unwrap()).unwrap();
    assert_eq!(on_disk["category"], json!(["写作2", "写作2/周报"]), "预设挂着的类别要跟着改");
}

#[tokio::test]
async fn create_from_device_writes_model_and_rejects_bad_shape() {
    let fixture = Fixture::new("create");
    let (status, response, _) = call(
        &fixture.app,
        "POST",
        "/v1/presets/Session-Template/create",
        Some(json!({ "name": "fresh", "summary": "新的", "category": ["工程"], "from": "device" })),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let on_disk: Value = serde_json::from_slice(&std::fs::read(fixture.file("Session-Template", "fresh")).unwrap()).unwrap();
    assert_eq!(on_disk["model"], json!({ "provider": "volc-ark", "modelId": "flash" }));
    assert_eq!(on_disk["v"], json!(1));

    let (status, error, _) = call(
        &fixture.app,
        "POST",
        "/v1/presets/Session-Template/create",
        Some(json!({ "name": "bad-name", "summary": "  " })),
        None,
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert!(error["error"].as_str().unwrap().contains("summary"), "{error}");
}

#[tokio::test]
async fn files_listing_and_manifest_write_gate() {
    let fixture = Fixture::new("files");
    let body = preset_body("listed", "列文件");
    let (status, _, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/listed", Some(body), None).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, listing, _) = call(&fixture.app, "GET", "/v1/presets/Session-Template/files?name=listed", None, None).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    assert!(
        listing["files"].as_array().unwrap().iter().any(|f| f["path"] == json!("preset.json")),
        "{listing}"
    );

    // 写清单文件要过形状闸：坏 JSON / 未知键都 400，且盘上原样。
    let before = std::fs::read(fixture.file("Session-Template", "listed")).unwrap();
    let (status, error, _) = call(
        &fixture.app,
        "PUT",
        "/v1/presets/Session-Template/file",
        Some(json!({ "name": "listed", "path": "preset.json", "text": "{\"v\":2}" })),
        None,
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(std::fs::read(fixture.file("Session-Template", "listed")).unwrap(), before);

    // 说明文件不走形状闸。
    let (status, _, _) = call(
        &fixture.app,
        "PUT",
        "/v1/presets/Session-Template/file",
        Some(json!({ "name": "listed", "path": "PRESET.md", "text": "说明书" })),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("Session-Template/listed/PRESET.md")).unwrap(),
        "说明书"
    );
}

#[tokio::test]
async fn ultimate_section_uses_ultimate_manifest_and_reference_gate() {
    let fixture = Fixture::new("ultimate");
    let (status, error, _) = call(
        &fixture.app,
        "POST",
        "/v1/presets/Ultimate-Template/create",
        Some(json!({ "name": "nightly", "summary": "每晚", "trigger": "nope", "session": "scratch", "params": {} })),
        None,
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert!(error["error"].as_str().unwrap().contains("不在 NOUS_TRIGGER"), "{error}");

    // 会话模板段里先有一条 scratch，触发器也在：这时才能建。
    let scratch = preset_body("scratch", "临时");
    let (status, _, _) = call(&fixture.app, "PUT", "/v1/presets/Session-Template/scratch", Some(scratch), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, response, _) = call(
        &fixture.app,
        "POST",
        "/v1/presets/Ultimate-Template/create",
        Some(json!({ "name": "nightly", "summary": "每晚", "trigger": "single-shot", "session": "scratch", "params": { "epoch": 1800000060 } })),
        None,
    ).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let on_disk: Value = serde_json::from_slice(&std::fs::read(fixture.ultimate("nightly")).unwrap()).unwrap();
    assert_eq!(on_disk["trigger"], json!("single-shot"));

    let (status, roster, _) = call(&fixture.app, "GET", "/v1/presets/Ultimate-Template", None, None).await;
    assert_eq!(status, StatusCode::OK, "{roster}");
    let item = &roster["presets"].as_array().unwrap()[0];
    assert_eq!(item["name"], json!("nightly"));
    assert_eq!(item["resolved"]["trigger"]["state"], json!("ok"));
    assert_eq!(item["resolved"]["session"]["state"], json!("ok"));
}
