//! `GET /v1/modules/updates`：真 Router 上打，源是本地假服务（**不打真网**）。
//!
//! 验四件事：端点真装进 Router、回包形状、缓存/穿透/304 的行为、上游挂了仍然是 200 且
//! `available:false`（不是「全都最新」，也不是 5xx）。
//!
//! 本文件里所有用例共用进程内的缓存（`updates::reset_cache`），所以用一个互斥锁串起来跑。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::Value;
use tower::ServiceExt;

const MANIFEST: &str = r#"{
  "release": "9.9.9",
  "modules": [
    { "name": "AboutME_ModuleRelationshipNetwork", "crate": "aboutme-module-relationship-network", "version": "9.9.9" }
  ]
}"#;

/// 假源的开关：0 = 正常回清单（带 ETag），1 = 回 500，2 = 带 `If-None-Match` 就回 304。
#[derive(Default)]
struct Fake {
    hits: AtomicUsize,
    mode: AtomicUsize,
    seen_etag: Mutex<Option<String>>,
}

impl Fake {
    fn set_mode(&self, mode: usize) {
        self.mode.store(mode, Ordering::SeqCst);
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

async fn fake_manifest(State(fake): State<Arc<Fake>>, headers: axum::http::HeaderMap) -> Response {
    fake.hits.fetch_add(1, Ordering::SeqCst);
    let seen = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    *fake.seen_etag.lock().unwrap() = seen.clone();
    match fake.mode.load(Ordering::SeqCst) {
        1 => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        2 if seen.as_deref() == Some("\"manifest-v1\"") => StatusCode::NOT_MODIFIED.into_response(),
        _ => (
            [(
                axum::http::header::ETAG,
                axum::http::HeaderValue::from_static("\"manifest-v1\""),
            )],
            MANIFEST,
        )
            .into_response(),
    }
}

/// 起一个只听回环的假源，返回它的 URL。
async fn fake_source(mode: usize) -> (String, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    fake.set_mode(mode);
    let app = Router::new()
        .route("/plugin-versions.json", get(fake_manifest))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/plugin-versions.json"), fake)
}

fn config(source: String) -> aboutme_module_relationship_network::updates::Config {
    aboutme_module_relationship_network::updates::Config {
        source,
        token: None,
        cache_path: None,
        ttl: std::time::Duration::from_secs(3600),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap()
}

fn app() -> Router {
    let mut router = Router::new();
    for route in aboutme_module_relationship_network::routes::routes::<()>() {
        router = route.mount(router);
    }
    router
}

async fn get_json(router: Router, uri: &str) -> (StatusCode, Value) {
    let response = router
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 512 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).expect("回包是 JSON"))
}

/// 进程内缓存是共享的，用例得串行跑。用 async-aware 的锁：guard 会跨 `await` 持着。
async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let guard = LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    aboutme_module_relationship_network::updates::reset_cache();
    guard
}

#[tokio::test]
async fn endpoint_is_declared_under_this_module() {
    let declared: Vec<(String, String, &'static str)> =
        aboutme_module_relationship_network::routes::routes::<()>()
            .iter()
            .map(|route| (route.method().to_string(), route.declared(), route.module()))
            .collect();
    assert!(
        declared.contains(&(
            "GET".to_string(),
            "GET /v1/modules/updates".to_string(),
            "AboutME_ModuleRelationshipNetwork",
        )),
        "端点没声明在注册表里：{declared:?}"
    );
}

/// 真 Router + 真 `Config::from_env`：形状对，且 `refresh=1` 能穿透缓存。
#[tokio::test]
async fn router_fetches_once_then_serves_cache() {
    let _guard = serial().await;
    let (url, fake) = fake_source(0).await;
    std::env::set_var("NOUS_UPDATES_SOURCE", &url);
    std::env::remove_var("NOUS_DATA");
    std::env::remove_var("NOUS_UPDATES_TOKEN");

    let (status, body) = get_json(app(), "/v1/modules/updates").await;
    std::env::remove_var("NOUS_UPDATES_SOURCE");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["available"], serde_json::json!(true));
    assert_eq!(body["stale"], serde_json::json!(false));
    assert_eq!(body["error"], Value::Null);
    assert!(body["checkedAt"].is_number(), "checkedAt 是 epoch 毫秒");
    assert_eq!(body["source"], serde_json::json!(url));
    assert_eq!(body["server"]["latest"], serde_json::json!("9.9.9"));
    assert_eq!(fake.hits(), 1);

    let ours = body["modules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "AboutME_ModuleRelationshipNetwork")
        .expect("自己在 modules 里");
    let declared = aboutme_module_relationship_network::PLUGIN_VERSIONS
        .iter()
        .find(|(name, _)| *name == aboutme_module_relationship_network::MODULE)
        .map(|(_, version)| *version)
        .expect("自己在版本表里");
    assert_eq!(ours["current"], serde_json::json!(declared));
    assert_eq!(ours["currentSource"], serde_json::json!("module"));
    assert_eq!(ours["latest"], serde_json::json!("9.9.9"));

    // 第二次（不带 refresh）：缓存命中，不再打上游。
    let cfg = config(url.clone());
    let again =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), false).await;
    assert_eq!(again["available"], serde_json::json!(true));
    assert_eq!(fake.hits(), 1, "TTL 内不该重复请求");

    // `refresh=1`：穿透缓存，真的再打一次。
    let forced =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), true).await;
    assert_eq!(forced["available"], serde_json::json!(true));
    assert_eq!(fake.hits(), 2, "refresh 必须穿透");
}

