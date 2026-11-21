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
use std::path::PathBuf;
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

/// epoch 毫秒（fire 与观测的时钟入口；store 层零时钟，时间都由这里传入）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// 装配路由。`state` 里的 `source` 决定数据从哪来，`commands` 决定写往哪去，
/// `hub` 决定事件往哪扇。
pub fn router(state: Arc<AppState>) -> Router {
    let router = Router::new()
        .route("/v1/sessions", get(sessions).post(create_session))
        .route("/v1/sessions/{id}/entries", get(entries))
        .route("/v1/sessions/{id}/messages", post(messages))
        .route("/v1/sessions/{id}/events", get(events))
        .route("/v1/sessions/{id}/abort", post(abort))
        .route("/v1/sessions/{id}/fork", post(fork))
        .route("/v1/machines", get(machines))
        .route("/v1/fs/dirs", get(fs_dirs))
        .route("/v1/models", get(models))
        .route("/v1/default", put(put_default))
        .route("/v1/catalog/refresh", post(catalog_refresh))
        .route("/v1/device", get(device))
        .route("/v1/unread", get(unread))
        .route("/v1/origins", get(origins))
        .route("/v1/read/{id}", post(mark_read))
        .route("/v1/sessions/{id}/rename", post(rename))
        .route("/v1/sessions/{id}/pin", post(pin))
        .route("/v1/sessions/{id}/config", post(config))
        .route("/v1/sessions/{id}/purge", post(purge))
        .route("/v1/selection", get(selection).post(write_selection))
        .route("/v1/skills", get(skills))
        .route("/v1/prompts", get(prompts))
        .route("/v1/extensions", get(extensions))
        .route("/v1/search", get(search))
        .route("/v1/triggers", get(triggers))
        .route("/v1/triggers/{code}/fire", post(fire_trigger))
        .route("/v1/triggers/{code}/fires", get(trigger_fires))
        .route("/v1/capabilities", get(capabilities))
        .route("/v1/stats/usage", get(crate::stats::usage))
        .route("/v1/stats/tasks", get(crate::stats::tasks))
        .route("/v1/stats/db", get(crate::stats::db))
        .route("/v1/openapi.json", get(crate::stats::openapi));
    // #48 的 documents 路由与 _routes 自省挂在同一份 state 上；两条分支的端点都保留。
    let router = crate::documents::attach(state.clone(), router);
    // #60：presets 用独立的写者（私库根 + 插件库路径），未装配时 handler 诚实回 409。
    match state.presets() {
        Some(store) => crate::presets::attach(store.clone(), router),
        None => router,
    }
}

/// `GET /v1/machines`：机器编目（机器 + 注解 + 能力字段 + 兼容 target 列表）。
async fn machines() -> Json<Value> {
    let file = std::env::var("NOUS_MACHINES_FILE")
        .ok()
        .filter(|text| !text.trim().is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    Json(nous_device::machines(file.as_deref()))
}

/// `GET /v1/fs/dirs` 的查询参数。未知字段由 axum 忽略，协议保持哑的。
#[derive(Debug, Deserialize)]
struct FsDirsQuery {
    path: Option<String>,
    machine: Option<String>,
    hidden: Option<String>,
}

/// 目录浏览上限。超出的项不计入 `dirs`，只把 `truncated` 翻成 true。
const FS_DIRS_LIMIT: usize = 500;

/// `GET /v1/fs/dirs`：只读列子目录。
///
/// 路径规则与本机判定都在 `nous_env::dirs`；显式远端 `machine` 走 env-ssh，
/// 编目里没有 → 404，连不上 → 502，绝不静默退回本机。
async fn fs_dirs(Query(query): Query<FsDirsQuery>) -> Result<Json<Value>> {
    let hidden = query.hidden.as_deref() == Some("1");
    if let Some(machine) = query.machine.as_deref().filter(|name| !name.is_empty()) {
        if !nous_env::is_local_machine(Some(machine)) {
            let result = nous_env_ssh::remote_subdirs(
                machine,
                query.path.as_deref().unwrap_or(""),
                hidden,
                FS_DIRS_LIMIT,
            )
            .await
            .map_err(fs_remote_error)?;
            return Ok(Json(serde_json::to_value(result).unwrap_or(Value::Null)));
        }
    }
    let result = nous_env::list_subdirs(
        query.path.as_deref().unwrap_or(""),
        hidden,
        FS_DIRS_LIMIT,
    )
    .map_err(fs_dirs_error)?;
    Ok(Json(serde_json::to_value(result).unwrap_or(Value::Null)))
}

fn fs_remote_error(error: nous_env_ssh::RemoteDirsError) -> ServerError {
    use nous_env_ssh::RemoteDirsError;
    match error {
        RemoteDirsError::MachineNotFound => ServerError::NotFound("机器不在编目里".to_string()),
        RemoteDirsError::Unreachable(message) => ServerError::BadGateway(message),
        RemoteDirsError::RelativePath => {
            ServerError::BadRequest("path 必须是绝对路径".to_string())
        }
        RemoteDirsError::NotFound => ServerError::NotFound("远端目录不存在".to_string()),
        RemoteDirsError::PermissionDenied => {
            ServerError::Forbidden("没有权限读这个远端目录".to_string())
        }
        RemoteDirsError::NotDirectory => ServerError::BadRequest("path 不是目录".to_string()),
        RemoteDirsError::Other => ServerError::Internal("远端目录读不出来".to_string()),
    }
}

fn fs_dirs_error(error: nous_env::BrowseError) -> ServerError {
    use nous_env::BrowseError;
    match error {
        BrowseError::RelativePath => {
            ServerError::BadRequest("path 必须是绝对路径".to_string())
        }
        BrowseError::NotFound => ServerError::NotFound("目录不存在".to_string()),
        BrowseError::PermissionDenied => {
            ServerError::Forbidden("没有权限读这个目录".to_string())
        }
        BrowseError::NotDirectory => ServerError::BadRequest("path 不是目录".to_string()),
        BrowseError::Other => ServerError::Internal("目录读不出来".to_string()),
    }
}

/// `GET /v1/sessions`：`{ items: [...] }`，新的在前（排序在 `nous-durable` 的投影里）。
async fn sessions(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let items = state.source().sessions()?;
    Ok(Json(json!({ "items": items })))
}

/// `POST /v1/sessions`：建会话，201 带生效配置。字段形状由 `tests/wire_shapes.rs` 钉住。
async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let created = state.commands().create(body).await?;
    Ok((StatusCode::CREATED, Json(serde_json::to_value(created).unwrap())))
}

