//! 最小轮次：pi 本体。
//!
//! 一轮 = `provider stream → 工具调用 → 唯一写者 {result, mutations} → 一条提交`。
//! durable 的 harness 是唯一写库者；本模块只定义轮次的公共面与终态语义。
//!
//! ## 三条不许破的规矩
//!
//! 1. **一轮只有一条提交**：assistant 终态、工具结果、清 live 全在同一批 [`StorageWrite`] 里，
//!    所以「事务没被打断」这件事不靠 if 判断，靠 `commit` 的原子性。
//! 2. **上下文从库里读，不按 id 硬扫**：`Session::entries` 走库里的 fork 链 + `head` 标记；
//!    `for id in 1..N { get_entry(id) }` 会静默截断历史，也会把每轮变成 N 次加锁查询。
//! 3. **形状不在这里发明**：工具结果就是 `pi.tool-result` entry（`model` 里一条
//!    `ToolResultMessage`），不另造 `data.toolResults` 之类的键。
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use nous_ai::provider::{Provider, SimpleStreamOptions};
use nous_ai::tool::AgentTool;
use nous_ai::{AssistantMessage, Context, Message, Model, StopReason};
use nous_durable::harness::clock::Clock;
use nous_durable::harness::generation::{
    self, assistant_entry, generation_change, live_record, ClassifyRequest, RetryPolicy,
    StreamRequest,
};
use nous_durable::session::{CommitOptions, Session};
use nous_durable::types::TerminalReason;
use nous_durable::{DocumentContent, Id, StorageWrite};
use tokio_util::sync::CancellationToken;

use crate::clock::SystemClock;

/// 一轮最多读进来的历史条数（`Session::entries` 的上限）。
///
/// 不是"上下文窗口"，是**防呆**：读历史这一步不该把整库拖进内存。真要做窗口裁剪，
/// 那是另一个模块（压缩 / `head`），不在这里偷偷截。
pub const CONTEXT_ENTRY_LIMIT: usize = 2_000;

/// 一轮里最多几轮工具循环。超了就当这一轮结束（不静默续命）。
pub const MAX_TOOL_ROUNDS: usize = 8;

/// 一轮的输入。
#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub conversation: i64,
    /// 终态 assistant entry 的 id（调用方用 `Session::mint_id` 取）。
    pub entry_id: Id,
    /// 这一轮的受理 id，写进终态 entry 的 `submissionId`。
    pub submission: nous_durable::Id,
    pub live: i64,
    pub model: Model,
    pub context: Context,
    pub attempt: u64,
}

/// 一轮的结果：正文 + 终态 + 本轮实际调用的 tool call id。
#[derive(Debug, Clone, PartialEq)]
pub struct TurnOutcome {
    pub submission: nous_durable::Id,
    pub assistant: AssistantMessage,
    pub terminal: TerminalReason,
    pub tool_calls: Vec<String>,
}

/// 应用层的轮次驱动器。
#[async_trait]
pub trait TurnDriver: Send + Sync {
    async fn run(&self, request: TurnRequest) -> Result<TurnOutcome, String>;
}

/// 组装根交下来的运行时依赖。
///
/// `Provider` / 工具表不在 sessions 里造：它们属于宿主配置，server 才是组装根。
#[derive(Clone)]
pub struct TurnRuntime {
    pub providers: Arc<dyn Provider>,
    pub tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
}

/// 客户端 abort 走同一个 token：cancellation 停 provider，落库终态仍由 `cancel` 做。
pub fn abort_token(session: &Session) -> CancellationToken {
    nous_durable::session::commands::cancel_token(session)
}

/// 扩展熔断监听：进程真的死后由它调 `commands::cancel(extension_crash)`。
///
/// 幂等与 first-wins 全在 `commands::cancel` 里；这里只做「谁死了 → 哪个会话 → 哪个归因」
/// 这一跳，不另造状态。
pub async fn on_extension_process_death(
    session: &Session,
    conversation: nous_durable::Id,
    extension: impl Into<String>,
) -> Result<
    nous_durable::session::commands::CancelOutcome,
    nous_durable::session::commands::CommandError,
