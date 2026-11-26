//! `Device` 插件自己的端点：机器 / 模型 / 默认模型 / 自省（`_routes` / `capabilities` /
//! `openapi.json`）/ 插件 README。
//!
//! 声明与 handler 在同一处（[`routes`] 里那条 `Route` 指向下面的函数），所以
//! 「注册表里写着有」和「真能打到」不会再有机会分叉（规则 11）。
//!
//! ## 九条端点里，只有第三条是真的「搬」
//!
//! 前三条下面这条分界线很关键，写在这里免得下次有人把「为什么不在插件里」问第二遍：
//!
//! - **编目三件套**（`/v1/models` / `PUT /v1/default` / `POST /v1/catalog/refresh`）：
//!   数据形状、来源优先级、原子写都在本 crate 的 `lib.rs`，搬过来只是把 handler
//!   从三行转发换成三行调用。
//! - **自省三件套**（`/v1/_routes` / `/v1/capabilities` / `/v1/openapi.json`）：
//!   **它们读的是注册表本身**。注册表 = 静态表（还没搬走的端点）∪ 各插件的 `routes()`
//!   声明。这份原料属于宿主（只有它同时看得见两边），所以走 [`Host::manifest`] /
//!   [`Host::registered`] 拿进来；**视图的组装（`enabled` / `reason` 判定、JSON 键名、
//!   OpenAPI 的 paths 折叠）留在本 crate** —— 那才是端点自己的工作，不是宿主的。
//! - **`GET /v1/device`**：形状归本 crate，`build` / `target` 两个字段是**产物身份**
//!   （对 `/proc/self/exe` 算 sha256），那是宿主才知道自己是什么二进制，走
//!   [`Host::build_id`] / [`Host::target_triple`]。
//! - **`GET /v1/plugins/{name}/readme`**：读盘 + 缓存 + 白名单都在本 crate
//!   （`readme.rs`），白名单来自 [`Host::manifest`]。
//!
//! ## 一个刻意的「反向」依赖
//!
//! `nous-sessions` 依赖本 crate（`ModelsResponse` 的形状）。所以本 crate **绝不能**
//! 依赖 `nous-server` / `nous-sessions` —— 那会成环。`plugin.toml` 的 `[requires]`
//! 只有 `nous-routes`，`cargo tree -p nous-device` 里看不到 `nous-server`，
//! 判据在 `tests/manifest.rs`。

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post, put};
use axum::Json;
use nous_routes::Declaration;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    load_models, refresh_remote_catalog, save_default, CatalogEnvironment, ModelRef, ModelsResponse,
};

/// 模块全名（`docs/plugins.md` 规则 12）：`Device`。
/// `/v1/_routes` 的 `module`、`/v1/capabilities` 的 `modules[].name` 都用这一个字面量。
///
/// 曾叫 `Nous_Device`（`Vendor_Module` 两段式）。改成单段与 `Models` / `Skill` 同形 ——
/// `vendor` 仍由 `plugin.toml` 的 `vendor = "Nous"` 带着走（规则 10），目录仍是
/// `crates/Nous/Device`，所以出品方一分没丢，只是名字不再冗余地重复一次。
pub const MODULE: &str = "Device";

// ── 宿主交出来的两种原料 ──────────────────────────────────────────────

/// 清单的一行：`docs/plugins.md` 主表 + `plugin.toml` 的版本。
///
/// 宿主负责「读清单 + 查版本」，本插件负责「按这一行判 enabled / 编成 JSON」。
/// 字段都是展示列，逐格抄自清单（`capabilities_manifest.rs` 在宿主侧重比）。
pub struct ManifestLine {
    pub name: &'static str,
    /// 清单关掉时的原因；`None` = 清单开着（还要再看有没有端点才知道实现没实现）。
    pub reason: Option<Refusal>,
    pub layer: &'static str,
    pub kind: &'static str,
    pub required: bool,
    pub crate_name: &'static str,
    pub description: &'static str,
    /// 这个**插件自己的** semver（规则 8），不是发行版号。
    pub version: String,
    /// 出品方（规则 10）；外挂 / 待建的模块是 `None` —— 不猜、不编一个顶上。
    pub vendor: Option<String>,
    /// 版本取不到时回退成发行版号，并把「这个数是发行版号」说出来（不静默）。
    pub version_from_release: bool,
}

/// 清单里一行「为什么不可用」。四值封闭，与 `docs/plugins.md` 的 reason 列同源。
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

