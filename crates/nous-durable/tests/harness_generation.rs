//! `stream_response` / `classify` / `convert_partial` 的判据。**核心是时间线，不是值**：
//!
//! 1. **节拍**：两次 partial 提交至少隔 `interval_ms`；
//! 2. **单飞**：没有两次提交重叠（`PartialEnd(k)` 一定早于 `PartialStart(k+1)`）；
//! 3. **终态在后**：`classify` 的终态提交之后，时间线上不许再出现 partial；
//! 4. **停表要真停**：冷却几个 `interval_ms`，提交次数不许再涨。
//!
//! 时钟用 `start_paused`：虚拟时间只会被 `sleep` / `sleep_until` 推进，所以四条都能直接读时间线
//! 断言，不用 `sleep` 猜。值判据在这条路径上永远是绿的（partial 写进去的正文与终态一致），
//! 所以一条也不用来当主判据。

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use nous_ai::{
    Api, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, StopReason, Usage,
};
use nous_durable::harness::abort::convert_partial;
use nous_durable::harness::generation::{
    classify, live_record, stream_response, Beat, Classification, ClassifyRequest, RetryPolicy,
    StreamRequest, Timeline,
};
use nous_durable::harness::live::LiveState;
use nous_durable::session::Session;
use nous_durable::{DocumentContent, StorageWrite};
use serde_json::{json, Value};

fn database(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nous-durable-generation-{tag}-{}-{:?}.sqlite",
        std::process::id(),
        std::thread::current().id()
    ));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    path
}

const LIVE: i64 = 100;
const CONVERSATION: i64 = 1;

/// 一份库：一条 conversation + 一份空的 `pi.live`。
async fn open(tag: &str) -> Session {
    let session = Session::open(database(tag)).expect("能打开");
    session
        .commit(
            vec![
                StorageWrite::Conversation(json!({ "id": CONVERSATION })),
                StorageWrite::DocumentCreate {
                    id: LIVE,
                    record: live_record(LIVE, CONVERSATION),
                    content: DocumentContent::Base {
                        version: 1,
                        value: json!({}),
                    },
                },
            ],
            Default::default(),
        )
        .await
        .expect("能建起来");
    session
}

async fn live_value(session: &Session) -> Value {
    session
        .snapshot(LIVE)
        .await
        .expect("能读")
        .expect("文档在")
        .value
}

async fn live_state(session: &Session) -> LiveState {
    serde_json::from_value(live_value(session).await).expect("能解析")
}

fn empty_partial() -> Arc<AssistantMessage> {
    Arc::new(AssistantMessage::empty(Api::Faux, "faux", "faux", 0))
}

/// 造一条「正文已经长到 `text`」的 partial。
fn partial_with_text(text: &str) -> Arc<AssistantMessage> {
    let mut message = AssistantMessage::empty(Api::Faux, "faux", "faux", 0);
    message.content = serde_json::from_value(json!([{ "type": "text", "text": text }]))
        .expect("块能解析");
    Arc::new(message)
}

fn terminal_message(text: &str) -> AssistantMessage {
    let mut message = AssistantMessage::empty(Api::Faux, "faux", "faux", 0);
    message.content = serde_json::from_value(json!([{ "type": "text", "text": text }]))
        .expect("块能解析");
    message.stop_reason = StopReason::Stop;
    message.usage = Usage::zero();
    message
}

/// 一条流：每 `gap` 吐一片正文，最后给终态。
fn streaming(
    chunks: Vec<String>,
    gap: Duration,
) -> AssistantMessageEventStream {
    let (mut producer, stream) = nous_ai::event_stream::create_assistant_message_event_stream();
    tokio::spawn(async move {
        let mut text = String::new();
        for chunk in chunks {
            tokio::time::sleep(gap).await;
            text.push_str(&chunk);
            let partial = partial_with_text(&text);
            if producer
                .push(AssistantMessageEvent::TextDelta {
                    content_index: 0,
                    delta: chunk,
                    partial,
                })
                .await
                == false
            {
                return;
            }
        }
        let message = terminal_message(&text);
        producer
            .push(AssistantMessageEvent::Done {
                reason: nous_ai::DoneReason::Stop,
                message,
            })
            .await;
        producer.close();
    });
    stream
}

fn request(interval_ms: u64) -> StreamRequest {
    StreamRequest {
        live: LIVE,
        conversation: CONVERSATION,
        attempt: 1,
        interval_ms,
    }
}

fn classify_request(entry_id: i64) -> ClassifyRequest {
    ClassifyRequest {
        live: LIVE,
        conversation: CONVERSATION,
        attempt: 1,
        entry_id,
        retry: RetryPolicy::default(),
        retryable: false,
        overflow: None,
    }
}

