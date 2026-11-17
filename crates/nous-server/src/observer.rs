//! `CommitObserver` 适配：把 `nous-durable` 的同步回调接成 SSE 扇出。
//!
//! `advance` 是**同步**契约（`session/publish.rs`）：不许 await、不许回调 Session、不许抛。
//! 所以这里只做两件事：投影（纯函数）+ `broadcast::send`（不阻塞）。投影失败时**不发帧**
//! 并记一条 `tracing::error!` —— 观察者没有错误通道，把库里的 bug 变成「静默断流」比
//! 「少一帧 + 一条日志」更难查；但 panic 会冒到提交调用方，也不能选。

use std::sync::Arc;

use nous_durable::session::publish::{CommitObserver, CommitPublication, Context, WatchEnd};

use crate::app::AppState;

pub struct SessionObserver {
    state: Arc<AppState>,
}

impl SessionObserver {
    pub fn new(state: Arc<AppState>) -> Arc<Self> {
        Arc::new(Self { state })
    }
}

impl CommitObserver for SessionObserver {
    fn advance(&self, publication: &CommitPublication, _context: &Context) {
        if let Err(error) = self.state.publish_publication(publication) {
            tracing_error(&error.to_string());
        }
    }

    fn end(&self, _end: &WatchEnd) {
        // 连接收尾在 `Session::close` 那边：SSE 的下游是客户端连接，不是观察者的生命周期。
        // 客户端会自己重连（第一帧 `snapshot` 就是为这个存在的）。
    }
}

fn tracing_error(message: &str) {
    // 不引 `tracing` 依赖：一条 stderr 足够，且不把观测栈带进传输层。
    eprintln!("nous-server: publication projection failed: {message}");
}
