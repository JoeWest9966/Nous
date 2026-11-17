//! 一次生成的流式落库：`packages/durable/src/harness/generation.ts` 的 `streamResponse` 与
//! `classify`（票 `#14`，`docs/harness.md` §1 ③）。
//!
//! ## 三处转译（不是照抄，改的地方都写在这里）
//!
//! 1. **没有 `runtime.commit(tx => ...)`**。上游在回调里拿事务句柄，本库
//!    [`Session::commit`] 只收一批写（`#13` 的决定），所以每次提交是「读快照 → 改 → 写
//!    `DocumentChange`」。两条读之间不是原子的：**`pi.live` 只有 harness 一个写者**（上游
//!    `live.ts` 同一前提），所以这条差别今天观察不到；真出现第二个写者要先把这个洞补上。
//! 2. **`deferred` 不做**。上游 partial 用 `PROGRESS_COMMIT`
//!    （`live.ts:236` `{ persistence: { kind: "deferred" } }`），本库
//!    [`CommitPersistence::Deferred`] 明确拒绝（`docs/decisions.md` deferred 那节），所以每次
//!    partial 都是一次真提交，节拍只由 `interval_ms` 时间闸决定。
//! 3. **`copyJson(partial)` 换成 `serde_json::to_value`**。上游要拷贝是因为 provider 会继续改
//!    那个可变对象；本库 `Arc<AssistantMessage>` 是不可变的，但 [`PartialChanges::take`] 要
//!    `&mut Value`（它就地清空被跟踪调用的参数），所以仍然序列化一份 —— 代价与上游的
//!    `copyJson` 同级。
//!
//! ## 提交节奏的四面（判据见 `docs/harness.md` §5）
//!
//! 1. **节拍**：两次提交至少隔 `interval_ms`（上游默认 100 ms）；
//! 2. **单飞**：任何时刻至多一次提交在飞。本库是**结构性**保证 —— 提交就在流循环里
//!    `await`，没有 `spawn`；判据仍写，因为一旦有人把提交挪进 `spawn`，它就会红；
//! 3. **终态在后**：`classify` 那次提交把 `generation` 清掉，时间线上不许出现「先终态、
//!    后 partial」；
//! 4. **停表要真停**：`stopped` 之后冷却一个 `interval_ms`，提交次数不许再涨。上游靠
//!    `timer?.()` 的取消函数；本库靠 drop 掉 `sleep_until` future —— 但判据不假设这件事，
//!    它只看提交次数（有人改用 `spawn` + 共享标志时，只 return 不取消，这条立刻红）。
//!
//! 尾帧**故意不提交**：上游 `finally` 只做 `stopped = true` / 取消定时器 / `await inFlight`，
//! 不碰 `pending` —— 紧接着 `classify` 会一次提交终态，补那一帧是覆盖一次再被覆盖。

use std::sync::Mutex;

use nous_ai::{AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, StopReason};
use nous_chord::delta::codec::Encoder;
use nous_chord::delta::draft::Draft;
use nous_chord::delta::{Revision, Seg};
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::session::{CommitOptions, Session, SessionError};
use crate::{DocumentContent, Id, StorageWrite};

use super::live::{Deferred, LiveState, Retry};
use super::partial::PartialChanges;
use super::HarnessError;

/// `pi.live` 的文档 kind（上游 `LiveDoc`：`kind: "pi.live"`、`scope: "conversation"`、
/// `history: latest`、`fork: initial`、`initial: {}`）。
pub const LIVE_KIND: &str = "pi.live";

/// `pi.live` 的文档记录，给 `DocumentCreate` 用。初始值永远是 `{}`（上游 `initial`）。
///
/// 上游 `defineDoc` 自己把地址写进库里；本库要先建出来（`DocumentCreate`），所以这个函数存在的
/// 理由就是：**别在调用点手抄这五个字段** —— `history` / `fork` 抄错一个，`pi.live` 就会开始
/// 留历史或留两份，而两者的判据都不一样。
pub fn live_record(id: Id, conversation: Id) -> Value {
    json!({
        "id": id,
        "kind": LIVE_KIND,
        "scope": { "kind": "conversation", "conversationId": conversation },
        "history": "latest",
        "fork": "initial",
    })
}