/// 时间线的违规清单。空 = 四条都守住了。抽出来是为了能用**造出来的**时间线证明每条都会红
/// （见 `every_timeline_face_has_a_fake_that_turns_it_red`）—— 只跑真实现的话，这些断言的
/// 实际含义永远无法验证，它们只是「反正都通过」。
fn timeline_violations(beats: &[(Beat, Instant)], interval_ms: u64) -> Vec<String> {
    let interval = Duration::from_millis(interval_ms);
    let mut violations = Vec::new();

    // ① 节拍。
    let starts: Vec<Instant> = beats
        .iter()
        .filter(|(beat, _)| *beat == Beat::PartialStart)
        .map(|(_, at)| *at)
        .collect();
    for pair in starts.windows(2) {
        let gap = pair[1].duration_since(pair[0]);
        if gap < interval {
            violations.push(format!("两次 partial 提交只隔了 {gap:?}，闸是 {interval:?}"));
        }
    }

    // ② 单飞：每次开始都在上一次落库之后。
    for (index, (beat, _)) in beats.iter().enumerate() {
        if *beat != Beat::PartialStart {
            continue;
        }
        let started_before_end = beats[..index]
            .iter()
            .rev()
            .find(|(beat, _)| *beat == Beat::PartialEnd)
            .map(|(_, at)| *at);
        if let Some(end) = started_before_end {
            if beats[index].1 < end {
                violations.push(format!("第 {index} 拍在上一次落库之前开始 —— 两次提交重叠了"));
            }
        }
    }

    // ③ 终态在后：终态那一拍之后不许再有 partial。
    if let Some(position) = beats.iter().position(|(beat, _)| *beat == Beat::Terminal) {
        let after: Vec<_> = beats[position + 1..]
            .iter()
            .filter(|(beat, _)| *beat == Beat::PartialStart)
            .collect();
        if !after.is_empty() {
            violations.push(format!("终态之后又落了 {} 次 partial", after.len()));
        }
    }

    violations
}

/// 四面一起判：节拍、单飞、终态在后、停表。
#[tokio::test(start_paused = true)]
async fn harness_stream_commits_partials_at_most_every_interval() {
    let session = open("timeline").await;
    let timeline = Timeline::new();
    let chunks: Vec<String> = (0..40).map(|_| "0123456789".to_string()).collect();
    let total: String = chunks.concat();

    let began = Instant::now();
    let message = stream_response(
        &session,
        &request(100),
        streaming(chunks.clone(), Duration::from_millis(10)),
        Some(&timeline),
    )
    .await
    .expect("流能跑完");

    // **等号，不是区间。** `start_paused` 下虚拟时钟精确到毫秒、同一份输入每次得到同一个数，
    // 所以「大概几次」是把这个数放宽成了一个抓不住东西的区间 —— 区间 2..=5 恰好给「在 finally
    // 里顺手补一帧」（4 次）留了位置。
    //
    // 算得出来：第一片 10 ms 到，闸定在 10+100=110 ms；此后每 100 ms 一次（落库之后才重新
    // 起闸），110 / 210 / 310；流在 400 ms 结束、410 ms 给终态，第四次闸（410 之后）永远到
    // 不了 ⇒ 恰好 3 次。
    let partials = timeline.count(Beat::PartialStart);
    assert_eq!(
        partials, 3,
        "40 片 × 10ms = 400ms 的流、闸 100ms ⇒ 恰好 3 次 partial 提交（110 / 210 / 310 ms）"
    );

    // 等号顺带钉住「闸在哪一刻第一次触发」：110 / 210 / 310 ms，不是 10 / 110 / 210。
    let starts: Vec<Instant> = timeline
        .beats()
        .into_iter()
        .filter(|(beat, _)| *beat == Beat::PartialStart)
        .map(|(_, at)| at)
        .collect();
    for (index, at) in starts.iter().enumerate() {
        let expected = Duration::from_millis(110 + 100 * index as u64);
        assert_eq!(
            at.duration_since(began),
            expected,
            "第 {index} 次 partial 提交的时刻不对（闸第一次就该在 110 ms）"
        );
    }

    // ①② 节拍与单飞（真流）。
    let violations = timeline_violations(&timeline.beats(), 100);
    assert!(violations.is_empty(), "违规：{violations:#?}");

    // 值判据只当兜底：partial 落进去的东西必须等于流到此刻的正文。
    let state = live_state(&session).await;
    let streamed = state
        .generation
        .as_ref()
        .and_then(|generation| generation.message.as_ref())
        .and_then(|message| message.get("content"))
        .and_then(|content| content.get(0))
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        total.starts_with(&streamed),
        "库里 {streamed:?} 不是流进来那段的前缀"
    );

    // ③ 终态在后。
    let terminal = classify(
        &session,
        &classify_request(9_001),
        &message,
        0,
        Some(&timeline),
    )
    .await
    .expect("终态能提交");
    assert_eq!(terminal, Classification::Answered);
    let violations = timeline_violations(&timeline.beats(), 100);
    assert!(violations.is_empty(), "违规：{violations:#?}");
    assert_eq!(
        live_state(&session).await.generation,
        None,
        "终态提交要清掉 generation（整键消失）"
    );

    // ④ 停表要真停：冷却 3 个 interval，提交次数不许再涨。
    let before = timeline.count(Beat::PartialStart);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        timeline.count(Beat::PartialStart),
        before,
        "停了之后还有 timer 在跑 —— stopped 只让回调 return，没取消定时器"
    );
}