/// `GET /v1/unread`：平铺 map（`{"id": {unread, readAt}}`），覆盖所有会话。
async fn unread(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    Ok(Json(state.commands().unread().await?))
}

/// `GET /v1/origins`：会话血缘反查，`?trigger=<code>` 按触发器过滤。
/// 数据在 durable 的 session 作用域文档里（与 names / pins 同一份），只读。
async fn origins(State(state): State<Arc<AppState>>, Query(query): Query<OriginsQuery>) -> Result<Json<Value>> {
    // 没有 durable 写后端的只读装配：诚实回空表，不 409（与其它只读列表端点一致）。
    let Some(session) = state.durable_session() else {
        return Ok(Json(json!({ "items": [] })));
    };
    Ok(Json(nous_durable::session::commands::origins_map(
        &session,
        query.trigger.as_deref(),
    ).await.map_err(|error| ServerError::Internal(error.to_string()))?))
}

#[derive(Debug, Deserialize)]
struct OriginsQuery {
    trigger: Option<String>,
}

/// `POST /v1/read/:id`：把水位推到最新，回 `{id, readAt, unread: 0}`。
async fn mark_read(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let result = state.commands().read(id).await?;
    Ok(Json(json!({
        "id": result.id,
        "readAt": result.read_at,
        "unread": result.unread,
    })))
}

#[derive(Debug, Deserialize)]
struct RenameBody {
    name: String,
}

/// `POST /v1/sessions/:id/rename`：trim、空 → 400；回生效的 `{id, name}`。
async fn rename(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let body: RenameBody = if body.is_empty() {
        return Err(ServerError::BadRequest("name is required".to_string()));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| ServerError::BadRequest(format!("rename body: {error}")))?
    };
    let result = state.commands().rename(id, body.name).await?;
    Ok(Json(json!({ "id": result.id, "name": result.name })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinBody {
    pinned: bool,
    request_id: Option<String>,
}

/// `POST /v1/sessions/:id/pin`：置顶写穿。`requestId` 必填，重放回第一次的回执。
async fn pin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let body: PinBody = if body.is_empty() {
        return Err(ServerError::BadRequest(
            "pinned and requestId are required".to_string(),
        ));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| ServerError::BadRequest(format!("pin body: {error}")))?
    };
    let request_id = body
        .request_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ServerError::BadRequest("requestId is required".to_string()))?;
    let result = state.commands().pin(id, body.pinned, request_id).await?;
    Ok(Json(serde_json::to_value(result).map_err(|error| {
        ServerError::Internal(format!("serialize pin receipt: {error}"))
    })?))
}


/// `GET /v1/device`：进程/宿主配置，About 面板读它。
async fn device(State(state): State<Arc<AppState>>) -> Json<Value> {
    let models = state.models();
    Json(crate::meta::device(&models))
}

