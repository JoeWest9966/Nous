//! `sessions` 模块自己的端点：`/v1/sessions*` 那 17 条（#22 / #70 / #61 / #85）。
//!
//! handler 住在插件里（Magento 2 那种「模块自带 controller」）。插件不认识
//! `server` 的 `AppState`，只声明它需要什么（[`Host`]），由宿主实现并注入。
//!
//! 这 17 条分四种接缝，全部落在本 crate 已经拥有（或已下沉）的三层上：
//!
//! - **读投影**（`sessions` / `unread` / `selection` / `search` / `origins` / `entries`）：
//!   走 [`Host::source`]；`entries` 的游标编解码在 `nous-durable`，这里只做参数校验。
//! - **写命令**（`create` / `rename` / `pin` / `config` / `purge` / `abort` / `fork` /
//!   `messages` / `read` / `write_selection`）：走 [`Host::commands`]；`messages` 是唯一
//!   会 `wait` 的一条，`waitMs` 的语义在 `CommandSource::submit` 里。
//! - **SSE**（`events`）：走 [`Host::hub`]。顺序是契约：**先 `snapshot`，再增量**，
//!   订阅在算快照**之前**做 —— 否则算快照那几毫秒里发出的帧会掉在地上。
//! - **血缘反查**（`origins`）：走 [`Host::durable_session`]；没有 durable 写后端时
//!   诚实回空表，不 409（与其它只读列表端点一致）。
//!
//! ## 错误形状只有一种
//!
//! 所有失败走 [`crate::SessionError`]：`{ error: string }` + 语义状态码。路径里的 `:id`
//! 用字符串接再自己 parse —— 让 axum 的 `Path<i64>` 拒掉会吐一段纯文本，破坏 `{error}`。

use std::convert::Infallible;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use futures::{Stream, StreamExt};
use nous_durable::session::Session;
use nous_durable::storage::cursor::{Cursor, ScanOrder};
use nous_durable::storage::sessions::ENTRIES_MAX_LIMIT;
use nous_wire::{Generation, RemoteEvent};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::coalesce::{self, Framing};
use crate::commands::{CommandSource, SubmitResult};
use crate::error::{Result, SessionError};
use crate::hub::Hub;
use crate::records::CreateSessionRequest;
use crate::source::SessionSource;

/// 模块名（规则 12 的全名是小写 `sessions`）。
///
/// **17 条端点里 16 条在这里，`POST /v1/sessions/{id}/messages` 留在 `server`** ——
/// 它要的是宿主组装根（`AssembledRuntime` 的 driver / completions）与提示词注入
/// （`nous-prompts` / skills 库），都是 app 层的装配物，不是会话语义。它注册时仍报
/// `MODULE`，所以 `/v1/capabilities` 上 sessions 还是 17 条。
pub const MODULE: &str = "sessions";

/// 宿主能力：这个插件只认识四件事。
pub trait Host {
    /// 读投影（会话树 / entry 历史 / 搜索 / 选中态 / 引用反查）。
    fn source(&self) -> Arc<dyn SessionSource>;

    /// 写命令（建会话 / 提交 / 中止 / 分叉 / 改名 / 置顶 / 配置 / 清理 / 已读）。
    fn commands(&self) -> Arc<dyn CommandSource>;

    /// 血缘反查用的活库句柄。只读装配没有，`None` 就诚实回空表。
    fn durable_session(&self) -> Option<Arc<Session>>;

    /// SSE 扇出通道。
    fn hub(&self) -> Arc<Hub>;

    /// 当前编目（`/v1/models` 那份）。`config` 换模型时按新模型的表夹档位。
    fn models(&self) -> nous_device::ModelsResponse;