/// 四条断言的**反向对照**：每条都用造出来的时间线/定时器证明它会红。
#[tokio::test(start_paused = true)]
async fn every_timeline_face_has_a_fake_that_turns_it_red() {
    let base = Instant::now();
    let at = |ms: u64| base + Duration::from_millis(ms);

    // ① 节拍：两次提交只隔 10ms（闸 100ms）。
    let too_fast = vec![
        (Beat::PartialStart, at(0)),
        (Beat::PartialEnd, at(1)),
        (Beat::PartialStart, at(10)),
        (Beat::PartialEnd, at(11)),
    ];
    assert!(
        timeline_violations(&too_fast, 100)
            .iter()
            .any(|line| line.contains("只隔了")),
        "节拍断言抓不住 10ms 两次提交"
    );

    // ② 单飞：第二次开始（5ms）早于第一次落库（20ms）。
    let overlapping = vec![
        (Beat::PartialStart, at(0)),
        (Beat::PartialEnd, at(20)),
        (Beat::PartialStart, at(5)),
        (Beat::PartialEnd, at(25)),
    ];
    assert!(
        timeline_violations(&overlapping, 100)
            .iter()
            .any(|line| line.contains("重叠")),
        "单飞断言抓不住重叠提交"
    );

    // ③ 终态在后：终态（100ms）之后又来了一次 partial（200ms）。
    let late_partial = vec![
        (Beat::PartialStart, at(0)),
        (Beat::PartialEnd, at(1)),
        (Beat::Terminal, at(100)),
        (Beat::PartialStart, at(200)),
        (Beat::PartialEnd, at(201)),
    ];
    assert!(
        timeline_violations(&late_partial, 100)
            .iter()
            .any(|line| line.contains("终态之后")),
        "终态在后断言抓不住「先终态、后 partial」"
    );

    // ④ 停表：造一个「stopped 只让回调 return、定时器还活着」的泄漏 —— 独立任务在冷却期内
    //    继续往时间线上记拍子。真判据写的是 `count(冷却后) == count(之前)`，这里断言它**不**
    //    相等，于是「冷却断言能看见活着的定时器」这件事被证明了。
    let probe = Arc::new(Timeline::new());
    let leaked = {
        let probe = Arc::clone(&probe);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            probe.record(Beat::PartialStart);
        })
    };
    let before = probe.count(Beat::PartialStart);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        probe.count(Beat::PartialStart) > before,
        "冷却断言看不见泄漏的定时器 —— 那它对真实现也是空的"
    );
    leaked.abort();
}

/// 终态 entry 是完整消息，`pi.live` 里 `generation` 整键消失（不是 `null`）。
#[tokio::test(start_paused = true)]
async fn the_terminal_commit_appends_the_answer_and_clears_the_partial() {
    let session = open("terminal").await;
    let message = stream_response(
        &session,
        &request(100),
        streaming(vec!["he".to_string(), "llo".to_string()], Duration::from_millis(10)),
        None,
    )
    .await
    .expect("流能跑完");
    classify(&session, &classify_request(9_002), &message, 0, None)
        .await
        .expect("终态能提交");

    let value = live_value(&session).await;
    assert!(
        value.get("generation").is_none(),
        "`generation` 必须整键消失，实际是 {value}"
    );
    let (entry, _) = session
        .get_entry(9_002)
        .await
        .expect("能读")
        .expect("entry 在");
    assert_eq!(entry.kind, "pi.assistant");
    let model = entry.model.expect("assistant entry 带 model");
    assert_eq!(model[0]["content"][0]["text"], json!("hello"));
}

/// 可重试的错误：entry 落库，`generation` 转 `retry`（带退避后的时间点）。
#[tokio::test(start_paused = true)]
async fn a_retryable_error_turns_the_generation_into_a_retry_marker() {
    let session = open("retry").await;
    let mut message = terminal_message("");
    message.stop_reason = StopReason::Error;
    message.error_message = Some("rate limited".to_string());
    let mut request = classify_request(9_003);
    request.retryable = true;

    let classification = classify(&session, &request, &message, 10_000, None)
        .await
        .expect("能提交");
    let at_ms = 10_000 + RetryPolicy::default().delay_for(1);
    assert_eq!(
        classification,
        Classification::Retry {
            at_ms,
            error: "rate limited".to_string()
        }
    );
    let retry = live_state(&session).await.generation.expect("有 generation").retry;
    let retry = retry.expect("有 retry");
    assert_eq!(retry.at, at_ms);
    assert_eq!(retry.error, "rate limited");
}

