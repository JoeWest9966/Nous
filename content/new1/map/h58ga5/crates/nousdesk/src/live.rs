//! 流式接收：tokio 侧读 SSE 攒进共享状态，UI 侧 16ms 取一次。
//!
//! **一个 token 一次 `cx.notify()` 是这一层最容易犯的错。** 所以分成两半：
//! - `Live::ingest`（tokio 任务里跑）：只改内存，不碰 UI。
//! - `Live::drain`（UI 侧 16ms 定时器里跑）：把攒下的文字取出来写进气泡。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nous_engine::SseEvent;

use crate::session_nav::entry_key;

/// 共享给 tokio 任务和 UI 的那份「正在生成的消息」。
#[derive(Default)]
pub struct LiveState {
    /// contentIndex → 已累积的文字。
    blocks: HashMap<usize, String>,
    /// 有没有新内容，UI 取走之后清掉。
    dirty: bool,
    /// 这一轮生成结束了。
    done: bool,
    /// 出错 / 没答上来，给 UI 显示一句话。
    error: Option<String>,
    /// 连上时引擎给的 `snapshot` 说「此刻没有正在生成的消息」。
    ///
    /// 这条是「粘住不动」的救命信息：上游报错（比如模型 id 不存在）时，失败事件可能在
    /// 我们订阅**之前**就发完了，之后这条流只剩一个空 snapshot，永远等不到 `message_end`。
    /// UI 靠它决定「再等几秒还是直接判这一轮没起来」。
    idle_snapshot: bool,
    /// SSE 首帧 `snapshot.entries[]` 带的时间戳：entry id（字符串）→ epoch ms。
    /// 按天分组目录读它。UI 每 16ms 来 drain 一次，只在「快照来了」时更新，
    /// 平时不碰 —— 目录不因此产生任何新状态。
    times: HashMap<String, i64>,
    /// 首帧到过没有（哪怕 entries 是空的，也说明「这份会话没有时间信息」）。
    snapshot_seen: bool,
    /// `times` 有没有变，UI 取走后清掉。
    times_dirty: bool,
    /// 工具卡片：名字和输出**分开存**。
    /// 以前是一个 `(id, "名字+输出")` 的二元组，名字会被拼到输出前面（「read第一段…」）——
    /// 以前没人渲染工具卡片所以看不出来，但谁先画谁踩。
    tools: Vec<ToolRun>,
}

/// 一次工具调用。`output` 是引擎 `append`/`set` 累起来的。
#[derive(Debug, Clone)]
pub struct ToolRun {
    pub id: String,
    pub name: String,
    pub output: String,
}

#[derive(Clone, Default)]
pub struct Live(Arc<Mutex<LiveState>>);

impl Live {
    pub fn new() -> Self {
        Self::default()
    }

    /// 开始新一轮：清掉上一轮的残留，**不换 Arc**。
    ///
    /// 常连 SSE 的合帧循环靠 `Arc::ptr_eq` 认身份；`send()` 里如果直接
    /// `Live::new()`，循环会以为「换轮了」当场退场，新的一轮就再也没人合帧。
    pub fn reset_round(&self) {
        let mut state = self.0.lock().unwrap();
        state.blocks.clear();
        state.dirty = false;
        state.done = false;
        state.error = None;
        state.idle_snapshot = false;
        state.tools.clear();
        // 换一轮不清时间：目录按天分组靠它，清了会闪回「更早」。
    }

