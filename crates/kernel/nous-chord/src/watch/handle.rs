//! 观察的生命周期：`WatchHandle` / `Watch` / `NativeWatch`。
//!
//! 上游用 `Promise` + `queueMicrotask` 做「listener 一次一个、期间的变化折叠」。
//! 纯 Rust 库里对应的是**显式泵**：`start` 装 listener，`advance` 只标记 pending，
//! 由上层 runtime 调 `poll()` 投递；`poll` 返回后 caller 可以再调，直到 `Idle`。
//! 没有 tokio 运行时、没有 `Promise`，也没有内联投递。
//!
//! 两种 watch 共享同一组字段与规则（上游 `WatchLifecycle` 的摊平版本）：
//! `start` 只能一次、结束只生效一次、`stop`/`abort`/`end` 都会释放对源的引用，
//! `end_after_delivery` 投递完 pending 再 `ended`，listener 抛错变成 `listener_error`。

use std::collections::VecDeque;
use std::sync::Arc;

use crate::context::{without_abort_signal, Context};
use crate::delta::{pending::{PendingOps, PendingTake}, Op};
use crate::json::JsonValue;

use super::call::{NativeWatchCall, WatchCall};
use super::end::WatchEnd;

/// 一次投递的结果，代替上游的 microtask drain。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// 没有待投递的帧。
    Idle,
    /// 投了一帧，可以再 poll 下一帧。
    Delivered,
    /// 源结束后投递完 pending，watch 终结（`end` 是终结原因）。
    Ended { end: WatchEnd },
}

/// listener 的形状。上游 `(call, context) => Promise<void>` —— 这里同步返回 `Result`，
/// `Err` 与上游的 throw / reject 等价（终结成 `listener_error`）。
pub type Listener<C> = Box<dyn FnMut(C, Context) -> Result<(), String> + Send + Sync>;

/// `start` 的失败原因。上游直接 `throw new Error`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchStartError {
    AlreadyStarted,
    Stopped,
}

impl std::fmt::Display for WatchStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchStartError::AlreadyStarted => write!(f, "Watch is already started"),
            WatchStartError::Stopped => write!(f, "Watch is stopped"),
        }
    }
}

impl std::error::Error for WatchStartError {}

/// 上游 `MAX_PENDING_NATIVE_VALUES = 100`：挂起值超过 `2 × 100` 折成一帧。
pub const MAX_PENDING_NATIVE_VALUES: usize = 100;

/// 一个 JSON watch 的句柄。上游 `WatchHandle<T>` 的可观察面：`value` 与 `closed`。
/// 这里没有 `Promise`：终结原因由 [`Watch::end`] 给，等待由上层包装。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchHandle<T> {
    pub value: T,
    pub stopped: bool,
}

impl<T> WatchHandle<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            stopped: false,
        }
    }
}

/// JSON watch。上游 `Watch<T>`：多帧在 `PendingOps` 里折叠；单帧原样透传；
/// 失败帧丢掉 pending，之后第一个 value 是整根替换。
pub struct Watch<T>
where
    T: Clone + Into<JsonValue> + Send + Sync + 'static,
{
    value: T,
    listener: Option<Listener<WatchCall<T>>>,
    started: bool,
    scheduled: bool,
    ending: bool,
    end: Option<WatchEnd>,
    pending_context: Option<Context>,
    ops: PendingOps,
    /// 只有一帧 pending 时原样透传；第二帧到达就丢掉（ops 仍在 `ops` 里）。
    lone: Option<WatchCall<T>>,
    newest_value: Option<T>,
    retired: bool,
    failure: Option<String>,
    replace: bool,
    detach: Option<Arc<dyn Fn() + Send + Sync>>,
    report: Option<Arc<dyn Fn(String) + Send + Sync>>,
    last_listener_context: Option<Context>,
}

impl<T> std::fmt::Debug for Watch<T>
where
    T: Clone + Into<JsonValue> + Send + Sync + std::fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch")
            .field("value", &self.value)
            .field("started", &self.started)
            .field("ending", &self.ending)
            .field("end", &self.end)
            .field("retired", &self.retired)
            .field("replace", &self.replace)
            .finish_non_exhaustive()
    }
}

