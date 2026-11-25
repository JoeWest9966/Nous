//! 端点级判据：`PUT /v1/providers/{id}/secret`。
//!
//! 这一条端点是「插件自带 handler + 宿主注入能力」的第二个样板：插件声明路由、自己写
//! handler，只要宿主给它两样东西（元素库仓库、密钥文件路径）。所以这里用**假宿主**
//! 起真 Router，从 HTTP 边界验到底 —— 不走 `server`，也不需要 `AppState`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use element_models::routes::{self, Host, Secrets};
use element_models::GROUP_ID;
use serde_json::{json, Value};
use tower::ServiceExt;

/// 假宿主：一个临时元素库 + 一个临时密钥文件。
#[derive(Clone)]
struct FakeHost {
    repo: Arc<nous_repo::Repo>,
    secrets: Option<PathBuf>,
}

impl Host for FakeHost {
    fn elements_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        Some(self.repo.clone())
    }
}

impl Secrets for FakeHost {
    fn secrets_file(&self) -> Option<PathBuf> {
        self.secrets.clone()
    }
}

/// 造一段元素库：一个组 + 一个供应商包。`api_key` 决定「这条密钥能不能由 Desk 改」。
fn elements_root(api_key: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("groups")).unwrap();
    std::fs::create_dir_all(root.join("packages")).unwrap();
    std::fs::write(
        root.join(format!("groups/{GROUP_ID}.json")),
        json!({
            "id": GROUP_ID,
            "name": "Element-Models",
            "packages": ["provider-wb2api"],
            "default": "wb2api/cn:fast",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("packages/provider-wb2api.json"),
        json!({
            "id": "provider-wb2api",
            "name": "Element",
            "kind": "provider",
            "provider": "wb2api",
            "baseUrl": "http://127.0.0.1:1/v1",
            "api": "openai-completions",
            "apiKey": api_key,
            "models": [{ "id": "cn:fast", "name": "Fast" }],
        })
        .to_string(),
    )
    .unwrap();
    tmp
}

fn app(elements: &Path, secrets: Option<PathBuf>) -> Router {
    let host = FakeHost { repo: Arc::new(nous_repo::Repo::open(elements).unwrap()), secrets };
    let mut router = Router::new();
    for route in routes::routes::<FakeHost>() {
        router = route.mount(router);
    }
    router.with_state(host)
}

async fn call(app: Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[test]
fn both_endpoints_are_declared_under_the_plugin_module() {
    let declared: Vec<(String, String, &str)> = routes::routes::<FakeHost>()
        .iter()
        .map(|route| {
            (
                route.method().to_string(),
                route.registry_path(),
                route.module(),
            )
        })
        .collect();
    assert_eq!(
        declared,
        vec![
            ("GET".to_string(), "/v1/models/elements".to_string(), "Element_Models"),
            (
                "PUT".to_string(),
                "/v1/providers/:id/secret".to_string(),
                "Element_Models"
            ),
        ],
        "注册表写法必须是 wire.md 的 :id 形式"
    );
}

#[tokio::test]
async fn secret_lands_in_the_host_file_and_never_comes_back_in_the_response() {
    let elements = elements_root("$WB2API_API_KEY");
    let secrets_dir = tempfile::tempdir().unwrap();
    let secrets = secrets_dir.path().join("providers.env");
    let package_before = std::fs::read(elements.path().join("packages/provider-wb2api.json")).unwrap();

    let (status, body) = call(
        app(elements.path(), Some(secrets.clone())),
        "PUT",
        "/v1/providers/wb2api/secret",
        json!({ "value": "sk-real-value" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["provider"], json!("wb2api"));
    assert_eq!(body["package"], json!("provider-wb2api"));
    assert_eq!(body["name"], json!("WB2API_API_KEY"));
    assert_eq!(body["replaced"], json!(false));
    assert_eq!(body["restartRequired"], json!(true));
    assert_eq!(body["path"], json!(secrets.to_string_lossy()));
    assert!(
        !body.to_string().contains("sk-real-value"),
        "回执不许回显密钥：{body}"
    );

    let written = std::fs::read_to_string(&secrets).unwrap();
    assert!(written.contains("WB2API_API_KEY=sk-real-value"), "{written}");

    let package_after = std::fs::read(elements.path().join("packages/provider-wb2api.json")).unwrap();
    assert_eq!(package_before, package_after, "元素库一个字节都不许改");
}

#[tokio::test]
async fn a_cmd_reference_is_refused_with_409_and_writes_nothing() {
    let elements = elements_root("!pass show wb2api");
    let secrets_dir = tempfile::tempdir().unwrap();
    let secrets = secrets_dir.path().join("providers.env");

    let (status, body) = call(
        app(elements.path(), Some(secrets.clone())),
        "PUT",
        "/v1/providers/wb2api/secret",
        json!({ "value": "sk-real" }),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("改不了"), "{body}");
    assert!(!secrets.exists(), "被拒绝的写入不许落盘");
}

#[tokio::test]
async fn an_unknown_provider_is_404() {
    let elements = elements_root("$WB2API_API_KEY");
    let secrets_dir = tempfile::tempdir().unwrap();

    let (status, body) = call(
        app(elements.path(), Some(secrets_dir.path().join("providers.env"))),
        "PUT",
        "/v1/providers/ghost/secret",
        json!({ "value": "sk" }),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"].as_str().unwrap().contains("ghost"), "{body}");
}

#[tokio::test]
async fn an_empty_value_is_422_with_a_field_hint() {
    let elements = elements_root("$WB2API_API_KEY");
    let secrets_dir = tempfile::tempdir().unwrap();

    let (status, body) = call(
        app(elements.path(), Some(secrets_dir.path().join("providers.env"))),
        "PUT",
        "/v1/providers/wb2api/secret",
        json!({ "value": "" }),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body["error"].as_str().unwrap().contains("空"), "{body}");
}

#[tokio::test]
async fn a_host_without_a_secrets_file_says_so_instead_of_pretending() {
    let elements = elements_root("$WB2API_API_KEY");

    let (status, body) = call(
        app(elements.path(), None),
        "PUT",
        "/v1/providers/wb2api/secret",
        json!({ "value": "sk" }),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body["error"].as_str().unwrap().contains("NOUS_SECRETS_FILE"), "{body}");
}

#[tokio::test]
async fn the_catalog_endpoint_still_answers_through_the_same_host() {
    let elements = elements_root("$WB2API_API_KEY");
    let (status, body) = call(
        app(elements.path(), None),
        "GET",
        "/v1/models/elements",
        Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], json!("ok"));
    assert_eq!(body["providers"][0]["key"], json!("env"));
    assert!(!body.to_string().contains("WB2API_API_KEY"), "编目不许回显引用名以外的值");
}
