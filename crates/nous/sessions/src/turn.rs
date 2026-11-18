//! 最小轮次：pi 本体。
//!
//! 一轮 = `provider stream → 工具调用 → 唯一写者 {result, mutations} → 一条提交`。
//! durable 的 harness 是唯一写库者；本模块只定义轮次的公共面与终态语义。
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use nous_ai::provider::{Provider, SimpleStreamOptions};
use nous_ai::tool::AgentTool;
use nous_ai::{AssistantMessage, Context, Message, Model, StopReason};
use nous_durable::actor::Actor;
use nous_durable::harness::clock::ManualClock;
use nous_durable::harness::generation::{
    self, assistant_entry, generation_change, live_record, ClassifyRequest, RetryPolicy,
    StreamRequest,
};
use nous_durable::session::{CommitOptions, Session};
use nous_durable::types::TerminalReason;

use nous_durable::{DocumentContent, Id, StorageWrite};
use tokio_util::sync::CancellationToken;

/// 一轮的输入。
#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub conversation: i64,
    /// 终态 assistant entry 的 id（调用方从 submission 的 user entry 派生）。
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
/// `ProviderRegistry` / `ToolRegistry` 不在 sessions 里造：它们属于宿主配置，
/// server 才是组装根。sessions 只消费这里的两张注册表。
#[derive(Clone)]
pub struct TurnRuntime {
    pub providers: Arc<dyn Provider>,
    pub tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
}

