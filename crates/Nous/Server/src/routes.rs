//! 路由：`wire.md` 的 `/v1/*`。
//!
//! 这一版落的六条（`#22` 票面）：
//!
//! - `GET  /v1/sessions` —— 会话树快照；
//! - `GET  /v1/sessions/:id/entries` —— 分页历史（不透明游标，上限 1000）；
//! - `POST /v1/sessions/:id/messages` —— 受理输入，202 `{submissionId, status}`；
//! - `GET  /v1/sessions/:id/events` —— SSE：先 `snapshot`，再合帧后的增量；
//! - `POST /v1/sessions/:id/abort` —— 200 `{status:"aborted"}`；
//! - `POST /v1/sessions/:id/fork` —— 201 `{id, at}`。
//!
//! 别的端点（`rename` / `archive` / `device` / `models` …）不在这里「顺手」加：每条都有
//! 自己的票面语义，先开票。加一个空壳 Handler 只会让客户端以为它通了。
//!
//! ## 错误形状只有一种
//!
//! 所有失败走 [`ServerError`]：`{ error: string }` + 语义状态码。路径里的 `:id` 用字符串
//! 接再自己 parse —— 让 axum 的 `Path<i64>` 拒掉会吐一段纯文本，破坏 `{error}`。

use std::convert::Infallible;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use nous_durable::storage::cursor::{Cursor, ScanOrder};
use nous_durable::storage::sessions::{ENTRIES_MAX_LIMIT};
use nous_sessions::turn::TurnRequest;
use nous_wire::{Generation, RemoteEvent};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::app::AppState;
use crate::coalesce::{self, Framing};
use crate::error::{Result, ServerError};
use nous_device::ModelsResponse;
use crate::records::CreateSessionRequest;
use crate::source::SessionSource;

/// 装配路由。`state` 里的 `source` 决定数据从哪来，`commands` 决定写往哪去，
/// `hub` 决定事件往哪扇。
pub fn router(state: Arc<AppState>) -> Router {
    let router = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/default", put(put_default))
        .route("/v1/catalog/refresh", post(catalog_refresh))
        .route("/v1/device", get(device))
        .route("/v1/capabilities", get(capabilities))
        // 插件人话 README：随产物发布到 `<产物目录>/docs/plugins/`，运行时读盘（不进二进制）。
        .route("/v1/plugins/{name}/readme", get(crate::plugin_docs::readme))
        .route("/v1/openapi.json", get(openapi))
        // #90：原先挂在 documents::attach 上的自省端点，下线后归主路由。
        .route("/v1/_routes", get(routes_view));
    // 资源仓管理面（技能 / 提示词 / 扩展的编辑页）：文件读写 + 新建 + 发布，翻译层在独立模块。
    // 模型入口：元素库里的约定组 → 编目投影（纯解析在 `element-models`）。
    // 插件自己的端点：`nous-routes` 的声明就是注册，注册表读的也是同一份。
    // `/v1/presets*` 的 32 条也走这条（#60 搬进 `Nous_PresetsAdmin`）：插件自带 handler，
    // 宿主只实现它的 `Host`（`plugins.rs`），未装配时插件自己回 409。
    let mut router = router;
    for route in crate::plugins::routes() {
        router = route.mount(router);
    }
    router.with_state(state.clone())
}

/// `GET /v1/openapi.json`：从注册表（静态表 ∪ 插件声明）生成的只读机读视图。
async fn openapi() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "openapi": "3.1.0",
        "info": { "title": "nous /v1", "version": env!("CARGO_PKG_VERSION") },
        "paths": crate::meta::openapi_paths(),
    }))
}

/// `GET /v1/_routes`：与 capabilities 同源的注册表视图（#47）。
async fn routes_view() -> axum::Json<serde_json::Value> {
    axum::Json(crate::meta::routes_view())
}

/// `GET /v1/sessions`：`{ items: [...] }`，新的在前（排序在 `nous-durable` 的投影里）。
async fn device(State(state): State<Arc<AppState>>) -> Json<Value> {
    let models = state.models();
    Json(crate::meta::device(&models))
}