/// `GET /v1/selection`：当前选中的会话，落库、重启后仍在。
async fn selection(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let result = state.commands().selection().await?;
    Ok(Json(json!({ "id": result.id })))
}

/// `POST /v1/selection {id}`：写选中的会话；`{id:null}` 清空。
async fn write_selection(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let body: SelectionBody = if body.is_empty() {
        return Err(ServerError::BadRequest("id is required".to_string()));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| ServerError::BadRequest(format!("selection body: {error}")))?
    };
    let result = state.commands().select(body.id).await?;
    Ok(Json(json!({ "id": result.id })))
}

#[derive(Debug, Deserialize)]
struct SelectionBody {
    id: Option<i64>,
}

/// `POST /v1/sessions/{id}/config`：逐键覆盖，回生效值。
async fn config(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let patch: Value = if body.is_empty() {
        return Err(ServerError::BadRequest("config patch is required".to_string()));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| ServerError::BadRequest(format!("config body: {error}")))?
    };
    // 档位表来自 /v1/models 的编目，不是请求体：换模型时按新模型的表夹档位。
    let mut patch = patch;
    if let Some(model) = patch.get("model") {
        let wanted = model
            .get("provider")
            .and_then(Value::as_str)
            .zip(model.get("modelId").and_then(Value::as_str));
        if let Some((provider, model_id)) = wanted {
            if let Some(item) = state
                .assembled_models()
                .unwrap_or_else(|| state.models().clone())
                .items
                .iter()
                .find(|item| item.provider == provider && item.model_id == model_id)
            {
                patch["thinkingLevels"] = json!(item.thinking_levels);
            }
        }
    }
    let result = state.commands().configure(id, patch).await?;
    Ok(Json(result.value))
}

/// `POST /v1/sessions/{id}/purge`：真删 + 墓碑；第二次 409。
async fn purge(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let result = state.commands().purge(id).await?;
    Ok(Json(json!({
        "id": result.id,
        "purged": true,
        "at": result.at,
    })))
}

/// `GET /v1/prompts`：A 阶段空表。
async fn prompts() -> Json<Value> {
    Json(crate::meta::prompts())
}

/// `GET /v1/extensions`：ext 冻结，空清单。`?usage=1` 时回答「谁在用」（#85）。
async fn extensions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<Value>> {
    if query.requested() {
        return Ok(Json(state.source().usage("extensions")?));
    }
    Ok(Json(crate::meta::extensions()))
}

