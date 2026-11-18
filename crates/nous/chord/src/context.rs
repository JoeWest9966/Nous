//! 调用作用域的值：取消 + 任意带类型的附加值。**全库只有这一份 `Context`。**
//!
//! ## 权威
//!
//! 形状照 `packages/chord/src/context/index.ts`（161 行）。TS 侧 `durable` 的
//! `env/index.ts` 里每个方法的 `context: Context` 拿的就是这一份 —— 所以 TS 只有一份，
//! 本库也必须只有一份。`nous-env::Context` 是指向这里的类型别名（AGENTS.md §二）。
//!
//! ## 和 TS 的三处差别
//!
//! 1. **`AbortSignal` → [`CancellationToken`]**（`tokio-util`，全库一致）。TS 的
//!    `AbortSignal.any([parent, signal])` 在 Rust 里不合成一个新 token，而是**沿链保留
//!    多个 token，语义是「任一取消即取消」**：省掉一次 spawn，也不丢调用方的 token
//!    （`is_cancelled` / [`Context::cancelled`] 都按这个语义判）。
//! 2. **`ContextKey<T>` 用 `TypeId` 做键**（TS 是每次 `Symbol()` 一个身份）。代价是
//!    同一个类型只能挂一个值 —— 本库够用，而且让 key 能写成 `const`。
//! 3. **`awaitWithContext` 的底层 future 会被丢弃**。TS 的 promise 没人 await 也会继续跑；
//!    Rust 的 future 不 poll 就不动。要让底层继续跑，先 `spawn` 出去，再
//!    `await_with_context(handle, &context)`。
//!
//! ## 不提货
//!
//! `withTelemetry` / `traceIn` 两处**没抄**：跨 `nous-telemetry` 的 span 接线还没定
//! （`nous-chord` 现在无内部依赖，硬接就是把没想清楚的东西焊进去）。
//! `SOURCES.md` 那一行记 `待提货` + 理由；接线时先用 [`ContextKey`] 挂，再决定要不要
//! 变成 `Context` 的一等成员。

use std::any::{Any, TypeId};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use tokio_util::sync::CancellationToken;

/// 一个有类型的 key。身份是 `TypeId`（见模块文档第 2 条）。
///
/// ```ignore
/// const PROGRESS_KEY: ContextKey<ProgressSink> = create_context_key("nous.progress");
/// ```
#[derive(Debug)]
pub struct ContextKey<T: 'static> {
    description: &'static str,
    value_type: PhantomData<fn() -> T>,
}

impl<T: 'static> ContextKey<T> {
    /// 给日志 / `Debug` 用；**不参与身份判定**（身份是 `TypeId::of::<T>()`）。
    pub const fn description(&self) -> &'static str {
        self.description
    }

    /// 这个 key 的身份。查表靠它，不靠 `description`。
    pub fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
}

impl<T: 'static> Clone for ContextKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for ContextKey<T> {}

/// 建一个 key。TS 的 `createContextKey<T>(description)`。
///
/// `const` 可用 —— 所以 `PROGRESS_KEY` 这类可以写成 `const`，不用 `LazyLock`。
pub const fn create_context_key<T: 'static>(description: &'static str) -> ContextKey<T> {
    ContextKey {
        description,
        value_type: PhantomData,
    }
}

/// 节点的取消槽：沿用父节点的、自己挂一个、或者**遮蔽**掉父节点（`withoutAbortSignal`）。
#[derive(Clone)]
enum CancelSlot {
    Inherit,
    Set(CancellationToken),
    Cleared,
}

#[derive(Clone)]
struct Entry {
    key_type: TypeId,
    description: &'static str,
    value: Arc<dyn Any + Send + Sync>,
}

#[derive(Clone)]
struct Node {
    parent: Context,
    entry: Option<Entry>,
    cancel: CancelSlot,
    /// `Display` 用：取消节点没有 `entry`，靠这个标出 `WithCancel` / `WithoutAbortSignal`。
    label: Option<&'static str>,
}