impl Refusal {
    /// 清单开着、但一条端点都没注册：`not_implemented`。
    ///
    /// 这条判定**只能在本插件里做** —— 它要「清单行」和「注册表里有没有它的端点」两个
    /// 信息同时在场，宿主交出来的两份原料正好凑齐。
    pub fn not_implemented(name: &str) -> Self {
        Self {
            code: "not_implemented",
            message: format!("{name} 模块尚未落地"),
        }
    }
}

/// 宿主能力。九个方法分成四组，见模块头的分界线。
///
/// 每个方法都回答「插件自己算不出来、只有宿主知道」的那一件事；能自己算的一律不进这里
/// （比如 `enabled` 就是本插件用 `manifest` + `registered` 现算的）。
pub trait Host {
    // —— 编目：`/v1/models` 的家 ——

    /// 当前编目（注入的静态表 / 装配 / 远端刷新的那份），`/v1/models` 与发消息共用。
    fn models(&self) -> ModelsResponse;

    /// 编目是不是「注入」进来的（测试快照 / faux 验收）。注入时装配编目优先。
    fn injected_models(&self) -> bool;

    /// 装配出来的编目（有 provider 注入时才有）。
    fn assembled_models(&self) -> Option<ModelsResponse>;

    /// 最近一次远端刷新成功的结果；失败不改这一份。
    fn refreshed_models(&self) -> Option<ModelsResponse>;

    /// 写回内存里的编目（刷新成功 / 失败留旧表都走它）。
    fn set_models(&self, models: ModelsResponse);

    /// 编目来源（内联 / 文件 / 空表）。
    fn catalog_env(&self) -> CatalogEnvironment;

    /// 元素库约定组投影出来的编目（模型入口在私库里时的那个来源）。
    fn group_catalog(&self) -> Option<ModelsResponse>;

    // —— 注册表：自省三件套的原料 ——

    /// 清单的每一行（含版本）。
    fn manifest(&self) -> Vec<ManifestLine>;

    /// 注册表 = 静态表（还没搬的端点）∪ 各插件的 `routes()` 声明。**未过滤**：
    /// 按清单开关过滤是本插件的事（`enabled` 的判定在 `capabilities` 里）。
    fn registered(&self) -> Vec<Declaration>;

    // —— 产物身份：`/v1/device` 的两个字段 ——

    /// 正在运行这份二进制的 sha256 前 8 位。
    fn build_id(&self) -> &'static str;

    /// 编译 target triple。
    fn target_triple(&self) -> &'static str;

    /// 发行版号（`CARGO_PKG_VERSION` 的宿主侧那份）。
    fn server_version(&self) -> &'static str;

    // —— 运行时自述 / 服务表 ——

    /// `faux` / `real` / …：`/v1/capabilities` 的 `runtime.provider` 用它说人话。
    fn provider_kind(&self) -> &'static str;

    /// 这个引擎对外报的地址（`NOUS_ORIGIN` 解析后的那个）。
    fn origin(&self) -> String;
}

impl<T: Host + ?Sized> Host for Arc<T> {
    fn models(&self) -> ModelsResponse {
        (**self).models()
    }
    fn injected_models(&self) -> bool {
        (**self).injected_models()
    }
    fn assembled_models(&self) -> Option<ModelsResponse> {
        (**self).assembled_models()
    }
    fn refreshed_models(&self) -> Option<ModelsResponse> {
        (**self).refreshed_models()
    }
    fn set_models(&self, models: ModelsResponse) {
        (**self).set_models(models)
    }
    fn catalog_env(&self) -> CatalogEnvironment {
        (**self).catalog_env()
    }
    fn group_catalog(&self) -> Option<ModelsResponse> {
        (**self).group_catalog()
    }
    fn manifest(&self) -> Vec<ManifestLine> {
        (**self).manifest()
    }
    fn registered(&self) -> Vec<Declaration> {
        (**self).registered()
    }
    fn build_id(&self) -> &'static str {
        (**self).build_id()
    }
    fn target_triple(&self) -> &'static str {
        (**self).target_triple()
    }
    fn server_version(&self) -> &'static str {
        (**self).server_version()
    }
    fn provider_kind(&self) -> &'static str {
        (**self).provider_kind()
    }
    fn origin(&self) -> String {
        (**self).origin()
    }
}

// ── 九条 handler ────────────────────────────────────────────────────