/// `GET /v1/skills`：空目录就是空表。`?usage=1` 时回答「谁在用」（#85）。
async fn skills(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<Value>> {
    if query.requested() {
        return Ok(Json(state.source().usage("skills")?));
    }
    Ok(Json(crate::meta::skills()))
}

/// `?usage=1`：反查模式开关。缺席 / 0 / false / 其它值都不是反查，
/// 不因为一个可选参数让整个端点 400（serde 的 `bool` 会把 `1` 判成非法）。
#[derive(Debug, Deserialize)]
struct UsageQuery {
    usage: Option<String>,
}

impl UsageQuery {
    fn requested(&self) -> bool {
        matches!(self.usage.as_deref(), Some("1" | "true"))
    }
}

/// `GET /v1/search?q=&kind=&limit=`（#85）：服务端扫一次会话正文 / 文档，
/// `q` 少于两个字符 → 400，`limit` 上限 100 且 `truncated` 明确。
#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    kind: Option<String>,
    limit: Option<usize>,
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>> {
    let q = query.q.unwrap_or_default().trim().to_string();
    if q.chars().count() < 2 {
        return Err(ServerError::BadRequest("q must be at least 2 characters".into()));
    }
    let kind = query.kind.as_deref().unwrap_or("all");
    if !matches!(kind, "all" | "sessions" | "documents") {
        return Err(ServerError::BadRequest("kind must be all/sessions/documents".into()));
    }
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let all = state.source().search(&q, kind)?;
    let total = all.len();
    let truncated = total > limit;
    let items: Vec<Value> = all
        .into_iter()
        .take(limit)
        .map(|item| serde_json::to_value(item).expect("SearchItem is serializable"))
        .collect();
    Ok(Json(json!({ "items": items, "total": total, "truncated": truncated })))
}

/// `GET /v1/triggers`：观测清单 + 调度健康（#58）。装配了 store 就读真行；
/// 空库仍回 `{triggers:[],root:null}`（#49 空表契约保留）。
///
/// body 与 CLI `nous-app --triggers` **同一个函数、同一份序列化**（#58 判据 4）。
async fn triggers(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let Some(runtime) = state.triggers() else {
        // 没装配不等于没形状：scheduler 一样在场，只是诚实回空（#58 的顶层带调度健康）。
        let mut report = crate::meta::triggers(Vec::new());
        report["scheduler"] = json!({
            "lastTickAt": Value::Null,
            "lagMs": 0,
            "tickMs": nous_store::SCHEDULER_TICK_MS,
        });
        return Ok(Json(report));
    };
    let store = runtime.store.lock().await;
    let report = crate::triggers::observation_report(
        &store,
        &runtime.definitions(),
        runtime.last_tick_at(),
        now_ms(),
    )?;
    Ok(Json(report))
}

/// `GET /v1/triggers/:code/fires`：这个触发器最近的账，时间倒序（#58）。
/// 不存在的触发器 → 404 + `{error}`（不许空 200），判据 6。
async fn trigger_fires(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    Query(limit): Query<FiresQuery>,
) -> Result<Json<Value>> {
    let runtime = match state.triggers() {
        Some(runtime) => runtime,
        None => return Err(ServerError::Conflict("triggers not configured".to_string())),
    };
    let store = runtime.store.lock().await;
    let history = crate::triggers::fire_history(&store, &runtime.definitions(), &code, limit.limit())?;
    Ok(Json(history))
}

/// `?limit=` 的查询参数：默认 50（票面值），上限 1000（`ENTRIES_MAX_LIMIT` 同一档）。
#[derive(Debug, Deserialize)]
struct FiresQuery {
    limit: Option<i64>,
}

impl FiresQuery {
    fn limit(&self) -> i64 {
        self.limit.unwrap_or(DEFAULT_FIRE_HISTORY_LIMIT).clamp(1, ENTRIES_MAX_LIMIT as i64)
    }
}

/// `?limit=` 的默认值与上限（票面 50；上限与 entries 同一档：`ENTRIES_MAX_LIMIT`）。
const DEFAULT_FIRE_HISTORY_LIMIT: i64 = 50;

/// `POST /v1/triggers/:code/fire`：手动点火。actor 从 token 派生（#50），现为 system。
async fn fire_trigger(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> std::result::Result<Json<Value>, ServerError> {
    let runtime = match state.triggers() {
        Some(runtime) => runtime,
        None => return Err(ServerError::Conflict("triggers not configured".to_string())),
    };
    let actor = nous_durable::actor::Actor::system("local");
    let now_ms = now_ms();
    let outcome = runtime.fire(&code, &actor, now_ms).await?;
    Ok(Json(serde_json::to_value(&outcome).unwrap_or(Value::Null)))
}

/// `GET /v1/capabilities`：desk 按 modules 显示/隐藏功能面。
/// #51 只改 `runtime.provider` 一段：kind 由装配决定，不另造一张能力表。
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
    // 装配注入的编目（faux 验收 / 测试快照）优先；否则走 device 的文件/内联来源。
    if state.injected_models() {
        if let Some(catalog) = state.assembled_models() {
            return Ok(Json(catalog));
        }
        return Ok(Json(state.models()));
    }
    if let Some(refreshed) = state.refreshed_models() {
        return Ok(Json(refreshed));
    }
    let response = nous_device::load_models(state.catalog_env())
        .map_err(ServerError::Internal)?;
    Ok(Json(response))
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

#[derive(Debug, Deserialize)]
struct EntriesQuery {
    limit: Option<String>,
    cursor: Option<String>,
    /// 按 entry 的 `kind` 过滤（审计可见性靠它，不加新端点，spec 8.5）。
    kind: Option<String>,
    /// `system=1` 时额外回 `items[].delta` 与回放后的 `system`，且不过滤 `pi.system`。
    system: Option<String>,
}

/// `GET /v1/sessions/:id/entries`：`{ items, next }`，新的在前。
///
/// `next` 是**不透明**游标（`d<base36>`），客户端只许原样回传。`limit` 超 1000 **截断不报错**；
/// 0 / 非数字 / 伪造的游标才是 400。
async fn entries(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<EntriesQuery>,
) -> Result<Json<Value>> {
    let session = parse_session(&id)?;
    let limit = match query.limit.as_deref() {
        None => ENTRIES_MAX_LIMIT,
        Some(text) => text
            .parse::<usize>()
            .map_err(|_| ServerError::BadRequest(format!("limit is not a number: {text:?}")))?,
    };
    if limit == 0 {
        return Err(ServerError::BadRequest(
            "limit must be at least 1".to_string(),
        ));
    }
    let limit = limit.min(ENTRIES_MAX_LIMIT);
    let cursor = match query.cursor.as_deref() {
        None => None,
        Some(text) => {
            let cursor = Cursor::decode(text)
                .map_err(|_| ServerError::BadRequest(format!("invalid cursor: {text:?}")))?;
            if cursor.order != ScanOrder::Descending {
                return Err(ServerError::BadRequest(
                    "cursor was issued for another order".to_string(),
                ));
            }
            Some(cursor)
        }
    };
    if !state.source().exists(session)? {
        return Err(ServerError::NotFound(format!(
            "session {session} not found"
        )));
    }
    let (mut items, next) = state
        .source()
        .entries_page(session, limit, cursor.as_ref())?;
    if let Some(kind) = query.kind.as_deref() {
        items.retain(|entry| entry.get("kind").and_then(Value::as_str) == Some(kind));
    }
    let with_system = query.system.as_deref() == Some("1");
    if with_system {
        for item in &mut items {
            if item.get("kind").and_then(Value::as_str) == Some("pi.system") {
                item["delta"] = json!(system_entry_text(item));
            }
        }
    } else {
        items.retain(|entry| entry.get("kind").and_then(Value::as_str) != Some("pi.system"));
    }
    let mut response = json!({
        "items": items,
        "next": next.map(|cursor| cursor.encode()),
    });
    if with_system {
        // 回放：按 entry id 升序遍历全部 pi.system，最后一条生效的提示词为当前 system。
        let mut all = state.source().entries(session)?;
        all.sort_by_key(|entry| entry.get("id").and_then(Value::as_i64).unwrap_or(0));
        let effective = all
            .iter()
            .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("pi.system"))
            .filter_map(system_entry_text)
            .last()
            .unwrap_or_default();
        response["system"] = json!(effective);
    }
    Ok(Json(response))
}

/// 一条 `pi.system` entry 自己带来的提示词文本。`model[]` 里 system 消息的 content
/// 可能是字符串，也可能是 `[{type:"text",text}]` 块。
fn system_entry_text(entry: &Value) -> Option<String> {
    let message = entry
        .get("model")?
        .as_array()?
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("system"))?;
    let content = message.get("content")?;
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            Some(text)
        }
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitBody {
    content: Value,
    #[serde(default)]
    request_id: Option<String>,
    /// #70：请求体显式指定的模型（优先级最高）。
    #[serde(default)]
    model: Option<nous_device::ModelRef>,
    /// `0`/省略 = 立即 202；`>0` = 最多等这么久，拿到答案回 200，超时回 202。
    #[serde(default)]
    wait_ms: Option<Value>,
    /// `queue`（默认）/ `interrupt`。忙时排队或先停当前轮次。
    #[serde(default)]
    when_busy: Option<Value>,
}

