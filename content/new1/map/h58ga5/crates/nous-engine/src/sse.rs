//! SSE 事件解析：把 `bytes_stream` 转成 `SseEvent` 流。
//!
//! 只做两件事：按空行切帧、把 `data:` 行喂给 serde。不存状态、不攒整条消息 ——
//! 攒是 UI 层的事（16ms 合帧）。
//!
//! 切帧按**字节**做，不先转 String：一个中文字 3 字节，TCP 完全可能在中间切开，
//! 逐 chunk 转码就会把半个字变成 U+FFFD（流式回答里偶发「丢字/花屏」）。
//! 所以：字节缓冲 → 切出完整帧 → 再 UTF-8 解码。
//!
//! 心跳（`: keepalive`）和 `retry:` 这种没有 `data:` 的帧**静默跳过**：
//! 它们在 SSE 规范里是合法的，当成错误会让整条流在生成途中断掉。

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
        /// 首帧带全量历史（含 `model[].timestamp`）。没带就是空表，别编时间。
        #[serde(default)]
        entries: Vec<SnapshotEntry>,
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

/// SSE 首帧 `snapshot.entries[]` 里的一条。
///
/// `GET /v1/entries` 不给时间，但快照里每条的 `model[].timestamp` 带着落库时刻
/// （epoch ms）。桌面端按天分组目录读它 —— 引擎没给就没有，不拿别的冒充。
/// 正文也在这里，但**只拿时间**：历史的真相仍以 `GET /v1/entries` 为准。
#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotEntry {
    pub id: serde_json::Value,
    pub kind: String,
    #[serde(default)]
    pub model: Vec<serde_json::Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

impl SnapshotEntry {
    /// 第一个 `model[].timestamp`，没有就是 `None`。
    pub fn timestamp(&self) -> Option<i64> {
        self.model
            .iter()
            .filter_map(|m| m.get("timestamp").and_then(|v| v.as_i64()))
            .next()
    }

    /// id 统一成字符串，跟 `GET /v1/entries` 那边对得上。
    pub fn key(&self) -> String {
        match &self.id {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
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
        buf: Vec::new(),
        ready: VecDeque::new(),
    }
}

pub struct SseParser<S> {
    inner: S,
    /// **字节**缓冲。不能是 String：切在多字节字符中间会丢字（见文件头）。
    buf: Vec<u8>,
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
                    self.buf.extend_from_slice(&chunk);
                    self.drain_frames();
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(SseError::Transport(e.to_string()))));
                }
                Poll::Ready(None) => {
                    // 流到底了，把残余 buf 当最后一帧（可能不完整，丢了也不该崩）。
                    if !self.buf.is_empty() {
                        let rest = std::mem::take(&mut self.buf);
                        if let Some(item) = decode_frame(&String::from_utf8_lossy(&rest)) {
                            self.ready.push_back(item);
                        }
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
        while let Some((end, skip)) = frame_boundary(&self.buf) {
            let frame: Vec<u8> = self.buf[..end].to_vec();
            self.buf.drain(..end + skip);
            // 空帧（连续空行）和白帧不当事件，但也不报错。
            if frame.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            if let Some(item) = decode_frame(&String::from_utf8_lossy(&frame)) {
                self.ready.push_back(item);
            }
        }
    }
}

/// 找最早的帧边界，返回 `(帧尾下标, 分隔符长度)`。
///
/// SSE 规范两种都合法：`\n\n`（引擎现在用的）和 `\r\n\r\n`（反代/CDN 会改写）。
/// 只看 `\n\n` 的话，一旦哪天前面站着个会改写行尾的代理，整条流就再也不出帧了。
fn frame_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = find_sub(buf, b"\n\n").map(|i| (i, 2));
    let crlf = find_sub(buf, b"\r\n\r\n").map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

