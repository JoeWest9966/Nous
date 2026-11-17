//! 会话数据从哪来。端点层只认这个 trait，不认具体存储。
//!
//! ## 为什么是一层 trait
//!
//! 判据不能依赖 `fixtures/agent.sqlite`：它是真实用户数据、不进 git，CI 上不存在。
//! 于是「HTTP 形状对不对」用假源测（手写 `SessionItem` / entry JSON），「fixture 读得出
//! 同形 JSON」由 `scripts/verify.sh` 第 2 段单独跑（`nous-fixture`）。两层各测一面。
//!
//! 生产实现是 [`ImportedSource`]：只读打开一份库，把 `nous-durable` 的三个投影函数接上来。
//! 名字里的「imported」是**旧引擎文档**那层知识（`storage/sessions.rs` 的模块文档），不是
//! 「一次性导入」—— 端点每次请求都重新扫一遍那几份文档。
//!
//! ## 为什么 `sessions` 每次重算
//!
//! `/v1/sessions` 是**快照**：客户端低频轮询它。缓存一份「会话树」等于在这里再造一个
//! 失效管理，而真正的真相在库里。fixture 189 条会话的全量扫描是毫秒级；等真有自己的写入
//! 路径、量级上来，再加缓存也来得及（那时有提交信号可用）。

use std::sync::Arc;

use nous_durable::storage::import::ImportedStore;
use nous_durable::storage::sessions::{
    render_entries, render_live_message, render_sessions, SessionItem,
};
use serde_json::Value;

use crate::error::{Result, ServerError};

/// 端点要读的三样东西。
pub trait SessionSource: Send + Sync {
    /// 会话树，新的在前（`GET /v1/sessions`）。
    fn sessions(&self) -> Result<Vec<SessionItem>>;

    /// 一个会话的 entry 历史，新的在前（`snapshot.entries` 用它）。
    fn entries(&self, session: i64) -> Result<Vec<Value>>;

    /// 正在生成的那条消息（`snapshot.generation.message`）；没有就是 `None`。
    fn live_message(&self, session: i64) -> Result<Option<Value>>;

    /// 会话在不在。不存在时端点返回 404，而不是空快照。
    fn exists(&self, session: i64) -> Result<bool> {
        Ok(self.sessions()?.iter().any(|item| item.id == session))
    }
}

/// 只读打开一份 `agent.sqlite`。**永远不写回去**。
pub struct ImportedSource {
    store: ImportedStore,
}

impl ImportedSource {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Arc<Self>> {
        let path = path.as_ref();
        let store = ImportedStore::read(path)
            .map_err(|error| ServerError::Internal(format!("open {}: {error}", path.display())))?;
        Ok(Arc::new(Self { store }))
    }
}

impl SessionSource for ImportedSource {
    fn sessions(&self) -> Result<Vec<SessionItem>> {
        render_sessions(&self.store)
            .map_err(|error| ServerError::Internal(format!("render /v1/sessions: {error}")))
    }

    fn entries(&self, session: i64) -> Result<Vec<Value>> {
        render_entries(&self.store, session)
            .map_err(|error| ServerError::Internal(format!("render entries: {error}")))
    }

    fn live_message(&self, session: i64) -> Result<Option<Value>> {
        render_live_message(&self.store, session)
            .map_err(|error| ServerError::Internal(format!("render live message: {error}")))
    }
}
