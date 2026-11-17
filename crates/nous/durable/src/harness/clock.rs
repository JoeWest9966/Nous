//! 时钟注入（#19）。
//!
//! 真相层**不读时钟**：`generation` 的节拍闸、`Timeline` 的观测时刻都由调用方注入。
//! 先例是 `nous-chord` 的 `watch`：注入时钟 + 显式 `poll()`。
//!
//! 判据：`grep -rn 'tokio 的时间 API' crates/kernel` 逐行空 —— 所以这里连测试用的定时器
//! 都不从 `tokio 的时间 API` 借。

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::sync::Notify;

/// 宿主的时钟。毫秒、单调（不回拨）。
#[async_trait]
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// 现在几毫秒（从宿主起点算；绝对值不重要，差值才重要）。
    fn now_ms(&self) -> u64;

    /// 等到 `at_ms` 才返回。**由宿主实现**（生产用真定时器，测试用虚拟时钟）。
    async fn wait_until(&self, at_ms: u64);
}

/// 虚拟时钟：时间只在 [`advance`](ManualClock::advance) / [`set_ms`](ManualClock::set_ms) 时前进。
///
/// 真相层的测试用它，因此不依赖真实 `sleep`，也不会因为 CI 慢而假红。
pub struct ManualClock {
    now: AtomicU64,
    notify: Notify,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new(0)
    }
}

impl ManualClock {
    pub fn new(start_ms: u64) -> Self {
        Self {
            now: AtomicU64::new(start_ms),
            notify: Notify::new(),
        }
    }

    /// 往前走 `ms` 毫秒，并唤醒所有等待者。
    pub fn advance(&self, ms: u64) {
        let now = self.now.fetch_add(ms, Ordering::SeqCst) + ms;
        self.notify.notify_waiters();
        let _ = now;
    }

    /// 直接把时间设到 `ms`（不倒退）。
    pub fn set_ms(&self, ms: u64) {
        self.now.fetch_max(ms, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

impl std::fmt::Debug for ManualClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManualClock")
            .field("now_ms", &self.now_ms())
            .finish()
    }
}

#[async_trait]
impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    async fn wait_until(&self, at_ms: u64) {
        loop {
            if self.now_ms() >= at_ms {
                return;
            }
            self.notify.notified().await;
        }
    }
}
