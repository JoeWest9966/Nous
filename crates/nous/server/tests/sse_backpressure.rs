//! SSE 未发送字节背压判据。
//!
//! 接缝：真 Router 的 `/v1/sessions/:id/events` + 只连不读的客户端。判据看的是
//! 「未发送字节撞 1MB 就断这条流」，不是帧数；快读客户端的帧序与内容不变。

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use common::FakeSource;
use nous_server::{router, AppState};
use nous_wire::RemoteEvent;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tower::ServiceExt;

/// 未发送字节上限（`ext-sdk` 的单帧上限同数）。测试独立写常量，免得实现把数字改小
/// 以后判据跟着一起绿。
const LIMIT: usize = 1024 * 1024;

/// 判据是 RSS 不涨，不是「返回了码」。读 `/proc/self/statm` 的第二项（页数）。
fn rss_bytes() -> usize {
    let text = std::fs::read_to_string("/proc/self/statm").expect("Linux /proc/self/statm");
    let pages: usize = text
        .split_whitespace()
        .nth(1)
        .expect("resident pages")
        .parse()
        .unwrap();
    pages * 4096
}

/// 往会话推一帧 payload 约为 `bytes` 的 entry_appended。
fn publish_chunk(state: &AppState, session: i64, bytes: usize) {
    let text = "x".repeat(bytes);
    let event = RemoteEvent::EntryAppended {
        entry: json!({ "text": text }),
    };
    let frame = serde_json::to_string(&event).unwrap();
    assert!(
        frame.len() >= bytes,
        "帧必须真的带这么多字节：{} < {bytes}",
        frame.len()
    );
    state.publish_event(session, event);
}

