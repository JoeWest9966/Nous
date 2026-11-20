//! `/v1/models` 与 `POST /v1/sessions` 的字段形状快照。
//!
//! 接缝：真 Router（axum `ServiceExt::oneshot`），请求/响应都是真 JSON。
//! `wire.md` 只索引端点与约束，字段形状以本文件两条测试为准：
//!
//! - [`models_response_pins_default_and_item_field_shape`] —— 默认模型 + `items[]` 形状；
//! - [`create_session_request_and_response_are_snapshotted`] —— 建会话请求/响应形状
//!   （含 `template: null` 与 `extensions` 语义）。
//!
//! 快照故意放在 crate 自己的 `snapshots/` 下、不引第三方快照库：文件就是断言，
//! 字段名一旦漂移（典型：camelCase 退化成 snake_case）diff 立刻可见，不需要理解宏。

mod common;

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_device::{ModelItem, ModelRef, ModelsResponse};
use nous_server::{router, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

/// 快照目录：跟着源码走，不依赖 cwd。
fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

/// 快照文件名 = 测试名 + `.json`；`pretty` 让 diff 按字段逐行可读。
fn snapshot(test: &str, value: &Value) {
    let path = snapshots_dir().join(format!("{test}.json"));
    let actual = serde_json::to_string_pretty(value).unwrap() + "\n";
    match std::fs::read_to_string(&path) {
        Ok(expected) if expected == actual => {}
        Ok(expected) => panic!(
            "字段形状快照漂移：{}\n（接受改动请核对 wire.md 语义后更新 .json 文件）\n--- expected ---\n{expected}\n--- actual ---\n{actual}",
            path.display()
        ),
        Err(_) => panic!(
            "快照不存在：{}（先按下面的 actual 建立文件）\n--- actual ---\n{actual}",
            path.display()
        ),
    }
}

/// 真 Router 的 GET：不起监听端口，直接拿响应 JSON。
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
    let body = serde_json::from_slice(&bytes).unwrap();
    (status, body)
}

/// 真 Router 的 POST：把请求 body 原样发进去，连状态码一起返回。
async fn post_json(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

/// 字段名按 `[expected...]` 钉住：多一个少一个都会红。
fn assert_fields(value: &Value, where_: &str, expected: &[&str]) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{where_} 必须是对象"));
    let mut actual: Vec<&str> = object.keys().map(String::as_str).collect();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected, "{where_} 的字段集合漂移");
}

/// 这条 body 同时把「camelCase 键」和 `template: null` / `extensions` 语义送过真 Router：
/// 服务端若按 snake_case 收，回包里的生效值会退回去，快照立刻漂移。
fn create_request() -> Value {
    json!({
        "name": "wire-shape",
        "model": { "provider": "snapshot", "modelId": "snapshot/model-1" },
        "thinkingLevel": "medium",
        "skills": ["code-review"],
        "template": null,
        "extensions": ["code-review", "nous"],
        "cwd": "/tmp",
        "machine": "local"
    })
}

#[tokio::test]
async fn models_response_pins_default_and_item_field_shape() {
    // 真 handler 的数据源：两个模型、不同 thinking levels，验证「按模型夹档」的形状。
    let catalog = ModelsResponse {
        default: Some(ModelRef {
            provider: "snapshot".into(),
            model_id: "snapshot/model-1".into(),
        }),
        items: vec![
            ModelItem {
                provider: "snapshot".into(),
                model_id: "snapshot/model-1".into(),
                name: "Snapshot One".into(),
                thinking_levels: vec!["off".into(), "medium".into(), "high".into()],
                context_window: Some(200_000),
                max_tokens: Some(16_384),
            },
            ModelItem {
                provider: "snapshot".into(),
                model_id: "snapshot/model-2".into(),
                name: "Snapshot Two".into(),
                thinking_levels: vec!["off".into()],
                context_window: None,
                max_tokens: None,
            },
        ],
        catalog: nous_device::CatalogDiagnostics {
            state: "ok",
            source: None,
            path: std::path::PathBuf::from("/test/models.json"),
        },
    };
    let app =
        router(AppState::with_source(common::FakeSource::two_sessions()).with_models(catalog));
    let (status, body) = get_json(&app, "/v1/models").await;
    assert_eq!(status, StatusCode::OK, "GET /v1/models 应 200");

    // 至少钉住「默认模型」和「列表 + 每项字段集合存在」。
    assert!(!body["default"].is_null(), "必须给默认模型");
    assert_fields(&body["default"], "default", &["modelId", "provider"]);
    let items = body["items"].as_array().expect("items 是数组");
    assert!(!items.is_empty(), "列表不能是空的，否则字段集合没背书");
    for item in items {
        assert_fields(
            item,
            "items[]",
            &[
                "contextWindow",
                "maxTokens",
                "modelId",
                "name",
                "provider",
                "thinkingLevels",
            ],
        );
        assert!(item["thinkingLevels"].is_array());
    }
    // camelCase 键出现就是契约退化成 snake_case。
    let raw = serde_json::to_string(&body).unwrap();
    assert!(!raw.contains("model_id"), "camelCase 退化成 snake_case");
    assert!(
        !raw.contains("context_window"),
        "camelCase 退化成 snake_case"
    );
    assert!(!raw.contains("max_tokens"), "camelCase 退化成 snake_case");
    assert!(
        !raw.contains("thinking_levels"),
        "camelCase 退化成 snake_case"
    );

    snapshot("models_response_pins_default_and_item_field_shape", &body);
}

#[tokio::test]
async fn create_session_request_and_response_are_snapshotted() {
    let request = create_request();
    // 请求字段集合先钉死：这八键就是 POST /v1/sessions 的 body 形状权威。
    assert_fields(
        &request,
        "请求 body",
        &[
            "cwd",
            "extensions",
            "machine",
            "model",
            "name",
            "skills",
            "template",
            "thinkingLevel",
        ],
    );
    assert!(request["template"].is_null(), "请求里 template 显式 null");
    assert_eq!(
        request["extensions"],
        serde_json::json!(["code-review", "nous"]),
        "请求里 extensions 是额外要加的清单"
    );
    snapshot(
        "create_session_request_and_response_are_snapshotted.request",
        &request,
    );

    // 建会话打真 durable 库（临时 sqlite），201 体是真写出来的，不是 mock 回显。
    let path = std::env::temp_dir().join(format!(
        "nous-wire-shape-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    let app = router(AppState::durable(&path).unwrap());
    let (status, body) = post_json(&app, "/v1/sessions", request.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "建会话应 201：{body}");

    // 201 响应字段集合整份钉住：少了任何一键，签名的契约就漂了。
    assert_fields(
        &body,
        "201 响应",
        &[
            "cwd",
            "extensions",
            "id",
            "machine",
            "missing",
            "missingExtensions",
            "missingTemplates",
            "model",
            "name",
            "readOnly",
            "skills",
            "template",
            "thinkingLevel",
        ],
    );
    assert!(
        body["template"].is_null(),
        "template 显式 null = 明确不要，不该被默认模板顶回来"
    );
    assert_eq!(
        body["extensions"], request["extensions"],
        "extensions 生效值由快照背书；只增不减的注册表语义由 wire.md 约束段承载"
    );
    let raw = serde_json::to_string(&body).unwrap();
    assert!(
        !raw.contains("thinking_level"),
        "camelCase 退化成 snake_case"
    );

    snapshot(
        "create_session_request_and_response_are_snapshotted.response",
        &body,
    );
}