/// 时间线上的一拍。判据只看这个序列，不看墙钟读数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    /// 一次 partial 提交开始。
    PartialStart,
    /// 一次 partial 提交落库。
    PartialEnd,
    /// `classify` 的终态提交落库。
    Terminal,
}

/// 时间线见证。判据在 `tests/harness_generation.rs`。
///
/// 上游没有这个东西 —— 它出现的理由是：节拍 / 单飞 / 终态在后 / 停表这四面**都只能靠时间线
/// 判**，值判据一条也判不出来。用 `tokio::time::Instant`：测试里 `start_paused` 时是虚拟时钟，
/// 于是断言不需要 `sleep` 猜。
#[derive(Debug, Default)]
pub struct Timeline {
    beats: Mutex<Vec<(Beat, Instant)>>,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一拍。生产路径只在 `stream_response` / `classify` 里调；判据与**反向对照**
    /// （`tests/harness_generation.rs`）也用它造「泄漏的定时器」那种假时间线。
    pub fn record(&self, beat: Beat) {
        self.beats
            .lock()
            .expect("timeline is not poisoned")
            .push((beat, Instant::now()));
    }

    /// 记下来的拍子（按发生顺序）。
    pub fn beats(&self) -> Vec<(Beat, Instant)> {
        self.beats.lock().expect("timeline is not poisoned").clone()
    }

    /// 某一类拍子的次数。
    pub fn count(&self, beat: Beat) -> usize {
        self.beats().iter().filter(|(seen, _)| *seen == beat).count()
    }
}

/// 一次流式回答的输入。
#[derive(Debug, Clone)]
pub struct StreamRequest {
    /// `pi.live` 的文档 id。
    pub live: Id,
    /// 这条会话的 id（终态 entry 用；partial 不写 entry）。
    pub conversation: Id,
    /// 第几次尝试（`generation.attempt`）。
    pub attempt: u64,
    /// 两次 partial 提交至少隔多久。上游 `settings.progress.partialIntervalMs`，默认 100。
    pub interval_ms: u64,
}

/// 把一条 `AssistantMessage` 变成可写的 JSON。上游 `copyJson(partial)`。
fn message_json(message: &AssistantMessage) -> Result<Value, HarnessError> {
    serde_json::to_value(message).map_err(|error| HarnessError::Shape(error.to_string()))
}

/// 等一个事件，或者等节拍到点。到点返回 `None`。
enum Tick {
    Event(Option<AssistantMessageEvent>),
    Interval,
}