    /// 运行时装配好的编目（有 provider 注入时优先）；`None` 就退回 [`Host::models`]。
    fn assembled_models(&self) -> Option<nous_device::ModelsResponse>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn source(&self) -> Arc<dyn SessionSource> {
        (**self).source()
    }
    fn commands(&self) -> Arc<dyn CommandSource> {
        (**self).commands()
    }
    fn durable_session(&self) -> Option<Arc<Session>> {
        (**self).durable_session()
    }
    fn hub(&self) -> Arc<Hub> {
        (**self).hub()
    }
    fn models(&self) -> nous_device::ModelsResponse {
        (**self).models()
    }
    fn assembled_models(&self) -> Option<nous_device::ModelsResponse> {
        (**self).assembled_models()
    }
}

async fn sessions<H: Host>(State(host): State<H>) -> Result<Json<Value>> {
    let items = host.source().sessions()?;
    Ok(Json(json!({ "items": items })))
}

/// `POST /v1/sessions`：建会话，201 带生效配置。字段形状由 `tests/wire_shapes.rs` 钉住。
async fn create_session<H: Host>(
    State(host): State<H>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<Value>)> {
    let created = host.commands().create(body).await?;
    Ok((StatusCode::CREATED, Json(serde_json::to_value(created).unwrap())))
}

/// `GET /v1/unread`：平铺 map（`{"id": {unread, readAt}}`），覆盖所有会话。
async fn unread<H: Host>(State(host): State<H>) -> Result<Json<Value>> {
    Ok(Json(host.commands().unread().await?))
}

/// `GET /v1/origins`：会话血缘反查，`?trigger=<code>` 按触发器过滤。
/// 数据在 durable 的 session 作用域文档里（与 names / pins 同一份），只读。
async fn origins<H: Host>(State(host): State<H>, Query(query): Query<OriginsQuery>) -> Result<Json<Value>> {
    // 没有 durable 写后端的只读装配：诚实回空表，不 409（与其它只读列表端点一致）。
    let Some(session) = host.durable_session() else {
        return Ok(Json(json!({ "items": [] })));
    };
    Ok(Json(nous_durable::session::commands::origins_map(
        &session,
        query.trigger.as_deref(),
    ).await.map_err(|error| SessionError::Internal(error.to_string()))?))
}

#[derive(Debug, Deserialize)]
struct OriginsQuery {
    trigger: Option<String>,
}

/// `POST /v1/read/:id`：把水位推到最新，回 `{id, readAt, unread: 0}`。
async fn mark_read<H: Host>(
    State(host): State<H>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let result = host.commands().read(id).await?;
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
async fn rename<H: Host>(
    State(host): State<H>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let body: RenameBody = if body.is_empty() {
        return Err(SessionError::BadRequest("name is required".to_string()));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| SessionError::BadRequest(format!("rename body: {error}")))?
    };
    let result = host.commands().rename(id, body.name).await?;
    Ok(Json(json!({ "id": result.id, "name": result.name })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinBody {
    pinned: bool,
    request_id: Option<String>,
}

/// `POST /v1/sessions/:id/pin`：置顶写穿。`requestId` 必填，重放回第一次的回执。
async fn pin<H: Host>(
    State(host): State<H>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let body: PinBody = if body.is_empty() {
        return Err(SessionError::BadRequest(
            "pinned and requestId are required".to_string(),
        ));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| SessionError::BadRequest(format!("pin body: {error}")))?
    };
    let request_id = body
        .request_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SessionError::BadRequest("requestId is required".to_string()))?;
    let result = host.commands().pin(id, body.pinned, request_id).await?;
    Ok(Json(serde_json::to_value(result).map_err(|error| {
        SessionError::Internal(format!("serialize pin receipt: {error}"))
    })?))
}


/// `GET /v1/device`：进程/宿主配置，About 面板读它。

async fn selection<H: Host>(State(host): State<H>) -> Result<Json<Value>> {
    let result = host.commands().selection().await?;
    Ok(Json(json!({ "id": result.id })))
}

/// `POST /v1/selection {id}`：写选中的会话；`{id:null}` 清空。
async fn write_selection<H: Host>(
    State(host): State<H>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let body: SelectionBody = if body.is_empty() {
        return Err(SessionError::BadRequest("id is required".to_string()));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| SessionError::BadRequest(format!("selection body: {error}")))?
    };
    let result = host.commands().select(body.id).await?;
    Ok(Json(json!({ "id": result.id })))
}