/// 把会话 template 的正文写进一条 `pi.system` entry，并回一条 message 给本轮 provider。
///
/// 正文进 `sections.prompts`（不是 `content`）：#74 的 `?system=1` 回放按 sections 合并，
/// 后续系统消息能按名字覆盖这一段；名字本身就是诊断位（`template: missing`）。
async fn inject_prompt_section(
    state: &Arc<AppState>,
    session_id: i64,
    submitted: &crate::commands::SubmitResult,
) -> Option<nous_ai::Message> {
    let template_name = state.commands().template(session_id).await.ok().flatten()?;
    let prompts_root = nous_env::PROMPTS.resolve();
    let (text, actor) = if let Some(template) = nous_prompts::read_template(&prompts_root, &template_name) {
        (nous_prompts::render(&template.body, &Default::default()), "prompts".to_string())
    } else {
        (format!("template: missing ({template_name})"), "prompts".to_string())
    };
    let message_value = serde_json::to_value(nous_ai::transform::system::section_patch(
        &[("prompts", Some(text.as_str()))], 0,
    )).ok()?;
    let durable = state.durable_session()?;
    let entry = serde_json::json!({
        "id": submitted.entry_id + 4,
        "conversationId": session_id,
        "kind": "pi.system",
        "model": [message_value],
        "actor": { "kind": "system", "id": actor },
    });
    let _ = durable.commit(
        vec![nous_durable::StorageWrite::Entry(entry)],
        nous_durable::session::CommitOptions::default(),
    ).await;
    Some(nous_ai::Message::System(Box::new(
        nous_ai::transform::system::section_patch(&[("prompts", Some(text.as_str()))], 0),
    )))
}

