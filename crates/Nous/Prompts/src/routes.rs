//! 本插件自己的端点：`/v1/prompts` 那 7 条。
//!
//! handler 住在插件里（Magento 2 那种「模块自带 controller」）。插件不认识
//! `server` 的 `AppState`，只声明它需要什么（[`Host`]），由宿主实现并注入。
//!
//! 七条端点分两半，接缝也不同：
//!
//! - `GET /v1/prompts` 是**只读投影**：盘上真相由 `nous_prompts::catalog_value` 解析，
//!   这里只补 `root` / `present` / `missing` 那三格（`#63` 形状）。
//! - 另外六条是**编辑页的管理面**（`files` / `file` / `create` / `publish`）：
//!   盘面规矩（名字校验、递归清单、骨架落盘、发布）在 `nous_repo::entries`，
//!   这里只做 HTTP ↔ 那层的翻译。**不 spawn git、不另建锁** ——
//!   `nous-repo` 是唯一的 git 进程调用面。
//!
//! 这票之前，五条管理面端点住在 `server/src/resource_admin.rs` 的**三仓共用骨架**里
//! （`Kind` 枚举：skills / prompts / extensions）。技能那一支先搬走，这里搬第二支，
//! 搬完那个文件只剩扩展一支。

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use nous_repo::Refusal;
use nous_routes::HttpRefusal;

/// 模块全名（规则 12：`<Vendor>_<Module>`）：`/v1/capabilities` 的 `modules[].name`、
/// `/v1/_routes` 的 `module` 标签、`GET /v1/plugins/:name/readme` 都按全名说话。
/// URL 路径 `/v1/prompts*` 与私库键 `prompts` 是小写世界，**不跟着改名**。
pub const MODULE: &str = "Nous_Prompts";

/// 私库在宿主仓库表里的键（`repos::from_env` 的名字）。
pub const REPO_KEY: &str = "prompts";

/// 宿主能力：这个插件只认识两件事。
///
/// 两个方法都返回 `Option` —— `None` 是「宿主没装配」（目录不存在），
/// 端点据此诚实回 409，**不退回空 200**：编辑器和「没有文件」是两件事。
pub trait Host {
    /// 模板私库的仓库句柄。
    fn prompts_repo(&self) -> Option<Arc<nous_repo::Repo>>;

    /// 模板私库的根目录（`NOUS_PROMPTS` 解析结果）。只读投影用它 ——
    /// `nous_prompts::catalog_value` 要的是路径，不是仓库句柄。
    fn prompts_root(&self) -> PathBuf;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn prompts_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        (**self).prompts_repo()
    }
    fn prompts_root(&self) -> PathBuf {
        (**self).prompts_root()
    }
}

type EndpointResult = std::result::Result<Json<Value>, HttpRefusal>;

/// `RepoError` → HTTP 回包：先由 `nous_repo::Refusal` 判语义，再逐字搬两格。
fn http(error: nous_repo::RepoError) -> HttpRefusal {
    let refusal = Refusal::from_repo(error);
    HttpRefusal::new(refusal.status, refusal.body())
}

fn http_conflict(message: impl Into<String>) -> HttpRefusal {
    HttpRefusal::conflict(message)
}

fn http_bad_request(message: impl Into<String>) -> HttpRefusal {
    HttpRefusal::bad_request(message)
}


/// 私库没装配时那句熟话（三个仓共用同一个形状，只换标签）。
fn repo<H: Host>(host: &H) -> std::result::Result<Arc<nous_repo::Repo>, HttpRefusal> {
    host.prompts_repo()
        .ok_or_else(|| http_conflict("模板私库未装配（NOUS_PROMPTS 指向的目录不存在？）"))
}

