//! 观察收尾的信号。上游 `packages/chord/src/watch.ts` 的 `WatchEnd`。
//!
//! 三个无载荷变体是「正常收尾」，两个带错误的是「收尾原因是个错误」：
//!
//! - `stopped` —— 调用方自己停的；
//! - `aborted` —— 上层 abort 掉了；
//! - `ended` —— 被观察的那个东西（任务/会话作用域）自己结束了；
//! - `listener_error` —— 监听器自己出错（上游会摘掉它并 end 到它手上）；
//! - `failed` —— 源失败（Session 关闭用 `failed("session closed")`，Session 失败带那个错）。
//!
//! 与上游的差别只有一处：`error` 在 TS 里是 `Error` / `unknown`，这里落成可读文本。
//! `WatchEnd` **不过线上**（它只是进程内回调的收尾信号），所以不做序列化，也不假装
//! `Error` 有对象形状。

/// 观察为什么停了。上游 `WatchEnd`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEnd {
    Stopped,
    Aborted,
    Ended,
    ListenerError { error: String },
    Failed { error: String },
}

impl WatchEnd {
    /// `failed` 的便捷构造：`failed("session closed")`。
    pub fn failed(error: impl Into<String>) -> Self {
        WatchEnd::Failed {
            error: error.into(),
        }
    }

    /// 上游 `ended` 的那个常量。
    pub const ENDED: WatchEnd = WatchEnd::Ended;

    /// 这次收尾带的是不是失败。
    pub fn is_failure(&self) -> bool {
        matches!(self, WatchEnd::ListenerError { .. } | WatchEnd::Failed { .. })
    }
}