/// 不可变的调用作用域值。`Clone` 很便宜（一个 `Arc`），取消是**共享**的。
///
/// 所有 `FileSystem` / `Shell` 方法都收 `&Context`（AGENTS.md §二：取消与进度用这一份）。
#[derive(Clone)]
pub struct Context {
    root: &'static str,
    node: Option<Arc<Node>>,
}

impl Context {
    /// 一个永不取消、什么值都没有的 context。
    ///
    /// `nous-env` 的 `Context::new()` 就是它；生命周期比一次调用长的服务层应当持有
    /// 一个 [`BACKGROUND_CONTEXT`] 并往下派生。
    pub const fn new() -> Self {
        BACKGROUND_CONTEXT
    }

    /// 挂一个值（替换或新增）。TS 的 `withContextValue`。
    pub fn with_value<T>(self, key: &ContextKey<T>, value: T) -> Self
    where
        T: Any + Send + Sync,
    {
        with_context_value(key, value, self)
    }

    /// 取消信号（最近的一层）。`None` 表示这个 context 不可取消。
    pub fn abort_signal(&self) -> Option<&CancellationToken> {
        let mut cursor = Some(self);
        while let Some(context) = cursor {
            match context.node.as_deref().map(|node| &node.cancel) {
                Some(CancelSlot::Set(token)) => return Some(token),
                Some(CancelSlot::Cleared) | None => return None,
                Some(CancelSlot::Inherit) => {}
            }
            cursor = context.node.as_deref().map(|node| &node.parent);
        }
        None
    }

    /// 链上所有生效的取消信号（最近的一层优先）。`withoutAbortSignal` 会截断这条链。
    ///
    /// 语义是「任一取消即整体取消」—— 对应 TS 的 `AbortSignal.any`。
    pub fn cancellation_tokens(&self) -> Vec<&CancellationToken> {
        let mut tokens = Vec::new();
        let mut cursor = Some(self);
        while let Some(context) = cursor {
            match context.node.as_deref().map(|node| &node.cancel) {
                Some(CancelSlot::Set(token)) => tokens.push(token),
                Some(CancelSlot::Cleared) | None => break,
                Some(CancelSlot::Inherit) => {}
            }
            cursor = context.node.as_deref().map(|node| &node.parent);
        }
        tokens
    }

    /// 链上任一信号已取消，就是已取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_tokens()
            .into_iter()
            .any(CancellationToken::is_cancelled)
    }

    /// 查一个值。找不到返回 `None`（TS 同语义，不 panic）。
    pub fn value<T: 'static>(&self, key: &ContextKey<T>) -> Option<&T> {
        let wanted = key.type_id();
        let mut cursor = Some(self);
        while let Some(context) = cursor {
            let Some(node) = context.node.as_deref() else {
                break;
            };
            if let Some(entry) = node.entry.as_ref() {
                if entry.key_type == wanted {
                    return (entry.value.as_ref() as &dyn Any).downcast_ref::<T>();
                }
            }
            cursor = Some(&node.parent);
        }
        None
    }

    /// 等到取消为止。没有取消信号时**永不返回**（和 TS 里 `await` 一个不会 settle 的
    /// promise 一样）；需要「永不取消」的完成路径请用 [`await_with_context`]。
    pub async fn cancelled(&self) {
        let tokens: Vec<CancellationToken> =
            self.cancellation_tokens().into_iter().cloned().collect();
        if tokens.is_empty() {
            return std::future::pending().await;
        }
        if tokens.iter().any(CancellationToken::is_cancelled) {
            return;
        }
        // 「任一完成」：把每个 token 的 cancelled() 都 pin 起来，逐个 poll。
        // 不用 `tokio::select!`（数量动态）也不用 `futures`（不引它）。
        let mut futures: Vec<Pin<Box<dyn Future<Output = ()> + Send + '_>>> = tokens
            .iter()
            .map(|token| Box::pin(token.cancelled()) as Pin<Box<dyn Future<Output = ()> + Send + '_>>)
            .collect();
        std::future::poll_fn(move |cx| {
            for future in futures.iter_mut() {
                if future.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(());
                }
            }
            Poll::Pending
        })
        .await
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("name", &self.to_string())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl std::fmt::Display for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 和 TS 一样从根往下拼：`[Context BACKGROUND_CONTEXT].WithValue(a).WithValue(b)`。
        let mut chain = Vec::new();
        let mut cursor = Some(self);
        while let Some(context) = cursor {
            let Some(node) = context.node.as_deref() else {
                break;
            };
            chain.push(
                node.entry
                    .as_ref()
                    .map(|entry| entry.description)
                    .or(node.label),
            );
            cursor = Some(&node.parent);
        }
        write!(f, "{}", self.root)?;
        for description in chain.into_iter().rev() {
            write!(f, ".WithValue({})", description.unwrap_or("anonymous"))?;
        }
        Ok(())
    }
}

