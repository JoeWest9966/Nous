//! #47：能连上的端点 + 能力声明的 wire 形状快照。
//!
//! 权威：`wire.md`（端点与约束）+ `NousDesk/crates/nous-engine/src/types.rs`（客户端真实期望）。
//! 断言只钉「字段在、类型对」，不钉「恰好这些字段」：`ReadReceipt` 等类型带 `#[serde(flatten)]`，
//! 多回字段是合法的。快照文件让 camelCase 漂移一眼可见。

mod common;

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn snapshot(test: &str, value: &Value) {
    let path = snapshots_dir().join(format!("{test}.json"));
    let actual = serde_json::to_string_pretty(value).unwrap() + "\n";
    match std::fs::read_to_string(&path) {
        Ok(expected) if expected == actual => {}
        Ok(expected) => panic!(
            "字段形状快照漂移：{}\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
            path.display()
        ),
        Err(_) => panic!(
            "快照不存在：{}（先按下面的 actual 建立文件）\n--- actual ---\n{actual}",
            path.display()
        ),
    }
}

async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// 对象里必须有这些键，且每个键的值都过 `check`。
fn fields(value: &Value, where_: &str, expected: &[(&str, fn(&Value) -> bool)]) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{where_} 必须是对象：{value}"));
    for (key, check) in expected {
        let actual = object
            .get(*key)
            .unwrap_or_else(|| panic!("{where_} 缺字段 {key}：{value}"));
        assert!(check(actual), "{where_}.{key} 类型不对：{actual}");
    }
}

fn is_string(v: &Value) -> bool {
    v.is_string()
}
fn is_string_or_null(v: &Value) -> bool {
    v.is_string() || v.is_null()
}
fn is_array(v: &Value) -> bool {
    v.is_array()
}
fn is_bool(v: &Value) -> bool {
    v.as_bool().is_some()
}

/// GET /v1/device —— `types.rs::Device`。`available` 一定是字符串数组，其余可空。
#[tokio::test]
async fn device_response_pins_client_shape() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/device").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/device 应 200");
    fields(
        &body,
        "device",
        &[
            ("self", is_string_or_null),
            ("origin", is_string_or_null),
            ("node", is_string_or_null),
            ("startedAt", |v| v.is_string() || v.is_number() || v.is_null()),
            ("model", |_| true), // types.rs: Option<Value> —— 空目录就是 null
            ("faux", is_bool),
            ("data", is_string_or_null),
            ("catalog", |v| v.is_object() || v.is_null()),
            ("available", is_array),
        ],
    );
    assert!(
        body["available"].as_array().unwrap().iter().all(Value::is_string),
        "available 是 string[]"
    );
    // 启动时刻每次进程不同；单独断言类型后归一，再快照其余形状。
    assert!(body["startedAt"].is_number());
    let mut stable = body.clone();
    stable["startedAt"] = json!(0);
    snapshot("device_response_pins_client_shape", &stable);
}

/// GET /v1/selection —— 客户端当 `Value` 收，`{id}` / `{id:null}`。
#[tokio::test]
async fn selection_response_pins_id_field() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/selection").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/selection 应 200");
    fields(&body, "selection", &[("id", |v| v.is_null() || v.is_number() || v.is_string())]);
    snapshot("selection_response_pins_id_field", &body);
}

/// GET /v1/skills —— `types.rs::Skills` / `Skill`。
#[tokio::test]
async fn skills_response_pins_client_shape() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/skills").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/skills 应 200");
    fields(
        &body,
        "skills",
        &[
            ("root", is_string_or_null),
            ("categories", is_array),
            ("skills", is_array),
        ],
    );
    assert!(body["categories"].as_array().unwrap().iter().all(Value::is_string));
    snapshot("skills_response_pins_client_shape", &body);
}

/// GET /v1/prompts —— `types.rs::Prompts` / `Prompt`。
#[tokio::test]
async fn prompts_response_pins_client_shape() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/prompts").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/prompts 应 200");
    fields(
        &body,
        "prompts",
        &[
            ("default", is_string_or_null),
            ("categories", is_array),
            ("prompts", is_array),
            ("root", is_string_or_null),
        ],
    );
    snapshot("prompts_response_pins_client_shape", &body);
}

/// GET /v1/extensions —— ext 冻结，返回空清单。
#[tokio::test]
async fn extensions_response_pins_empty_items() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/extensions").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/extensions 应 200");
    fields(&body, "extensions", &[("items", is_array)]);
    assert!(body["items"].as_array().unwrap().is_empty(), "ext 冻结，items 空");
    snapshot("extensions_response_pins_empty_items", &body);
}

/// GET /v1/capabilities —— desk 按 modules 显示/隐藏功能面。
#[tokio::test]
async fn capabilities_declares_modules_and_reasons() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/capabilities").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/capabilities 应 200");
    fields(&body, "capabilities", &[("modules", is_array)]);
    let modules = body["modules"].as_array().unwrap();
    let by_name: std::collections::BTreeMap<&str, &Value> = modules
        .iter()
        .map(|m| (m["name"].as_str().expect("name 是字符串"), m))
        .collect();
    for name in [
        "sessions",
        "presets",
        "archive",
        "combos",
        "triggers",
        "fs",
        "extensions",
    ] {
        let module = by_name
            .get(name)
            .unwrap_or_else(|| panic!("capabilities 缺模块 {name}"));
        fields(module, "module", &[("name", is_string), ("enabled", is_bool)]);
    }
    assert_eq!(by_name["sessions"]["enabled"], json!(true));
    assert_eq!(by_name["extensions"]["enabled"], json!(true));
    for name in ["presets", "archive", "combos", "triggers", "fs"] {
        assert_eq!(by_name[name]["enabled"], json!(false), "{name} 该关掉");
        assert!(
            by_name[name]["reason"].is_string(),
            "{name} 关掉必须给一句 reason"
        );
    }
    for name in ["sessions", "extensions"] {
        let endpoints = by_name[name]["endpoints"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}.endpoints 该是数组"));
        assert!(endpoints.iter().all(Value::is_string));
    }
    snapshot("capabilities_declares_modules_and_reasons", &body);
}