/// 流一次请求，返回终态消息。partial 按 `interval_ms` 落 `pi.live`；**终态由
/// [`classify`] 提交**，这里不碰。
pub async fn stream_response(
    session: &Session,
    request: &StreamRequest,
    mut events: AssistantMessageEventStream,
    timeline: Option<&Timeline>,
) -> Result<AssistantMessage, HarnessError> {
    let interval = std::time::Duration::from_millis(request.interval_ms.max(1));
    let mut changes = PartialChanges::new();
    let mut pending: Option<Value> = None;
    let mut deadline: Option<Instant> = None;

    loop {
        let tick = match deadline {
            // 定时器就是这条 `sleep_until`：`finally` 里不重建它，就等于取消（见模块文档第 4 面）。
            Some(at) => tokio::select! {
                event = events.next() => Tick::Event(event),
                _ = tokio::time::sleep_until(at) => Tick::Interval,
            },
            None => Tick::Event(events.next().await),
        };
        match tick {
            Tick::Interval => {
                deadline = None;
                flush(session, request, &mut changes, &mut pending, timeline).await?;
            }
            Tick::Event(None) => break,
            Tick::Event(Some(event)) => {
                // 上游这三条 `continue`：终态事件与「还没有内容」的 partial 都不记、不排提交。
                if matches!(
                    event,
                    AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
                ) {
                    continue;
                }
                let partial = partial_of(&event);
                let empty = partial
                    .as_ref()
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty);
                if empty {
                    continue;
                }
                changes.observe(&event);
                if let Some(partial) = partial {
                    pending = Some(partial);
                }
                if deadline.is_none() {
                    deadline = Some(Instant::now() + interval);
                }
            }
        }
    }

    // 上游 `finally` 的三件事：停表、取消定时器、等在飞的那次提交。
    // 本库没有并发的提交可等（提交是 inline 的），`deadline` 也不持有定时器 —— 那个
    // `sleep_until` 是 `select!` 里的临时 future，select 一结束就 drop 掉，所以这里**不需要**
    // 再取消；`pending` 保持不动，就是「尾帧故意不提交」。
    // 上游 `return await events.result()`：拿终态消息。流被关掉（没有终态）是形状错误。
    let terminal = events;
    terminal
        .result()
        .await
        .map_err(|_| HarnessError::Shape("the stream closed without a terminal message".to_string()))
}

/// 事件里带的 partial 快照（终态事件没有）。
fn partial_of(event: &AssistantMessageEvent) -> Option<Value> {
    let message = match event {
        AssistantMessageEvent::Start { partial }
        | AssistantMessageEvent::TextStart { partial, .. }
        | AssistantMessageEvent::TextDelta { partial, .. }
        | AssistantMessageEvent::TextEnd { partial, .. }
        | AssistantMessageEvent::ThinkingStart { partial, .. }
        | AssistantMessageEvent::ThinkingDelta { partial, .. }
        | AssistantMessageEvent::ThinkingEnd { partial, .. }
        | AssistantMessageEvent::ToolCallStart { partial, .. }
        | AssistantMessageEvent::ToolCallDelta { partial, .. }
        | AssistantMessageEvent::ToolCallEnd { partial, .. } => partial,
        AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. } => return None,
    };
    message_json(message).ok()
}

/// 一次 partial 提交：把攒下的增量写进 `pi.live.generation`。
async fn flush(
    session: &Session,
    request: &StreamRequest,
    changes: &mut PartialChanges,
    pending: &mut Option<Value>,
    timeline: Option<&Timeline>,
) -> Result<(), HarnessError> {
    let Some(partial) = pending.take() else {
        return Ok(());
    };
    let mut message = partial;
    let taken = changes.take(&mut message);
    if let Some(timeline) = timeline {
        timeline.record(Beat::PartialStart);
    }
    let snapshot = session
        .snapshot(request.live)
        .await?
        .ok_or(HarnessError::MissingDocument(request.live))?;
    let current = Revision::new(snapshot.version as u64);
    let mut draft = Draft::new(&snapshot.value, current);
    // 上游 `live.generation ??= { attempt }`。**必须写进 draft**，不能先塞进 base 的拷贝里：
    // 塞进 base 会让这一截「凭空存在」，diff 看不到它，产出的 op 就会引用一个库里没有的路径
    // （第一次绕这个坑时读回来直接 `unresolvable path: [Key("generation")]`）。
    if snapshot.value.get("generation").map_or(true, Value::is_null) {
        draft
            .set(&[Seg::key("generation")], json!({ "attempt": request.attempt }))
            .map_err(|error| HarnessError::Ops(error.to_string()))?;
    }
    changes
        .commit(&mut draft, &[Seg::key("generation")], &mut message, taken)
        .map_err(|error| HarnessError::Ops(error.to_string()))?;
    let trusted = draft
        .prepare(current)
        .map_err(|error| HarnessError::Ops(error.to_string()))?;
    let ops = trusted.into_ops();
    if ops.is_empty() {
        // 这一片没有产生改动（例如同一段文本又喂了一遍）：不发一次空提交，账本也不前进。
        if let Some(timeline) = timeline {
            timeline.record(Beat::PartialEnd);
        }
        return Ok(());
    }
    let writes = vec![StorageWrite::DocumentChange {
        id: request.live,
        content: DocumentContent::Delta {
            version: snapshot.version,
            ops: Value::Array(Encoder::new().encode(&ops)),
        },
    }];
    session.commit(writes, CommitOptions::default()).await?;
    if let Some(timeline) = timeline {
        timeline.record(Beat::PartialEnd);
    }
    Ok(())
}