/// `GET /v1/machines`：机器编目（机器 + 注解 + 能力字段 + 兼容 target 列表）。
///
/// `NOUS_MACHINES_FILE` 给了就用它（读不出来退回内置表），没给就是内置表。
/// 路径解析归宿主，这里只认一个「已经解好的文件路径」。
pub async fn machines() -> Json<Value> {
    let file = std::env::var("NOUS_MACHINES_FILE")
        .ok()
        .filter(|text| !text.trim().is_empty())
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_file());
    Json(crate::machines(file.as_deref()))
}

/// `GET /v1/models`：默认模型 + 列表 + 来源诊断块。
///
/// 装配注入的编目（faux 验收 / 测试快照）优先：展示读装配出来的那份，
/// 没有再走 [`current_catalog`] 的完整顺序。
pub async fn models<H: Host>(State(host): State<H>) -> Json<ModelsResponse> {
    if host.injected_models() {
        if let Some(catalog) = host.assembled_models() {
            return Json(catalog);
        }
    }
    // `current_catalog` 只在「编目来源全空」时回错误；那种情况诚实回空表 + 诊断块
    // （`state: "missing"`），不拿 500 糊过去 —— Desk 的「设备」页要能显示「还没配」。
    match current_catalog(&host) {
        Ok(catalog) => Json(catalog),
        Err(_) => Json(ModelsResponse::default()),
    }
}

/// 「当前编目」只有这一个出处：`GET /v1/models` 和发消息的模型解析都读它，
/// 不许各读各的（线上因此出过 503：展示读了元素组，解析读的是空的文件来源）。
///
/// 顺序：注入的静态编目（测试快照 / faux 验收）→ 远端刷新 → 元素库的模型入口组
/// → 文件/内联来源。
/// `messages` 还留在 `server`，它起一轮前也要按同一个顺序解模型引用 —— 所以这个函数
/// 是 `pub` 的：编目的读取顺序只有这一处，跨 crate 也只许调它。
pub fn current_catalog<H: Host>(host: &H) -> Result<ModelsResponse, String> {
    if host.injected_models() {
        return Ok(host.models());
    }
    if let Some(refreshed) = host.refreshed_models() {
        return Ok(refreshed);
    }
    if let Some(response) = host.group_catalog() {
        return Ok(response);
    }
    load_models(&host.catalog_env())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefaultBody {
    provider: String,
    #[serde(rename = "modelId")]
    model_id: String,
}

/// `PUT /v1/default`：把默认模型原子写回同一份编目；模型不在 items 里 → 400。
pub async fn put_default<H: Host>(
    State(host): State<H>,
    Json(body): Json<DefaultBody>,
) -> Result<Json<Value>, Refused> {
    let env = host.catalog_env();
    if env.inline.is_some() {
        return Err(Refused::bad_request(
            "NOUS_MODELS 是内联编目，不能写回 default；请用 NOUS_MODELS_FILE",
        ));
    }
    let model = ModelRef {
        provider: body.provider,
        model_id: body.model_id,
    };
    save_default(&env.file_path, &model).map_err(Refused::bad_request)?;
    Ok(Json(json!({ "default": model, "wrote": true })))
}

/// `POST /v1/catalog/refresh`：拉一次远端编目，成功写缓存并换入内存；失败留旧表带 error。
/// 响应形状 `{default, items, fetchedAt, error?}`：token 只经内存请求头，不进响应/日志。
pub async fn catalog_refresh<H: Host>(State(host): State<H>) -> Result<Json<Value>, Refused> {
    let env = host.catalog_env();
    match refresh_remote_catalog(&env).await {
        Ok(models) => {
            host.set_models(models.clone());
            let mut body = serde_json::to_value(&models).map_err(|error| Refused::internal(error.to_string()))?;
            body["fetchedAt"] = models
                .catalog
                .fetched_at
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null);
            Ok(Json(body))
        }
        Err(error) => {
            // 失败保留旧编目：内存里最近一次刷新的那份优先；否则从本地缓存/文件读回。
            let mut old = host
                .refreshed_models()
                .unwrap_or_else(|| load_models(&env).unwrap_or_default());
            old.catalog.error = Some(error);
            host.set_models(old.clone());
            let top_error = old.catalog.error.clone().unwrap_or_default();
            let mut body = serde_json::to_value(&old).map_err(|error| Refused::internal(error.to_string()))?;
            body["fetchedAt"] = old
                .catalog
                .fetched_at
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null);
            body["error"] = Value::String(top_error);
            Ok(Json(body))
        }
    }
}