    /// 这两份 `Live` 是不是**同一轮**（每次 `send()` 都会换一个新的）。
    ///
    /// 用来让上一轮遗留的 16ms 循环自己退场。踩过：旧循环还活着，它按自己的看门狗
    /// 把新一轮的 `streaming` 改成 false，界面就卡在「停止」和红条之间来回撕。
    pub fn same_round(&self, other: &Live) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }

    /// tokio 侧：吃一个 SSE 事件。**不许在这里碰 UI。**
    pub fn ingest(&self, event: &SseEvent) {
        let mut state = self.0.lock().unwrap();
        match event {
            SseEvent::Snapshot { generation, entries } => {
                // 快照里的全量历史带着 `model[].timestamp`。只取时间，不取正文 ——
                // 历史的真相仍是 GET /v1/entries。
                let mut times = HashMap::new();
                for e in entries {
                    if let Some(ts) = e.timestamp() {
                        times.insert(entry_key(&e.id), ts);
                    }
                }
                if !times.is_empty() || !state.snapshot_seen {
                    state.times = times;
                    state.snapshot_seen = true;
                    state.times_dirty = true;
                }
                // 常连：刚连上（或重连）时引擎先推一帧 snapshot。有正在生成的消息就续上，
                // 没有就只当「连上了」。**别把它当错误，也别清空已有内容。**
                if let Some(text) = generation
                    .as_ref()
                    .and_then(|g| g.message.clone())
                    .and_then(|m| message_text(&Some(m)))
                    && !text.is_empty()
                {
                    state.blocks.clear();
                    state.blocks.insert(0, text);
                    state.done = false;
                    state.error = None;
                    state.dirty = true;
                }
                // 快照里**没有**正在生成的消息 = 此刻是空的（这一轮结束过、或者还没轮到它）。
                // UI 靠这个决定「再等 8 秒就判这一轮没起来」，不然会永远停在「停止」。
                state.idle_snapshot = generation
                    .as_ref()
                    .and_then(|g| g.message.as_ref())
                    .is_none();
                state.dirty = true;
            }
            SseEvent::MessageStart { message } => {
                if message.as_ref().and_then(|m| m.role.as_deref()) == Some("assistant") {
                    state.blocks.clear();
                    // 常连的流可能已经结束过一轮：`done` 不复位，下一轮的第一帧就会被
                    // 当成「结束了」，气泡当场被清掉。
                    state.done = false;
                    state.error = None;
                    state.dirty = true;
                }
            }
            SseEvent::MessageUpdate { changes } => {
                for change in changes {
                    match change {
                        // 块开头：block 里已经有全文（可能为空）。
                        nous_engine::Change::TextStart {
                            content_index,
                            block,
                        } => {
                            if let Some(text) = block_text(block) {
                                state.blocks.insert(*content_index, text);
                                state.dirty = true;
                            }
                        }
                        // 思考块不进气泡（那是思考，不是回答）。
                        nous_engine::Change::ThinkingStart { .. }
                        | nous_engine::Change::ThinkingDelta { .. } => {}
                        nous_engine::Change::ToolcallStart { .. }
                        | nous_engine::Change::ToolcallDelta { .. } => {}
                        nous_engine::Change::TextDelta {
                            content_index,
                            delta,
                        } => {
                            state
                                .blocks
                                .entry(*content_index)
                                .or_default()
                                .push_str(delta);
                            state.dirty = true;
                        }
                        nous_engine::Change::Block {
                            content_index,
                            block,
                        } => {
                            state
                                .blocks
                                .insert(*content_index, block_text(block).unwrap_or_default());
                            state.dirty = true;
                        }
                        nous_engine::Change::Message { message } => {
                            // 结束时给整条：以它为准。
                            if let Some(text) = message_text(message) {
                                state.blocks.clear();
                                state.blocks.insert(0, text);
                                state.dirty = true;
                            }
                        }
                        nous_engine::Change::Other => {}
                    }
                }
            }
            SseEvent::MessageEnd { entry } => {
                if entry.as_ref().map(|e| e.kind.as_str()) == Some("pi.assistant") {
                    state.done = true;
                    state.dirty = true;
                }
            }
            SseEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
            } => {
                let name = tool_name.clone().unwrap_or_else(|| "工具".into());
                state.tools.push(ToolRun {
                    id: tool_call_id.clone(),
                    name,
                    output: String::new(),
                });
                state.dirty = true;
            }
            SseEvent::ToolExecutionUpdate {
                tool_call_id,
                output,
            } => {
                let Some(output) = output else { return };
                let chunk = output
                    .get("append")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        output
                            .get("set")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                if let Some(run) = state.tools.iter_mut().find(|run| &run.id == tool_call_id) {
                    run.output.push_str(&chunk);
                    state.dirty = true;
                }
            }
            SseEvent::Submission { record } => {
                if record.as_ref().and_then(|r| r.status.as_deref()) == Some("unanswered") {
                    let why = record
                        .as_ref()
                        .and_then(|r| r.detail.clone().or_else(|| r.reason.clone()))
                        .unwrap_or_else(|| "没答上来".into());
                    state.error = Some(why);
                    state.done = true;
                    state.dirty = true;
                }
            }
            SseEvent::TaskFailed { message } => {
                state.error = Some(message.clone().unwrap_or_else(|| "任务挂了".into()));
                state.done = true;
                state.dirty = true;
            }
            _ => {}
        }
    }

    pub fn fail(&self, message: String) {
        let mut state = self.0.lock().unwrap();
        state.error = Some(message);
        state.done = true;
        state.dirty = true;
    }

    /// UI 侧：有变化就把整份文字取走（按 contentIndex 拼好）。没有就回 None。
    pub fn drain(&self) -> Option<Drained> {
        let mut state = self.0.lock().unwrap();
        if !state.dirty {
            return None;
        }
        state.dirty = false;
        let mut indexes: Vec<usize> = state.blocks.keys().copied().collect();
        indexes.sort_unstable();
        let text = indexes
            .into_iter()
            .filter_map(|i| state.blocks.get(&i).cloned())
            .collect::<Vec<_>>()
            .join("");
        Some(Drained {
            text,
            done: state.done,
            error: state.error.clone(),
            idle_snapshot: state.idle_snapshot,
            tools: state.tools.clone(),
            times: if state.times_dirty {
                state.times_dirty = false;
                Some(state.times.clone())
            } else {
                None
            },
        })
    }
}