/// 重试策略。上游 `settings.retry`；本库只抄**判定与退避参数**，不抄「恢复」（`docs/harness.md`
/// §3.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub enabled: bool,
    pub max_retries: u64,
    /// 第 `attempt` 次失败后的等待：`delay_ms * 2^(attempt - 1)`。
    pub delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: 3,
            delay_ms: 1_000,
        }
    }
}

impl RetryPolicy {
    pub fn delay_for(&self, attempt: u64) -> u64 {
        let shift = attempt.saturating_sub(1).min(16) as u32;
        self.delay_ms.saturating_mul(1u64 << shift)
    }
}

/// `classify` 的输入。
#[derive(Debug, Clone)]
pub struct ClassifyRequest {
    pub live: Id,
    pub conversation: Id,
    pub attempt: u64,
    /// 终态 assistant entry 的 id。上游 `appendEntry` 自己 mint id；本库
    /// `StorageWrite::Entry` 要求记录里带 `id`，所以由调用方给。
    pub entry_id: Id,
    pub retry: RetryPolicy,
    /// 这次失败是不是「可重试」那一类。上游 `isRetryableAssistantError` 还没提货（`retry.rs`
    /// 那一行仍是「待提货」），所以判定留在调用方 —— 把策略抄进来却没有那个判定，等于假装。
    pub retryable: bool,
    /// 上下文溢出（上游在 `generate` 里算出来的，`classify` 只消费）。
    pub overflow: Option<String>,
}

/// `classify` 的结论。上游是 `Next`；本库只落这四种去向。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// 答完（或进了工具轮）：终态 entry 已落库，`generation` 已清。
    Answered,
    /// 可重试的错误：终态 entry 已落库，`generation` 转 `retry`。
    Retry { at_ms: u64, error: String },
    /// 延后响应：`generation` 转 `deferred`，`pollAt` 是下次轮询时间。
    Poll { poll_at_ms: u64 },
    /// 上下文溢出：终态 entry 已落库，`generation` 已清。
    Overflow { error: String },
}