/// `GET /v1/device`：`node` 是引擎版本标识（About 面板显示），`startedAt` 是 epoch 毫秒。
pub async fn device<H: Host>(State(host): State<H>) -> Json<Value> {
    let body = crate::device(&host.models());
    let default = body["model"].clone();
    // 字段顺序按 wire.md 的既有快照钉住（serde_json 开了 preserve_order）。
    Json(json!({
        "self": body["self"],
        "origin": body["origin"],
        "node": host.server_version(),
        "version": host.server_version(),
        "build": host.build_id(),
        "target": host.target_triple(),
        "startedAt": body["startedAt"],
        "model": default,
        "faux": body["faux"],
        "data": body["data"],
        "catalog": body["catalog"],
        "available": body["available"],
    }))
}

/// `GET /v1/capabilities`：一个发行版一张清单，endpoints 全从注册表生成。
///
/// 形状是前向兼容的：未来（#51）可能给 module 加可选的 `kind` 字段，那是**加法**，
/// 不替换 `name` / `enabled` / `endpoints` / `reason` 的既有语义。
pub async fn capabilities<H: Host>(State(host): State<H>) -> Json<Value> {
    let kind = host.provider_kind();
    let configured = kind == "faux";
    let message = if configured {
        "假 provider，回复固定"
    } else {
        "没有配置真实 provider 凭证，发消息会 503"
    };
    let mut body = capabilities_body(&host.manifest(), &host.registered(), &host.origin(), host.server_version());
    body["runtime"]["provider"] = json!({
        "configured": configured,
        "kind": kind,
        "message": message,
    });
    Json(body)
}

/// `GET /v1/_routes`：debug 视图与 capabilities 从同一张表吐出。
///
/// `modules` 覆盖全部清单行（含未实现的，便于 desk 调试），`routes` 只列清单开着且
/// 代码注册了的端点——和 enabled 模块的 `endpoints` 多重集相等（不声明不存在的端点，
/// 未装配运行时的端点仍回 200 空清单，但真路径永远来自同一张表）。
pub async fn routes_view<H: Host>(State(host): State<H>) -> Json<Value> {
    Json(routes_body(&host.manifest(), &host.registered()))
}

/// `GET /v1/openapi.json`：从注册表（静态表 ∪ 插件声明）生成的只读机读视图。
pub async fn openapi<H: Host>(State(host): State<H>) -> Json<Value> {
    Json(openapi_body(
        &host.manifest(),
        &host.registered(),
        host.server_version(),
    ))
}

/// `GET /v1/plugins/{name}/readme`：插件「人话 README」的只读口（实现见 `readme.rs`）。
pub async fn readme<H: Host>(
    State(host): State<H>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> axum::response::Response {
    crate::readme::readme(&host.manifest(), &name)
}

// ── 视图组装：纯函数，能单测 ────────────────────────────────────────

/// 注册表 = 静态表 ∪ 插件声明，按清单开关过滤。
///
/// 路径统一是 `:id` 形式（`wire.md` 的写法）；插件交上来的是 axum 的 `{id}`，
/// 由 `Route::registry_path` 折成同一种，注册表里不会出现两种写法。
fn enabled_routes<'a>(
    manifest: &'a [ManifestLine],
    registered: &'a [Declaration],
) -> impl Iterator<Item = &'a Declaration> + 'a {
    registered.iter().filter(move |route| {
        manifest
            .iter()
            .find(|line| line.name == route.module)
            .is_some_and(|line| line.reason.is_none())
    })
}