/// 最早的一处出现位置。手写比引一个 memchr 划算。
fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// 解一帧。
///
/// 返回 `None` = 这帧不该变成事件（心跳 `: keepalive`、`retry:` 之类没有 `data:` 的行），
/// **静默跳过**。以前这里返回 Err，结果心跳一来整条流就被判「生成失败」。
fn decode_frame(frame: &str) -> Option<Result<SseEvent, SseError>> {
    // 一帧里可能有多个 `data:` 行，拼接后 parse。qrd3wh 一个事件一个 data 行，
    // 这里 join 是为了以后万一多行 data 也不崩。
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .collect::<Vec<_>>()
        .join("\n");

    if data.is_empty() {
        return None;
    }
    // data 行存在但 JSON 坏：**照旧报错**。引擎改了字段就该在我们这里炸，
    // 静默吞掉等于让协议漂移没人知道。
    Some(serde_json::from_str(&data).map_err(|e| SseError::Parse(format!("{e}：{data}"))))
}

// 测试名用中文写：读起来就是那句断言。非 snake_case 的告警在这里关掉。
#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    /// 把字节块喂进解析器，用 noop waker 一直 poll 到结束。
    /// `stream::iter` 永远立刻 Ready，不需要 async runtime。
    fn collect_chunks(chunks: Vec<Vec<u8>>) -> Vec<Result<SseEvent, SseError>> {
        let items: Vec<Result<Bytes, reqwest::Error>> =
            chunks.into_iter().map(|c| Ok(Bytes::from(c))).collect();
        let mut parser = parse(futures_util::stream::iter(items));
        let waker = futures_util::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut out = Vec::new();
        loop {
            match Pin::new(&mut parser).poll_next(&mut cx) {
                Poll::Ready(Some(item)) => out.push(item),
                // 真实流里 Pending 会等网络；这里 iter 不会 Pending，但别写死。
                Poll::Ready(None) | Poll::Pending => break,
            }
        }
        out
    }

    fn collect(text: &str) -> Vec<Result<SseEvent, SseError>> {
        collect_chunks(vec![text.as_bytes().to_vec()])
    }

    fn event(text: &str) -> SseEvent {
        let mut all = collect(text);
        assert_eq!(all.len(), 1, "应当只有一帧，实际 {} 帧", all.len());
        all.remove(0).expect("这一帧不该报错")
    }

    #[test]
    fn 一帧解析出事件() {
        match event("data: {\"type\":\"task_failed\",\"message\":\"boom\"}\n\n") {
            SseEvent::TaskFailed { message } => assert_eq!(message.as_deref(), Some("boom")),
            other => panic!("事件不对：{other:?}"),
        }
    }

    #[test]
    fn 一个块里两帧按顺序出() {
        let all = collect(
            "data: {\"type\":\"message_start\",\"message\":{\"role\":\"assistant\"}}\n\n\
             data: {\"type\":\"message_end\",\"entry\":{\"kind\":\"pi.assistant\"}}\n\n",
        );
        assert_eq!(all.len(), 2);
        assert!(matches!(
            all[0].as_ref().unwrap(),
            SseEvent::MessageStart { .. }
        ));
        assert!(matches!(
            all[1].as_ref().unwrap(),
            SseEvent::MessageEnd { .. }
        ));
    }

    #[test]
    fn crlf_行尾也认() {
        // 反代 / CDN 会把行尾改写成 \r\n，只认 \n\n 的话整条流就不出帧了。
        match event("data: {\"type\":\"task_failed\",\"message\":\"crlf\"}\r\n\r\n") {
            SseEvent::TaskFailed { message } => assert_eq!(message.as_deref(), Some("crlf")),
            other => panic!("事件不对：{other:?}"),
        }
    }

    #[test]
    fn 心跳帧静默跳过() {
        // 以前这里报「没有 data 的帧」，UI 层当生成失败 —— 一条心跳就能毁掉整次回答。
        let all = collect(": keepalive\n\n: ping\n\ndata: {\"type\":\"task_failed\"}\n\n");
        assert_eq!(all.len(), 1);
        assert!(all[0].is_ok());
    }

    #[test]
    fn retry_帧静默跳过() {
        assert!(collect("retry: 3000\n\n").is_empty());
    }

    #[test]
    fn 中文被切在两个_chunk_中间不丢字() {
        // 这是真 bug 的回归测试：流式回答里偶发「丢字/花屏」。
        let mut raw = "data: {\"type\":\"message_update\",\"changes\":[{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"你好世界\"}]}\n\n"
            .as_bytes()
            .to_vec();
        let cut = raw
            .windows(3)
            .position(|w| w == "你".as_bytes())
            .expect("应当有中文")
            + 1; // 正好切在多字节字符中间
        let tail = raw.split_off(cut);
        let head = raw;
        let all = collect_chunks(vec![head, tail]);
        assert_eq!(all.len(), 1);
        match all.into_iter().next().unwrap().unwrap() {
            SseEvent::MessageUpdate { changes } => match &changes[0] {
                Change::TextDelta { delta, .. } => assert_eq!(delta, "你好世界"),
                other => panic!("change 不对：{other:?}"),
            },
            other => panic!("事件不对：{other:?}"),
        }
    }

    #[test]
    fn 多行_data_拼起来() {
        let all = collect("data: {\"type\":\"task_failed\",\ndata: \"message\":\"joined\"}\n\n");
        assert_eq!(all.len(), 1);
        match all.into_iter().next().unwrap().unwrap() {
            SseEvent::TaskFailed { message } => assert_eq!(message.as_deref(), Some("joined")),
            other => panic!("事件不对：{other:?}"),
        }
    }

    #[test]
    fn 未知事件落进_other() {
        assert!(matches!(
            event("data: {\"type\":\"brand_new_event\"}\n\n"),
            SseEvent::Other
        ));
    }

    #[test]
    fn 未知_change_落进_other() {
        match event(
            "data: {\"type\":\"message_update\",\"changes\":[{\"type\":\"brand_new_change\"}]}\n\n",
        ) {
            SseEvent::MessageUpdate { changes } => {
                assert!(matches!(changes[0], Change::Other));
            }
            other => panic!("事件不对：{other:?}"),
        }
    }

    #[test]
    fn 尾部残缺帧不会崩() {
        // 不完整的 JSON 仍然要报错（引擎改了字段就该在我们这儿炸），但不能 panic。
        let all = collect("data: {\"type\":\"snap");
        assert_eq!(all.len(), 1);
        assert!(all[0].is_err());
    }

    #[test]
    fn 空帧不算事件() {
        assert!(collect("\n\n\n\n").is_empty());
    }

    /// 首帧 snapshot 里的 entries 带着 `model[].timestamp` —— 桌面端按天分组
    /// 目录就靠它。`generation` 没有也不影响，别让缺字段把整帧丢了。
    #[test]
    fn snapshot_里的历史时间戳() {
        let data = r#"{"type":"snapshot","generation":null,"entries":[
            {"id":9,"kind":"pi.user","model":[{"role":"user","timestamp":1793156783434}]},
            {"id":12,"kind":"pi.system","model":[{"role":"system"}]}
        ]}"#;
        let ev: SseEvent = serde_json::from_str(data).expect("解析");
        let SseEvent::Snapshot { entries, .. } = ev else { panic!("不是快照") };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key(), "9");
        assert_eq!(entries[0].timestamp(), Some(1793156783434));
        assert_eq!(entries[1].timestamp(), None, "没给的就不编");
    }

    /// 老形状（没有 entries 字段）照样能解，别把旧引擎一棍子打死。
    #[test]
    fn 快照没有_entries_也能解() {
        let ev: SseEvent = serde_json::from_str(r#"{"type":"snapshot","generation":null}"#)
            .expect("解析");
        let SseEvent::Snapshot { entries, .. } = ev else { panic!("不是快照") };
        assert!(entries.is_empty());
    }
}