/// 上下文溢出：**不可重试**，entry 落库，`generation` 清掉。
#[tokio::test(start_paused = true)]
async fn an_overflow_is_never_retried() {
    let session = open("overflow").await;
    let mut message = terminal_message("");
    message.stop_reason = StopReason::Error;
    message.error_message = Some("context too long".to_string());
    let mut request = classify_request(9_004);
    request.retryable = true; // 就算调用方说是可重试的，溢出不重试
    request.overflow = Some("context too long".to_string());

    let classification = classify(&session, &request, &message, 0, None)
        .await
        .expect("能提交");
    assert_eq!(
        classification,
        Classification::Overflow {
            error: "context too long".to_string()
        }
    );
    assert!(live_state(&session).await.generation.is_none());
}

/// abort：把在流的 partial 落成一条 `stopReason: "aborted"` 的 assistant entry。
#[tokio::test(start_paused = true)]
async fn convert_partial_writes_an_aborted_assistant_entry() {
    let session = open("aborted").await;
    // 20 片 × 10ms = 200ms，闸 100ms ⇒ 至少落过一次 partial（尾帧不提交，所以流必须够长）。
    let chunks: Vec<String> = (0..20).map(|_| "x".to_string()).collect();
    stream_response(
        &session,
        &request(100),
        streaming(chunks, Duration::from_millis(10)),
        None,
    )
    .await
    .expect("流能跑完");
    // 流跑完但还没 classify：这时 partial 还在 `generation.message` 里。
    assert!(live_state(&session).await.generation.is_some());

    let entry_id = convert_partial(&session, LIVE, CONVERSATION, 9_005)
        .await
        .expect("能转换")
        .expect("写了一条");
    assert_eq!(entry_id, 9_005);
    let (entry, _) = session.get_entry(entry_id).await.expect("能读").expect("在");
    assert_eq!(entry.kind, "pi.assistant");
    let model = entry.model.expect("带 model");
    assert_eq!(model[0]["stopReason"], json!("aborted"));
    let text = model[0]["content"][0]["text"].as_str().expect("有正文");
    assert!(
        !text.is_empty() && "xxxxxxxxxxxxxxxxxxxx".starts_with(text),
        "落进去的必须是流到那一刻的正文，实际 {text:?}"
    );
    assert!(
        live_state(&session).await.generation.is_none(),
        "转换之后 generation 要清掉"
    );
}

/// abort 落在终态之后：`generation` 已经清掉，**不写第二条 entry**（committed state stays consistent）。
#[tokio::test(start_paused = true)]
async fn an_abort_after_the_terminal_commit_writes_nothing() {
    let session = open("abort-after").await;
    let message = stream_response(
        &session,
        &request(100),
        streaming(vec!["done".to_string()], Duration::from_millis(10)),
        None,
    )
    .await
    .expect("流能跑完");
    classify(&session, &classify_request(9_006), &message, 0, None)
        .await
        .expect("终态能提交");

    let converted = convert_partial(&session, LIVE, CONVERSATION, 9_007)
        .await
        .expect("不该失败");
    assert_eq!(converted, None, "终态之后 abort 不许再写一条 aborted entry");
    assert!(
        session.get_entry(9_007).await.expect("能读").is_none(),
        "9_007 不该被写进去"
    );
}

/// 「还没有内容」的 partial 不排提交：第一个事件是 `start` 时不落空帧。
#[tokio::test(start_paused = true)]
async fn an_empty_partial_never_commits() {
    let session = open("empty").await;
    let timeline = Timeline::new();
    let (mut producer, stream) = nous_ai::event_stream::create_assistant_message_event_stream();
    tokio::spawn(async move {
        producer
            .push(AssistantMessageEvent::Start {
                partial: empty_partial(),
            })
            .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        producer
            .push(AssistantMessageEvent::Done {
                reason: nous_ai::DoneReason::Stop,
                message: terminal_message(""),
            })
            .await;
        producer.close();
    });
    stream_response(&session, &request(100), stream, Some(&timeline))
        .await
        .expect("流能跑完");
    assert_eq!(
        timeline.count(Beat::PartialStart),
        0,
        "没有内容的 partial 不该落帧（客户端会闪一下空白）"
    );
    assert!(
        live_value(&session).await.get("generation").is_none(),
        "整条流没有内容，`pi.live` 不该长出 generation"
    );
}