/// `/v1/capabilities` 的正文（`runtime.provider` 由调用方补，它要宿主的 provider 状态）。
pub fn capabilities_body(
    manifest: &[ManifestLine],
    registered: &[Declaration],
    origin: &str,
    server_version: &str,
) -> Value {
    let enabled: Vec<&Declaration> = enabled_routes(manifest, registered).collect();
    let modules: Vec<Value> = manifest
        .iter()
        .map(|line| {
            let name = line.name;
            let own: Vec<&Declaration> =
                enabled.iter().copied().filter(|route| route.module == name).collect();
            // 清单关掉的模块即便有代码也不声明；清单开着但没代码就 not_implemented。
            let reason = match &line.reason {
                Some(reason) => Some(Refusal {
                    code: reason.code,
                    message: reason.message.clone(),
                }),
                None => own.is_empty().then(|| Refusal::not_implemented(name)),
            };
            // #88：六列是新加的，老桌面端只读 name/enabled/endpoints/reason，照旧能解析。
            // #93：`version` 是这个**插件自己的**版本，不是发行版号 —— 否则 14 个模块
            // 永远显示同一个数，「关于 → 插件」列表上看不出谁动过。取不到才回退发行
            // 版号，并加 `versionSource: "release"` 如实说明（回退不静默）。
            let mut module = json!({
                "name": name,
                "version": line.version,
                "kind": line.kind,
                "layer": line.layer,
                "required": line.required,
                "crate": line.crate_name,
                // 规则 10：`vendor` 是出品方（`nous` / `data` …），桌面端拿它当副标题。
                "vendor": line.vendor,
                "description": line.description,
            });
            if line.version_from_release {
                module["versionSource"] = json!("release");
            }
            match reason {
                Some(refusal) => {
                    module["enabled"] = json!(false);
                    module["reason"] = json!({
                        "code": refusal.code,
                        "message": refusal.message,
                    });
                }
                None => {
                    module["enabled"] = json!(true);
                    module["endpoints"] = json!(own
                        .iter()
                        .map(|route| format!("{} {}", route.method, route.path))
                        .collect::<Vec<_>>());
                }
            }
            module
        })
        .collect();
    json!({
        "contract": 1,
        "server": { "name": "nous", "version": server_version },
        "modules": modules,
        "runtime": {
            "provider": {
                "configured": false,
                "message": "没有配置真实 provider 凭证，发消息会 503",
            }
        },
        // 一个地址 + 服务表：engine 是它自己，triggers 尚未实现。
        "services": [
            { "name": "engine", "base": origin },
            { "name": "triggers", "base": origin, "enabled": true },
        ],
    })
}

/// `/v1/_routes` 的正文。
pub fn routes_body(manifest: &[ManifestLine], registered: &[Declaration]) -> Value {
    json!({
        "routes": enabled_routes(manifest, registered)
            .map(|route| json!({
                "module": route.module,
                "method": route.method,
                "path": route.path,
            }))
            .collect::<Vec<_>>(),
        "modules": manifest
            .iter()
            .map(|line| json!({ "name": line.name, "enabled": line.reason.is_none() }))
            .collect::<Vec<_>>(),
    })
}

/// `/v1/openapi.json` 的正文：paths 从同一张注册表折出来。
pub fn openapi_body(manifest: &[ManifestLine], registered: &[Declaration], server_version: &str) -> Value {
    let mut paths = serde_json::Map::new();
    for route in enabled_routes(manifest, registered) {
        let entry = paths
            .entry(route.path.clone())
            .or_insert_with(|| json!({}));
        if let Some(object) = entry.as_object_mut() {
            object.insert(
                route.method.to_ascii_lowercase(),
                json!({ "operationId": format!("{} {}", route.method, route.path) }),
            );
        }
    }
    json!({
        "openapi": "3.1.0",
        "info": { "title": "nous /v1", "version": server_version },
        "paths": Value::Object(paths),
    })
}

// ── 错误：本 crate 自己的 400/500，不借用宿主的 `ServerError` ─────────

/// 本插件自己的 HTTP 拒绝：`put_default` 的 400、`catalog_refresh` 的 500。
///
/// 与 `nous-repo::Refusal` / `nous-sessions::SessionError` 同一个取向：插件的错误类型
/// **就地定义**，不做公共 `AppError`（`decisions.md` 2026-11-26）。这里只需要两格，
/// 所以就只有两格 —— 不预先摆出用不上的状态码。
pub struct Refused {
    status: u16,
    message: String,
}

impl Refused {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            message: message.into(),
        }
    }
}

impl axum::response::IntoResponse for Refused {
    fn into_response(self) -> axum::response::Response {
        let status = axum::http::StatusCode::from_u16(self.status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!({ "error": self.message }))).into_response()
    }
}

// ── 端点表 ──────────────────────────────────────────────────────────

