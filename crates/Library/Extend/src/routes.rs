//! 本插件自己的端点：`/v1/extensions*` 那 15 条。
//!
//! handler 住在插件里（Magento 2 那种「模块自带 controller」）。插件不认识
//! `server` 的 `AppState`，只声明它需要什么（[`Host`]），由宿主实现并注入。
//!
//! 十五条端点分三个接缝：
//!
//! - **编辑页 4 条**（`files` / `file` / `publish`）：盘面规矩（名字 / 路径校验、
//!   递归清单、骨架落盘、发布）在 `nous_repo::entries`，要的是私库句柄 [`Host::ext_repo`]。
//!   **不 spawn git、不另建锁** —— `nous-repo` 是唯一的 git 进程调用面。
//! - **管理面 9 条**（`create` / `duplicate` / `install` / `uninstall` /
//!   `categories` ×2 / `category` / `trash` / `trash/purge`）：目录动作在
//!   [`crate::AdminStore`] 的私库盘面上，运行动作经同一个 [`crate::Runtime`]，
//!   这一层不另存注册表。
//! - **清单与重载 2 条**（`GET /v1/extensions`、`POST /v1/extensions/reload`）：
//!   只读投影 + 按盘上现状重装。[`Host::ext_usage`] 那条是 `?usage=1` 的反查（#85）：
//!   会话记录归 `nous-durable`，插件不自己开库。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::Json;
use nous_repo::Refusal;
use nous_routes::HttpRefusal;
use serde::Deserialize;
use serde_json::{json, Value};

/// 模块全名（规则 12：`<Vendor>_<Module>`）：`/v1/capabilities` 的 `modules[].name`、
/// `/v1/_routes` 的 `module` 标签、`GET /v1/plugins/:name/readme` 都按全名说话。
/// URL 路径 `/v1/extensions*` 与私库键 `extend` 是小写世界，**跟全名是两回事**。
pub const MODULE: &str = "Extend";

/// 私库在宿主仓库表里的键（`repos::from_env` 的名字）。
///
/// 注意与 URL 段的差别：私库键是 `extend`，URL 是 `extensions` —— 这条差异在
/// 搬家前就存在（`docs/decisions.md` 记过），搬家不改口径。
pub const REPO_KEY: &str = "extend";

/// 宿主能力：这个插件只认识四件事。
///
/// 每个方法都返回 `Option` —— `None` 是「宿主没装配」（目录不存在 / 没起运行时）。
/// 四条端点的分工见模块文档；`ext_usage` 返回 `Result` 而不是 `Option`，
/// 因为反查失败要说清是**哪种**失败（搬过来之前是 `?` 直传状态码）。
pub trait Host {
    /// 扩展私库的仓库句柄（编辑页的写锁、etag、git 提交都在里面）。
    fn ext_repo(&self) -> Option<Arc<nous_repo::Repo>>;

    /// 管理面：目录动作的盘面（`root` / `trash` / HEAD 回退钩子）。
    fn ext_admin(&self) -> Option<&crate::AdminStore>;

    /// 运行时：`items()` / `reload_all()` / `start_one()` / `uninstall()` / `state_of()`。
    fn ext_runtime(&self) -> Option<&Arc<crate::Runtime>>;

    /// `?usage=1`：谁在用这个扩展（#85）。会话记录归 durable，插件不自己开库。
    fn ext_usage(&self) -> std::result::Result<Value, HttpRefusal>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn ext_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        (**self).ext_repo()
    }
    fn ext_admin(&self) -> Option<&crate::AdminStore> {
        (**self).ext_admin()
    }
    fn ext_runtime(&self) -> Option<&Arc<crate::Runtime>> {
        (**self).ext_runtime()
    }
    fn ext_usage(&self) -> std::result::Result<Value, HttpRefusal> {
        (**self).ext_usage()
    }
}

type EndpointResult = std::result::Result<Json<Value>, HttpRefusal>;

