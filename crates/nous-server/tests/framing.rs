//! 合帧判据。四条都是**同一份实现**的两个分支，不是两套转发器。
//!
//! 1. 1,000 个 `text_delta` 合帧后 `< 100` 帧，且按序拼回去**一个字不少**；
//! 2. 首帧在第一批 delta 到达后**一个窗口内**出现（`start_paused`，精确到毫秒）；
//! 3. `text_start` 的 `block.text` 已含全文：客户端规则（重置累加器 → 只加 delta）拼出来
//!    必须**正好**等于 `HEAD|` + deltas，不多不少；
//! 4. 负向对照 `Framing::Immediate` 同样 1,000 条 → `≥ 900` 帧。没有这一条，第 1 条的
//!    「帧少」可能只是因为它把字吞了。

use std::time::Duration;

use nous_server::{coalesce, Framing, FRAME_INTERVAL};
use nous_wire::{MessageChange, RemoteEvent};
use serde_json::json;
use tokio::sync::mpsc;

/// 让派生的合帧任务跑完手上的活（`start_paused` 下 `yield_now` 不会推进时钟）。
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

fn update(change: MessageChange) -> RemoteEvent {
    RemoteEvent::MessageUpdate {
        changes: vec![change],
        message_id: None,
        message: None,
    }
}

fn delta(text: impl Into<String>) -> RemoteEvent {
    update(MessageChange::TextDelta {
        content_index: 0,
        delta: text.into(),
    })
}

/// 客户端按 `wire.md` 应用补丁流的规则：`text_start` 重置累加器（`block.text` 已是全文），
/// `text_delta` 追加，`block` / `message` 以它为准。
fn applied_text(frames: &[RemoteEvent]) -> String {
    let mut text = String::new();
    for frame in frames {
        let RemoteEvent::MessageUpdate { changes, .. } = frame else {
            continue;
        };
        for change in changes {
            match change {
                MessageChange::TextStart { block, .. } | MessageChange::Block { block, .. } => {
                    text = block
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                }
                MessageChange::TextDelta { delta, .. } => text.push_str(delta),
                MessageChange::Message { message } => {
                    text = message
                        .get("content")
                        .and_then(serde_json::Value::as_array)
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                                .collect::<String>()
                        })
                        .unwrap_or_default();
                }
                _ => {}
            }
        }
    }
    text
}

/// 把 1,000 个 delta 灌进合帧器，再把输出全收下来。
async fn run_feeding_thousand(framing: Framing) -> (Vec<RemoteEvent>, String) {
    let (input_tx, input_rx) = mpsc::channel(2048);
    let (output_tx, mut output_rx) = mpsc::channel(2048);
    let task = coalesce::spawn(framing, input_rx, output_tx);

    let mut expected = String::new();
    for index in 0..1_000 {
        let text = match index {
            0 => "HEAD|".to_string(),
            999 => "|TAIL".to_string(),
            _ => format!("[{index:04}]"),
        };
        expected.push_str(&text);
        input_tx.send(delta(text)).await.expect("合帧器还活着");
    }

    // 合帧窗口不会自己过去：推一下时钟，再收。
    tokio::time::advance(FRAME_INTERVAL + Duration::from_millis(1)).await;

    let mut frames = Vec::new();
    loop {
        match tokio::time::timeout(FRAME_INTERVAL * 2, output_rx.recv()).await {
            Ok(Some(frame)) => frames.push(frame),
            _ => break,
        }
    }
    drop(input_tx);
    let _ = task.await;
    (frames, expected)
}

#[tokio::test(start_paused = true)]
async fn a_thousand_text_deltas_coalesce_into_fewer_than_a_hundred_frames() {
    let (frames, expected) = run_feeding_thousand(Framing::Windowed).await;
    assert!(
        frames.len() < 100,
        "合帧后该远少于 100 帧，实测 {}",
        frames.len()
    );
    assert_eq!(
        applied_text(&frames),
        expected,
        "按序应用后必须一字不少（首尾与顺序都在）"
    );
}