/// 上游回 304：复用缓存、`checkedAt` 前进、模块数据不变；把 `If-None-Match` 发出去了。
#[tokio::test]
async fn not_modified_reuses_cache_and_sends_etag() {
    let _guard = serial().await;
    let (url, fake) = fake_source(0).await;
    let cfg = config(url);
    let first =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), true).await;
    assert_eq!(fake.hits(), 1);
    let first_checked = first["checkedAt"].as_i64().unwrap();

    fake.set_mode(2);
    let second =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), true).await;
    assert_eq!(fake.hits(), 2);
    assert_eq!(
        *fake.seen_etag.lock().unwrap(),
        Some("\"manifest-v1\"".to_string())
    );
    assert_eq!(
        second["available"],
        serde_json::json!(true),
        "304 不是「没数据」"
    );
    assert_eq!(second["stale"], serde_json::json!(false));
    assert!(
        second["checkedAt"].as_i64().unwrap() >= first_checked,
        "304 也算一次成功校验，时间要往前走"
    );
    assert_eq!(
        second["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "AboutME_ModuleRelationshipNetwork")
            .unwrap()["latest"],
        serde_json::json!("9.9.9"),
        "304 之后数据还在"
    );
}

/// 上游 500：**200** + `available:false` + `error`，不是 5xx、也不是「全都最新」。
#[tokio::test]
async fn upstream_error_is_200_and_unavailable() {
    let _guard = serial().await;
    let (url, fake) = fake_source(1).await;
    let cfg = config(url);
    let body =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), true).await;
    assert_eq!(body["available"], serde_json::json!(false));
    assert_eq!(body["stale"], serde_json::json!(false));
    assert_eq!(body["checkedAt"], Value::Null);
    assert!(
        body["error"].as_str().unwrap().contains("500"),
        "错误文本要点出上游状态：{}",
        body["error"]
    );
    for item in body["modules"].as_array().unwrap() {
        assert_eq!(item["latest"], Value::Null, "没数据就没有 latest");
    }
    assert_eq!(fake.hits(), 1);
}

/// 有旧数据、这次上游挂了：`stale:true` 继续给（旧总比没有强，但必须标出来）。
#[tokio::test]
async fn stale_cache_survives_upstream_failure() {
    let _guard = serial().await;
    let (url, fake) = fake_source(0).await;
    let cfg = config(url);
    let fresh =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), true).await;
    assert_eq!(fresh["stale"], serde_json::json!(false));

    fake.set_mode(1);
    let body =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), true).await;
    assert_eq!(body["available"], serde_json::json!(true));
    assert_eq!(body["stale"], serde_json::json!(true));
    assert!(body["error"].is_string());
    assert_eq!(
        body["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "AboutME_ModuleRelationshipNetwork")
            .unwrap()["latest"],
        serde_json::json!("9.9.9"),
        "旧数据还在"
    );
}

/// 源根本连不上：`available:false` + 错误，不 panic、不挂住（3s 超时内返回）。
#[tokio::test]
async fn unreachable_source_is_reported_not_panicked() {
    let _guard = serial().await;
    // 本机一个没人听的端口：连接立即被拒。
    let cfg = config("http://127.0.0.1:9/plugin-versions.json".to_string());
    let body =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), true).await;
    assert_eq!(body["available"], serde_json::json!(false));
    assert!(body["error"].is_string(), "要说人话");
}

/// 清单不是预期形状（比如上游给了 HTML 错误页）：也算失败，不 panic。
#[tokio::test]
async fn malformed_manifest_is_an_error() {
    let _guard = serial().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let app = Router::new().route(
            "/plugin-versions.json",
            get(|| async { "<html>not json</html>" }),
        );
        let _ = axum::serve(listener, app).await;
    });
    let cfg = config(format!("http://{addr}/plugin-versions.json"));
    let body =
        aboutme_module_relationship_network::updates::handle_with(&cfg, &client(), true).await;
    assert_eq!(body["available"], serde_json::json!(false));
    assert!(
        body["error"].as_str().unwrap().contains("清单"),
        "{}",
        body["error"]
    );
}
/// 仓库根那份 `plugin-versions.json`（引擎的上游清单）与**编译进产物的**版本表逐条相等。
///
/// 这份断言把两层漂移钉住：清单生成物忘了重写（`verify.sh` 0e 抓），以及 build.rs 抄错字段
/// （这里抓）。两边都是机械转录，对不上就是有一边错了。
#[test]
fn repo_manifest_matches_the_compiled_table() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../plugin-versions.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "读 {} 失败：{error}（跑 scripts/plugin-versions.py --write-json）",
            path.display()
        )
    });
    let manifest: aboutme_module_relationship_network::updates::Manifest =
        serde_json::from_str(&text).expect("plugin-versions.json 是清单形状");

    let compiled: std::collections::BTreeMap<&str, &str> =
        aboutme_module_relationship_network::PLUGIN_VERSIONS
            .iter()
            .map(|(name, version)| (*name, *version))
            .collect();
    let listed: std::collections::BTreeMap<&str, &str> = manifest
        .modules
        .iter()
        .map(|item| (item.name.as_str(), item.version.as_str()))
        .collect();
    assert_eq!(
        compiled, listed,
        "plugin-versions.json 与编译期版本表不一致（清单重写 / build.rs 抄错）"
    );
}