pub struct Drained {
    pub text: String,
    pub done: bool,
    pub error: Option<String>,
    /// 见 `LiveState::idle_snapshot`。
    pub idle_snapshot: bool,
    pub tools: Vec<ToolRun>,
    /// 首帧时间戳有没有变（变了就把整张表递给 UI，目录重算一次）。
    pub times: Option<HashMap<String, i64>>,
}

/// 从 `{type:"text", text:"…"}` 这种块里取正文。不是 text 块就返回 None。
fn block_text(block: &Option<serde_json::Value>) -> Option<String> {
    let block = block.as_ref()?;
    if block.get("type").and_then(|t| t.as_str()) != Some("text") {
        return None;
    }
    Some(block.get("text").and_then(|t| t.as_str())?.to_string())
}

/// 从整条 assistant message 里把 text 块拼起来（`content` 是块数组）。
fn message_text(message: &Option<serde_json::Value>) -> Option<String> {
    let message = message.as_ref()?;
    if message.get("role").and_then(|r| r.as_str()) != Some("assistant") {
        return None;
    }
    let content = message.get("content")?.as_array()?;
    let mut out = String::new();
    for block in content {
        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                out.push_str(text);
            }
        }
    }
    Some(out)
}

// 测试名用中文写：读起来就是那句断言。非 snake_case 的告警在这里关掉。
#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use nous_engine::{Change, SseEvent};

    fn text_start(index: usize, text: &str) -> SseEvent {
        SseEvent::MessageUpdate {
            changes: vec![Change::TextStart {
                content_index: index,
                block: Some(serde_json::json!({ "type": "text", "text": text })),
            }],
        }
    }

    fn text_delta(index: usize, delta: &str) -> SseEvent {
        SseEvent::MessageUpdate {
            changes: vec![Change::TextDelta {
                content_index: index,
                delta: delta.into(),
            }],
        }
    }

    fn assistant_start() -> SseEvent {
        SseEvent::MessageStart {
            message: Some(nous_engine::sse::RoleMessage {
                role: Some("assistant".into()),
            }),
        }
    }

    fn assistant_end() -> SseEvent {
        SseEvent::MessageEnd {
            entry: Some(nous_engine::sse::KindEntry {
                kind: "pi.assistant".into(),
                extra: Default::default(),
            }),
        }
    }

    #[test]
    fn 文字按_delta_累加_取走之后不再重复给() {
        let live = Live::new();
        live.ingest(&assistant_start());
        live.ingest(&text_start(0, ""));
        live.ingest(&text_delta(0, "你"));
        live.ingest(&text_delta(0, "好"));

        let d1 = live.drain().expect("有变化就该给");
        assert_eq!(d1.text, "你好");
        assert!(!d1.done);
        // UI 每 16ms 取一次：没新内容就回 None，别让 UI 白重绘。
        assert!(live.drain().is_none(), "第二次 drain 应该是 None");
    }

    #[test]
    fn 多个_contentIndex_按序拼接() {
        let live = Live::new();
        live.ingest(&assistant_start());
        live.ingest(&text_delta(1, "后半"));
        live.ingest(&text_delta(0, "前半"));
        let d = live.drain().expect("drain");
        // 引擎给的 contentIndex 是权威顺序，不能按到达顺序拼。
        assert_eq!(d.text, "前半后半");
    }

    #[test]
    fn message_change_整条替换会覆盖已有块() {
        let live = Live::new();
        live.ingest(&assistant_start());
        live.ingest(&text_delta(0, "写了一半"));
        let _ = live.drain();

        // 结束时引擎给整条正文 —— 以它为准。
        live.ingest(&SseEvent::MessageUpdate {
            changes: vec![Change::Message {
                message: Some(serde_json::json!({
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "最终正文" }]
                })),
            }],
        });
        assert_eq!(live.drain().expect("drain").text, "最终正文");
    }

    #[test]
    fn 新一轮_messageStart_清掉上一轮的字() {
        let live = Live::new();
        live.ingest(&assistant_start());
        live.ingest(&text_delta(0, "上一轮"));
        let _ = live.drain();

        live.ingest(&assistant_start());
        live.ingest(&text_delta(0, "这一轮"));
        assert_eq!(live.drain().expect("drain").text, "这一轮");
    }

    #[test]
    fn 思考块不进正文() {
        let live = Live::new();
        live.ingest(&assistant_start());
        live.ingest(&SseEvent::MessageUpdate {
            changes: vec![
                Change::ThinkingStart {
                    content_index: 0,
                    block: None,
                },
                Change::ThinkingDelta {
                    content_index: 0,
                    delta: "我在想".into(),
                },
            ],
        });
        // 思考不是回答：气泡里不该出现它的字。
        let d = live.drain().expect("dirty 应该被置上了");
        assert_eq!(d.text, "");
    }

    #[test]
    fn 收尾时_done_置上_正文还在() {
        let live = Live::new();
        live.ingest(&assistant_start());
        live.ingest(&text_delta(0, "答完了"));
        live.ingest(&assistant_end());
        let d = live.drain().expect("drain");
        assert!(d.done, "收尾要告诉 UI 该重拉历史了");
        assert_eq!(d.text, "答完了");
    }

    #[test]
    fn 别的_entry_kind_不算收尾() {
        let live = Live::new();
        live.ingest(&SseEvent::MessageEnd {
            entry: Some(nous_engine::sse::KindEntry {
                kind: "pi.user".into(),
                extra: Default::default(),
            }),
        });
        assert!(live.drain().is_none(), "用户消息结束不该收尾");
    }

    #[test]
    fn 没答上来要带原因() {
        let live = Live::new();
        live.ingest(&SseEvent::Submission {
            record: Some(nous_engine::sse::SubmissionRecord {
                status: Some("unanswered".into()),
                reason: Some("模型没回".into()),
                detail: None,
            }),
        });
        let d = live.drain().expect("drain");
        assert!(d.done);
        assert_eq!(d.error.as_deref(), Some("模型没回"));
    }

    #[test]
    fn task_failed_要带消息() {
        let live = Live::new();
        live.ingest(&SseEvent::TaskFailed {
            message: Some("炸了".into()),
        });
        let d = live.drain().expect("drain");
        assert_eq!(d.error.as_deref(), Some("炸了"));
        assert!(d.done);
    }

    #[test]
    fn 新旧两轮不是同一份_live() {
        // 旧循环靠这个判断自己该退场，别去改新一轮的 streaming。
        let a = Live::new();
        let b = Live::new();
        assert!(a.same_round(&a));
        assert!(a.same_round(&a.clone()));
        assert!(!a.same_round(&b));
    }

    #[test]
    fn 空快照说明此刻没在生成() {
        // 上游报错时失败事件可能早于我们订阅，之后只剩这一个空 snapshot。
        // UI 靠 idle_snapshot 决定「再等 8 秒就判这一轮没起来」，不然永远停在「停止」。
        let live = Live::new();
        live.ingest(&SseEvent::Snapshot { generation: None, entries: vec![] });
        let d = live.drain().expect("快照也算一次变化");
        assert!(d.idle_snapshot);
        assert!(d.text.is_empty());
        assert!(!d.done);
    }

    #[test]
    fn 有正在生成的消息就不是空快照() {
        let live = Live::new();
        live.ingest(&SseEvent::Snapshot {
            generation: Some(nous_engine::sse::Generation {
                message: Some(serde_json::json!({ "role": "assistant" })),
            }),
            entries: vec![],
        });
        assert!(!live.drain().expect("drain").idle_snapshot);
    }

    #[test]
    fn 工具输出累加() {
        let live = Live::new();
        live.ingest(&SseEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: Some("read".into()),
        });
        for chunk in ["第一段", "第二段"] {
            live.ingest(&SseEvent::ToolExecutionUpdate {
                tool_call_id: "t1".into(),
                output: Some(serde_json::json!({ "append": chunk })),
            });
        }
        let d = live.drain().expect("drain");
        assert_eq!(d.tools.len(), 1);
        assert_eq!(d.tools[0].name, "read", "名字不能被拼进输出里");
        assert_eq!(d.tools[0].output, "第一段第二段");
    }
}