/// 客户端 abort 走同一个 token：cancellation 停 provider，落库终态仍由 `cancel` 做。
pub fn abort_token(session: &Session) -> CancellationToken {
    nous_durable::session::commands::cancel_token(&session)
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

/// 一个具体的轮次驱动器：provider 流 → 工具调用 → durable harness 一次提交。
pub struct HarnessTurnDriver {
    session: Arc<Session>,
    runtime: TurnRuntime,
    /// 这一轮写进 entry 的 actor。来自组装根；将来认证落地只换源头，不改这里。
    actor: Actor,
    /// SSE 出口。生产由 server 注入，测试可观测。
    on_event: Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>,
}

impl HarnessTurnDriver {
    pub fn new(session: Arc<Session>, runtime: TurnRuntime) -> Self {
        Self {
            on_event: Arc::new(|_| {}),
            session,
            runtime,
            actor: Actor::system("nous-app"),
        }
    }

    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }

    pub fn with_event_sink(
        mut self,
        sink: Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>,
    ) -> Self {
        self.on_event = sink;
        self
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

        for id in 1..100_000 {
            let Some((entry, _)) = self
                .session
                .get_entry(id)
                .await
                .map_err(|error| error.to_string())?
            else {
                break;
            };
            if entry.conversation_id != conversation {
                continue;
            }
            if let Some(messages) = entry.model {
                for message in messages {
                    if let Ok(message) = serde_json::from_value::<Message>(message) {
                        context.messages.push(message);
                    }
                }
            }
        }
        let tool_list: Vec<nous_ai::Tool> = self
            .runtime
            .tools
            .values()
            .map(|tool| tool.schema().clone())
            .collect();
        context.tools = tool_list;

        let clock = std::sync::Arc::new(ManualClock::new(0));
        let opts = SimpleStreamOptions::new().with_signal(abort_token(&self.session));
        let stream = self
            .runtime
            .providers
            .stream_simple(&model, &context, &opts)
            .await;
        let stream_request = StreamRequest {
            live,
            conversation,
            attempt,
            // 一轮只有终态那一条提交：partial 的节拍开得比任何一轮都长，
            // 让 stream_response 的 flush 永不触发（它只在该子模块里写 pi.live）。
            interval_ms: u64::MAX,
        };
        let mut assistant = generation::stream_response(
            &self.session,
            &stream_request,
            stream,
            None,
            clock.clone(),
        )
        .await
        .map_err(|error| error.to_string())?;

        let mut tool_calls = Vec::new();
        let tool_blocks: Vec<_> = assistant
            .content
            .iter()
            .filter_map(|block| match block {
                nous_ai::Content::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
            .collect();
        for call in tool_blocks {
            tool_calls.push(call.id.clone());
            let tool = self
                .runtime
                .tools
                .get(&call.name)
                .ok_or_else(|| format!("unknown tool: {}", call.name))?;
            let result = tool
                .execute(
                    &call.id,
                    call.arguments.clone(),
                    abort_token(&self.session),
                    std::sync::Arc::new(|_| {}),
                )
                .await
                .map_err(|error| error.to_string())?;
            context.messages.push(Message::ToolResult(Box::new(
                nous_ai::types::ToolResultMessage {
                    role: Default::default(),
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    content: result.into_content(),
                    details: None,
                    usage: None,
                    added_tool_names: Vec::new(),
                    is_error: false,
                    timestamp: chrono_millis() as i64,
                },
            )));
        }

        if !tool_calls.is_empty() {
            let stream = self
                .runtime
                .providers
                .stream_simple(&model, &context, &opts)
                .await;
            assistant =
                generation::stream_response(&self.session, &stream_request, stream, None, clock)
                    .await
                    .map_err(|error| error.to_string())?;
        }

        let terminal = if assistant.stop_reason == StopReason::Aborted {
            return Err("aborted turn must be settled through cancel(), not a normal commit".into());
        } else {
            nous_durable::types::TerminalReason::Done
        };
        // 一轮的所有写一次提交：建 live、partial 若已有、终态 entry、清 live、submission done。
        let snapshot = self
            .session
            .snapshot(live)
            .await
            .map_err(|error| error.to_string())?;
        let entry_id: Id = entry_id;
        let mut writes = Vec::new();
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
        // 终态 entry 同时记下工具调用与结果（同一个 submissionId），不再另造 id。
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
                    actor: self.actor.clone(),
                },
                &assistant,
                &self.actor,
            )
            .map_err(|error| error.to_string())?;
        final_entry["submissionId"] = serde_json::json!(submission);
        if !tool_calls.is_empty() {
            final_entry["data"] = serde_json::json!({
                "toolCalls": tool_calls,
                "toolResults": assistant.content.iter().filter_map(|block| match block {
                    nous_ai::Content::Text(text) => Some(text.text.clone()),
                    _ => None,
                }).collect::<Vec<_>>(),
            });
        }
        writes.push(nous_durable::StorageWrite::Entry(final_entry));
        if live_existed {
            writes.push(
                generation_change(live, &live_snapshot, None).map_err(|error| error.to_string())?,
            );
        }
        // 一轮只有这一个写者：建 live、写终态、清 live、关 submission 全在这一条里。
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
                match change {
                    nous_durable::session::CommitChange::Entry(entry) => {
                        // 终态/工具 entry 都是定性事件：按 wire 的形状交给出口，
                        // 不让 SSE 投影长在会话层里（投影仍在 server）。
                        let value = serde_json::to_value(&entry).unwrap_or_default();
                        let event = match entry.kind.as_str() {
                            "pi.user" => nous_wire::RemoteEvent::MessageStart {
                                message: entry
                                    .model
                                    .as_ref()
                                    .and_then(|m| m.last())
                                    .cloned()
                                    .unwrap_or(value.clone()),
                                message_id: Some(entry.id.to_string()),
                            },
                            "pi.assistant" => nous_wire::RemoteEvent::MessageEnd {
                                entry: Some(value.clone()),
                                message: None,
                                message_id: None,
                            },
                            _ => nous_wire::RemoteEvent::EntryAppended {
                                entry: value.clone(),
                            },
                        };
                        (self.on_event)(&event);
                        if entry.kind == "pi.user" {
                            (self.on_event)(&nous_wire::RemoteEvent::EntryAppended {
                                entry: value,
                            });
                        }
                    }
                    _ => {}
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

fn chrono_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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