/// `RepoError` → HTTP 回包：先由 `nous_repo::Refusal` 判语义，再逐字搬两格。
fn http(error: nous_repo::RepoError) -> HttpRefusal {
    let refusal = Refusal::from_repo(error);
    HttpRefusal::new(refusal.status, refusal.body())
}

/// 扩展侧错误 → HTTP。
///
/// 两套判法在搬家前就分开写在 `ext_admin.rs` 里，这里逐字保留：
/// - [`bad`]：参数 / 盘面问题一律 400（`install` / `uninstall` / 类别 / 垃圾站）。
/// - [`conflict`]：`create` / `duplicate` 的「名字撞车」是 409、「找不到」是 404，
///   靠 `ExtError` 的人话里那句「已存在 / 已在源码根」判别（搬前就是这么判的）。
fn bad(error: crate::ExtError) -> HttpRefusal {
    HttpRefusal::bad_request(error.to_string())
}

fn conflict(error: crate::ExtError) -> HttpRefusal {
    let message = error.to_string();
    if message.contains("已存在") || message.contains("已在源码根") {
        HttpRefusal::conflict(message)
    } else {
        HttpRefusal::not_found(message)
    }
}

/// 私库没装配时那句熟话（三个仓共用同一个形状，只换标签）。
fn repo<H: Host>(host: &H) -> std::result::Result<Arc<nous_repo::Repo>, HttpRefusal> {
    host.ext_repo().ok_or_else(|| {
        HttpRefusal::conflict("扩展私库未装配（NOUS_EXTEND 指向的目录不存在？）")
    })
}

fn admin<H: Host>(host: &H) -> std::result::Result<&crate::AdminStore, HttpRefusal> {
    host.ext_admin()
        .ok_or_else(|| HttpRefusal::conflict("扩展管理面未装配"))
}

fn runtime<H: Host>(host: &H) -> std::result::Result<&Arc<crate::Runtime>, HttpRefusal> {
    host.ext_runtime()
        .ok_or_else(|| HttpRefusal::conflict("extensions runtime not configured"))
}

// ── 请求体 / 查询参数 ────────────────────────────────────────────────

/// `?usage=1`：反查模式开关。缺席 / 0 / false / 其它值都不是反查，
/// 不因为一个可选参数让整个端点 400（serde 的 `bool` 会把 `1` 判成非法）。
#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub usage: Option<String>,
}

