//! 会话数据从哪来。端点层只认这个 trait，不认具体存储。
//!
//! ## 为什么是一层 trait
//!
//! 判据不能依赖 `fixtures/agent.sqlite`：它是真实用户数据、不进 git，CI 上不存在。
//! 于是「HTTP 形状对不对」用假源测（手写 `SessionItem` / entry JSON），「fixture 读得出
//! 同形 JSON」由 `scripts/verify.sh` 第 2 段单独跑（`nous-fixture`）。两层各测一面。
//!
//! ## 为什么每次请求重新读，而不是开库时缓存一份
//!
//! `POST /messages` / `fork` 会往**同一份库**写。缓存一份 `ImportedStore` 等于在端点层
//! 再造一个失效管理：写进去的会话在 `/v1/sessions` 里看不见，测试就会去测缓存而不是测库。
//! 189 条会话的全量读是毫秒级；真到了量级瓶颈，那时候已经有提交信号可以用来失效。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nous_durable::storage::cursor::{Cursor, ScanOrder};
use nous_durable::storage::import::ImportedStore;
use nous_durable::storage::sessions::{
    render_entries_page, render_live_message, render_sessions, SessionItem, ENTRIES_MAX_LIMIT,
};
use serde_json::Value;

use crate::error::{Result, ServerError};

/// 端点要读的两类东西：会话快照 + 分页历史。
pub trait SessionSource: Send + Sync {
    /// 会话树，新的在前（`GET /v1/sessions`）。
    fn sessions(&self) -> Result<Vec<SessionItem>>;

    /// 一个会话的 entry 历史，新的在前（`snapshot.entries` 用它）。
    fn entries(&self, session: i64) -> Result<Vec<Value>>;

    /// 一页 entry：`(items, next)`。默认实现从 [`entries`](Self::entries) 里切，
    /// 够假源用；真源覆写它走库里的游标扫描。
    fn entries_page(
        &self,
        session: i64,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<(Vec<Value>, Option<Cursor>)> {
        // 上限是**端点策略**（`wire.md` 的 1000），不在这里悄悄夹 —— 否则路由层漏夹也测不出来。
        let limit = limit.max(1);
        let after = cursor.map(|cursor| cursor.after).unwrap_or(i64::MAX);
        let mut items: Vec<Value> = self
            .entries(session)?
            .into_iter()
            .filter(|entry| entry.get("id").and_then(Value::as_i64).unwrap_or(0) < after)
            .collect();
        let next = if items.len() > limit {
            items.truncate(limit);
            items
                .last()
                .and_then(|entry| entry.get("id").and_then(Value::as_i64))
                .map(|id| Cursor::new(id, ScanOrder::Descending))
        } else {
            None
        };
        Ok((items, next))
    }

    /// 正在生成的那条消息（`snapshot.generation.message`）；没有就是 `None`。
    fn live_message(&self, session: i64) -> Result<Option<Value>>;

    /// 会话在不在。不存在时端点返回 404，而不是空快照。
    fn exists(&self, session: i64) -> Result<bool> {
        Ok(self.sessions()?.iter().any(|item| item.id == session))
    }
}

/// 只读打开一份库（上游 v2 或本库自己的都行）。**永远不写回去**。
pub struct ImportedSource {
    path: PathBuf,
}

impl ImportedSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        let path = path.as_ref().to_path_buf();
        // 开库时就试读一次：路径错了要在装配期报，不要等第一个请求。
        ImportedStore::read(&path)
            .map_err(|error| ServerError::Internal(format!("open {}: {error}", path.display())))?;
        Ok(Arc::new(Self { path }))
    }

    fn store(&self) -> Result<ImportedStore> {
        ImportedStore::read(&self.path).map_err(|error| {
            ServerError::Internal(format!("read {}: {error}", self.path.display()))
        })
    }
}

impl SessionSource for ImportedSource {
    fn sessions(&self) -> Result<Vec<SessionItem>> {
        render_sessions(&self.store()?)
            .map_err(|error| ServerError::Internal(format!("render /v1/sessions: {error}")))
    }

    fn entries(&self, session: i64) -> Result<Vec<Value>> {
        nous_durable::storage::sessions::render_entries(&self.store()?, session)
            .map_err(|error| ServerError::Internal(format!("render entries: {error}")))
    }

    fn entries_page(
        &self,
        session: i64,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<(Vec<Value>, Option<Cursor>)> {
        render_entries_page(&self.store()?, session, limit, cursor)
            .map_err(|error| ServerError::Internal(format!("render entries page: {error}")))
    }

    fn live_message(&self, session: i64) -> Result<Option<Value>> {
        render_live_message(&self.store()?, session)
            .map_err(|error| ServerError::Internal(format!("render live message: {error}")))
    }
}
