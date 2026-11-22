//! #61 combos：组合开关的 HTTP 判据（不同源：盘面文件 ↔ HTTP 回包）。
//!
//! 接缝：真 Router（`ServiceExt::oneshot`）+ 临时目录里的真 `preset.json`。
//! 不读 fixtures、不起监听端口、不碰 `/var/lib/nous`。
//! 名册不是常量：每次 GET 都重扫盘，所以测试在两次 GET 之间直接改盘。

mod common;

use std::fs;
use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

/// `NOUS_TEST_COMBOS_WRITE_FAIL` 是**进程级**注入开关：谁把它打开，同一进程里所有 PATCH
/// 都会写失败。所以凡是打 PATCH 的判据都得排它（否则全量并发跑时，别的判据会随机红 ——
/// 2026-11-22 实测过）。与 `capabilities_shapes.rs` 的 `CONTENT_ENV_LOCK` 同一手法。
static PATCH_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn temp_dir(_tag: &str) -> tempfile::TempDir {
    tempfile::tempdir().expect("临时目录")
}

/// 与 #60 同布局：`<root>/<section>/<name>/preset.json`。
fn write_preset(root: &Path, section: &str, name: &str, body: &Value) {
    let dir = root.join(section).join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("preset.json"),
        serde_json::to_vec_pretty(body).unwrap(),
    )
    .unwrap();
}