#[derive(Debug, Deserialize)]
struct SelectionBody {
    id: Option<i64>,
}

/// `POST /v1/sessions/{id}/config`：逐键覆盖，回生效值。
async fn config<H: Host>(
    State(host): State<H>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let patch: Value = if body.is_empty() {
        return Err(SessionError::BadRequest("config patch is required".to_string()));
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| SessionError::BadRequest(format!("config body: {error}")))?
    };
    // 档位表来自 /v1/models 的编目，不是请求体：换模型时按新模型的表夹档位。
    let mut patch = patch;
    if let Some(model) = patch.get("model") {
        let wanted = model
            .get("provider")
            .and_then(Value::as_str)
            .zip(model.get("modelId").and_then(Value::as_str));
        if let Some((provider, model_id)) = wanted {
            if let Some(item) = host
                .assembled_models()
                .unwrap_or_else(|| host.models().clone())
                .items
                .iter()
                .find(|item| item.provider == provider && item.model_id == model_id)
            {
                patch["thinkingLevels"] = json!(item.thinking_levels);
            }
        }
    }
    let result = host.commands().configure(id, patch).await?;
    Ok(Json(result.value))
}

/// `POST /v1/sessions/{id}/purge`：真删 + 墓碑；第二次 409。
async fn purge<H: Host>(
    State(host): State<H>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let id = parse_session(&id)?;
    let result = host.commands().purge(id).await?;
    Ok(Json(json!({
        "id": result.id,
        "purged": true,
        "at": result.at,
    })))
}



/// `GET /v1/search?q=&kind=&limit=`（#85）：服务端扫一次会话正文 / 文档，
/// `q` 少于两个字符 → 400，`limit` 上限 100 且 `truncated` 明确。
#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    kind: Option<String>,
    limit: Option<usize>,
}

