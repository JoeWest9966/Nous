//! 调用作用域的值 —— **只有一份**，住在 `nous-chord`。
//!
//! ## 为什么这里不自己定义 `Context`
//!
//! TS 侧只有一份：`packages/chord/src/context/index.ts` 的 `Context`，
//! `packages/durable/src/env/index.ts` 每个方法的最后一个参数 `context: Context` 拿的就是它。
//! 本库早先为了「`nous-env` 无内部依赖」自造过一个 `Context{cancel, progress}` —— 那是**两套**。
//! `#12` 一开工就会炸：durable 的每个函数同时拿到 env 与 chord，两套 `Context` 参数要一路转。
//!
//! 所以 `AGENTS.md` §二 的依赖图改成 `nous-env → nous-chord`：取消与进度用 chord 那份，
//! 本模块只做三件事 ——
//!
//! 1. **重导出** chord 的 [`Context`]（`pub use`，不是新类型）；
//! 2. 定义**进度**这一次扩展（[`Progress`] / [`ProgressSink`] / [`PROGRESS_KEY`]）；
//! 3. 用扩展 trait [`ContextExt`] 挂上 `nous-env` 自己的检查（错误类型在这里，
//!    chord 不可能认识 `FileError`）。
//!
//! ## 进度为什么是 key，不是字段
//!
//! 上游 `Context` 的成员是 `abortSignal` + `telemetry` + 一个泛型 `value(key)` 查表。
//! 进度在 TS 里就是 telemetry 的事；本库的 `Progress` 是**扩展**（TS 没有这个类型），
//! 挂成 [`ContextKey`] 而不是结构体字段的好处是：`Context` 的形状和上游一致，
//! 以后要接 `nous-telemetry` 的 span 是「再挂一个 key」，不是改结构体。
//!
//! 接线的位置在装配层（Nous 服务层）：它把 `ProgressSink` 接到 telemetry / UI 上。
//! `nous-env` 这一层不认识 telemetry，也不需要认识。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::error::{ExecutionError, FileError, FileErrorCode};

pub use nous_chord::context::{
    await_with_context, create_context_key, with_abort_signal, with_cancel, with_context_value,
    without_abort_signal, CancelHandle, Canceled, Context, ContextKey, BACKGROUND_CONTEXT,
    TODO_CONTEXT,
};

/// A progress sample. 本库扩展（TS 走 telemetry span，见模块文档）。
///
/// `done` / `total` 是**调用方自己定的单位**（字节、行、条目都行）；环境只负责报数，
/// 不解释。`label` 用于日志/UI 里那一行字。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Progress {
    pub done: u64,
    pub total: Option<u64>,
    pub label: Option<String>,
}

/// Where progress goes. `None` 表示没有人在听 —— 所有 `report_progress` 都是空操作。
pub type ProgressSink = Arc<dyn Fn(&Progress) + Send + Sync>;

/// 进度这个值的 key。挂/读都走它，不往 `Context` 结构体里加字段。
pub const PROGRESS_KEY: ContextKey<ProgressSink> = create_context_key("nous.progress");

/// 把一个进度出口挂到 context 上（父 context 不变）。
pub fn with_progress(context: Context, sink: ProgressSink) -> Context {
    with_context_value(&PROGRESS_KEY, sink, context)
}

/// 读 key 报一次进度；没人听就是空操作。
pub fn report_progress(context: &Context, progress: Progress) {
    if let Some(sink) = context.value(&PROGRESS_KEY) {
        sink(&progress);
    }
}

/// 把 `nous-env` 的错误与便捷方法挂到共享的 [`Context`] 上。
///
/// 为什么是 trait 而不是结构体上的方法：`Context` 定义在 `nous-chord`，Rust 不许给外部类型
/// 写固有方法；而 `check()` 要返回 `FileError`，那是本 crate 的类型 —— chord 不认识。
/// **trait 面（`FileSystem` / `Shell` 的 26 + 2 个方法）一个都没动**，改的只是这三种
/// 便捷方法的存在形式。
pub trait ContextExt: Sized {
    /// 一个「设了取消令牌」的 context（旧的 `Context::with_cancel(token)`）。
    fn with_cancel(cancel: CancellationToken) -> Self;

