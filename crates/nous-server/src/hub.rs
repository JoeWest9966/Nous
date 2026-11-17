//! SSE 的扇出：一个会话一条广播通道，`CommitObserver` 把 publication 投影成帧后扔进来。
//!
//! ## 为什么不用 `tokio::sync::broadcast` 的 `Lagged`
//!
//! 用。慢客户端落后时 `RecvError::Lagged(n)` 是**唯一**正确的信号：客户端漏了 n 帧，补丁流
//! 已经断了，必须重连拿 `snapshot`。这里不替它「重放缓冲」——补丁流不是可独立应用的快照
//! （`wire.md` / `docs/harness.md` §2.2），补一段旧帧只会拼出错位的气泡。
//!
//! ## 没有订阅者时扔掉
//!
//! 没人连 SSE 时帧没有意义（`snapshot` 才是真相）。`broadcast` 的 `send` 在没有接收端时
//! 返回 `Err`，这里**显式忽略**：不是静默丢 bug，是「没有观众」这件事本来就不该反压提交路径。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nous_wire::RemoteEvent;
use tokio::sync::broadcast;

/// 每条通道的缓冲帧数。落后超过这个数就只能重连（见模块文档）。
const CHANNEL_CAPACITY: usize = 256;

/// 一个会话 id → 一条广播通道。
#[derive(Default)]
pub struct Hub {
    channels: Mutex<HashMap<i64, broadcast::Sender<RemoteEvent>>>,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 订阅一个会话。第一次订阅时建通道；通道随 `Hub` 活，不随订阅者活 ——
    /// 否则「连接建立前刚发出的帧」会因为通道刚建而丢。
    pub fn subscribe(&self, session: i64) -> broadcast::Receiver<RemoteEvent> {
        self.sender(session).subscribe()
    }

    /// 发一帧。**同步**：调用点在 `CommitObserver::advance` 里，不许 await。
    pub fn publish(&self, session: i64, event: RemoteEvent) {
        // `send` 只在「一个接收端都没有」时失败；那不是错误。
        let _ = self.sender(session).send(event);
    }

    pub fn publish_all(&self, session: i64, events: impl IntoIterator<Item = RemoteEvent>) {
        for event in events {
            self.publish(session, event);
        }
    }

    fn sender(&self, session: i64) -> broadcast::Sender<RemoteEvent> {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        channels
            .entry(session)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }
}
