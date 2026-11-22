//! 判据共用的假数据源。
//!
//! 不读 `fixtures/agent.sqlite`：那是真实用户数据、不进 git，CI 上不存在。
//! 「HTTP 形状」用假源测；「fixture 读得出同形 JSON」由 `scripts/verify.sh` 第 2 段测。
//! 两面分开，谁红了就改谁。

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use nous_durable::storage::sessions::SessionItem;
use nous_server::{Result, ServerError, SessionSource};
use serde_json::{json, Value};

pub struct FakeSource {
    pub sessions: Vec<SessionItem>,
    pub entries: HashMap<i64, Vec<Value>>,
    pub live: HashMap<i64, Value>,
}

impl FakeSource {
    /// 两条会话：id 2（有名字、有 model）在前，id 1 在后。
    pub fn two_sessions() -> Arc<Self> {
        let mut live = HashMap::new();
        live.insert(
            1,
            json!({"role": "assistant", "content": [{"type": "text", "text": "streaming"}]}),
        );
        let mut entries = HashMap::new();
        entries.insert(
            1,
            vec![json!({
                "id": 11,
                "kind": "pi.assistant",
                "text": "hello",
                "parent": null,
                "model": [{"role": "assistant", "timestamp": 1_700_000_000_000_i64,
                           "content": [{"type": "text", "text": "hello"}]}]
            })],
        );
        Arc::new(Self {
            sessions: vec![item(2, "second"), item(1, "first")],
            entries,
            live,
        })
    }
}

impl SessionSource for FakeSource {
    fn sessions(&self) -> Result<Vec<SessionItem>> {
        Ok(self.sessions.clone())
    }

    fn entries(&self, session: i64) -> Result<Vec<Value>> {
        Ok(self.entries.get(&session).cloned().unwrap_or_default())
    }

    fn live_message(&self, session: i64) -> Result<Option<Value>> {
        Ok(self.live.get(&session).cloned())
    }
}

/// 一个允许出现的会话项：其余字段给 `wire.md` 允许的空值（`name` 空串是「没命名」）。
pub fn item(id: i64, name: &str) -> SessionItem {
    SessionItem {
        id,
        name: name.to_string(),
        parent: None,
        owner: None,
        model: Some("test/model-1".into()),
        thinking_level: None,
        thinking_levels: vec![],
        skills: vec![],
        template: None,
        read_only: false,
        busy: None,
        updated_at: None,
        last: None,
        archived: None,
        pinned: false,
    }
}

/// 一条 `pi.live` 文档记录（只用到 kind 与作用域；其余字段 `wire.md` 不依赖）。
pub fn live_record(id: i64, conversation: i64) -> nous_durable::types::DocumentRecord {
    use nous_durable::types::{DocumentHistory, DocumentRecord, DocumentScope};
    DocumentRecord {
        id,
        kind: "pi.live".to_string(),
        key: None,
        created_at: 1,
        retired_at: None,
        scope: DocumentScope::Conversation {
            conversation_id: conversation,
        },
        history: Some(DocumentHistory::Latest),
        fork: None,
    }
}

/// 读 SSE 的第一帧数据（`data: ...` 后面那一行）。超时给 2 秒，够用且不挂死。
pub async fn first_sse_data(body: axum::body::Body) -> String {
    use futures::StreamExt;
    let mut stream = body.into_data_stream();
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .expect("SSE 第一帧 2 秒内到")
        .expect("SSE 流不该先关")
        .expect("SSE 第一块读得出来");
    let text = String::from_utf8(chunk.to_vec()).expect("SSE 是 UTF-8");
    text.lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or_else(|| panic!("SSE 第一块里没有 data 行：{text:?}"))
        .to_string()
}

/// 不存在的会话用这个错误：`NotFound`。
pub fn not_found() -> ServerError {
    ServerError::NotFound("nope".into())
}
