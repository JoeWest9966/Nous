//! 路由：`wire.md` 的 `/v1/*`。
//!
//! 目前落两条（`#22` 的验收顺序就是这两条）：
//!
//! - `GET /v1/sessions` —— 会话树快照；
//! - `GET /v1/sessions/:id/events` —— SSE：先 `snapshot`，然后合帧后的增量。
//!
//! 别的端点（`entries` / `messages` / `abort` / `device` / `archive` …）不在这里「顺手」
//! 加：每条都有自己的票面语义，先开票。加一个空壳 Handler 只会让客户端以为它通了。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use futures::Stream;
use nous_wire::{Generation, RemoteEvent};
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

use crate::app::AppState;
use crate::coalesce::{self, Framing};
use crate::error::{Result, ServerError};
use crate::source::SessionSource;

/// 装配路由。`state` 里的 `source` 决定数据从哪来，`hub` 决定事件往哪扇。
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/sessions", get(sessions))
        .route("/v1/sessions/{id}/events", get(events))
        .with_state(state)
}

/// `GET /v1/sessions`：`{ items: [...] }`，新的在前（排序在 `nous-durable` 的投影里）。
async fn sessions(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>> {
    let items = state.source().sessions()?;
    Ok(Json(json!({ "items": items })))
}

/// `GET /v1/sessions/:id/events` —— SSE。
///
/// 顺序是契约：**先 `snapshot`**，再增量。订阅在算快照**之前**做：否则算快照那几毫秒里
/// 发出的帧会掉在地上，客户端要等到下一次生成才知道。
async fn events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
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
    Ok(Sse::new(futures::StreamExt::chain(first, rest))
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