/// 根 context：没有值、不可取消。服务层拿它当所有调用的父亲。
pub const BACKGROUND_CONTEXT: Context = Context {
    root: "[Context BACKGROUND_CONTEXT]",
    node: None,
};

/// 还没接线的占位 context。TS 拿它当「这里本该有 context 但没有」的显式标记。
pub const TODO_CONTEXT: Context = Context {
    root: "[Context TODO_CONTEXT]",
    node: None,
};

/// 派生一个多挂一个值的 context（父节点不变）。TS 的 `withContextValue`。
pub fn with_context_value<T>(key: &ContextKey<T>, value: T, parent: Context) -> Context
where
    T: Any + Send + Sync,
{
    let node = Node {
        parent: Context {
            root: parent.root,
            node: parent.node.clone(),
        },
        entry: Some(Entry {
            key_type: key.type_id(),
            description: key.description(),
            value: Arc::new(value),
        }),
        cancel: CancelSlot::Inherit,
        label: None,
    };
    Context {
        root: parent.root,
        node: Some(Arc::new(node)),
    }
}

/// 派生一个「父信号或新信号任一取消都算取消」的 context。TS 的 `withAbortSignal`
/// （那边是 `AbortSignal.any`；这里沿链保留两个 token，见模块文档第 1 条）。
pub fn with_abort_signal(signal: CancellationToken, context: Context) -> Context {
    derive_cancel(context, CancelSlot::Set(signal), "cancel")
}

/// 保留所有值，**只去掉调用方取消**。只给必须做完的清理用。TS 的 `withoutAbortSignal`。
pub fn without_abort_signal(context: Context) -> Context {
    derive_cancel(context, CancelSlot::Cleared, "withoutAbortSignal")
}

fn derive_cancel(context: Context, slot: CancelSlot, label: &'static str) -> Context {
    let node = Node {
        parent: Context {
            root: context.root,
            node: context.node.clone(),
        },
        entry: None,
        cancel: slot,
        label: Some(label),
    };
    Context {
        root: context.root,
        node: Some(Arc::new(node)),
    }
}

/// 一个可独立取消的子 context。TS 的 `withCancel` 返回 `{ context, cancel }`。
///
/// `Clone` 出来的 handle **共享同一个 token**：三个触发源（客户端 abort / 内核超时 /
/// 扩展进程死）各持一个 clone，任何一处 `cancel()` 都打在同一条通道上。
#[derive(Clone)]
pub struct CancelHandle {
    token: CancellationToken,
}

impl CancelHandle {
    /// 取消（可以带理由，本库只记字符串）。
    pub fn cancel(&self) {
        self.token.cancel();
    }

    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
}

/// 派生一个可独立取消的子 context。父 context 不被修改。
pub fn with_cancel(context: Context) -> (Context, CancelHandle) {
    let token = CancellationToken::new();
    let derived = with_abort_signal(token.clone(), context);
    (derived, CancelHandle { token })
}

/// 取消原因。TS 复用 `signal.reason`；这里只有一种，理由由调用方自己的错误类型表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Canceled;

impl std::fmt::Display for Canceled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the operation was aborted")
    }
}

impl std::error::Error for Canceled {}