/// 终态消息的去向。**这一次提交同时清掉 partial** —— 上游 `appendAssistant` 之后
/// `delete live.generation`，所以终态之后不会再有 partial 落进来（时间线第 3 面）。
pub async fn classify(
    session: &Session,
    request: &ClassifyRequest,
    message: &AssistantMessage,
    now_ms: u64,
    timeline: Option<&Timeline>,
) -> Result<Classification, HarnessError> {
    let entry = assistant_entry(request, message)?;
    let snapshot = session
        .snapshot(request.live)
        .await?
        .ok_or(HarnessError::MissingDocument(request.live))?;

    // 延后响应：不写 entry（上游这一支只改 `generation`）。
    if message.stop_reason == StopReason::Deferred {
        let poll_at = message
            .deferred
            .as_ref()
            .and_then(|handle| handle.poll_after_ms)
            .map_or(DEFAULT_POLL_AFTER_MS, |ms| ms.max(0) as u64);
        let poll_at_ms = now_ms + poll_at.max(1);
        let generation = super::live::LiveGeneration {
            attempt: request.attempt,
            message: None,
            retry: None,
            deferred: Some(Deferred { poll_at: poll_at_ms }),
        };
        let write = generation_change(request.live, &snapshot, Some(&generation))?;
        session.commit(vec![write], CommitOptions::default()).await?;
        if let Some(timeline) = timeline {
            timeline.record(Beat::Terminal);
        }
        return Ok(Classification::Poll { poll_at_ms });
    }

    let retrying = message.stop_reason == StopReason::Error
        && request.overflow.is_none()
        && request.retryable
        && request.retry.enabled
        && request.attempt <= request.retry.max_retries;

    let mut writes = vec![StorageWrite::Entry(entry)];
    if retrying {
        let error = message.error_message.clone().unwrap_or_default();
        let at_ms = now_ms + request.retry.delay_for(request.attempt);
        let generation = super::live::LiveGeneration {
            attempt: request.attempt,
            message: None,
            retry: Some(Retry { at: at_ms, error: error.clone() }),
            deferred: None,
        };
        writes.push(generation_change(
            request.live,
            &snapshot,
            Some(&generation),
        )?);
        session.commit(writes, CommitOptions::default()).await?;
        if let Some(timeline) = timeline {
            timeline.record(Beat::Terminal);
        }
        return Ok(Classification::Retry { at_ms, error });
    }

    // 答完 / 溢出：entry 落库，`generation` 整键消失（不是设成 `null`）。
    writes.push(generation_change(request.live, &snapshot, None)?);
    session.commit(writes, CommitOptions::default()).await?;
    if let Some(timeline) = timeline {
        timeline.record(Beat::Terminal);
    }
    match request.overflow.clone() {
        Some(error) => Ok(Classification::Overflow { error }),
        None => Ok(Classification::Answered),
    }
}

/// 上游 `DEFAULT_POLL_AFTER_MS`。
pub const DEFAULT_POLL_AFTER_MS: u64 = 5_000;

/// 一条 assistant entry（上游 `appendAssistant`：`{ model: [message] }`，kind `pi.assistant`）。
fn assistant_entry(
    request: &ClassifyRequest,
    message: &AssistantMessage,
) -> Result<Value, HarnessError> {
    Ok(json!({
        "id": request.entry_id,
        "conversationId": request.conversation,
        "kind": "pi.assistant",
        "model": [message_json(message)?],
    }))
}

/// 把 `pi.live.generation` 改成给定值（`None` = 整键删掉，不是设成 `null`）。
fn generation_change(
    live: Id,
    snapshot: &crate::StoredDocument,
    generation: Option<&super::live::LiveGeneration>,
) -> Result<StorageWrite, HarnessError> {
    let current = Revision::new(snapshot.version as u64);
    let mut draft = Draft::new(&snapshot.value, current);
    let path = [Seg::key("generation")];
    match generation {
        Some(generation) => {
            let value = serde_json::to_value(generation)
                .map_err(|error| HarnessError::Shape(error.to_string()))?;
            draft
                .set(&path, value)
                .map_err(|error| HarnessError::Ops(error.to_string()))?;
        }
        None => draft
            .remove(&path)
            .map_err(|error| HarnessError::Ops(error.to_string()))?,
    }
    let trusted = draft
        .prepare(current)
        .map_err(|error| HarnessError::Ops(error.to_string()))?;
    Ok(StorageWrite::DocumentChange {
        id: live,
        content: DocumentContent::Delta {
            version: snapshot.version,
            ops: Value::Array(Encoder::new().encode(&trusted.into_ops())),
        },
    })
}

/// 让 `SessionError` 能被 `?`。
impl From<SessionError> for HarnessError {
    fn from(error: SessionError) -> Self {
        HarnessError::Session(error)
    }
}

/// 读 `pi.live` 的当前值；文档不在就是缺口（不是「空值」）。
pub async fn live_state(session: &Session, live: Id) -> Result<LiveState, HarnessError> {
    let snapshot = session
        .snapshot(live)
        .await?
        .ok_or(HarnessError::MissingDocument(live))?;
    serde_json::from_value(snapshot.value).map_err(|error| HarnessError::Shape(error.to_string()))
}
