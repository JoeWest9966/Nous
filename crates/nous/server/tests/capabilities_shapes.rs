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
            ("version", is_string),
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

/// GET /v1/capabilities —— 拷问结果 §1–§7：contract/server/11 模块/reason 对象/runtime。
#[tokio::test]
async fn capabilities_declares_modules_and_reasons() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, body) = get_json(&app, "/v1/capabilities").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/capabilities 应 200");
    fields(
        &body,
        "capabilities",
        &[
            ("contract", |v| v.is_i64()),
            ("server", |v| v.is_object()),
            ("modules", is_array),
            ("runtime", |v| v.is_object()),
        ],
    );
    assert_eq!(body["contract"], json!(1));
    fields(
        &body["server"],
        "server",
        &[("name", is_string), ("version", is_string)],
    );
    assert_eq!(body["server"]["name"], json!("nous"));
    fields(
        &body["runtime"]["provider"],
        "runtime.provider",
        &[("configured", is_bool), ("message", is_string)],
    );

    let modules = body["modules"].as_array().unwrap();
    let by_name: std::collections::BTreeMap<&str, &Value> = modules
        .iter()
        .map(|m| (m["name"].as_str().expect("name 是字符串"), m))
        .collect();
    // 名字 == docs/plugins.md 的 module 列（评论拍板：ext，不是 extensions）。
    for name in [
        "sessions",
        "documents",
        "presets",
        "archive",
        "combos",
        "triggers",
        "extensions",
        "skills",
        "prompts",
        "selection",
        "device",
        "ext",
    ] {
        if name == "extensions" {
            continue; // 旧名不许出现；下面的断言单独钉死
        }
        let module = by_name
            .get(name)
            .unwrap_or_else(|| panic!("capabilities 缺模块 {name}"));
        fields(module, "module", &[("name", is_string), ("enabled", is_bool)]);
        if module["enabled"].as_bool() == Some(true) {
            let endpoints = module["endpoints"].as_array().unwrap_or_else(|| {
                panic!("enabled:true 的 {name} 必须有 endpoints 数组")
            });
            assert!(
                endpoints.iter().all(|e| e.is_string()),
                "{name}.endpoints 元素是字符串"
            );
            assert!(
                module.get("reason").is_none(),
                "enabled:true 不该带 reason"
            );
        } else {
            let reason = module["reason"]
                .as_object()
                .unwrap_or_else(|| panic!("enabled:false 的 {name} 必须有 reason 对象"));
            let code = reason["code"].as_str().unwrap();
            assert!(
                matches!(
                    code,
                    "not_implemented"
                        | "disabled_by_manifest"
                        | "frozen"
                        | "needs_credentials"
                ),
                "非法 reason.code: {code}"
            );
            assert!(reason["message"].is_string(), "reason.message 是人话");
            assert!(
                module.get("endpoints").is_none(),
                "enabled:false 不该声明 endpoints"
            );
        }
    }
    assert!(by_name.get("extensions").is_none(), "模块名是 ext，不是 extensions");
    assert_eq!(by_name["sessions"]["enabled"], json!(true));
    assert_eq!(by_name["ext"]["enabled"], json!(false));
    assert_eq!(by_name["ext"]["reason"]["code"], json!("frozen"));
    snapshot("capabilities_declares_modules_and_reasons", &body);
}

/// 拷问结果 §8：_routes ↔ enabled 模块 endpoints 多重集相等（双向）。
/// 两侧都来自 `meta` 的同一注册表；这里从 HTTP 响应分别取值，不让手抄数组蒙混。
#[tokio::test]
async fn route_registry_and_enabled_endpoints_are_the_same_multiset() {
    use std::collections::BTreeMap;
    fn tally(items: Vec<String>) -> BTreeMap<String, usize> {
        let mut map = BTreeMap::new();
        for item in items {
            *map.entry(item).or_insert(0) += 1;
        }
        map
    }

    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let (status, capabilities) = get_json(&app, "/v1/capabilities").await;
    assert_eq!(status, StatusCode::OK);
    let declared: Vec<String> = capabilities["modules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|module| module["enabled"].as_bool() == Some(true))
        .flat_map(|module| {
            module["endpoints"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
        })
        .collect();
    let registry: Vec<String> = nous_server::meta::routes_view()
        .as_array()
        .unwrap()
        .iter()
        .map(|route| {
            format!(
                "{} {}",
                route["method"].as_str().unwrap(),
                route["path"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        tally(declared),
        tally(registry),
        "_routes 与 endpoints 必须是同一个多重集"
    );
    // 负向哨兵：注册表里每条声明都得真能在 Router 上打到（不是 404）。
    for route in nous_server::meta::ROUTES {
        if !nous_server::meta::route_enabled(route.module) {
            continue;
        }
        let axum_path = route.path.replace(":id", "{id}").replace("/v1/read/{id}", "/v1/read/2");
        let axum_path = axum_path.replace("/v1/sessions/{id}", "/v1/sessions/2");
        let uri = format!("/{}", axum_path.trim_start_matches('/'));
        let request = Request::builder()
            .method(route.method)
            .uri(&uri)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert!(
            response.status() != StatusCode::NOT_FOUND,
            "注册表声明的 {} {uri} 在 router 里不存在",
            route.method
        );
    }
}