#[tokio::test(start_paused = true)]
async fn the_first_frame_arrives_within_one_window_of_the_first_delta() {
    let (input_tx, input_rx) = mpsc::channel(16);
    let (output_tx, mut output_rx) = mpsc::channel(16);
    let task = coalesce::spawn(Framing::Windowed, input_rx, output_tx);

    input_tx.send(delta("x")).await.unwrap();
    settle().await;
    // 窗口还差 1 ms：不许已经发出来。
    tokio::time::advance(FRAME_INTERVAL - Duration::from_millis(1)).await;
    settle().await;
    assert!(
        output_rx.try_recv().is_err(),
        "窗口没到就发帧，等于没合帧"
    );

    // 只给 2 ms 余量：攒到 200 ms 再发的实现在这里必红（`timeout` 会把虚拟时钟推过去）。
    let frame = tokio::time::timeout(Duration::from_millis(2), output_rx.recv())
        .await
        .expect("首帧必须在窗口内出现，实测超时")
        .expect("合帧器不该先关");
    assert!(matches!(frame, RemoteEvent::MessageUpdate { .. }));
    drop(input_tx);
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn text_start_carries_the_whole_prefix_and_deltas_append_from_there() {
    let (input_tx, input_rx) = mpsc::channel(16);
    let (output_tx, mut output_rx) = mpsc::channel(16);
    let task = coalesce::spawn(Framing::Windowed, input_rx, output_tx);

    input_tx
        .send(update(MessageChange::TextStart {
            content_index: 0,
            block: json!({"type": "text", "text": "HEAD|"}),
        }))
        .await
        .unwrap();
    for text in ["a", "b", "c"] {
        input_tx.send(delta(text)).await.unwrap();
    }
    tokio::time::advance(FRAME_INTERVAL + Duration::from_millis(1)).await;

    let mut frames = Vec::new();
    while let Ok(Some(frame)) =
        tokio::time::timeout(FRAME_INTERVAL * 2, output_rx.recv()).await
    {
        frames.push(frame);
    }
    // 服务端如果既在 `text_start` 里带全文、又从头再发一遍 delta，这里会变成 `HEAD|HEAD|abc`：
    // 每一帧都合法，字符却重复了。
    assert_eq!(applied_text(&frames), "HEAD|abc");
    drop(input_tx);
    let _ = task.await;
}

#[tokio::test(start_paused = true)]
async fn the_negative_control_sends_almost_every_delta_as_its_own_frame() {
    let (frames, _expected) = run_feeding_thousand(Framing::Immediate).await;
    assert!(
        frames.len() >= 900,
        "关掉合帧后该有 ~1000 帧，实测 {} —— 这个分支没有真的关掉合帧，别改说法",
        frames.len()
    );
}

#[tokio::test(start_paused = true)]
async fn a_state_change_flushes_pending_deltas_first_and_is_never_coalesced() {
    let (input_tx, input_rx) = mpsc::channel(16);
    let (output_tx, mut output_rx) = mpsc::channel(16);
    let task = coalesce::spawn(Framing::Windowed, input_rx, output_tx);

    input_tx.send(delta("d")).await.unwrap();
    input_tx
        .send(RemoteEvent::MessageEnd {
            entry: Some(json!({"id": 1, "kind": "pi.assistant"})),
            message: None,
            message_id: None,
        })
        .await
        .unwrap();
    tokio::task::yield_now().await;

    // 时空转换帧不攒：它一到，缓冲里的 delta 先出，然后它自己出。
    let first = tokio::time::timeout(Duration::from_millis(1), output_rx.recv())
        .await
        .expect("delta 不该等窗口")
        .unwrap();
    assert!(matches!(first, RemoteEvent::MessageUpdate { .. }));
    let second = tokio::time::timeout(Duration::from_millis(1), output_rx.recv())
        .await
        .expect("message_end 不该等窗口")
        .unwrap();
    assert!(matches!(second, RemoteEvent::MessageEnd { .. }));
    drop(input_tx);
    let _ = task.await;
}
