//! 转录读面：按**会话**取 entry，给轮次装配上下文用。
//!
//! 为什么不放在 `Session` 的方法表上：那是 A3 那张签过字的公开面（六条），
//! 轮次装配要的是「读一段」，不是「再加一个会话入口」—— 所以做成本模块的自由函数。
//!
//! 为什么不按 id 扫：entry id 是**全局单调**的（一条序列被所有会话共用），
//! 「从 1 往上递增、遇到缺失就停」在真库上第一跳就断，请求体里 `messages` 会是空的。

use crate::storage::cursor::ScanOrder;
use crate::storage::storage::EntryQuery;
use crate::session::{Id, Session, SessionError};
use crate::types::records::EntryRecord;

/// 一个会话的转录，按 id **升序**，最多最新的 `limit` 条（截的是最老的）。
pub async fn entries_in(
    session: &Session,
    conversation: Id,
    limit: usize,
) -> Result<Vec<EntryRecord>, SessionError> {
    let guard = session.storage.lock().await;
    let storage = guard.as_ref().ok_or(SessionError::Closed)?;
    let query = EntryQuery {
        conversation_id: conversation,
        min_entry_id: None,
        max_entry_id: None,
        order: Some(ScanOrder::Descending),
    };
    let page = storage
        .scan_entries(&query, limit, None)
        .map_err(SessionError::Storage)?
        .map_err(|refusal| SessionError::Shape(format!("transcript refused: {refusal}")))?;
    let mut records: Vec<EntryRecord> = Vec::with_capacity(page.items.len());
    for value in page.items {
        records.push(
            serde_json::from_value(value)
                .map_err(|error| SessionError::Shape(format!("transcript entry: {error}")))?,
        );
    }
    records.reverse();
    Ok(records)
}
