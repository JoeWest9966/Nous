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
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use nous_durable::storage::cursor::{Cursor, ScanOrder};
use nous_durable::storage::sessions::ENTRIES_MAX_LIMIT;
use nous_wire::{Generation, RemoteEvent};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::app::AppState;
use crate::coalesce::{self, Framing};
use crate::error::{Result, ServerError};
use crate::models::ModelsResponse;
use crate::records::CreateSessionRequest;
use crate::source::SessionSource;

/// 装配路由。`state` 里的 `source` 决定数据从哪来，`commands` 决定写往哪去，
/// `hub` 决定事件往哪扇。
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/sessions", get(sessions).post(create_session))
        .route("/v1/sessions/{id}/entries", get(entries))
        .route("/v1/sessions/{id}/messages", post(messages))
        .route("/v1/sessions/{id}/events", get(events))
        .route("/v1/sessions/{id}/abort", post(abort))
        .route("/v1/sessions/{id}/fork", post(fork))
        .route("/v1/machines", get(machines))
        .route("/v1/models", get(models))
        .with_state(state)
}

/// `GET /v1/machines`：机器编目（机器 + 注解 + 能力字段 + 兼容 target 列表）。
async fn machines() -> Json<Value> {
    Json(crate::machines::catalog())
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

/// `GET /v1/models`：默认模型 + 列表。字段形状由 `tests/wire_shapes.rs` 钉住。
///
/// 本层不认识 provider，空目录是诚实的「不知道」；宿主可换入带 catalog 的 `AppState`。
async fn models(State(state): State<Arc<AppState>>) -> Json<ModelsResponse> {
    Json(state.models().clone())
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
        return Err(ServerError::BadRequest("limit must be at least 1".to_string()));
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
        return Err(ServerError::NotFound(format!("session {session} not found")));
    }
    let (mut items, next) = state.source().entries_page(session, limit, cursor.as_ref())?;
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
    let result = state
        .commands()
        .submit(session, body.content, body.request_id)
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "submissionId": result.submission_id,
            "status": result.status,
        })),
    ))
}

/// `POST /v1/sessions/:id/abort`：没有活跃任务时 409（不是 200「假装停了」）。
async fn abort(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
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

    let first = futures::stream::once(async move { Ok(frame(snapshot)) });
    let rest = futures::stream::unfold(framed_rx, |mut framed| async move {
        framed.recv().await.map(|event| (Ok(frame(event)), framed))
    });
    // 心跳：`wire.md` 没规定间隔，15 s 足够穿过常见反代的 60 s 空闲超时。
    Ok(Sse::new(first.chain(rest))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// 一帧 SSE。`wire.md` 的形状是**数据里带 `type`**，不是 SSE 的 `event:` 名 ——
/// 所以这里不设事件名，只写 `data:`。
fn frame(event: RemoteEvent) -> Event {
    // `RemoteEvent` 全是 `Serialize`；序列化失败只可能是 `Unknown`（不产出）。
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
    Event::default().data(data)
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
async fn forward(mut subscription: broadcast::Receiver<RemoteEvent>, tx: mpsc::Sender<RemoteEvent>) {
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
