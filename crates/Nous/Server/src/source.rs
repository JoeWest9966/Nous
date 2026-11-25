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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nous_durable::storage::cursor::{Cursor, ScanOrder};
use nous_durable::storage::import::ImportedStore;
use nous_durable::storage::sessions::{
    render_entries_page, render_live_message, SessionItem,
};
pub use nous_durable::SearchItem;
use serde_json::{json, Value};

use crate::error::{Result, ServerError};

/// 端点要读的两类东西：会话快照 + 分页历史。
pub trait SessionSource: Send + Sync {
    /// 会话树，新的在前（`GET /v1/sessions`）。
    fn sessions(&self) -> Result<Vec<SessionItem>>;

    /// 把 `device` 编目的档位表交给投影层。默认什么都不做：假源不关心档位，
    /// 真源覆写它，在装配期把编目折成纯数据（kernel 不依赖 app 层的编目 crate）。
    fn set_model_levels(&self, _models: &nous_device::ModelsResponse) {}

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

    /// #85：一次性搜索投影。服务端扫一次库，不让客户端逐会话拉历史。
    fn search(&self, query: &str, kind: &str) -> Result<Vec<SearchItem>> {
        let _ = (query, kind);
        Err(ServerError::Conflict("search not configured".into()))
    }

    /// #85：谁在用某个扩展 / 技能。读会话记录的对应字段。
    fn usage(&self, field: &str) -> Result<Value> {
        let _ = field;
        Ok(json!({}))
    }
}

/// 只读打开一份库（上游 v2 或本库自己的都行）。**永远不写回去**。
pub struct ImportedSource {
    path: PathBuf,
    model_levels: std::sync::Mutex<BTreeMap<String, Vec<String>>>,
}

impl ImportedSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Arc<Self>> {
        let path = path.as_ref().to_path_buf();
        // 开库时就试读一次：路径错了要在装配期报，不要等第一个请求。
        ImportedStore::read(&path)
            .map_err(|error| ServerError::Internal(format!("open {}: {error}", path.display())))?;
        Ok(Arc::new(Self {
            path,
            model_levels: std::sync::Mutex::new(BTreeMap::new()),
        }))
    }

    /// 把 `device` 编目折成 `"provider/modelId" → thinkingLevels`。
    /// 只在装配期算一次；端点层不理解模型语义。
    fn fold_levels(models: &nous_device::ModelsResponse) -> BTreeMap<String, Vec<String>> {
        models
            .items
            .iter()
            .map(|item| {
                (
                    format!("{}/{}", item.provider, item.model_id),
                    item.thinking_levels.clone(),
                )
            })
            .collect()
    }

    fn store(&self) -> Result<ImportedStore> {
        ImportedStore::read(&self.path).map_err(|error| {
            ServerError::Internal(format!("read {}: {error}", self.path.display()))
        })
    }
}

impl SessionSource for ImportedSource {
    fn set_model_levels(&self, models: &nous_device::ModelsResponse) {
        *self.model_levels.lock().unwrap_or_else(|e| e.into_inner()) =
            Self::fold_levels(models);
    }

    fn sessions(&self) -> Result<Vec<SessionItem>> {
        let levels = self.model_levels.lock().unwrap_or_else(|e| e.into_inner());
        nous_durable::storage::sessions::render_sessions_with_levels(&self.store()?, &levels)
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

    fn search(&self, query: &str, kind: &str) -> Result<Vec<SearchItem>> {
        let store = self.store()?;
        let map_err = |error| ServerError::Internal(format!("search {kind}: {error}"));
        let mut items = match kind {
            "sessions" => nous_durable::search_sessions(&store, query).map_err(map_err)?,
            "documents" => nous_durable::search_documents(&store, query).map_err(map_err)?,
            _ => {
                let mut items = nous_durable::search_sessions(&store, query).map_err(map_err)?;
                items.extend(nous_durable::search_documents(&store, query).map_err(map_err)?);
                items
            }
        };
        // `all` 是两类命中的合并：会话 id 是数字（新的在前），文档地址是字符串，
        // 两者没有同一条时间轴。按 kind 分组 + 组内降序，顺序稳定可断言。
        items.sort_by(|a, b| {
            b.kind
                .cmp(&a.kind)
                .then_with(|| match (a.kind, b.kind) {
                    ("sessions", "sessions") => {
                        let left = a.id.parse::<i64>().unwrap_or(0);
                        let right = b.id.parse::<i64>().unwrap_or(0);
                        right.cmp(&left)
                    }
                    _ => b.id.cmp(&a.id),
                })
        });
        Ok(items)
    }

    fn usage(&self, field: &str) -> Result<Value> {
        nous_durable::usage(&self.store()?, field)
            .map_err(|error| ServerError::Internal(format!("usage {field}: {error}")))
    }
}