> {
    let _extension = extension.into();
    nous_durable::session::commands::cancel(
        session,
        nous_durable::session::commands::CancelRequest {
            conversation,
            reason: TerminalReason::ExtensionCrash,
        },
    )
    .await
}

/// 起一轮要的句柄：`(会话, provider, 工具表, 模型)` 四样凑齐就能跑。
///
/// **给组装根用的窄接口**：路由只需要「拿一个句柄、`spawn` 出去」，不必在自己的 crate 里
/// 写出 `nous_ai` 的类型（那样端点层就又依赖模型层了）。id 在 [`run`](Self::run) 里现 mint，
/// 所以「装配了但没跑」不会在库里留 id 洞。
pub struct TurnHandle {
    session: Arc<Session>,
    providers: Arc<dyn Provider>,
    tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
    model: Model,
}

impl TurnHandle {
    /// 离线 / 回放用的句柄：provider 是 `nous-ai` 的 faux，脚本给什么就答什么。
    ///
    /// 生产装配走 [`TurnHandle::new`]（真 provider）。这里之所以住在 lib 而不是 test，
    /// 是因为**端到端判据**（HTTP → 库里多一条 assistant entry）需要一个不联网的 provider，
    /// 而那个判据必须能在 CI 上跑。
    pub fn faux(
        session: Arc<Session>,
        tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
        script: nous_ai::providers::faux::FauxScript,
    ) -> Self {
        let provider = nous_ai::providers::faux::FauxProvider::new(script);
        let model = provider.default_model().clone();
        Self {
            session,
            // `FauxProvider::new` 本来就返回 `Arc<FauxProvider>`：别套两层。
            providers: provider as Arc<dyn Provider>,
            tools,
            model,
        }
    }

    pub fn new(
        session: Arc<Session>,
        providers: Arc<dyn Provider>,
        tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
        model: Model,
    ) -> Self {
        Self {
            session,
            providers,
            tools,
            model,
        }
    }

    /// 跑一轮：先 mint 两个 id（`live` 文档 + 终态 entry），再交给驱动器。
    pub async fn run(self, conversation: i64, submission: Id) -> Result<TurnOutcome, String> {
        let live = self
            .session
            .mint_id()
            .await
            .map_err(|error| error.to_string())?;
        let entry_id = self
            .session
            .mint_id()
            .await
            .map_err(|error| error.to_string())?;
        let driver = HarnessTurnDriver::new(
            Arc::clone(&self.session),
            TurnRuntime {
                providers: self.providers,
                tools: self.tools,
            },
        );
        driver
            .run(TurnRequest {
                conversation,
                submission,
                entry_id,
                live,
                model: self.model,
                context: Context::new(Vec::new()),
                attempt: 1,
            })
            .await
    }
}

/// 一个具体的轮次驱动器：provider 流 → 工具调用 → durable harness 一次提交。
pub struct HarnessTurnDriver {
    session: Arc<Session>,
    runtime: TurnRuntime,
    /// 生产用真时钟；测试注入虚拟时钟。
    clock: Arc<dyn Clock>,
    /// SSE 出口。生产由 server 注入，测试可观测。
    on_event: Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>,
}

impl HarnessTurnDriver {
    /// 生产装配：真时钟。
    pub fn new(session: Arc<Session>, runtime: TurnRuntime) -> Self {
        Self::with_clock(session, runtime, Arc::new(SystemClock::new()))
    }

    /// 注入时钟（测试用 `ManualClock`）。
    pub fn with_clock(session: Arc<Session>, runtime: TurnRuntime, clock: Arc<dyn Clock>) -> Self {
        Self {
            on_event: Arc::new(|_| {}),
            session,
            runtime,
            clock,
        }
    }

    pub fn with_event_sink(
        mut self,
        sink: Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>,
    ) -> Self {
        self.on_event = sink;
        self
    }

    /// 从库里读这一轮要喂给模型的历史。
    ///
    /// 解析不出 `Message` 的载荷（`pi.system` 那种带 `toolsAdded` 的、将来的新 kind）
    /// **跳过并计数**：静默丢弃是 bug 温床，计数至少能在日志里露头。
    async fn load_history(&self, conversation: i64, context: &mut Context) -> Result<usize, String> {
        let entries = self
            .session
            .entries(conversation, CONTEXT_ENTRY_LIMIT)
            .await
            .map_err(|error| error.to_string())?;
        let mut restored = 0usize;
        for entry in &entries {
            let Some(messages) = &entry.model else {
                continue;
            };
            for message in messages {
                match serde_json::from_value::<Message>(message.clone()) {
                    Ok(message) => {
                        context.messages.push(message);
                        restored += 1;
                    }
                    Err(_) => continue,
                }
            }
        }
        Ok(restored)
    }
}