/// 等到 `future` 完成 **或** 这个 context 被取消。
///
/// 取消只让**这个等待者**返回 `Err(Canceled)`；底层 future 被丢弃（Rust 与 TS 的差别，
/// 见模块文档第 3 条）。要让底层继续跑：先 `spawn`，再把它传进来。
pub async fn await_with_context<F>(future: F, context: &Context) -> Result<F::Output, Canceled>
where
    F: Future,
{
    if context.is_cancelled() {
        return Err(Canceled);
    }
    let mut future = std::pin::pin!(future);
    let mut cancelled = std::pin::pin!(context.cancelled());
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(output) = future.as_mut().poll(cx) {
            return Poll::Ready(Ok(output));
        }
        if cancelled.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(Canceled));
        }
        Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    const COUNTER_KEY: ContextKey<u32> = create_context_key("test.counter");
    const COUNTER_KEY_TWIN: ContextKey<u32> = create_context_key("test.counter.twin");
    const OTHER_KEY: ContextKey<String> = create_context_key("test.other");

    #[test]
    fn background_context_has_nothing() {
        assert!(BACKGROUND_CONTEXT.value(&COUNTER_KEY).is_none());
        assert!(!BACKGROUND_CONTEXT.is_cancelled());
        assert!(BACKGROUND_CONTEXT.cancellation_tokens().is_empty());
        assert_eq!(
            BACKGROUND_CONTEXT.to_string(),
            "[Context BACKGROUND_CONTEXT]"
        );
        assert_eq!(TODO_CONTEXT.to_string(), "[Context TODO_CONTEXT]");
    }

    #[test]
    fn derived_values_shadow_the_parent_and_leave_it_untouched() {
        let parent = with_context_value(&COUNTER_KEY, 1u32, BACKGROUND_CONTEXT);
        let child = with_context_value(&COUNTER_KEY, 2u32, parent.clone());
        assert_eq!(parent.value(&COUNTER_KEY), Some(&1));
        assert_eq!(child.value(&COUNTER_KEY), Some(&2));
        // 父节点不受影响 —— `Clone` 是值语义。
        assert_eq!(child.value(&OTHER_KEY), None);
        let with_other = with_context_value(&OTHER_KEY, "x".to_string(), child.clone());
        assert_eq!(with_other.value(&OTHER_KEY).map(String::as_str), Some("x"));
        assert_eq!(with_other.value(&COUNTER_KEY), Some(&2));
        assert!(child.to_string().ends_with(".WithValue(test.counter)"));
        assert!(without_abort_signal(child.clone())
            .to_string()
            .ends_with(".WithValue(withoutAbortSignal)"));
    }

    #[test]
    fn keys_are_identified_by_type_not_by_description() {
        // 同一个 T 的两个 key 命中同一个槽（TypeId 身份）—— 这是本库的取舍，写进模块文档。
        let context = with_context_value(&COUNTER_KEY, 7u32, BACKGROUND_CONTEXT);
        assert_eq!(context.value(&COUNTER_KEY_TWIN), Some(&7));
        // 不同 T 互不可见。
        assert_eq!(context.value(&OTHER_KEY), None);
    }

    #[test]
    fn values_are_shared_across_clones() {
        let counter = Arc::new(AtomicUsize::new(0));
        let context = with_context_value(&COUNTER_KEY, 0u32, BACKGROUND_CONTEXT);
        let share = context.clone();
        assert_eq!(share.value(&COUNTER_KEY), Some(&0));
        counter.fetch_add(1, Ordering::Relaxed);
        assert_eq!(context.value(&COUNTER_KEY), Some(&0));
    }

    #[tokio::test]
    async fn abort_signals_combine_as_any() {
        let parent_token = CancellationToken::new();
        let child_token = CancellationToken::new();
        let parent = with_abort_signal(parent_token.clone(), BACKGROUND_CONTEXT);
        let child = with_abort_signal(child_token.clone(), parent);
        assert_eq!(child.cancellation_tokens().len(), 2);
        assert!(!child.is_cancelled());

        // 父取消 → 子也算取消（`AbortSignal.any` 语义）。
        parent_token.cancel();
        assert!(child.is_cancelled());

        // 另一条：子取消 → 也取消。
        let parent_token = CancellationToken::new();
        let child_token = CancellationToken::new();
        let child = with_abort_signal(
            child_token.clone(),
            with_abort_signal(parent_token.clone(), BACKGROUND_CONTEXT),
        );
        child_token.cancel();
        assert!(child.is_cancelled());
        assert!(!parent_token.is_cancelled(), "不反向传染");
    }

    #[tokio::test]
    async fn without_abort_signal_shadows_the_parent_and_keeps_values() {
        let token = CancellationToken::new();
        let context = with_context_value(
            &COUNTER_KEY,
            5u32,
            with_abort_signal(token.clone(), BACKGROUND_CONTEXT),
        );
        let cleanup = without_abort_signal(context);
        assert!(!cleanup.is_cancelled());
        assert!(cleanup.cancellation_tokens().is_empty());
        token.cancel();
        assert!(!cleanup.is_cancelled(), "清理路径不受调用方取消影响");
        assert_eq!(cleanup.value(&COUNTER_KEY), Some(&5), "值全部保留");
    }

    #[tokio::test]
    async fn with_cancel_gives_an_independent_child() {
        let parent = BACKGROUND_CONTEXT;
        let (child, handle) = with_cancel(parent.clone());
        assert!(!child.is_cancelled());
        handle.cancel();
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());

        // 兄弟之间互不影响。
        let (sibling, sibling_handle) = with_cancel(parent.clone());
        assert!(!sibling.is_cancelled());
        sibling_handle.cancel();
        assert!(sibling.is_cancelled());
    }

    /// 测试定时器：不借 `tokio 的时间 API`（#19 的 grep 逐行空）。
    async fn test_sleep(duration: Duration) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            std::thread::sleep(duration);
            let _ = tx.send(());
        });
        let _ = rx.await;
    }

    async fn test_timeout<F: std::future::Future>(
        duration: Duration,
        future: F,
    ) -> Result<F::Output, ()> {
        tokio::select! {
            value = future => Ok(value),
            _ = test_sleep(duration) => Err(()),
        }
    }

    #[tokio::test]
    async fn cancelled_resolves_on_any_token() {
        let token = CancellationToken::new();
        let context = with_abort_signal(token.clone(), BACKGROUND_CONTEXT);
        let waiting = tokio::spawn(async move {
            context.cancelled().await;
        });
        test_sleep(Duration::from_millis(10)).await;
        token.cancel();
        test_timeout(Duration::from_secs(1), waiting)
            .await
            .expect("cancelled() must resolve")
            .unwrap();
    }

    #[tokio::test]
    async fn await_with_context_returns_the_value_or_cancels_the_waiter() {
        let context = BACKGROUND_CONTEXT;
        assert_eq!(await_with_context(async { 7 }, &context).await, Ok(7));

        let token = CancellationToken::new();
        let cancelled = with_abort_signal(token.clone(), BACKGROUND_CONTEXT);
        let waiter = tokio::spawn(async move {
            await_with_context(std::future::pending::<()>(), &cancelled).await
        });
        test_sleep(Duration::from_millis(10)).await;
        token.cancel();
        assert_eq!(
            test_timeout(Duration::from_secs(1), waiter)
                .await
                .expect("waiter must return")
                .unwrap(),
            Err(Canceled)
        );

        // 已经取消的 context 立刻返回，不 poll 底层 future。
        let already = with_abort_signal(token, BACKGROUND_CONTEXT);
        let mut polled = false;
        let result = await_with_context(
            async {
                polled = true;
            },
            &already,
        )
        .await;
        assert_eq!(result, Err(Canceled));
        assert!(!polled, "取消后不该再 poll 底层 future");
    }

    #[test]
    fn cancel_handle_exposes_its_token_for_backends() {
        let (context, handle) = with_cancel(BACKGROUND_CONTEXT);
        assert_eq!(context.cancellation_tokens()[0], handle.token());
    }
}