impl UsageQuery {
    fn requested(&self) -> bool {
        matches!(self.usage.as_deref(), Some("1" | "true"))
    }
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

#[derive(Debug, Deserialize, Default)]
pub struct PublishBody {
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DuplicateBody {
    pub name: String,
    pub to: String,
    #[serde(default)]
    pub unbind: bool,
}

#[derive(Debug, Deserialize)]
pub struct NameBody {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CategoryBody {
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CategoriesBody {
    pub op: String,
    pub name: String,
    #[serde(default)]
    pub to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TrashPurgeBody {
    pub entry: String,
}

// ── 清单与重载（2 条）──────────────────────────────────────────────

/// `GET /v1/extensions`：运行时没装配就是空清单（`{"items": []}`），不 404、不报错 ——
/// 「没起运行时」和「一个扩展都没有」在 wire 上是同一句话，搬前就这么答的。
/// `?usage=1` 时改答「谁在用」（#85）。
async fn extensions<H: Host>(
    State(host): State<H>,
    Query(query): Query<UsageQuery>,
) -> EndpointResult {
    if query.requested() {
        return Ok(Json(host.ext_usage()?));
    }
    let items = host.ext_runtime().map(|runtime| runtime.items());
    Ok(Json(json!({ "items": items.unwrap_or_default() })))
}

/// `POST /v1/extensions/reload`：按盘上现状重装所有已装载扩展（#81）。
/// 返回 `{ reloaded: [name...] }`；失败的保留旧进程，错误进扩展自己的诊断。
async fn reload<H: Host>(State(host): State<H>) -> EndpointResult {
    let reloaded = runtime(&host)?.reload_all();
    Ok(Json(json!({ "reloaded": reloaded })))
}

// ── 编辑页（4 条，盘面规矩在 `nous_repo::entries`）──────────────────

/// `GET /v1/extensions/files?name=`：递归清单，正文与附件一起给（不按扩展名筛）。
async fn files<H: Host>(
    State(host): State<H>,
    Query(query): Query<NameQuery>,
) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&query.name).map_err(http)?;
    let list = nous_repo::list_entry_files(&repo, &name).map_err(http)?;
    Ok(Json(json!({ "name": name, "files": list })))
}

/// `GET /v1/extensions/file?name=&path=`：正文 + `etag`（写回时当 `base`）。
async fn file<H: Host>(
    State(host): State<H>,
    Query(query): Query<FileQuery>,
) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&query.name).map_err(http)?;
    let path = nous_repo::check_path(&query.path).map_err(http)?;
    nous_repo::require_entry(&repo, &name, LABEL).map_err(http)?;
    let (text, etag) =
        nous_repo::read_text(&repo, &format!("{name}/{path}")).map_err(http)?;
    Ok(Json(json!({ "name": name, "path": path, "text": text, "etag": etag })))
}

/// `PUT /v1/extensions/file`：只落盘、不提交。`base` 不符时 `nous_repo` 回 409。
async fn save<H: Host>(State(host): State<H>, Json(body): Json<SaveBody>) -> EndpointResult {
    let repo = repo(&host)?;
    let name = nous_repo::check_name(&body.name).map_err(http)?;
    let path = nous_repo::check_path(&body.path).map_err(http)?;
    nous_repo::require_entry(&repo, &name, LABEL).map_err(http)?;
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

/// `POST /v1/extensions/publish`：把私库里 dirty 的路径提交（并尝试 push）。
/// body 可以省；`message` 空着就用那句默认文案。
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
        .unwrap_or(DEFAULT_MESSAGE);
    let published = nous_repo::publish_entry(&repo, message).map_err(http)?;
    Ok(Json(serde_json::to_value(published).unwrap_or(Value::Null)))
}

// ── 管理面（9 条）────────────────────────────────────────────────

/// `POST /v1/extensions/create`：落骨架（`nous.json` / `README.md` / `main.rs`）。
async fn create<H: Host>(
    State(host): State<H>,
    Json(body): Json<CreateBody>,
) -> std::result::Result<(StatusCode, Json<Value>), HttpRefusal> {
    let store = admin(&host)?;
    let result = store.create(&crate::CreateOptions {
        name: body.name,
        category: body.category,
    });
    // 201 是「真的造出来了」，与搬前的 status 逐字一致。
    match result {
        Ok(value) => Ok((StatusCode::CREATED, Json(value))),
        Err(error) => Err(conflict(error)),
    }
}

/// `POST /v1/extensions/duplicate`：复制一份（默认解绑句柄）。
async fn duplicate<H: Host>(
    State(host): State<H>,
    Json(body): Json<DuplicateBody>,
) -> std::result::Result<(StatusCode, Json<Value>), HttpRefusal> {
    let store = admin(&host)?;
    let result = store.duplicate(&crate::DuplicateOptions {
        name: body.name,
        to: body.to,
        unbind: body.unbind,
    });
    match result {
        Ok(value) => Ok((StatusCode::CREATED, Json(value))),
        Err(error) => Err(conflict(error)),
    }
}

/// `POST /v1/extensions/uninstall`：源码进垃圾站（`data/extend-trash/`），不是删掉。
/// 运行时在装配了就顺带停掉它 —— 停止失败不改回执（源码已经挪走了）。
async fn uninstall<H: Host>(State(host): State<H>, Json(body): Json<NameBody>) -> EndpointResult {
    let store = admin(&host)?;
    let result = store.uninstall(&body.name).map_err(bad)?;
    if let Some(runtime) = host.ext_runtime() {
        let _ = runtime.uninstall(&body.name);
    }
    Ok(Json(result))
}

/// `POST /v1/extensions/install`：从私库装一份骨架到扩展根，再交给运行时启动。
///
/// 没起运行时的时候**不假装装好了**：回执里 `state` 写 `stopped`，界面照实显示。
async fn install<H: Host>(State(host): State<H>, Json(body): Json<NameBody>) -> EndpointResult {
    let store = admin(&host)?;
    let mut result = store.install(&body.name).map_err(bad)?;
    let Some(runtime) = host.ext_runtime() else {
        result["state"] = json!("stopped");
        return Ok(Json(result));
    };
    let loaded =
        crate::LoadedManifest::load(&store.root.join(&body.name)).map_err(bad)?;
    runtime.start_one(loaded).map_err(bad)?;
    // 运行时内部五态的 `normal` 在 wire 上叫 `running`（与 `items[]` 同规矩）。
    let state = match runtime.state_of(&body.name).map(|state| state.as_str()) {
        Some("normal") => "running",
        Some(other) => other,
        None => "stopped",
    };
    result["state"] = json!(state);
    Ok(Json(result))
}

/// `GET /v1/extensions/categories`：类别词表。
async fn get_categories<H: Host>(State(host): State<H>) -> EndpointResult {
    Ok(Json(json!({ "categories": admin(&host)?.categories() })))
}

/// `POST /v1/extensions/categories`：类别词表的增删改（`op` 决定动作）。
async fn post_categories<H: Host>(
    State(host): State<H>,
    Json(body): Json<CategoriesBody>,
) -> EndpointResult {
    let store = admin(&host)?;
    json(store.edit_categories(&body.op, &body.name, body.to.as_deref()))
}

/// `POST /v1/extensions/category`：给某个扩展换类别（`null` = 移出类别）。
async fn category<H: Host>(State(host): State<H>, Json(body): Json<CategoryBody>) -> EndpointResult {
    let store = admin(&host)?;
    json(store.set_category(&body.name, body.category.as_deref()))
}

/// `GET /v1/extensions/trash`：垃圾站。卸载不是删除 —— 这些目录还在
/// `data/extend-trash/` 里，界面上得能看见。
async fn get_trash<H: Host>(State(host): State<H>) -> EndpointResult {
    Ok(Json(json!({ "items": admin(&host)?.trash_items() })))
}

/// `POST /v1/extensions/trash/purge`：真删一份垃圾站条目。
async fn purge_trash<H: Host>(
    State(host): State<H>,
    Json(body): Json<TrashPurgeBody>,
) -> EndpointResult {
    let store = admin(&host)?;
    json(store.purge_trash(&body.entry))
}

/// `ExtError` 结果的 JSON 面：搬前那句 `to_value(...).unwrap_or(Value::Null)` 逐字保留。
fn json<T: serde::Serialize>(value: std::result::Result<T, crate::ExtError>) -> EndpointResult {
    let value = value.map_err(bad)?;
    Ok(Json(serde_json::to_value(value).unwrap_or(Value::Null)))
}

/// 私库条目在报错里怎么称呼自己（与搬前的 `Kind::Extensions.label()` 一致）。
const LABEL: &str = "扩展";

/// 发布时的默认文案（与旧面板逐字一致）。
const DEFAULT_MESSAGE: &str = "extend: 更新扩展仓库";

/// 这 15 条端点的声明（`server` 只做遍历 → mount → 收集）。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        nous_routes::Route::new("GET", "/v1/extensions", MODULE, || get(extensions::<H>)),
        nous_routes::Route::new("POST", "/v1/extensions/reload", MODULE, || post(reload::<H>)),
        nous_routes::Route::new("GET", "/v1/extensions/files", MODULE, || get(files::<H>)),
        nous_routes::Route::new("GET", "/v1/extensions/file", MODULE, || get(file::<H>)),
        nous_routes::Route::new("PUT", "/v1/extensions/file", MODULE, || put(save::<H>)),
        nous_routes::Route::new("POST", "/v1/extensions/publish", MODULE, || {
            post(publish::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/extensions/create", MODULE, || post(create::<H>)),
        nous_routes::Route::new("POST", "/v1/extensions/duplicate", MODULE, || {
            post(duplicate::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/extensions/install", MODULE, || {
            post(install::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/extensions/uninstall", MODULE, || {
            post(uninstall::<H>)
        }),
        nous_routes::Route::new("GET", "/v1/extensions/categories", MODULE, || {
            get(get_categories::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/extensions/categories", MODULE, || {
            post(post_categories::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/extensions/category", MODULE, || {
            post(category::<H>)
        }),
        nous_routes::Route::new("GET", "/v1/extensions/trash", MODULE, || {
            get(get_trash::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/extensions/trash/purge", MODULE, || {
            post(purge_trash::<H>)
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个什么都不装配的宿主：用来钉「没装就说没装」，也用来枚举声明。
    #[derive(Clone)]
    struct Bare;

    impl Host for Bare {
        fn ext_repo(&self) -> Option<Arc<nous_repo::Repo>> {
            None
        }
        fn ext_admin(&self) -> Option<&crate::AdminStore> {
            None
        }
        fn ext_runtime(&self) -> Option<&Arc<crate::Runtime>> {
            None
        }
        fn ext_usage(&self) -> std::result::Result<Value, HttpRefusal> {
            Ok(json!({}))
        }
    }

    #[test]
    fn declarations_are_the_fifteen_extensions_endpoints() {
        let declared = routes::<Bare>();
        assert_eq!(declared.len(), 15);
        for route in &declared {
            assert!(
                route.path().starts_with("/v1/extensions"),
                "端点路径不在这张票的范围里：{}",
                route.path()
            );
            assert_eq!(route.module(), MODULE);
        }
        // 同一路径上的不同方法各算一条（`GET`/`PUT /v1/extensions/file`）。
        let mut pairs: Vec<(&str, &str)> = declared
            .iter()
            .map(|route| (route.method(), route.path()))
            .collect();
        pairs.sort();
        assert!(pairs.contains(&("GET", "/v1/extensions/file")));
        assert!(pairs.contains(&("PUT", "/v1/extensions/file")));
    }

    #[test]
    fn usage_query_accepts_only_true_ish() {
        for raw in ["1", "true"] {
            assert!(UsageQuery { usage: Some(raw.into()) }.requested());
        }
        for raw in ["0", "false", "yes", ""] {
            assert!(!UsageQuery { usage: Some(raw.into()) }.requested());
        }
        assert!(!UsageQuery { usage: None }.requested());
    }

    /// 取拒绝的状态码。写 `match` 而不是 `unwrap_err()`：`Ok` 那侧是 `Arc<Repo>` /
    /// `Arc<Runtime>`，两个都没实现 `Debug`，`unwrap_err` 编不过。
    fn status_of<T>(result: std::result::Result<T, HttpRefusal>) -> u16 {
        match result {
            Ok(_) => panic!("没装配的宿主不该成功"),
            Err(refusal) => refusal.status,
        }
    }

    #[test]
    fn missing_host_capabilities_say_so_instead_of_pretending() {
        // 私库 / 管理面 / 运行时没装配 → 409，并且说清是哪一半；
        // 不退回空 200：编辑器和「一个扩展都没有」是两件事。
        assert_eq!(status_of(repo(&Bare)), 409);
        assert_eq!(status_of(admin(&Bare)), 409);
        assert_eq!(status_of(runtime(&Bare)), 409);
    }
}