    /// 挂进度出口，返回 `self` 供链式调用。
    fn with_progress(self, sink: ProgressSink) -> Self;

    /// 最近的取消令牌（诊断 / 后端自己 `select!` 时用）。不可取消就是 `None`。
    fn cancellation_token(&self) -> Option<&CancellationToken>;

    /// 已被取消就立刻失败，带 `FileErrorCode::Aborted`。
    ///
    /// 后端在动手前后各调一次，取消就表现为 `aborted`，而不是「写了一半」。
    fn check(&self) -> Result<(), FileError>;

    /// 同上，用 shell 的错误类型。
    fn check_exec(&self) -> Result<(), ExecutionError>;

    /// 报一次进度（见 [`report_progress`]）。
    fn report_progress(&self, progress: Progress);
}

impl ContextExt for Context {
    fn with_cancel(cancel: CancellationToken) -> Self {
        with_abort_signal(cancel, BACKGROUND_CONTEXT)
    }

    fn with_progress(self, sink: ProgressSink) -> Self {
        crate::context::with_progress(self, sink)
    }

    fn cancellation_token(&self) -> Option<&CancellationToken> {
        self.abort_signal()
    }

    fn check(&self) -> Result<(), FileError> {
        if self.is_cancelled() {
            Err(FileError::new(FileErrorCode::Aborted, "operation aborted"))
        } else {
            Ok(())
        }
    }

    fn check_exec(&self) -> Result<(), ExecutionError> {
        if self.is_cancelled() {
            Err(ExecutionError::aborted())
        } else {
            Ok(())
        }
    }

    fn report_progress(&self, progress: Progress) {
        crate::context::report_progress(self, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_context_is_not_cancelled() {
        let context = Context::new();
        assert!(!context.is_cancelled());
        assert!(context.check().is_ok());
        assert!(context.check_exec().is_ok());
        assert!(context.cancellation_token().is_none());
    }

    #[test]
    fn cancelling_the_shared_token_fails_checks() {
        let token = CancellationToken::new();
        let context = Context::with_cancel(token.clone());
        token.cancel();
        assert!(context.is_cancelled());
        assert_eq!(context.check().unwrap_err().code, FileErrorCode::Aborted);
        assert_eq!(
            context.check_exec().unwrap_err().code,
            crate::error::ExecutionErrorCode::Aborted
        );
    }

    #[test]
    fn progress_goes_to_the_sink_and_nowhere_when_absent() {
        // No sink: must not panic.
        Context::new().report_progress(Progress {
            done: 1,
            total: Some(2),
            label: None,
        });

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = {
            let seen = seen.clone();
            Arc::new(move |progress: &Progress| seen.lock().unwrap().push(progress.clone()))
        };
        let context = Context::new().with_progress(sink);
        context.report_progress(Progress {
            done: 3,
            total: Some(9),
            label: Some("scan".into()),
        });
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].done, 3);
        assert_eq!(seen[0].label.as_deref(), Some("scan"));
    }

    #[test]
    fn progress_travels_with_the_context_tree() {
        // 挂在一个子 context 上，兄弟分支看不到 —— 键查表是沿链的。
        let seen = Arc::new(std::sync::Mutex::new(0u32));
        let counter = seen.clone();
        let sink: ProgressSink = Arc::new(move |_| *counter.lock().unwrap() += 1);
        let parent = Context::new();
        let child = parent.clone().with_progress(sink);
        child.report_progress(Progress::default());
        parent.report_progress(Progress::default());
        assert_eq!(*seen.lock().unwrap(), 1);
    }

    #[test]
    fn one_context_type_for_the_whole_library() {
        // 这里的关键判据：`nous_env::Context` 就是 `nous_chord::context::Context`
        // （同一个 TypeId —— 不是两个长得像的类型）。
        assert_eq!(
            std::any::TypeId::of::<Context>(),
            std::any::TypeId::of::<nous_chord::context::Context>()
        );
    }

    #[test]
    fn cancel_scopes_are_independent_per_child() {
        let parent = Context::new();
        let (child, handle) = with_cancel(parent.clone());
        handle.cancel();
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
        // 清理路径：去掉取消，写入照做。
        let cleanup = without_abort_signal(child);
        assert!(!cleanup.is_cancelled());
        assert!(cleanup.check().is_ok());
    }
}