fn read_preset(root: &Path, section: &str, name: &str) -> Value {
    let bytes = fs::read(root.join(section).join(name).join("preset.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn ultimate(enabled: bool, trigger: &str, session: &str) -> Value {
    json!({
        "name": "nightly",
        "summary": "每晚",
        "description": "",
        "category": ["默认"],
        "spec": {
            "v": 1,
            "trigger": trigger,
            "params": { "at": "22:00" },
            "session": session,
            "enabled": enabled,
        },
    })
}

fn app_with(root: &Path) -> Router {
    let state = AppState::with_source(common::FakeSource::two_sessions())
        .with_combos_root(root.to_path_buf());
    router(state)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>, if_match: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    if let Some(etag) = if_match {
        builder = builder.header("if-match", etag);
    }
    let request = builder
        .body(Body::from(body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
    (status, body)
}

async fn get(app: &Router, section: &str) -> (StatusCode, Value) {
    call(app, "GET", &format!("/v1/combos/{section}"), None, None).await
}

fn find<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["combos"]
        .as_array()
        .unwrap_or_else(|| panic!("combos 不是数组：{body}"))
        .iter()
        .find(|item| item["name"] == json!(name))
        .unwrap_or_else(|| panic!("combos 里没有 {name}：{body}"))
}

/// 判据 1：GET 不是常量 —— 盘上新建 → 多一条；删掉 → 少一条。
#[tokio::test]
async fn roster_follows_the_disk_not_a_baked_in_array() {
    let dir = temp_dir("roster");
    let root = dir.path();
    let app = app_with(root);

    let (status, empty) = get(&app, "Ultimate-Template").await;
    assert_eq!(status, StatusCode::OK, "空目录也要 200：{empty}");
    assert_eq!(empty["combos"].as_array().unwrap().len(), 0);

    write_preset(root, "Ultimate-Template", "nightly", &ultimate(true, "daily", "morning"));
    let (_, one) = get(&app, "Ultimate-Template").await;
    assert_eq!(one["combos"].as_array().unwrap().len(), 1, "盘上多一个目录 → 清单多一条");
    let combo = find(&one, "nightly");
    assert_eq!(combo["name"], json!("nightly"));
    assert_eq!(combo["trigger"], json!("daily"));
    assert_eq!(combo["params"], json!({ "at": "22:00" }));
    assert_eq!(combo["session"], json!("morning"));
    assert!(combo["enabled"].as_bool().is_some(), "enabled 是 bool");
    assert!(combo["problem"].is_string(), "problem 是人话字符串");

    fs::remove_dir_all(root.join("Ultimate-Template").join("nightly")).unwrap();
    let (_, gone) = get(&app, "Ultimate-Template").await;
    assert_eq!(gone["combos"].as_array().unwrap().len(), 0, "删掉目录 → 清单少一条");
}

/// 判据 2：enabled 是算的 —— 引用失效 → false + problem 非空。
#[tokio::test]
async fn enabled_tracks_reference_validity_not_just_the_switch() {
    let dir = temp_dir("references");
    let root = dir.path();
    let triggers = dir.path().join("triggers");
    fs::create_dir_all(triggers.join("daily")).unwrap();
    write_preset(root, "Ultimate-Template", "nightly", &ultimate(true, "daily", "morning"));
    write_preset(root, "Session-Template", "morning", &json!({
        "name": "morning", "summary": "早报", "description": "", "category": ["默认"],
    }));
    let app = router(AppState::with_source(common::FakeSource::two_sessions())
        .with_combos_roots(root.to_path_buf(), triggers.clone()));

    let (_, before) = get(&app, "Ultimate-Template").await;
    let combo = find(&before, "nightly");
    assert_eq!(combo["enabled"], json!(true), "引用都在 → 开着");
    assert_eq!(combo["problem"], json!(""), "没毛病时 problem 空串");

    // 不同源地把 trigger 改名（盘上直接动目录），引擎必须重扫并算出失效。
    fs::rename(triggers.join("daily"), triggers.join("weekly")).unwrap();
    let (_, after) = get(&app, "Ultimate-Template").await;
    let combo = find(&after, "nightly");
    assert_eq!(combo["enabled"], json!(false), "引用的 trigger 不在了 → 关掉");
    assert!(
        combo["problem"].as_str().is_some_and(|s| s.contains("daily")),
        "problem 点名失效的引用：{combo}"
    );

    // 会话模板引用失效同样算关掉。
    fs::create_dir_all(triggers.join("daily")).unwrap();
    fs::remove_dir_all(root.join("Session-Template").join("morning")).unwrap();
    let (_, no_session) = get(&app, "Ultimate-Template").await;
    assert_eq!(find(&no_session, "nightly")["enabled"], json!(false));
    assert!(find(&no_session, "nightly")["problem"].as_str().is_some_and(|s| s.contains("morning")));

    // spec.enabled=false 是显式关：引用都在也不开。
    write_preset(root, "Ultimate-Template", "nightly", &ultimate(false, "daily", "morning"));
    let (_, off) = get(&app, "Ultimate-Template").await;
    assert_eq!(find(&off, "nightly")["enabled"], json!(false));
}

/// 判据 3+4：PATCH 只翻这一位（别的组合 / 另一段不动）；If-Match 412；requestId 幂等。
#[tokio::test]
async fn patch_is_scoped_versioned_and_idempotent() {
    let _env_guard = PATCH_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = temp_dir("patch");
    let root = dir.path();
    write_preset(root, "Ultimate-Template", "nightly", &ultimate(true, "daily", "morning"));
    write_preset(root, "Ultimate-Template", "weekly", &ultimate(true, "daily", "morning"));
    write_preset(root, "Session-Template", "morning", &json!({
        "name": "morning", "summary": "早报", "description": "", "category": ["默认"],
    }));
    write_preset(root, "Session-Template", "nightly", &json!({
        "name": "nightly", "summary": "晚报", "description": "", "category": ["默认"],
    }));
    let triggers = temp_dir("patch-triggers");
    fs::create_dir_all(triggers.path().join("daily")).unwrap();
    let app = router(AppState::with_source(common::FakeSource::two_sessions())
        .with_combos_roots(root.to_path_buf(), triggers.path().to_path_buf()));

    let (_, before) = get(&app, "Ultimate-Template").await;
    let etag = find(&before, "nightly")["version"].as_str().unwrap().to_string();
    let other_bytes = fs::read(root.join("Session-Template").join("nightly").join("preset.json")).unwrap();
    let (_, sessions_before) = get(&app, "Session-Template").await;
    let other_version = find(&sessions_before, "nightly")["version"].as_str().unwrap().to_string();

    let (status, receipt) = call(
        &app,
        "PATCH",
        "/v1/combos/Ultimate-Template/nightly",
        Some(json!({ "enabled": false, "requestId": "req-1" })),
        Some(&etag),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PATCH 200：{receipt}");
    assert_eq!(receipt["enabled"], json!(false));
    assert_eq!(receipt["requestId"], json!("req-1"));
    let after_disk = read_preset(root, "Ultimate-Template", "nightly");
    assert_eq!(after_disk["spec"]["enabled"], json!(false), "写穿 spec.enabled 那一位");
    assert_eq!(after_disk["spec"]["trigger"], json!("daily"), "别的字段不许动");
    assert_eq!(after_disk["name"], json!("nightly"));

    // 读回两次比对：另一条与另一段都不变。
    for _ in 0..2 {
        let (_, ultimate) = get(&app, "Ultimate-Template").await;
        assert_eq!(find(&ultimate, "nightly")["enabled"], json!(false));
        assert_eq!(find(&ultimate, "weekly")["enabled"], json!(true));
        let (_, sessions) = get(&app, "Session-Template").await;
        assert_eq!(find(&sessions, "nightly")["version"].as_str().unwrap(), other_version.as_str(), "另一段 version 没变");
        assert_eq!(fs::read(root.join("Session-Template").join("nightly").join("preset.json")).unwrap(), other_bytes, "另一段盘上没被动");
    }

    // 错 If-Match：412，盘上文件一个字节都不变。
    let bytes_before = fs::read(root.join("Ultimate-Template").join("nightly").join("preset.json")).unwrap();
    let (status, conflict) = call(
        &app,
        "PATCH",
        "/v1/combos/Ultimate-Template/nightly",
        Some(json!({ "enabled": true, "requestId": "req-2" })),
        Some("v999"),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED, "412：{conflict}");
    assert!(conflict["error"].is_string());
    let bytes_after = fs::read(root.join("Ultimate-Template").join("nightly").join("preset.json")).unwrap();
    assert_eq!(bytes_before, bytes_after, "412 后盘上文件不许变");

    // 同 requestId 重放：回第一次那份，version 不涨。
    let first_version = receipt["version"].as_str().unwrap().to_string();
    let (status, replay) = call(
        &app,
        "PATCH",
        "/v1/combos/Ultimate-Template/nightly",
        Some(json!({ "enabled": true, "requestId": "req-1" })),
        Some(&etag),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "重放仍 200：{replay}");
    assert_eq!(replay["version"], json!(first_version), "version 不涨（坑 99）");
    assert_eq!(replay["enabled"], json!(false), "回第一次的回执，不是这次的入参");
    assert_eq!(read_preset(root, "Ultimate-Template", "nightly")["spec"]["enabled"], json!(false), "重放不写盘");
}

/// PATCH 参数/名字的错误形状：`{error}`，状态码跟着语义走。
#[tokio::test]
async fn patch_rejects_bad_input_with_the_wire_error_shape() {
    let _env_guard = PATCH_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = temp_dir("bad-input");
    let root = dir.path();
    write_preset(root, "Ultimate-Template", "nightly", &ultimate(true, "daily", "morning"));
    let app = app_with(root);
    let (_, body) = get(&app, "Ultimate-Template").await;
    let etag = find(&body, "nightly")["version"].as_str().unwrap().to_string();

    let (status, missing) = call(
        &app, "PATCH", "/v1/combos/Ultimate-Template/nightly",
        Some(json!({ "enabled": false, "requestId": "r" })), None,
    ).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED, "缺 If-Match → 412：{missing}");
    assert!(missing["error"].is_string());

    let (status, no_id) = call(
        &app, "PATCH", "/v1/combos/Ultimate-Template/nightly",
        Some(json!({ "enabled": false })), Some(&etag),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "缺 requestId → 400：{no_id}");
    assert!(no_id["error"].is_string());

    let (status, wrong) = call(
        &app, "PATCH", "/v1/combos/Ultimate-Template/nope",
        Some(json!({ "enabled": false, "requestId": "r" })), Some(&etag),
    ).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不存在的组合 → 404：{wrong}");
    assert!(wrong["error"].is_string());

    let (status, wrong_section) = get(&app, "No-Such-Section").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "只认两个段：{wrong_section}");
    assert!(wrong_section["error"].is_string());
}

/// review 3：词法合法的目录名也可能是符号链接，realpath 必须拒逃逸，
/// 且盘上不能出现新文件（负向哨兵：关掉 realpath 检查这条红）。
#[tokio::test]
async fn symlink_escape_is_rejected_without_writing_outside_the_root() {
    use std::os::unix::fs::symlink;

    let dir = temp_dir("symlink-escape");
    let root = dir.path();
    let outside = dir.path().parent().unwrap().join(format!("{}-outside", dir.path().file_name().unwrap().to_string_lossy()));
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&outside).unwrap();
    let section = root.join("Ultimate-Template");
    fs::create_dir_all(&section).unwrap();
    symlink(&outside, section.join("nightly")).unwrap();

    let app = app_with(root);
    let (status, body) = call(
        &app,
        "PATCH",
        "/v1/combos/Ultimate-Template/nightly",
        Some(json!({ "enabled": false, "requestId": "escape-1" })),
        Some("\"whatever\""),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "符号链接逃逸必须 400：{body}");
    assert!(!outside.join("preset.json").exists(), "盘上不许出现逃逸文件");
    assert!(!root.join(".combos-receipts/Ultimate-Template/nightly/escape-1.json").exists(),
        "被拒的请求不许留回执");
    let _ = fs::remove_dir_all(&outside);
}

/// review 10（盘上半）：回执写在变更前；preset 写失败时回执不残留，
/// 重放不会拿到一份「变更没发生却说发生了」的回执。
#[tokio::test]
async fn a_failed_preset_write_does_not_leave_an_orphan_receipt() {
    let _env_guard = PATCH_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let dir = temp_dir("receipt-fail");
    let root = dir.path();
    write_preset(root, "Ultimate-Template", "nightly", &ultimate(true, "daily", "morning"));
    let app = app_with(root);
    let (_, body) = get(&app, "Ultimate-Template").await;
    let etag = find(&body, "nightly")["version"].as_str().unwrap().to_string();
    let before = read_preset(root, "Ultimate-Template", "nightly");

    // 注入：在写 preset 前失败（模拟盘写错误），回执若已先写就必须回滚删掉。
    std::env::set_var("NOUS_TEST_COMBOS_WRITE_FAIL", "1");
    let (status, body) = call(
        &app, "PATCH", "/v1/combos/Ultimate-Template/nightly",
        Some(json!({ "enabled": false, "requestId": "fail-1" })), Some(&etag),
    ).await;
    std::env::remove_var("NOUS_TEST_COMBOS_WRITE_FAIL");
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "写失败要明确报错：{body}");
    assert_eq!(read_preset(root, "Ultimate-Template", "nightly"), before, "失败不许改盘");
    assert!(!root.join(".combos-receipts/Ultimate-Template/nightly/fail-1.json").exists(),
        "失败不许留下孤儿回执");

    // 重放：不能返回失败那次的回执，应重新执行成功。
    let (status, replay) = call(
        &app, "PATCH", "/v1/combos/Ultimate-Template/nightly",
        Some(json!({ "enabled": false, "requestId": "fail-1" })), Some(&etag),
    ).await;
    assert_eq!(status, StatusCode::OK, "重放应该重新执行一次：{replay}");
    assert_eq!(read_preset(root, "Ultimate-Template", "nightly")["spec"]["enabled"], json!(false));
}
