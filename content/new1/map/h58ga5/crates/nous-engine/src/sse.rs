//! SSE 事件解析：把 `bytes_stream` 转成 `SseEvent` 流。
//!
//! 只做两件事：按 `\n\n` 切帧、把 `data:` 行喂给 serde。不存状态、不攒整条消息 ——
//! 攒是 UI 层的事（16ms 合帧）。

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum SseError {
    #[error("HTTP 流断了：{0}")]
    Transport(String),
    #[error("一帧看不懂：{0}")]
    Parse(String),
}

/// qrd3wh `/v1/sessions/:id/events` 的一帧。
///
/// 字段名按引擎原样（camelCase），tag 是 `type`。我们关心的事件单独列出来，
/// 其余一律掉进 `Other` —— 引擎加新事件不该让我们编译失败。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseEvent {
    Snapshot {
        generation: Option<Generation>,
    },
    MessageStart {
        message: Option<RoleMessage>,
    },
    MessageUpdate {
        #[serde(default)]
        changes: Vec<Change>,
    },
    MessageEnd {
        entry: Option<KindEntry>,
    },
    EntryAppended {
        entry: Option<KindEntry>,
    },
    Submission {
        record: Option<SubmissionRecord>,
    },
    TaskFailed {
        message: Option<String>,
    },
    ToolExecutionStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: Option<String>,
    },
    ToolExecutionUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        output: Option<serde_json::Value>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Generation {
    pub message: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoleMessage {
    pub role: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KindEntry {
    pub kind: String,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubmissionRecord {
    pub status: Option<String>,
    pub reason: Option<String>,
    pub detail: Option<String>,
}

/// `message_update` 的 `changes[]`。
///
/// 全集来自 `@earendil-works/pi-durable` 的 `MessageChange`
/// （`dist/harness/events.d.ts`）—— **不是猜的**。少接一种就是「能发出去但字不出来」。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Change {
    /// 一个内容块开头，`block` 里已有（可能为空的）全文。
    TextStart {
        #[serde(rename = "contentIndex", default)]
        content_index: usize,
        block: Option<serde_json::Value>,
    },
    ThinkingStart {
        #[serde(rename = "contentIndex", default)]
        content_index: usize,
        block: Option<serde_json::Value>,
    },
    ToolcallStart {
        #[serde(rename = "contentIndex", default)]
        content_index: usize,
        block: Option<serde_json::Value>,
    },
    TextDelta {
        #[serde(rename = "contentIndex", default)]
        content_index: usize,
        delta: String,
    },
    ThinkingDelta {
        #[serde(rename = "contentIndex", default)]
        content_index: usize,
        delta: String,
    },
    ToolcallDelta {
        #[serde(rename = "contentIndex", default)]
        content_index: usize,
        delta: serde_json::Value,
    },
    /// 整块替换（一个块后续的变化都已在批里）。
    Block {
        #[serde(rename = "contentIndex", default)]
        content_index: usize,
        block: Option<serde_json::Value>,
    },
    Message {
        message: Option<serde_json::Value>,
    },
    #[serde(other)]
    Other,
}

/// 把 reqwest 的字节流解析成事件流。
pub fn parse<S>(inner: S) -> SseParser<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    SseParser {
        inner,
        buf: String::new(),
        ready: VecDeque::new(),
    }
}

pub struct SseParser<S> {
    inner: S,
    buf: String,
    ready: VecDeque<Result<SseEvent, SseError>>,
}

impl<S> Stream for SseParser<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<SseEvent, SseError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(item) = self.ready.pop_front() {
                return Poll::Ready(Some(item));
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buf.push_str(&String::from_utf8_lossy(&chunk));
                    self.drain_frames();
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(SseError::Transport(e.to_string()))));
                }
                Poll::Ready(None) => {
                    // 流到底了，把残余 buf 当最后一帧（可能不完整，丢了也不该崩）。
                    if !self.buf.trim().is_empty() {
                        let rest = std::mem::take(&mut self.buf);
                        self.ready.push_back(decode_frame(&rest));
                        continue;
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S> SseParser<S> {
    fn drain_frames(&mut self) {
        while let Some(pos) = self.buf.find("\n\n") {
            let frame = self.buf[..pos].to_string();
            self.buf.drain(..=pos + 1);
            if !frame.trim().is_empty() {
                self.ready.push_back(decode_frame(&frame));
            }
        }
    }
}

fn decode_frame(frame: &str) -> Result<SseEvent, SseError> {
    // 一帧里可能有多个 `data:` 行，拼接后 parse。qrd3wh 一个事件一个 data 行，
    // 这里 join 是为了以后万一多行 data 也不崩。
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .collect::<Vec<_>>()
        .join("\n");

    if data.is_empty() {
        return Err(SseError::Parse(format!("没有 data 的帧：{frame:?}")));
    }
    serde_json::from_str(&data).map_err(|e| SseError::Parse(format!("{e}：{data}")))
}