async fn search<H: Host>(
    State(host): State<H>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>> {
    let q = query.q.unwrap_or_default().trim().to_string();
    if q.chars().count() < 2 {
        return Err(SessionError::BadRequest("q must be at least 2 characters".into()));
    }
    let kind = query.kind.as_deref().unwrap_or("all");
    if !matches!(kind, "all" | "sessions" | "documents") {
        return Err(SessionError::BadRequest("kind must be all/sessions/documents".into()));
    }
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let all = host.source().search(&q, kind)?;
    let total = all.len();
    let truncated = total > limit;
    let items: Vec<Value> = all
        .into_iter()
        .take(limit)
        .map(|item| serde_json::to_value(item).expect("SearchItem is serializable"))
        .collect();
    Ok(Json(json!({ "items": items, "total": total, "truncated": truncated })))
}

/// `GET /v1/capabilities`：desk 按 modules 显示/隐藏功能面。
/// #51 只改 `runtime.provider` 一段：kind 由装配决定，不另造一张能力表。

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
async fn entries<H: Host>(
    State(host): State<H>,
    Path(id): Path<String>,
    Query(query): Query<EntriesQuery>,
) -> Result<Json<Value>> {
    let session = parse_session(&id)?;
    let limit = match query.limit.as_deref() {
        None => ENTRIES_MAX_LIMIT,
        Some(text) => text
            .parse::<usize>()
            .map_err(|_| SessionError::BadRequest(format!("limit is not a number: {text:?}")))?,
    };
    if limit == 0 {
        return Err(SessionError::BadRequest(
            "limit must be at least 1".to_string(),
        ));
    }
    let limit = limit.min(ENTRIES_MAX_LIMIT);
    let cursor = match query.cursor.as_deref() {
        None => None,
        Some(text) => {
            let cursor = Cursor::decode(text)
                .map_err(|_| SessionError::BadRequest(format!("invalid cursor: {text:?}")))?;
            if cursor.order != ScanOrder::Descending {
                return Err(SessionError::BadRequest(
                    "cursor was issued for another order".to_string(),
                ));
            }
            Some(cursor)
        }
    };
    if !host.source().exists(session)? {
        return Err(SessionError::NotFound(format!(
            "session {session} not found"
        )));
    }
    let (mut items, next) = host
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
        let mut all = host.source().entries(session)?;
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


/// `POST /v1/sessions/:id/abort`：没有活跃任务时 409（不是 200「假装停了」）。
async fn abort<H: Host>(State(host): State<H>, Path(id): Path<String>) -> Result<Json<Value>> {
    let session = parse_session(&id)?;
    host.commands().abort(session).await?;
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
async fn fork<H: Host>(
    State(host): State<H>,
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
            .map_err(|error| SessionError::BadRequest(format!("fork body: {error}")))?
    };
    let result = host
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
async fn events<H: Host>(
    State(host): State<H>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    let id = parse_session(&id)?;
    let source = host.source().clone();
    if !source.exists(id)? {
        return Err(SessionError::NotFound(format!("session {id} not found")));
    }
    let subscription = host.hub().subscribe(id);
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
        return Err(SessionError::Internal(
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
        .ok_or_else(|| SessionError::NotFound(format!("session {id} not found")))?;
    Ok(RemoteEvent::Snapshot {
        session: Some(
            serde_json::to_value(&session)
                .map_err(|error| SessionError::Internal(format!("serialize session: {error}")))?,
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

/// 路径里的 `:id` 用字符串接再自己 parse —— 让 axum 的 `Path<i64>` 拒掉会吐纯文本。
/// `messages` 还留在 `server`，所以它也得是 `pub`。
pub fn parse_session(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .map_err(|_| SessionError::BadRequest(format!("session id is not a number: {raw:?}")))
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
/// 端点唯一真相：`/v1/_routes`、`/v1/capabilities`、`docs/plugin-versions.md`
/// 三处都从这里读，`plugin.toml` 里**没有**端点表（规则 11）。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        nous_routes::Route::new("GET", "/v1/sessions", MODULE, || get(sessions::<H>)),
        nous_routes::Route::new("POST", "/v1/sessions", MODULE, || post(create_session::<H>)),
        nous_routes::Route::new("GET", "/v1/sessions/{id}/entries", MODULE, || get(entries::<H>)),
        nous_routes::Route::new("GET", "/v1/sessions/{id}/events", MODULE, || get(events::<H>)),
        nous_routes::Route::new("POST", "/v1/sessions/{id}/abort", MODULE, || post(abort::<H>)),
        nous_routes::Route::new("POST", "/v1/sessions/{id}/fork", MODULE, || post(fork::<H>)),
        nous_routes::Route::new("POST", "/v1/sessions/{id}/rename", MODULE, || post(rename::<H>)),
        nous_routes::Route::new("POST", "/v1/sessions/{id}/pin", MODULE, || post(pin::<H>)),
        nous_routes::Route::new("POST", "/v1/sessions/{id}/config", MODULE, || post(config::<H>)),
        nous_routes::Route::new("POST", "/v1/sessions/{id}/purge", MODULE, || post(purge::<H>)),
        nous_routes::Route::new("GET", "/v1/unread", MODULE, || get(unread::<H>)),
        nous_routes::Route::new("GET", "/v1/origins", MODULE, || get(origins::<H>)),
        nous_routes::Route::new("POST", "/v1/read/{id}", MODULE, || post(mark_read::<H>)),
        nous_routes::Route::new("GET", "/v1/selection", MODULE, || get(selection::<H>)),
        nous_routes::Route::new("POST", "/v1/selection", MODULE, || post(write_selection::<H>)),
        nous_routes::Route::new("GET", "/v1/search", MODULE, || get(search::<H>)),
    ]
}