impl<T> Watch<T>
where
    T: Clone + Into<JsonValue> + Send + Sync + 'static,
{
    pub fn new(
        value: T,
        detach: Option<Arc<dyn Fn() + Send + Sync>>,
        report: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Self {
        Self {
            value,
            listener: None,
            started: false,
            scheduled: false,
            ending: false,
            end: None,
            pending_context: None,
            ops: PendingOps::new(),
            lone: None,
            newest_value: None,
            retired: false,
            failure: None,
            replace: false,
            detach,
            report,
            last_listener_context: None,
        }
    }

    /// 当前值（获取时是初值，投递后是最新值；`failed` 帧不更新）。
    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn end(&self) -> Option<&WatchEnd> {
        self.end.as_ref()
    }

    /// listener 拿到的上一个 context（由 `without_abort_signal` 派生）。
    pub fn last_listener_context(&self) -> Option<&Context> {
        self.last_listener_context.as_ref()
    }

    pub fn start(&mut self, listener: Listener<WatchCall<T>>) -> Result<(), WatchStartError> {
        if self.started {
            return Err(WatchStartError::AlreadyStarted);
        }
        if self.end.is_some() {
            return Err(WatchStartError::Stopped);
        }
        self.started = true;
        self.listener = Some(listener);
        self.scheduled = self.pending_context.is_some();
        Ok(())
    }

    /// 收一帧（上游 `advance`）。
    pub fn advance(&mut self, call: WatchCall<T>, context: Context) {
        if matches!(call, WatchCall::Failed { .. }) {
            let WatchCall::Failed { error } = call else {
                unreachable!()
            };
            self.fail(error, context);
            return;
        }
        if !self.open() {
            return;
        }
        if self.replace {
            self.failure = None;
        } else {
            self.lone = if self.pending_context.is_some() {
                None
            } else {
                Some(call.clone())
            };
            if let WatchCall::Value { ops, .. } = &call {
                self.ops.push(Arc::clone(ops));
            }
        }
        if let WatchCall::Value { value, .. } = &call {
            self.newest_value = Some(value.clone());
        }
        self.pending_context = Some(context);
        if matches!(&call, WatchCall::Value { retired: true, .. }) {
            self.retired = true;
        }
        self.scheduled = self.started;
    }

    /// 可恢复的失败（上游 `fail`）：pending 全丢，value 不更新。
    pub fn fail(&mut self, error: impl Into<String>, context: Context) {
        if !self.open() {
            return;
        }
        self.ops.take();
        self.lone = None;
        self.newest_value = None;
        self.failure = Some(error.into());
        self.pending_context = Some(context);
        self.replace = true;
        self.scheduled = self.started;
    }

    /// 有新帧 / 失败 / 源结束时由上层调一次，重新置位泵（上游 `pending()`）。
    pub fn arm(&mut self) {
        self.scheduled = self.started;
    }

    /// 投递一帧。没有内联投递，`advance` 只置位。
    pub fn poll(&mut self, fallback_context: &Context) -> PollOutcome {
        if self.end.is_some() || !self.started || !self.scheduled {
            return PollOutcome::Idle;
        }
        if self.listener.is_none() {
            return PollOutcome::Idle;
        }
        if self.pending_context.is_none() {
            if self.ending {
                return PollOutcome::Ended {
                    end: self.terminate(WatchEnd::Ended),
                };
            }
            self.scheduled = false;
            return PollOutcome::Idle;
        }
        let _ = fallback_context;
        let frame_context = self.pending_context.clone().expect("checked above");
        let call = self.take_next();
        if let WatchCall::Value { value, .. } = &call {
            self.value = value.clone();
        }
        // 上游 #drain 投递一帧后回到 while 顶部：pending 空但源已结束时，
        // 下一次 poll 负责发 `Ended`，所以保留 scheduled。
        self.scheduled = self.pending_context.is_some() || self.ending;
        let listener_context = without_abort_signal(frame_context);
        self.last_listener_context = Some(listener_context.clone());
        // listener 先取出来，避免和 take_next() 同时持有 &mut self。
        let mut listener = self.listener.take().expect("checked above");
        match listener(call, listener_context) {
            Ok(()) => {
                self.listener = Some(listener);
                PollOutcome::Delivered
            }
            Err(error) => {
                if self.end.is_some() {
                    self.listener = Some(listener);
                    return PollOutcome::Idle;
                }
                if let Some(report) = &self.report {
                    report(error.clone());
                }
                PollOutcome::Ended {
                    end: self.terminate(WatchEnd::ListenerError { error }),
                }
            }
        }
    }

    pub fn stop(&mut self) -> WatchEnd {
        self.terminate(WatchEnd::Stopped)
    }

    pub fn abort(&mut self) -> WatchEnd {
        self.terminate(WatchEnd::Aborted)
    }

    pub fn end_with(&mut self, end: WatchEnd) -> WatchEnd {
        self.terminate(end)
    }

    /// 源不再给帧：投递完 pending 再 `ended`。
    pub fn end_after_delivery(&mut self) {
        if self.end.is_some() || self.ending {
            return;
        }
        self.ending = true;
        if self.pending_context.is_none() {
            self.terminate(WatchEnd::Ended);
        }
    }

    fn open(&self) -> bool {
        self.end.is_none() && !self.ending
    }

    fn take_next(&mut self) -> WatchCall<T> {
        if self.replace {
            return self.take_after_failure();
        }
        let lone = self.lone.take();
        let value = self
            .newest_value
            .take()
            .expect("pending context implies a value");
        let retired = std::mem::take(&mut self.retired);
        self.pending_context = None;
        if let Some(lone) = lone {
            return lone;
        }
        let ops = match self.ops.take() {
            PendingTake::Ops(ops) => ops,
            PendingTake::Replace => Arc::new(vec![Op::Replace(value.clone().into())]),
        };
        WatchCall::Value {
            value,
            ops,
            retired,
        }
    }

    fn take_after_failure(&mut self) -> WatchCall<T> {
        self.pending_context = None;
        if let Some(error) = self.failure.take() {
            return WatchCall::Failed { error };
        }
        let value = self
            .newest_value
            .take()
            .expect("pending context implies a value");
        let retired = std::mem::take(&mut self.retired);
        self.replace = false;
        WatchCall::Value {
            ops: Arc::new(vec![Op::Replace(value.clone().into())]),
            value,
            retired,
        }
    }

    fn terminate(&mut self, end: WatchEnd) -> WatchEnd {
        if let Some(existing) = &self.end {
            return existing.clone();
        }
        self.end = Some(end.clone());
        self.scheduled = false;
        self.pending_context = None;
        self.ops.take();
        self.lone = None;
        self.newest_value = None;
        self.retired = false;
        self.failure = None;
        self.replace = false;
        self.listener = None;
        if let Some(detach) = self.detach.take() {
            detach();
        }
        end
    }
}

/// 原生值 watch。上游 `NativeWatch<T>`：每帧一个值；挂起值超过 `2 × 100` 时
/// 折成最新一个（或 `replace` 钩子给的值）。
pub struct NativeWatch<T>
where
    T: Clone + Send + Sync + 'static,
{
    value: T,
    listener: Option<Listener<NativeWatchCall<T>>>,
    started: bool,
    scheduled: bool,
    ending: bool,
    end: Option<WatchEnd>,
    pending: VecDeque<(T, Context)>,
    replace: Option<Arc<dyn Fn() -> T + Send + Sync>>,
    failure: Option<String>,
    detach: Option<Arc<dyn Fn() + Send + Sync>>,
    report: Option<Arc<dyn Fn(String) + Send + Sync>>,
    last_listener_context: Option<Context>,
}

impl<T> std::fmt::Debug for NativeWatch<T>
where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeWatch")
            .field("value", &self.value)
            .field("pending", &self.pending.len())
            .field("end", &self.end)
            .finish_non_exhaustive()
    }
}

