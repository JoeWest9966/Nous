//! `CommitObserver` 适配：把 `nous-durable` 的同步回调接成 SSE 扇出。
//!
//! `advance` 是**同步**契约（`session/publish.rs`）：不许 await、不许回调 Session、不许抛。
//! 所以这里只做两件事：投影（纯函数，见 [`crate::project`]）+ `broadcast::send`（不阻塞）。
//! 投影失败时**不发帧**并记一条 stderr —— 观察者没有错误通道，把库里的 bug 变成「静默断流」
//! 比「少一帧 + 一条日志」更难查；但 panic 会冒到提交调用方，也不能选。
//!
//! ## 为什么不持 `AppState`
//!
//! 这个适配器只需要两样东西：一条提交公开、一条扇出通道。持整个宿主会让它跟着
//! `AppState` 的每个字段一起搬家 —— 而它本来只认 [`Hub`]。

use std::sync::Arc;

use nous_durable::session::publish::{CommitObserver, CommitPublication, Context, WatchEnd};

use crate::hub::Hub;
use crate::project;

pub struct SessionObserver {
    hub: Arc<Hub>,
}

impl SessionObserver {
    pub fn new(hub: Arc<Hub>) -> Arc<Self> {
        Arc::new(Self { hub })
    }
}

impl CommitObserver for SessionObserver {
    fn advance(&self, publication: &CommitPublication, _context: &Context) {
        if let Err(error) = publish_publication(&self.hub, publication) {
            tracing_error(&error.to_string());
        }
    }

    fn end(&self, _end: &WatchEnd) {
        // 连接收尾在 `Session::close` 那边：SSE 的下游是客户端连接，不是观察者的生命周期。
        // 客户端会自己重连（第一帧 `snapshot` 就是为这个存在的）。
    }
}

/// 一份提交公开 → 零到多帧，逐条扔进对应会话的通道。
///
/// 一条改动属于哪个会话由 [`project::session_of`] 判；判不出来（比如 `Conversation`
/// 元数据）就跳过 —— `wire.md` 没有会话元数据帧。
pub fn publish_publication(
    hub: &Hub,
    publication: &CommitPublication,
) -> crate::error::Result<()> {
    for change in &publication.changes {
        let Some(session) = project::session_of(change) else {
            continue;
        };
        hub.publish_all(session, project::of_change(change)?);
    }
    Ok(())
}

fn tracing_error(message: &str) {
    // 不引 `tracing` 依赖：一条 stderr 足够，且不把观测栈带进传输层。
    eprintln!("nous-sessions: publication projection failed: {message}");
}
