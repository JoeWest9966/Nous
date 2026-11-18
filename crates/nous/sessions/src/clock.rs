//! 生产用的时钟。
//!
//! `durable` 只定义 [`Clock`] 这个 trait 和一个给测试用的 `ManualClock` —— `crates/kernel` 层
//! 不许直接读时钟，所以"真时钟"必须由**应用层**提供。这个文件就是那个提供者。
//!
//! 换算：`durable` 的时钟是「从宿主起点算的毫秒数」，绝对值不重要、差值才重要，所以这里拿
//! 进程启动时刻当零点（`OnceLock` 里存一次），不引入 `chrono` / 系统时间漂移。

use async_trait::async_trait;
use std::sync::OnceLock;
use std::time::Instant;

use nous_durable::harness::clock::Clock;

fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// 单调时钟：`now_ms` 只前进不后退，`wait_until` 用 tokio 定时器真睡。
#[derive(Debug, Default)]
pub struct SystemClock;

impl SystemClock {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        origin().elapsed().as_millis() as u64
    }

    async fn wait_until(&self, at_ms: u64) {
        let now = self.now_ms();
        if at_ms > now {
            tokio::time::sleep(std::time::Duration::from_millis(at_ms - now)).await;
        }
    }
}
