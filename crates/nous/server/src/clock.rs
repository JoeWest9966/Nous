//! 生产注入的单调时钟适配：只把系统毫秒读一次交给 `#73` 的纯判定。
//!
//! 真相层与判定都不自己读时钟；适配只在 `main` 装配一次（`docs/pitfalls.md`
//! 「注入时钟」那条）。`wait_until` 由真正的调度循环用，试跑判定只走 `now_ms`。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use nous_durable::harness::clock::Clock;

/// 生产时钟：毫秒 epoch；`wait_until` 用 tokio 定时器（只有 app 层允许等待）。
#[derive(Debug)]
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
    }

    async fn wait_until(&self, at_ms: u64) {
        let now = self.now_ms();
        if at_ms > now {
            tokio::time::sleep(std::time::Duration::from_millis(at_ms - now)).await;
        }
    }
}

/// 给 `main` 的单一装配口。
pub fn system_clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}
