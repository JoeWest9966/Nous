//! 一帧观察的载荷。上游 `WatchCall<T>` / `NativeWatchCall<T>`。
//!
//! `ops` 用 `Arc<Vec<Op>>` 与 `delta::PendingOps::take()` 的形状一致；同一批可以同时
//! 喂给多个消费者。`error` 在 TS 是 `unknown`，这里落成可读文本（与 `WatchEnd` 一样）。

use std::sync::Arc;

use crate::delta::Op;

/// JSON 源的一次 listener 调用，也是源交给 watch 的一帧。上游 `WatchCall<T>`。
#[derive(Debug, Clone, PartialEq)]
pub enum WatchCall<T> {
    /// 新值、从上一个值变到它的 op、以及跟随的文档是否退休过。
    Value {
        value: T,
        ops: Arc<Vec<Op>>,
        retired: bool,
    },
    /// 源暂时算不出值；watch 保持打开，`value` 留着上一次的。
    Failed { error: String },
}

/// 原生值的一次 listener 调用。上游 `NativeWatchCall<T>`：没有 ops，一帧一个值。
#[derive(Debug, Clone, PartialEq)]
pub enum NativeWatchCall<T> {
    Value { value: T },
    Failed { error: String },
}

impl<T> WatchCall<T> {
    pub fn value(value: T, ops: impl Into<Arc<Vec<Op>>>) -> Self {
        WatchCall::Value {
            value,
            ops: ops.into(),
            retired: false,
        }
    }

    pub fn is_value(&self) -> bool {
        matches!(self, WatchCall::Value { .. })
    }
}

impl<T> NativeWatchCall<T> {
    pub fn value(value: T) -> Self {
        NativeWatchCall::Value { value }
    }
}