#[async_trait]
impl TurnDriver for HarnessTurnDriver {
    async fn run(&self, request: TurnRequest) -> Result<TurnOutcome, String> {
        let TurnRequest {
            conversation,
            submission,
            entry_id,
            live,
            model,
            mut context,
            attempt,
        } = request;

        self.load_history(conversation, &mut context).await?;
        context.tools = self
            .runtime
            .tools
            .values()
            .map(|tool| tool.schema().clone())
            .collect();

        let opts = SimpleStreamOptions::new().with_signal(abort_token(&self.session));
        let stream_request = StreamRequest {
            live,
            conversation,
            attempt,
            // 一轮只有终态那一条提交：partial 的节拍开得比任何一轮都长，
            // 让 stream_response 的 flush 永不触发（它只在该子模块里写 pi.live）。
            interval_ms: u64::MAX,
        };

        // 一轮里的写全在这一批：中间 assistant entry（带工具调用）、每条 `pi.tool-result`、
        // 终态 assistant entry、清 live —— 同一个 `submissionId`。
        // 顺序 = 真数据的顺序（`fixtures/agent.small.sqlite`：assistant → tool-result → assistant），
        // 而 id 是按顺序 mint 的，所以先 mint 的 id 一定更小。
        let mut tool_writes: Vec<StorageWrite> = Vec::new();
        let mut tool_calls: Vec<String> = Vec::new();
        let mut assistant: Option<AssistantMessage> = None;

        for round in 0..MAX_TOOL_ROUNDS {
            let stream = self
                .runtime
                .providers
                .stream_simple(&model, &context, &opts)
                .await;
            let message = generation::stream_response(
                &self.session,
                &stream_request,
                stream,
                None,
                self.clock.clone(),
            )
            .await
            .map_err(|error| error.to_string())?;
            if message.stop_reason == StopReason::Aborted {
                // 取消的落库归 `cancel`（`#38`）：这里不写终态，也不假装这一轮正常结束。
                return Err("aborted turn must be settled through cancel(), not a normal commit"
                    .to_string());
            }

            let pending: Vec<nous_ai::ToolCall> = message
                .content
                .iter()
                .filter_map(|block| match block {
                    nous_ai::Content::ToolCall(call) => Some(call.clone()),
                    _ => None,
                })
                .collect();

            if pending.is_empty() {
                assistant = Some(message);
                break;
            }

            // 带工具调用的这条 assistant 消息本身就是一条 entry（不带 `reason`：它不终态）。
            let record_id = self
                .session
                .mint_id()
                .await
                .map_err(|error| error.to_string())?;
            let record = generation::assistant_entry_non_terminal(
                &ClassifyRequest {
                    live,
                    conversation,
                    attempt,
                    entry_id: record_id,
                    retry: RetryPolicy::default(),
                    retryable: false,
                    overflow: None,
                    reason: TerminalReason::Done,
                },
                &message,
            )
            .map_err(|error| error.to_string())?;
            let mut record = record;
            record["submissionId"] = serde_json::json!(submission);
            tool_writes.push(StorageWrite::Entry(record));

            for call in pending {
                tool_calls.push(call.id.clone());
                let tool = self
                    .runtime
                    .tools
                    .get(&call.name)
                    .ok_or_else(|| format!("unknown tool: {}", call.name))?;
                let outcome = tool
                    .execute(
                        &call.id,
                        call.arguments.clone(),
                        abort_token(&self.session),
                        Arc::new(|_| {}),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                let result = nous_ai::types::ToolResultMessage {
                    role: Default::default(),
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    content: outcome.into_content(),
                    details: None,
                    usage: None,
                    added_tool_names: Vec::new(),
                    is_error: false,
                    timestamp: self.clock.now_ms() as i64,
                };
                let result_id = self
                    .session
                    .mint_id()
                    .await
                    .map_err(|error| error.to_string())?;
                tool_writes.push(StorageWrite::Entry(serde_json::json!({
                    "id": result_id,
                    "conversationId": conversation,
                    "kind": "pi.tool-result",
                    "submissionId": submission,
                    // 上游 `ToolResultEntry`：`data` 是结构化诊断，没有就是空数组。
                    "data": { "diagnostics": [] },
                    "model": [serde_json::to_value(&result).map_err(|error| error.to_string())?],
                })));
                context.messages.push(Message::ToolResult(Box::new(result)));
            }

            // 预算用完还带工具调用：用它当终态正文收尾（不静默续命，也不丢这一轮的输出）。
            if round + 1 == MAX_TOOL_ROUNDS {
                assistant = Some(message);
            }
        }

        let assistant = assistant
            .ok_or_else(|| "turn produced no assistant message".to_string())?;
        let terminal = TerminalReason::Done;

        // 一轮的所有写一次提交：建 live（若还没建）、工具结果、终态 entry、清 live。
        let snapshot = self
            .session
            .snapshot(live)
            .await
            .map_err(|error| error.to_string())?;
        let mut writes: Vec<StorageWrite> = Vec::new();
        if snapshot.is_none() {
            writes.push(StorageWrite::DocumentCreate {
                id: live,
                record: serde_json::to_value(live_record(live, conversation)).unwrap(),
                content: DocumentContent::Base {
                    version: 0,
                    value: serde_json::json!({}),
                },
            });
        }
        let live_existed = snapshot.is_some();
        let live_snapshot = snapshot.unwrap_or(nous_durable::StoredDocument {
            record: serde_json::to_value(live_record(live, conversation)).unwrap(),
            version: 0,
            value: serde_json::json!({}),
            deltas_since_base: 0,
        });
        writes.extend(tool_writes);

        let mut final_entry = assistant_entry(
            &ClassifyRequest {
                live,
                conversation,
                attempt,
                entry_id,
                retry: RetryPolicy::default(),
                retryable: false,
                overflow: None,
                reason: terminal,
            },
            &assistant,
        )
        .map_err(|error| error.to_string())?;
        final_entry["submissionId"] = serde_json::json!(submission);
        writes.push(StorageWrite::Entry(final_entry));
        if live_existed {
            writes.push(
                generation_change(live, &live_snapshot, None).map_err(|error| error.to_string())?,
            );
        }

        // 一轮只有这一个写者：上面所有写在这一条提交里落库。
        let committed = self
            .session
            .commit(writes, CommitOptions::default())
            .await
            .map_err(|error| error.to_string())?;
        if committed.stored == 0 {
            return Err(format!("turn commit refused: {:?}", committed.refused));
        }

        if let Some(publication) = committed.publication {
            for change in publication.changes {
                if let nous_durable::session::CommitChange::Entry(entry) = change {
                    // 终态/工具 entry 都是定性事件：按 wire 的形状交给出口，
                    // 不让 SSE 投影长在会话层里（投影仍在 server）。
                    let value = serde_json::to_value(&entry).unwrap_or_default();
                    let event = match entry.kind.as_str() {
                        "pi.user" => nous_wire::RemoteEvent::MessageStart {
                            message: entry
                                .model
                                .as_ref()
                                .and_then(|messages| messages.last())
                                .cloned()
                                .unwrap_or(value.clone()),
                            message_id: Some(entry.id.to_string()),
                        },
                        "pi.assistant" => nous_wire::RemoteEvent::MessageEnd {
                            entry: Some(value),
                            message: None,
                            message_id: None,
                        },
                        _ => nous_wire::RemoteEvent::EntryAppended { entry: value },
                    };
                    (self.on_event)(&event);
                }
            }
        }

        Ok(TurnOutcome {
            submission,
            assistant,
            terminal,
            tool_calls,
        })
    }
}

/// 客户端 abort / 内核超时 / 扩展进程死三源共用的 token 观察点。
pub struct AbortWatcher {
    token: tokio_util::sync::CancellationToken,
}

impl AbortWatcher {
    pub fn new(token: tokio_util::sync::CancellationToken) -> Self {
        Self { token }
    }
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
    pub fn cancelled(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.token.cancelled()
    }
}