impl<T> NativeWatch<T>
where
    T: Clone + Send + Sync + 'static,
{
    pub fn new(
        value: T,
        detach: Option<Arc<dyn Fn() + Send + Sync>>,
        report: Option<Arc<dyn Fn(String) + Send + Sync>>,
        replace: Option<Arc<dyn Fn() -> T + Send + Sync>>,
    ) -> Self {
        Self {
            value,
            listener: None,
            started: false,
            scheduled: false,
            ending: false,
            end: None,
            pending: VecDeque::new(),
            replace,
            failure: None,
            detach,
            report,
            last_listener_context: None,
        }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn end(&self) -> Option<&WatchEnd> {
        self.end.as_ref()
    }

    pub fn last_listener_context(&self) -> Option<&Context> {
        self.last_listener_context.as_ref()
    }

    pub fn start(&mut self, listener: Listener<NativeWatchCall<T>>) -> Result<(), WatchStartError> {
        if self.started {
            return Err(WatchStartError::AlreadyStarted);
        }
        if self.end.is_some() {
            return Err(WatchStartError::Stopped);
        }
        self.started = true;
        self.listener = Some(listener);
        self.scheduled = !self.pending.is_empty() || self.failure.is_some();
        Ok(())
    }

    /// 收一个值（上游 `advance`）。
    pub fn advance(&mut self, value: T, context: Context) {
        if !self.open() {
            return;
        }
        self.failure = None;
        if self.pending.len() < 2 * MAX_PENDING_NATIVE_VALUES {
            self.pending.push_back((value, context));
        } else {
            let value = match &self.replace {
                Some(replace) => replace(),
                None => value,
            };
            self.pending.clear();
            self.pending.push_back((value, context));
        }
        self.scheduled |= self.started;
    }

    pub fn fail(&mut self, error: impl Into<String>, context: Context) {
        if !self.open() {
            return;
        }
        self.pending.clear();
        self.failure = Some(error.into());
        self.pending.push_back((self.value.clone(), context));
        self.scheduled |= self.started;
    }

    pub fn poll(&mut self, _fallback_context: &Context) -> PollOutcome {
        if self.end.is_some() || !self.started || !self.scheduled {
            return PollOutcome::Idle;
        }
        if self.listener.is_none() {
            return PollOutcome::Idle;
        }
        if self.pending.is_empty() {
            if self.ending {
                return PollOutcome::Ended {
                    end: self.terminate(WatchEnd::Ended),
                };
            }
            self.scheduled = false;
            return PollOutcome::Idle;
        }
        let (value, context) = self.pending.pop_front().expect("checked above");
        let call = if let Some(error) = self.failure.take() {
            NativeWatchCall::Failed { error }
        } else {
            self.value = value.clone();
            NativeWatchCall::Value { value }
        };
        self.scheduled = !self.pending.is_empty() || self.ending;
        let listener_context = without_abort_signal(context);
        self.last_listener_context = Some(listener_context.clone());
        // listener 先取出来，避免和 take_next() 同时持有 &mut self。
        let mut listener = self.listener.take().expect("checked above");
        match listener(call, listener_context) {
            Ok(()) => {
                self.listener = Some(listener);
                PollOutcome::Delivered
            }
            Err(error) => {
                if self.end.is_some() {
                    self.listener = Some(listener);
                    return PollOutcome::Idle;
                }
                if let Some(report) = &self.report {
                    report(error.clone());
                }
                PollOutcome::Ended {
                    end: self.terminate(WatchEnd::ListenerError { error }),
                }
            }
        }
    }

    pub fn stop(&mut self) -> WatchEnd {
        self.terminate(WatchEnd::Stopped)
    }

    pub fn abort(&mut self) -> WatchEnd {
        self.terminate(WatchEnd::Aborted)
    }

    pub fn end_with(&mut self, end: WatchEnd) -> WatchEnd {
        self.terminate(end)
    }

    pub fn end_after_delivery(&mut self) {
        if self.end.is_some() || self.ending {
            return;
        }
        self.ending = true;
        if self.pending.is_empty() {
            self.terminate(WatchEnd::Ended);
        }
    }

    fn open(&self) -> bool {
        self.end.is_none() && !self.ending
    }

    fn terminate(&mut self, end: WatchEnd) -> WatchEnd {
        if let Some(existing) = &self.end {
            return existing.clone();
        }
        self.end = Some(end.clone());
        self.scheduled = false;
        self.pending.clear();
        self.replace = None;
        self.failure = None;
        self.listener = None;
        if let Some(detach) = self.detach.take() {
            detach();
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::path;
    use crate::json::JsonValue;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn set(path_str: &str, value: JsonValue) -> Arc<Vec<Op>> {
        Arc::new(vec![Op::Set(path(&[path_str]), value)])
    }

    #[test]
    fn three_writes_to_the_same_path_fold_into_one_delivery() {
        let mut watch = Watch::<JsonValue>::new(json!({"v": 0}), None, None);
        let delivered = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&delivered);
        watch
            .start(Box::new(move |call, _context| {
                if let WatchCall::Value { ops, .. } = call {
                    seen.lock().unwrap().push(ops.as_ref().clone());
                }
                Ok(())
            }))
            .unwrap();
        let context = crate::context::BACKGROUND_CONTEXT;
        // listener 还未投第一帧时又来两帧 —— 连续写同一路径。
        watch.advance(WatchCall::value(json!({"v": 1}), set("v", json!(1))), context.clone());
        watch.advance(WatchCall::value(json!({"v": 2}), set("v", json!(2))), context.clone());
        watch.advance(WatchCall::value(json!({"v": 3}), set("v", json!(3))), context.clone());
        let mut outcome = watch.poll(&context);
        while outcome == PollOutcome::Delivered && !delivered.lock().unwrap().is_empty() {
            outcome = watch.poll(&context);
        }
        let calls = delivered.lock().unwrap();
        assert_eq!(calls.len(), 1, "同路径连续三次写应折成一帧");
        assert_eq!(calls[0], vec![Op::Set(path(&["v"]), json!(3))]);
        assert_eq!(watch.value(), &json!({"v": 3}));
    }

    #[test]
    fn single_frame_passes_through_without_folding() {
        let mut watch = Watch::<JsonValue>::new(json!({"v": 0}), None, None);
        let ops_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&ops_seen);
        watch
            .start(Box::new(move |call, _context| {
                if let WatchCall::Value { ops, .. } = call {
                    seen.lock().unwrap().push(Arc::clone(&ops));
                }
                Ok(())
            }))
            .unwrap();
        let batches = vec![set("a", json!(1)), set("b", json!(2))];
        watch.advance(WatchCall::value(json!({"a": 1}), batches[0].clone()), crate::context::BACKGROUND_CONTEXT);
        watch.poll(&crate::context::BACKGROUND_CONTEXT);
        watch.advance(WatchCall::value(json!({"a": 1, "b": 2}), batches[1].clone()), crate::context::BACKGROUND_CONTEXT);
        watch.poll(&crate::context::BACKGROUND_CONTEXT);
        assert_eq!(*ops_seen.lock().unwrap(), batches);
    }

    #[test]
    fn listener_error_ends_the_watch_and_detaches() {
        let detaches = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&detaches);
        let mut watch = Watch::<JsonValue>::new(
            json!(0),
            Some(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
            None,
        );
        watch
            .start(Box::new(|_call, _context| Err("boom".into())))
            .unwrap();
        watch.advance(
            WatchCall::value(json!(1), Arc::new(vec![])),
            crate::context::BACKGROUND_CONTEXT,
        );
        assert_eq!(
            watch.poll(&crate::context::BACKGROUND_CONTEXT),
            PollOutcome::Ended {
                end: WatchEnd::ListenerError {
                    error: "boom".into()
                }
            }
        );
        assert_eq!(detaches.load(Ordering::SeqCst), 1);
        assert_eq!(watch.stop(), WatchEnd::ListenerError { error: "boom".into() });
    }

    #[test]
    fn failure_drops_pending_and_next_value_is_a_root_replace() {
        let mut watch = Watch::<JsonValue>::new(json!(0), None, None);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        watch
            .start(Box::new(move |call, _context| {
                seen.lock().unwrap().push(call);
                Ok(())
            }))
            .unwrap();
        let context = crate::context::BACKGROUND_CONTEXT;
        watch.advance(WatchCall::value(json!(1), set("v", json!(1))), context.clone());
        watch.fail("temporary", context.clone());
        watch.advance(WatchCall::value(json!(2), set("v", json!(2))), context.clone());
        // 上游 `advance()`：`replace` 状态下后来到的 value 清掉 pending failure；
        // 所以 listener 只收到一次「整根替换」，不会先看到那个失败。
        assert_eq!(watch.poll(&context), PollOutcome::Delivered);
        assert_eq!(watch.poll(&context), PollOutcome::Idle);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            WatchCall::Value { value, ops, .. } => {
                assert_eq!(value, &json!(2));
                assert_eq!(ops.as_ref(), &vec![Op::Replace(json!(2))]);
            }
            _ => panic!("expected value"),
        }
        drop(calls);

        // 另一种顺序：失败先投出去，后面的 value 才是根替换。
        let mut watch = Watch::<JsonValue>::new(json!(0), None, None);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        watch
            .start(Box::new(move |call, _context| {
                seen.lock().unwrap().push(call);
                Ok(())
            }))
            .unwrap();
        watch.fail("temporary", context.clone());
        assert_eq!(watch.poll(&context), PollOutcome::Delivered);
        watch.arm();
        watch.advance(WatchCall::value(json!(3), set("v", json!(3))), context.clone());
        assert_eq!(watch.poll(&context), PollOutcome::Delivered);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(matches!(calls[0], WatchCall::Failed { .. }));
        match &calls[1] {
            WatchCall::Value { ops, .. } => {
                assert_eq!(ops.as_ref(), &vec![Op::Replace(json!(3))]);
            }
            _ => panic!("expected value"),
        }
    }

    #[test]
    fn end_after_delivery_keeps_pending_then_ends() {
        let mut watch = Watch::<JsonValue>::new(json!(0), None, None);
        watch
            .start(Box::new(|_call, _context| Ok(())))
            .unwrap();
        let context = crate::context::BACKGROUND_CONTEXT;
        watch.advance(WatchCall::value(json!(1), set("v", json!(1))), context.clone());
        watch.end_after_delivery();
        assert_eq!(watch.poll(&context), PollOutcome::Delivered);
        assert_eq!(watch.poll(&context), PollOutcome::Ended { end: WatchEnd::Ended });
    }

    #[test]
    fn context_given_to_listener_has_cancellation_stripped() {
        let token = tokio_util::sync::CancellationToken::new();
        let context = crate::context::with_abort_signal(token.clone(), crate::context::BACKGROUND_CONTEXT);
        let mut watch = Watch::<JsonValue>::new(json!(0), None, None);
        let cancelled_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&cancelled_seen);
        watch
            .start(Box::new(move |_call, context| {
                flag.store(context.is_cancelled(), Ordering::SeqCst);
                Ok(())
            }))
            .unwrap();
        token.cancel();
        watch.advance(WatchCall::value(json!(1), Arc::new(vec![])), context.clone());
        assert_eq!(watch.poll(&context), PollOutcome::Delivered);
        assert!(!cancelled_seen.load(Ordering::SeqCst));
    }

    #[test]
    fn native_watch_keeps_every_value_in_order() {
        let mut watch = NativeWatch::new(0u32, None, None, None);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let values = Arc::clone(&seen);
        watch
            .start(Box::new(move |call, _context| {
                if let NativeWatchCall::Value { value } = call {
                    values.lock().unwrap().push(value);
                }
                Ok(())
            }))
            .unwrap();
        let context = crate::context::BACKGROUND_CONTEXT;
        for value in 1..=3 {
            watch.advance(value, context.clone());
        }
        while watch.poll(&context) == PollOutcome::Delivered {}
        assert_eq!(*seen.lock().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn native_watch_folds_a_backlog_past_twice_the_cap() {
        let mut watch = NativeWatch::new(0u32, None, None, None);
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let values = Arc::clone(&seen);
        watch
            .start(Box::new(move |call, _context| {
                if let NativeWatchCall::Value { value } = call {
                    values.lock().unwrap().push(value);
                }
                Ok(())
            }))
            .unwrap();
        let context = crate::context::BACKGROUND_CONTEXT;
        for value in 1..=205 {
            watch.advance(value, context.clone());
        }
        while watch.poll(&context) == PollOutcome::Delivered {}
        // 第 201 个把 200 个挂起折成它一个；之后 202..=205 另投。
        assert_eq!(*seen.lock().unwrap(), vec![201, 202, 203, 204, 205]);
    }
}