/// `POST /v1/sessions/:id/messages`：不传 `waitMs` 时立即 202。
///
/// `requestId` 是幂等锚：同一个值打两次返回同一个 `submissionId`，库里也只多一条 submission
/// （判据在 `tests/commands.rs` 与 `tests/endpoints.rs`）。
async fn messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SubmitBody>,
) -> Result<impl IntoResponse> {
    let session = parse_session(&id)?;
    let wait_ms = match &body.wait_ms {
        None | Some(Value::Null) => 0u64,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| ServerError::BadRequest("waitMs must be a non-negative integer".to_string()))?,
    };
    let when_busy = match &body.when_busy {
        None | Some(Value::Null) => "queue",
        Some(Value::String(value)) if value == "queue" || value == "interrupt" => value.as_str(),
        _ => {
            return Err(ServerError::BadRequest(
                "whenBusy must be queue or interrupt".to_string(),
            ))
        }
    };
    if when_busy == "interrupt" && state.commands().busy(session).await? {
        // #70：先停当前轮次（abort 会 cancel 同一个 token、写 user_cancel 终态），
        // 再受理新的一条。停不掉就报错，不许「假装打断」。
        state.commands().abort(session).await?;
    }
    let faux = state.provider_kind() == "faux";
    // #70：模型解析在**受理之前**。没有来源就 503，不许先 202 再沉默。
    let catalog = if state.injected_models() {
        state.models().clone()
    } else {
        nous_device::load_models(state.catalog_env()).map_err(ServerError::ServiceUnavailable)?
    };
    let session_model = state
        .source()
        .sessions()?
        .into_iter()
        .find(|item| item.id == session)
        .and_then(|item| item.model);
    let reference = crate::runtime::resolve_model_ref(
        body.model.as_ref(),
        session_model.as_deref(),
        &catalog,
    )
    .ok_or_else(|| ServerError::ServiceUnavailable("没有可用模型".to_string()))?;
    let result = state
        .commands()
        .submit(session, body.content, body.request_id)
        .await?;

    // #70：受理后一律起一轮。faux 只决定模型来源，不决定「跑不跑」。
    // 事件只用 wire 已有的类型：user entry 的 message_start/entry_appended 由
    // durable 的 CommitObserver → project 投影发出，assistant 由轮次的投影发；
    // 这里不再手工补帧（之前补的假 entry 是重复且形状不对的）。
    let completion_rx = if wait_ms > 0 {
        state.runtime().map(|runtime| runtime.completions.subscribe())
    } else {
        None
    };
    if let Some(runtime) = state.runtime() {
        let submitted = result.clone();
        let driver = runtime.driver.clone();
        let completions = runtime.completions.clone();
        let commands = state.commands().clone();
        // #51：faux 验收模式没显式指定模型时仍用固定 echo 模型；否则用解析出的引用构造。
        let model = if faux && body.model.is_none() {
            nous_ai::Model::new(
                crate::runtime::FAUX_MODEL_ID,
                "Faux Echo",
                nous_ai::Api::Faux,
                "faux",
                "http://localhost:0",
            )
        } else {
            crate::runtime::model_from_ref(&reference)
        };
        // #72：会话 template 的单值投影，在起轮前落一条 pi.system（与 #74 回放同一条线）。
        // 缺模板只记诊断，不阻断轮次。注入失败也照跑（诊断可查，不让用户卡住）。
        let system_message = inject_prompt_section(&state, session, &submitted).await;
        let mut turn_context = nous_ai::Context::new(Vec::new());
        if let Some(message) = system_message {
            turn_context.messages.push(message);
        }
        tokio::spawn(async move {
            let outcome = driver
                .run(TurnRequest {
                    conversation: session,
                    submission: submitted.submission_id,
                    entry_id: submitted.assistant_entry_id,
                    live: submitted.live_id,
                    model,
                    context: turn_context,
                    attempt: 1,
                })
                .await;
            match outcome {
                Ok(outcome) => {
                    let _ = completions.send((submitted.submission_id, outcome.entry));
                }
                Err(error) => {
                    // #70：失败也必须收敛 submission，不能停在 queued 假装受理。
                    eprintln!("nous-server: 轮次失败：{error}");
                    if let Err(fail_error) = commands
                        .fail_submission(session, submitted.submission_id, "model_error")
                        .await
                    {
                        eprintln!("nous-server: submission 收敛失败：{fail_error}");
                    }
                }
            }
        });
    }

    if wait_ms > 0 {
        if let Some(mut rx) = completion_rx {
            let deadline = tokio::time::Duration::from_millis(wait_ms);
            loop {
                match tokio::time::timeout(deadline, rx.recv()).await {
                    Ok(Ok((id, answer))) if id == result.submission_id => {
                        return Ok((StatusCode::OK, Json(answer)));
                    }
                    Ok(Ok(_)) => continue,
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Closed))
                    | Err(_) => break,
                }
            }
        }
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "submissionId": result.submission_id,
                "status": "running",
            })),
        ));
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "submissionId": result.submission_id,
            "status": result.status,
        })),
    ))
}