#[derive(Debug, Deserialize)]
pub struct NameQuery {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct SaveBody {
    pub name: String,
    pub path: String,
    pub text: String,
    /// 省掉 = 硬写（新建 / 覆盖）；给上 = If-Match，不符回 409。
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    /// 提示词用它（落 `nous.json` 的 `summary`）。
    #[serde(default)]
    pub summary: String,
    /// 提示词可给正文；不给就落一个 `# <name>` 标题。
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub category: Vec<String>,
}

/// 复制：`{name, to}` —— 把 `name` 那条整目录复制成 `to`。
#[derive(Debug, Deserialize)]
pub struct DuplicateBody {
    pub name: String,
    pub to: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct PublishBody {
    #[serde(default)]
    pub message: Option<String>,
}

/// `GET /v1/prompts`：目录不存在就是空表，不伪造内容、不 404。
///
/// 盘上真相由 `nous_prompts` 解析（`PROMPT.md` + `nous.json` + 根目录 `settings.json`）。
/// 这里只做 wire 投影：`root` / `present` / `missing` 三格是 #63 的形状
/// （「没装」和「不知道在哪」可区分），其余键由 `catalog_value` 原样透传。
async fn prompts<H: Host>(State(host): State<H>) -> EndpointResult {
    let root = host.prompts_root();
    let present = root.is_dir();
    let shown = Value::String(root.to_string_lossy().into_owned());
    let mut body = if present {
        json!({ "root": shown, "present": true, "missing": Value::Null })
    } else {
        json!({
            "root": shown,
            "present": false,
            "missing": format!(
                "私库路径不存在：{}（设 {} 指到你的副本）",
                nous_env::PROMPTS.default,
                nous_env::PROMPTS.var
            ),
        })
    };
    if present {
        let catalog = crate::catalog_value(&root);
        for (key, value) in catalog.as_object().expect("catalog 是对象") {
            body[key.clone()] = value.clone();
        }
    } else {
        body["default"] = Value::Null;
        body["categories"] = json!([]);
        body["prompts"] = json!([]);
    }
    Ok(Json(body))
}

/// `GET /v1/prompts/files?name=`：递归清单，正文与附件一起给（不按扩展名筛）。
async fn files<H: Host>(State(host): State<H>, Query(query): Query<NameQuery>) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&query.name).map_err(http)?;
    let list = nous_repo::list_entry_files(&repo, &name).map_err(http)?;
    Ok(Json(json!({ "name": name, "files": list })))
}

/// `GET /v1/prompts/file?name=&path=`：正文 + `etag`（写回时当 `base`）。
async fn file<H: Host>(State(host): State<H>, Query(query): Query<FileQuery>) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&query.name).map_err(http)?;
    let path = nous_repo::check_path(&query.path).map_err(http)?;
    nous_repo::require_entry(&repo, &name, "模板").map_err(http)?;
    let (text, etag) =
        nous_repo::read_text(&repo, &format!("{name}/{path}")).map_err(http)?;
    Ok(Json(json!({ "name": name, "path": path, "text": text, "etag": etag })))
}

/// `PUT /v1/prompts/file`：只落盘、不提交。`base` 不符时 `nous_repo` 回 409。
async fn save<H: Host>(State(host): State<H>, Json(body): Json<SaveBody>) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&body.name).map_err(http)?;
    let path = nous_repo::check_path(&body.path).map_err(http)?;
    nous_repo::require_entry(&repo, &name, "模板").map_err(http)?;
    let created = body.base.is_none();
    let etag = nous_repo::write_text(
        &repo,
        &format!("{name}/{path}"),
        &body.text,
        body.base.as_deref(),
    )
    .map_err(http)?;
    Ok(Json(json!({ "name": name, "path": path, "etag": etag, "created": created })))
}

/// `POST /v1/prompts/create`：落骨架两个文件，**不提交**。
async fn create<H: Host>(
    State(host): State<H>,
    Json(body): Json<CreateBody>,
) -> std::result::Result<(StatusCode, Json<Value>), HttpRefusal> {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&body.name).map_err(http)?;
    let summary = body.summary.trim();
    if summary.is_empty() {
        return Err(http_bad_request(
            "summary 不能空 —— 人靠这一行决定选不选它",
        ));
    }
    let skeleton = nous_repo::Skeleton::prompt(&name, summary, &body.body).with_category(body.category);
    nous_repo::create_entry(&repo, &name, &skeleton).map_err(http)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "name": name, "path": name, "files": 2 })),
    ))
}