/// `GET /v1/selection`：当前选中的会话，落库、重启后仍在。
async fn capabilities(State(state): State<Arc<AppState>>) -> Json<Value> {
    let kind = state.provider_kind();
    let configured = kind == "faux";
    let message = if configured {
        "假 provider，回复固定"
    } else {
        "没有配置真实 provider 凭证，发消息会 503"
    };
    let mut body = crate::meta::capabilities(state.origin());
    body["runtime"]["provider"] = json!({
        "configured": configured,
        "kind": kind,
        "message": message,
    });
    Json(body)
}

/// `POST /v1/catalog/refresh`：拉一次远端编目，成功写缓存并换入内存；失败留旧表带 error。
/// 响应形状 `{default, items, fetchedAt, error?}`：token 只经内存请求头，不进响应/日志。
async fn catalog_refresh(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    match nous_device::refresh_remote_catalog(state.catalog_env()).await {
        Ok(models) => {
            state.set_models(models.clone());
            let mut body = serde_json::to_value(&models).map_err(|error| ServerError::Internal(error.to_string()))?;
            body["fetchedAt"] = models.catalog.fetched_at.clone().map(Value::String).unwrap_or(Value::Null);
            Ok(Json(body))
        }
        Err(error) => {
            // 失败保留旧编目：内存里最近一次刷新的那份优先；否则从本地缓存/文件读回。
            let mut old = state.refreshed_models().unwrap_or_else(|| {
                nous_device::load_models(state.catalog_env()).unwrap_or_default()
            });
            old.catalog.error = Some(error);
            state.set_models(old.clone());
            let top_error = old.catalog.error.clone().unwrap_or_default();
            let mut body = serde_json::to_value(&old).map_err(|error| ServerError::Internal(error.to_string()))?;
            body["fetchedAt"] = old.catalog.fetched_at.clone().map(Value::String).unwrap_or(Value::Null);
            body["error"] = Value::String(top_error);
            Ok(Json(body))
        }
    }
}

/// `GET /v1/models`：默认模型 + 列表 + 来源诊断块。
///
/// 编目来源由 `nous-device` 收口：内联（`NOUS_MODELS`）> 文件 > 诚实空表；
/// faux 装配仍有自己的固定编目（它优先于文件，便于本地验收）。
/// #79：最近一次远端刷新成功的结果在内存里优先，失败不改这一份。
async fn models(State(state): State<Arc<AppState>>) -> Result<Json<ModelsResponse>> {
    // 装配注入的编目（faux 验收 / 测试快照）优先：展示读装配出来的那份，没有再读静态注入的。
    if state.injected_models() {
        if let Some(catalog) = state.assembled_models() {
            return Ok(Json(catalog));
        }
    }
    current_catalog(&state).map(Json).map_err(ServerError::Internal)
}

/// 「当前编目」只有这一个出处：`GET /v1/models` 和发消息的模型解析都读它，
/// 不许各读各的（线上因此出过 503：展示读了元素组，解析读的是空的文件来源）。
///
/// 顺序：注入的静态编目（测试快照 / faux 验收）→ 远端刷新 → 元素库的模型入口组 → device 文件/内联来源。
/// 注入时读 `state.models()`，与改动前发消息的行为一致；展示路由在调用前另有「装配编目优先」。
fn current_catalog(state: &Arc<AppState>) -> std::result::Result<ModelsResponse, String> {
    if state.injected_models() {
        return Ok(state.models());
    }
    if let Some(refreshed) = state.refreshed_models() {
        return Ok(refreshed);
    }
    if let Some(response) = crate::models::group_catalog(state) {
        return Ok(response);
    }
    nous_device::load_models(state.catalog_env())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefaultBody {
    provider: String,
    #[serde(rename = "modelId")]
    model_id: String,
}

/// `PUT /v1/default`：把默认模型原子写回同一份编目；模型不在 items 里 → 400。
async fn put_default(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DefaultBody>,
) -> Result<Json<Value>> {
    let env = state.catalog_env();
    if env.inline.is_some() {
        return Err(ServerError::BadRequest(
            "NOUS_MODELS 是内联编目，不能写回 default；请用 NOUS_MODELS_FILE".to_string(),
        ));
    }
    let path = &env.file_path;
    let model = nous_device::ModelRef {
        provider: body.provider,
        model_id: body.model_id,
    };
    nous_device::save_default(path, &model)
        .map_err(|error| ServerError::BadRequest(error))?;
    Ok(Json(json!({ "default": model, "wrote": true })))
}

