//! Invocation-scoped values threaded through every `FileSystem` / `Shell` call.
//!
//! ## 为什么不照 TS 的 `Context`
//!
//! TS 的 `Context`（`packages/chord/src/context/index.ts`）是 `abortSignal` +
//! `TelemetrySpan` + 一个泛型 `value(key)` 查表。本库**不能**照抄那两样：
//!
//! - `AbortSignal` → [`CancellationToken`]（tokio 的，`nous-ai` 已经用它，全库一致）；
//! - `TelemetrySpan` 来自 `nous-telemetry`，而 `nous-env` **无内部依赖**
//!   （AGENTS.md §二）。所以进度改成**一个可选 sink**，由装配层（Nous 服务层）
//!   接到 telemetry 上。trait 面因此不认识 telemetry。
//!
//! 这是本库对上游唯一的结构性改动，也是 #6 票面同意的那个改动：**改一次，
//! 不散进 116 个调用点**。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::error::{ExecutionError, FileError, FileErrorCode};

/// A progress sample. 本库扩展（TS 走 telemetry span，见模块文档）。
///
/// `done` / `total` 是**调用方自己定的单位**（字节、行、条目都行）；环境
/// 只负责报数，不解释。`label` 用于日志/UI 里那一行字。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Progress {
    pub done: u64,
    pub total: Option<u64>,
    pub label: Option<String>,
}

/// Where progress goes. `None` 表示没有人在听 —— 所有 `report_progress` 都是空操作。
pub type ProgressSink = Arc<dyn Fn(&Progress) + Send + Sync>;

/// Immutable invocation-scoped values.
///
/// 所有 `FileSystem` / `Shell` 方法都收 `&Context`（TS 的最后那个 `context` 参数）。
/// `Clone` 很便宜（两个 `Arc`），子任务各拿一份即可 —— 取消是**共享**的。
#[derive(Clone, Default)]
pub struct Context {
    cancel: CancellationToken,
    progress: Option<ProgressSink>,
}

impl Context {
    /// A context that is never cancelled and reports no progress.
    pub fn new() -> Self {
        Self::default()
    }

    /// A context whose lifetime is tied to an existing token (share, don't wrap).
    pub fn with_cancel(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            progress: None,
        }
    }

    /// Attach a progress sink. Returns `self` for chaining.
    pub fn with_progress(mut self, sink: ProgressSink) -> Self {
        self.progress = Some(sink);
        self
    }

    /// The shared cancellation token. Backends poll this; callers cancel it.
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancel
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Fail fast when the call was aborted. Backends call this before/while doing
    /// work so cancellation surfaces as `aborted`, not as a half-done mutation.
    pub fn check(&self) -> Result<(), FileError> {
        if self.cancel.is_cancelled() {
            Err(FileError::new(FileErrorCode::Aborted, "operation aborted"))
        } else {
            Ok(())
        }
    }

    /// Same, with the shell's error type.
    pub fn check_exec(&self) -> Result<(), ExecutionError> {
        if self.cancel.is_cancelled() {
            Err(ExecutionError::aborted())
        } else {
            Ok(())
        }
    }

    /// Report one sample. No-op when nobody is listening.
    pub fn report_progress(&self, progress: Progress) {
        if let Some(sink) = self.progress.as_ref() {
            sink(&progress);
        }
    }
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("cancelled", &self.cancel.is_cancelled())
            .field("has_progress_sink", &self.progress.is_some())
            .finish()
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
    }

    #[test]
    fn cancelling_the_shared_token_fails_checks() {
        let token = CancellationToken::new();
        let context = Context::with_cancel(token.clone());
        token.cancel();
        assert!(context.is_cancelled());
        assert_eq!(
            context.check().unwrap_err().code,
            FileErrorCode::Aborted
        );
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
}