/// `POST /v1/prompts/duplicate`：整目录复制一条模板（`name` → `to`），改名字，**不提交**。
async fn duplicate<H: Host>(
    State(host): State<H>,
    Json(body): Json<DuplicateBody>,
) -> std::result::Result<(StatusCode, Json<Value>), HttpRefusal> {
    let repo = repo(&host)?;
    let dup = nous_repo::duplicate_entry(&repo, &body.name, &body.to, None).map_err(http)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(dup).unwrap_or(Value::Null)),
    ))
}

/// `POST /v1/prompts/publish`：把私库里 dirty 的路径提交（并尝试 push）。
/// body 可以省；`message` 空着就用旧面板那句默认文案。
async fn publish<H: Host>(
    State(host): State<H>,
    Json(body): Json<PublishBody>,
) -> EndpointResult {
    let repo = repo(&host)?;
    let message = body
        .message
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("prompt: 更新模板仓库");
    let published = nous_repo::publish_entry(&repo, message).map_err(http)?;
    Ok(Json(serde_json::to_value(published).unwrap_or(Value::Null)))
}

/// 本插件的端点表：路径只写在这里（axum 的 `{id}` 形式），
/// `nous-routes` 自动折成注册表要的 `:id`。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        nous_routes::Route::new("GET", "/v1/prompts", MODULE, || get(prompts::<H>)),
        nous_routes::Route::new("GET", "/v1/prompts/files", MODULE, || get(files::<H>)),
        nous_routes::Route::new("GET", "/v1/prompts/file", MODULE, || get(file::<H>)),
        nous_routes::Route::new("PUT", "/v1/prompts/file", MODULE, || put(save::<H>)),
        nous_routes::Route::new("POST", "/v1/prompts/create", MODULE, || post(create::<H>)),
        nous_routes::Route::new("POST", "/v1/prompts/publish", MODULE, || post(publish::<H>)),
        nous_routes::Route::new("POST", "/v1/prompts/duplicate", MODULE, || post(duplicate::<H>)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared() -> Vec<String> {
        routes::<Arc<NullHost>>()
            .iter()
            .map(|route| route.declared())
            .collect()
    }

    struct NullHost;

    impl Host for NullHost {
        fn prompts_repo(&self) -> Option<Arc<nous_repo::Repo>> {
            None
        }
        fn prompts_root(&self) -> PathBuf {
            PathBuf::from("/nonexistent")
        }
    }

    /// 声明逐字：路径写错、漏一条、方法错，这一条立刻红。
    #[test]
    fn declares_all_seven_endpoints_verbatim() {
        assert_eq!(
            declared(),
            vec![
                "GET /v1/prompts",
                "GET /v1/prompts/files",
                "GET /v1/prompts/file",
                "PUT /v1/prompts/file",
                "POST /v1/prompts/create",
                "POST /v1/prompts/publish",
                "POST /v1/prompts/duplicate",
            ]
        );
    }

    #[test]
    fn module_name_is_the_lowercase_contract_name() {
        for route in routes::<Arc<NullHost>>() {
            assert_eq!(route.module(), "Nous_Prompts");
        }
    }

    /// 未装配 = 409 且是那句熟话（不是空 200，也不是 404）。
    #[tokio::test]
    async fn unassembled_repo_is_a_conflict_not_an_empty_list() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let mut router = axum::Router::new();
        for route in &routes::<Arc<NullHost>>() {
            router = route.mount(router);
        }
        let router = router.with_state(Arc::new(NullHost));
        let request = Request::builder()
            .uri("/v1/prompts/files?name=demo")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("NOUS_PROMPTS"),
            "文案要点名环境变量：{body}"
        );
    }

    /// 只读投影在目录不存在时给 `present:false` + 空表，不 404。
    #[tokio::test]
    async fn missing_root_is_an_honest_empty_catalog() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let mut router = axum::Router::new();
        for route in &routes::<Arc<NullHost>>() {
            router = route.mount(router);
        }
        let router = router.with_state(Arc::new(NullHost));
        let request = Request::builder()
            .uri("/v1/prompts")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["present"], json!(false));
        assert_eq!(body["prompts"], json!([]));
        assert_eq!(body["categories"], json!([]));
        assert!(
            body["missing"].as_str().unwrap().contains("NOUS_PROMPTS"),
            "missing 要点名环境变量：{body}"
        );
    }
}
