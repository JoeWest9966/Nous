//! 进程内的装配点：数据源 + 广播通道，以及「一次提交 → 这个会话的订阅者」这一跳。
//!
//! 生产里 `advance` 由 `nous_durable::session::Session` 的 `CommitObserver` 同步调用。
//! 那一层不许 await（上游契约），所以这里整条路径是同步的：投影 → `broadcast::send`。
//! 合帧在连接侧（[`crate::coalesce`]），不在提交侧 —— 提交侧攒帧会让「工具启动」晚到。

use std::sync::Arc;

use nous_durable::session::publish::{CommitChange, CommitPublication, DocumentCommitChange};
use nous_wire::RemoteEvent;

use crate::error::Result;
use crate::hub::Hub;
use crate::project;
use crate::source::SessionSource;

/// 路由共享的状态。
pub struct AppState {
    source: Arc<dyn SessionSource>,
    hub: Arc<Hub>,
}

impl AppState {
    pub fn new(source: Arc<dyn SessionSource>, hub: Arc<Hub>) -> Arc<Self> {
        Arc::new(Self { source, hub })
    }

    /// 用默认 `Hub` 装配。
    pub fn with_source(source: Arc<dyn SessionSource>) -> Arc<Self> {
        Self::new(source, Hub::new())
    }

    pub fn source(&self) -> &Arc<dyn SessionSource> {
        &self.source
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    /// 一次提交 → 帧，发给**每个改动自己所属**的会话。
    ///
    /// 不是「整个 AppState 一个会话」：`Session` 不绑会话，一次提交里的 `Entry` / `Document`
    /// 各自带 `conversationId`（session 作用域的文档在 `record.scope` 里）。按改动分派，
    /// 免得把 A 会话的 delta 推到 B 会话的 SSE 上。
    pub fn publish_publication(&self, publication: &CommitPublication) -> Result<()> {
        for change in &publication.changes {
            let Some(session) = session_of(change) else {
                continue;
            };
            self.hub
                .publish_all(session, project::of_change(change)?);
        }
        Ok(())
    }

    /// 把一个已投成帧的事件直接发给会话。判据用这条注入**合帧器的真实输入**。
    pub fn publish_event(&self, session: i64, event: RemoteEvent) {
        self.hub.publish(session, event);
    }
}

/// 一条改动属于哪个会话。
fn session_of(change: &CommitChange) -> Option<i64> {
    match change {
        CommitChange::Conversation(record) => Some(record.id),
        CommitChange::Entry(record) => Some(record.conversation_id),
        CommitChange::Task(record) => Some(record.conversation_id),
        CommitChange::Submission(record) => Some(record.fields.conversation_id),
        CommitChange::Document(document) => match document {
            DocumentCommitChange::Change {
                conversation_id, ..
            }
            | DocumentCommitChange::Retire {
                conversation_id, ..
            } => *conversation_id,
            DocumentCommitChange::Copy {
                conversation_id, ..
            } => Some(*conversation_id),
        },
    }
}