/// 起一个真监听端口，返回基址。用真 TCP 而不是 `oneshot`：只有真 socket 才能在客户端
/// 不读时把背压传回服务端，内存 channel 会被 tower 提前 poll 掉。
async fn serve(state: Arc<AppState>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// 只连不读：发完 GET 就把 socket 留在手里，不再 `read`。服务端撞顶后必须自己断。
async fn idle_client_is_disconnected(state: Arc<AppState>, session: i64) -> bool {
    let base = serve(state.clone()).await;
    let mut socket = TcpStream::connect(base.trim_start_matches("http://"))
        .await
        .unwrap();
    socket
        .write_all(
            format!("GET /v1/sessions/{session}/events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    // 给服务端时间建立订阅、写出 snapshot。
    tokio::time::sleep(Duration::from_millis(50)).await;
    let rss_before = rss_bytes();

    // 逐帧推到远超 1MB：socket 缓冲本身有几 MB，得让对端彻底不读才能把背压顶回来。
    // 帧数（64）远低于旧上限 256，旧实现不会因帧数断；16MB 也远低于 64×1MB RSS 余量。
    for _ in 0..64 {
        publish_chunk(&state, session, 256 * 1024);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    // 服务端断开时 socket 里可能还堆着几 MB 未读数据，读到真正的 EOF（read == 0）
    // 才算断；读回来的内容直接丢掉。
    let disconnected = timeout(Duration::from_secs(8), async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match socket.read(&mut buf).await {
                Ok(0) => return true,
                Ok(_) => continue,
                Err(_) => return true,
            }
        }
    })
    .await
    .unwrap_or(false);
    let rss_after = rss_bytes();
    let delta = rss_after.saturating_sub(rss_before);
    let ceiling = 64 * LIMIT;
    eprintln!(
        "[rss] before={} after={} delta={} ceiling={} disconnected={disconnected}",
        rss_before, rss_after, delta, ceiling
    );
    assert!(delta < ceiling, "RSS 涨幅 {} >= 上限 {ceiling}", delta);
    disconnected
}

#[tokio::test]
async fn an_idle_client_is_disconnected_once_unsent_bytes_exceed_one_mb() {
    let state = AppState::with_source(FakeSource::two_sessions());
    assert!(
        idle_client_is_disconnected(state, 1).await,
        "只连不读的客户端累计 {LIMIT} 未发送字节后必须被断开"
    );
}

/// 每连接上限：一个只连不读的客户端被断，不能踢掉同时正常读的客户端。
#[tokio::test]
async fn a_slow_client_does_not_disturb_a_fast_one() {
    let state = AppState::with_source(FakeSource::two_sessions());
    let base = serve(state.clone()).await;
    let addr = base.trim_start_matches("http://");

    // 快读客户端：连上、读 snapshot，之后一直读。
    let mut fast = TcpStream::connect(addr).await.unwrap();
    fast.write_all(
        b"GET /v1/sessions/1/events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    // 持续读：整个灌包期间都不停下来，这才是「读得快的客户端」。
    let mut buf = vec![0u8; 64 * 1024];
    let mut seen = 0usize;
    timeout(Duration::from_secs(2), async {
        while seen == 0 {
            seen += fast.read(&mut buf).await.unwrap();
        }
    })
    .await
    .expect("快客户端该先拿到 snapshot");
    assert!(seen > 0);
    let (reader_done_tx, reader_done_rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
    tokio::spawn(async move {
        let mut received = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match fast.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    received.extend_from_slice(&buf[..n]);
                    if received
                        .windows(b"after-slow-kicked".len())
                        .any(|w| w == b"after-slow-kicked")
                    {
                        break;
                    }
                }
            }
        }
        let _ = reader_done_tx.send(received);
    });

    // 慢客户端：只连不读，灌到背压断开。
    let mut slow = TcpStream::connect(addr).await.unwrap();
    slow.write_all(
        b"GET /v1/sessions/1/events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    for _ in 0..64 {
        publish_chunk(&state, 1, 256 * 1024);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    // 慢客户端断开后，快客户端仍能继续收到新帧。
    tokio::time::sleep(Duration::from_millis(200)).await;
    state.publish_event(
        1,
        RemoteEvent::EntryAppended {
            entry: json!({ "id": 999, "text": "after-slow-kicked" }),
        },
    );
    // 读任务一旦发现 EOF 就说明快客户端也被断了；它把收到的全部字节回传。
    let received = timeout(Duration::from_secs(3), reader_done_rx)
        .await
        .expect("快客户端该在 3 秒内拿到后续帧")
        .expect("读任务不该掉");
    assert!(
        received
            .windows(b"after-slow-kicked".len())
            .any(|w| w == b"after-slow-kicked"),
        "快客户端被慢客户端连累：没收到后续帧"
    );
}

/// 快读客户端：同样的会话，snapshot + 两个增量帧的帧序和内容都不变。
#[tokio::test]
async fn a_fast_reader_keeps_snapshot_then_events_in_order() {
    let state = AppState::with_source(FakeSource::two_sessions());
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/sessions/1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();

    async fn next(stream: &mut axum::body::BodyDataStream) -> Value {
        use futures::StreamExt;
        let chunk = timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("2 秒内该到一帧")
            .expect("流不该先关")
            .expect("块读得出来");
        let text = String::from_utf8(chunk.to_vec()).unwrap();
        let data = text
            .lines()
            .find_map(|l| l.strip_prefix("data: "))
            .expect("data 行");
        serde_json::from_str(data).expect("帧是 JSON")
    }

    assert_eq!(next(&mut stream).await["type"], json!("snapshot"));
    state.publish_event(
        1,
        RemoteEvent::EntryAppended {
            entry: json!({ "id": 1, "text": "one" }),
        },
    );
    assert_eq!(next(&mut stream).await["entry"]["text"], json!("one"));
    state.publish_event(
        1,
        RemoteEvent::EntryAppended {
            entry: json!({ "id": 2, "text": "two" }),
        },
    );
    assert_eq!(next(&mut stream).await["entry"]["text"], json!("two"));
}