/// `POST /v1/sessions/:id/abort`：没有活跃任务时 409（不是 200「假装停了」）。
async fn abort(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<Value>> {
    let session = parse_session(&id)?;
    state.commands().abort(session).await?;
    Ok(Json(json!({ "status": "aborted" })))
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ForkBody {
    #[serde(default)]
    entry_id: Option<i64>,
    #[serde(default)]
    name: Option<String>,
}

/// `POST /v1/sessions/:id/fork`：201 `{ id, at }`。body 可省（= 从最后一条分叉、不带名字）。
async fn fork(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse> {
    let session = parse_session(&id)?;
    // 空 body 与 `{}` 等价（`wire.md`：`entryId` / `name` 都可省）。用 `Bytes` 自己解，
    // 因为 `Json<T>` 会把空 body 当成 400 —— 那是提取器的口味，不是契约。
    let body: ForkBody = if body.is_empty() {
        ForkBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| ServerError::BadRequest(format!("fork body: {error}")))?
    };
    let result = state
        .commands()
        .fork(session, body.entry_id, body.name)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": result.id, "at": result.at })),
    ))
}

/// 每条 SSE 连接的未发送字节硬顶（spec ch6 建议 1MB，本实现把它钉死为硬顶的一部分）。
/// 它和 `ext-sdk` 的单帧上限同数，但两者是**独立上限、各自生效**：帧层管单条 RPC 帧，
/// 这里管一条 SSE 连接的全部未发送字节；谁都不许调另一个的数。超了就断这条连接，
/// 让客户端重连拿新 snapshot。不是帧数：一条大帧自己就能把内存打穿。
const SSE_UNSENT_LIMIT: usize = 1024 * 1024;