/// 端点唯一真相：`/v1/_routes`、`/v1/capabilities`、`docs/plugin-versions.md`
/// 三处都从这里读，`plugin.toml` 里**没有**端点表（规则 11）。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        nous_routes::Route::new("GET", "/v1/machines", MODULE, || get(machines)),
        nous_routes::Route::new("GET", "/v1/models", MODULE, || get(models::<H>)),
        nous_routes::Route::new("PUT", "/v1/default", MODULE, || put(put_default::<H>)),
        nous_routes::Route::new("POST", "/v1/catalog/refresh", MODULE, || {
            post(catalog_refresh::<H>)
        }),
        nous_routes::Route::new("GET", "/v1/device", MODULE, || get(device::<H>)),
        nous_routes::Route::new("GET", "/v1/capabilities", MODULE, || get(capabilities::<H>)),
        nous_routes::Route::new("GET", "/v1/_routes", MODULE, || get(routes_view::<H>)),
        nous_routes::Route::new("GET", "/v1/openapi.json", MODULE, || get(openapi::<H>)),
        nous_routes::Route::new("GET", "/v1/plugins/{name}/readme", MODULE, || {
            get(readme::<H>)
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(name: &'static str, reason: Option<Refusal>) -> ManifestLine {
        ManifestLine {
            name,
            reason,
            layer: "app",
            kind: "编译期",
            required: false,
            crate_name: "`x`",
            description: "d",
            version: "0.1.0".to_string(),
            vendor: Some("Nous".to_string()),
            version_from_release: false,
        }
    }

    fn route(method: &'static str, path: &str, module: &'static str) -> Declaration {
        Declaration {
            method,
            path: path.to_string(),
            module,
        }
    }

    /// 清单开着、但一条端点都没注册 → `not_implemented`；有端点 → enabled。
    #[test]
    fn a_manifest_line_without_endpoints_is_not_implemented() {
        let manifest = vec![line("has", None), line("empty", None)];
        let registered = vec![route("GET", "/v1/x", "has")];
        let body = capabilities_body(&manifest, &registered, "http://x", "9.9.9");
        let modules = body["modules"].as_array().unwrap();
        assert_eq!(modules[0]["enabled"], json!(true));
        assert_eq!(modules[0]["endpoints"], json!(["GET /v1/x"]));
        assert_eq!(modules[1]["enabled"], json!(false));
        assert_eq!(modules[1]["reason"]["code"], json!("not_implemented"));
    }

    /// 清单关掉的模块即便有代码也不声明端点（判据：关掉就是关掉）。
    #[test]
    fn a_closed_manifest_line_hides_its_endpoints_even_if_registered() {
        let manifest = vec![line(
            "closed",
            Some(Refusal {
                code: "frozen",
                message: "冻结".to_string(),
            }),
        )];
        let registered = vec![route("GET", "/v1/x", "closed")];
        let caps = capabilities_body(&manifest, &registered, "http://x", "9.9.9");
        assert_eq!(caps["modules"][0]["enabled"], json!(false));
        assert!(caps["modules"][0].get("endpoints").is_none(), "关掉的模块不许带端点表");
        let view = routes_body(&manifest, &registered);
        assert_eq!(view["routes"], json!([]), "注册表视图也不许出现关掉的端点");
    }

    /// 三处视图同源：`_routes.routes` 与 enabled 模块的 `endpoints` 多重集相等。
    #[test]
    fn routes_view_and_capabilities_agree_on_the_same_registry() {
        let manifest = vec![line("a", None), line("b", None)];
        let registered = vec![
            route("GET", "/v1/a", "a"),
            route("POST", "/v1/b", "b"),
            route("GET", "/v1/b", "b"),
        ];
        let caps = capabilities_body(&manifest, &registered, "http://x", "9.9.9");
        let mut from_caps: Vec<String> = caps["modules"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|module| module["enabled"] == json!(true))
            .flat_map(|module| {
                module["endpoints"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|text| text.as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        let view = routes_body(&manifest, &registered);
        let mut from_view: Vec<String> = view["routes"]
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
        from_caps.sort();
        from_view.sort();
        assert_eq!(from_caps, from_view);
    }

    /// openapi 折叠：同一路径的多个方法进同一个 path 对象。
    #[test]
    fn openapi_folds_methods_under_one_path() {
        let manifest = vec![line("a", None)];
        let registered = vec![
            route("GET", "/v1/a", "a"),
            route("POST", "/v1/a", "a"),
        ];
        let body = openapi_body(&manifest, &registered, "9.9.9");
        assert_eq!(body["paths"]["/v1/a"]["get"]["operationId"], json!("GET /v1/a"));
        assert_eq!(body["paths"]["/v1/a"]["post"]["operationId"], json!("POST /v1/a"));
        assert_eq!(body["info"]["version"], json!("9.9.9"));
    }

    /// 回退不静默：版本取不到时带 `versionSource: "release"`。
    #[test]
    fn a_release_fallback_is_declared_not_silent() {
        let mut only = line("a", None);
        only.version_from_release = true;
        let body = capabilities_body(&[only], &[route("GET", "/v1/a", "a")], "http://x", "9.9.9");
        assert_eq!(body["modules"][0]["versionSource"], json!("release"));
    }
}
