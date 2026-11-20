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
        .route("/v1/models", get(models))
        .route("/v1/default", put(put_default))
        .route("/v1/device", get(device))
        .route("/v1/selection", get(selection))
        .route("/v1/unread", get(unread))
        .route("/v1/read/{id}", post(mark_read))
        .route("/v1/sessions/{id}/rename", post(rename))
        .route("/v1/sessions/{id}/pin", post(pin))
        .route("/v1/skills", get(skills))
        .route("/v1/prompts", get(prompts))
        .route("/v1/extensions", get(extensions))
        .route("/v1/triggers", get(triggers))
        .route("/v1/triggers/{code}/fire", post(fire_trigger))
        .route("/v1/triggers/{code}/fires", get(trigger_fires))
        .route("/v1/capabilities", get(capabilities));
    // #48 的 documents 路由与 _routes 自省挂在同一份 state 上；两条分支的端点都保留。
    crate::documents::attach(state, router)
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
    Json(crate::meta::device(state.models()))
}

/// `GET /v1/selection`：当前选中的会话（配置里没有就 null）。
async fn selection() -> Json<Value> {
    Json(crate::meta::selection())
}

/// `GET /v1/skills`：空目录就是空表。
async fn skills() -> Json<Value> {
    Json(crate::meta::skills())
}

/// `GET /v1/prompts`：A 阶段空表。
async fn prompts() -> Json<Value> {
    Json(crate::meta::prompts())
}

/// `GET /v1/extensions`：ext 冻结，空清单。
async fn extensions() -> Json<Value> {
    Json(crate::meta::extensions())
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

/// `GET /v1/models`：默认模型 + 列表 + 来源诊断块。
///
/// 编目来源由 `nous-device` 收口：内联（`NOUS_MODELS`）> 文件 > 诚实空表；
/// faux 装配仍有自己的固定编目（它优先于文件，便于本地验收）。
async fn models(State(state): State<Arc<AppState>>) -> Result<Json<ModelsResponse>> {
    // 装配注入的编目（faux 验收 / 测试快照）优先；否则走 device 的文件/内联来源。
    let injected = state.assembled_models();
    let response = match injected {
        Some(catalog) => catalog, // faux 验收 / 显式注入的编目
        None if state.models().items.is_empty() && state.models().default.is_none() => {
            nous_device::load_models(state.catalog_env()).map_err(ServerError::Internal)?
        }
        None => state.models().clone(),
    };
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
    Ok(Json(json!({
        "items": items,
        "next": next.map(|cursor| cursor.encode()),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitBody {
    content: Value,
    #[serde(default)]
    request_id: Option<String>,
    /// 只要出现就拒绝：本库只实现「立即 202 + 靠 SSE 收内容」这条路径。
    #[serde(default)]
    wait_ms: Option<Value>,
    /// 受理时忙不忙的处理。当前只有一种行为（照样排队），所以显式收下、显式忽略。
    #[serde(default)]
    when_busy: Option<Value>,
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
    if body.wait_ms.is_some() {
        return Err(ServerError::BadRequest(
            "waitMs is not supported; listen on /events for the answer".to_string(),
        ));
    }
    let _ = body.when_busy;
    let faux = state.provider_kind() == "faux";
    let result = state
        .commands()
        .submit(session, body.content, body.request_id)
        .await?;

    // #51：只有显式 faux 装配才在受理后跑一轮。真 provider 的解析/503 语义不在本票改；
    // 没有装配时保持原行为（只受理）。
    // 事件只用 wire 已有的类型：user entry 的 message_start/entry_appended 由
    // durable 的 CommitObserver → project 投影发出，assistant 由轮次的投影发；
    // 这里不再手工补帧（之前补的假 entry 是重复且形状不对的）。
    if faux {
        if let Some(runtime) = state.runtime() {
            let submitted = result.clone();
            let driver = runtime.driver.clone();
            let model = nous_ai::Model::new(
                crate::runtime::FAUX_MODEL_ID,
                "Faux Echo",
                nous_ai::Api::Faux,
                "faux",
                "http://localhost:0",
            );
            tokio::spawn(async move {
                let _ = driver
                    .run(TurnRequest {
                        conversation: session,
                        submission: submitted.submission_id,
                        entry_id: submitted.assistant_entry_id,
                        live: submitted.live_id,
                        model,
                        context: nous_ai::Context::new(Vec::new()),
                        attempt: 1,
                    })
                    .await;
            });
        }
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