/// `GET /v1/sessions/:id/events` —— SSE。
///
/// 顺序是契约：**先 `snapshot`**，再增量。订阅在算快照**之前**做：否则算快照那几毫秒里
/// 发出的帧会掉在地上，客户端要等到下一次生成才知道。
async fn events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    let id = parse_session(&id)?;
    let source = state.source().clone();
    if !source.exists(id)? {
        return Err(ServerError::NotFound(format!("session {id} not found")));
    }
    let subscription = state.hub().subscribe(id);
    let snapshot = snapshot(&*source, id)?;

    let (raw_tx, raw_rx) = mpsc::channel(256);
    tokio::spawn(forward(subscription, raw_tx));
    let (framed_tx, framed_rx) = mpsc::channel(64);
    coalesce::spawn(Framing::Windowed, raw_rx, framed_tx);

    // 背压计的是**未发送字节**：生产端把帧编码进队列时记账，消费端取走后销账。
    // 快读客户端的 outstanding 始终接近 0，不会被累计发送量误伤。
    let outstanding = Arc::new(AtomicUsize::new(0));
    let (encoded_tx, encoded_rx) = mpsc::channel::<PendingFrame>(8);
    tokio::spawn(encode_with_backpressure(
        id,
        framed_rx,
        encoded_tx,
        outstanding.clone(),
    ));

    let Some(snapshot_data) = encode_data(&snapshot, SSE_UNSENT_LIMIT) else {
        return Err(ServerError::Internal(
            "snapshot frame exceeds SSE limit".into(),
        ));
    };
    let first = futures::stream::once(async move { Ok(Event::default().data(snapshot_data)) });
    let rest = futures::stream::unfold(
        (encoded_rx, outstanding, None::<UnsentGuard>),
        |(mut rx, outstanding, previous)| async move {
            // 上一轮的守卫在这一 poll 开始时被 drop（销账）；即便上次返回的帧
            // 从未交给 socket，也会被回收。
            drop(previous);
            let Some(frame) = rx.recv().await else {
                return None;
            };
            // 销账交给守卫的 `Drop`，不在这一行手工 `fetch_sub`：客户端断开会取消
            // 这个 future，手工那行就不会跑，配额永久泄漏。守卫跟着帧一起生命周期。
            let PendingFrame { data, guard } = frame;
            Some((
                Ok(Event::default().data(data)),
                (rx, outstanding, Some(guard)),
            ))
        },
    );
    // 心跳：`wire.md` 没规定间隔，15 s 足够穿过常见反代的 60 s 空闲超时。
    Ok(Sse::new(first.chain(rest)).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// `wire.md` 的帧形状是**数据里带 `type`**，不是 SSE 的 `event:` 名 —— 这里只编码
/// `data:` 负载。
///
/// 序列化直接写进**受预算约束的 writer**：超限就中止，不先生成完整字符串再检查。
/// "拒绝发生在分配之前" 的判据就在这一层 —— 不会为了丢掉一帧而先分配它的全尺寸副本。
fn encode_data(event: &RemoteEvent, limit: usize) -> Option<String> {
    let mut writer = LimitedWriter {
        buf: Vec::new(),
        limit,
    };
    // `RemoteEvent` 全是 `Serialize`；序列化失败只可能是 `Unknown`（不产出）。
    serde_json::to_writer(&mut writer, event).ok()?;
    String::from_utf8(writer.buf).ok()
}

/// 有硬上限的 `Write`：写满 `limit` 字节后立刻报错，不再延长缓冲区。
struct LimitedWriter {
    buf: Vec<u8>,
    limit: usize,
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.buf.len() + bytes.len() > self.limit {
            return Err(io::Error::other("SSE frame exceeds unsent byte limit"));
        }
        self.buf.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 一条已编码、尚未交给客户端的 SSE 帧。`guard` 在帧被消费/丢弃时销账。
struct PendingFrame {
    data: String,
    guard: UnsentGuard,
}

/// 未发送字节的 RAII 销账守卫。`Drop` 一定跑：正常消费、future 被取消、客户端断开
/// 都走同一条路。没有它，取消一次连接就永久泄漏那几字节配额。
struct UnsentGuard {
    outstanding: Arc<AtomicUsize>,
    bytes: usize,
}

impl Drop for UnsentGuard {
    fn drop(&mut self) {
        self.outstanding.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

/// 把合帧后的事件编码入队，并维护「未发送字节」。
///
/// 检查在 `send().await` **之前**：撞顶时不再把帧交给队列，任务退出、下游流关闭，
/// 客户端重连拿 `snapshot`。这是 `Lagged` 那条断流路径在字节维度的对应物。
async fn encode_with_backpressure(
    session_id: i64,
    mut framed: mpsc::Receiver<RemoteEvent>,
    tx: mpsc::Sender<PendingFrame>,
    outstanding: Arc<AtomicUsize>,
) {
    while let Some(event) = framed.recv().await {
        let Some(data) = encode_data(&event, SSE_UNSENT_LIMIT) else {
            return;
        };
        let bytes = data.len();
        let queued = outstanding.load(Ordering::Relaxed) + bytes;
        if queued > SSE_UNSENT_LIMIT {
            // 协议哑：不加事件类型，留一行服务端日志给事后排查。
            eprintln!(
                "nous-server: sse backpressure: session={session_id} bytes={queued} reason=backpressure"
            );
            return;
        }
        outstanding.fetch_add(bytes, Ordering::Relaxed);
        let guard = UnsentGuard {
            outstanding: outstanding.clone(),
            bytes,
        };
        if tx.send(PendingFrame { data, guard }).await.is_err() {
            return;
        }
    }
}

/// `snapshot` 帧：会话 + 正在生成的消息 + entry 历史（`entries` 是唯一的时间源）。
fn snapshot(source: &dyn SessionSource, id: i64) -> Result<RemoteEvent> {
    let session = source
        .sessions()?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| ServerError::NotFound(format!("session {id} not found")))?;
    Ok(RemoteEvent::Snapshot {
        session: Some(
            serde_json::to_value(&session)
                .map_err(|error| ServerError::Internal(format!("serialize session: {error}")))?,
        ),
        generation: source.live_message(id)?.map(|message| Generation {
            message: Some(message),
        }),
        entries: source.entries(id)?,
    })
}

/// 广播 → mpsc。`Lagged` 表示客户端落后超过缓冲：补丁流已断，**断开连接**让客户端
/// 重连拿新 `snapshot`。不补旧帧（补丁流不是可独立应用的快照）。
async fn forward(
    mut subscription: broadcast::Receiver<RemoteEvent>,
    tx: mpsc::Sender<RemoteEvent>,
) {
    loop {
        match subscription.recv().await {
            Ok(event) => {
                if tx.send(event).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => return,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

fn parse_session(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .map_err(|_| ServerError::BadRequest(format!("session id is not a number: {raw:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 超限帧在序列化途中就中止：没有先分配它的全尺寸副本。
    #[test]
    fn an_oversized_frame_is_refused_before_it_is_fully_materialized() {
        let event = RemoteEvent::EntryAppended {
            entry: json!({ "blob": "x".repeat(4 * SSE_UNSENT_LIMIT) }),
        };
        let mut writer = LimitedWriter {
            buf: Vec::new(),
            limit: SSE_UNSENT_LIMIT,
        };
        let error = serde_json::to_writer(&mut writer, &event).unwrap_err();
        assert!(error.is_io(), "应当是 writer 在超限处中止：{error}");
        assert!(
            writer.buf.len() <= SSE_UNSENT_LIMIT,
            "中止时缓冲区不该膨胀到全尺寸：{}",
            writer.buf.len()
        );
        assert!(encode_data(&event, SSE_UNSENT_LIMIT).is_none());
    }
}
