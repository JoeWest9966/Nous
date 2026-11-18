//! session 作用域单例文档的共用读写助手。
//!
//! `qrd3wh.sessions` 这个地址（`{kind, scope:{kind:"session"}}`）在库里只允许一个在世化身。
//! 所以不管是建会话、改名还是置顶，都先 `find_document` 找它、再 `materialize` 出当前值，
//! 写回时**只补自己管的那个键**：一次 `DocumentChange` 更新同一份文档，只有一份都没有才
//! `DocumentCreate`。谁在这里私开一份新文档，第二次就会被 storage 的地址唯一性拒绝。
//!
//! 历史形状保持兼容：旧引擎把这个地址上的整份 JSON 当 names 表用，本库的 `render_sessions`
//! 只读 `names` / `pins`，不认识 `config`，所以这里也绝不替别人删键。

use serde_json::{json, Value};

use crate::types::DocumentScope;
use crate::{DocumentContent, DocumentPoint, Id, Storage, StorageWrite};

use super::SessionError;

/// session 作用域的单例文档：`(化身 id, 当前值)`；没建过给 `(None, None)`。
pub(super) fn session_scope_document(
    storage: &Storage,
    kind: &str,
) -> Result<(Option<Id>, Option<Value>), SessionError> {
    let record = storage.find_document(
        &json!({ "kind": kind, "scope": { "kind": "session" } }),
        DocumentPoint::Current,
    )?;
    let Some(record) = record else {
        return Ok((None, None));
    };
    let id = record
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| SessionError::Shape("session scope document has no id".to_string()))?;
    Ok((
        Some(id),
        storage.materialize(id, DocumentPoint::Current)?.map(|doc| doc.value),
    ))
}

/// 同地址已有一份在世文档就整份 base 覆盖它；否则新建一份。
pub(super) fn upsert_session_document(
    storage: &mut Storage,
    kind: &str,
    existing: Option<Id>,
    value: Value,
) -> Result<Vec<StorageWrite>, SessionError> {
    let content = DocumentContent::Base { version: 1, value };
    Ok(match existing {
        Some(id) => vec![StorageWrite::DocumentChange { id, content }],
        None => {
            let id = storage.mint_id()?;
            vec![StorageWrite::DocumentCreate {
                id,
                record: json!({
                    "id": id,
                    "kind": kind,
                    "scope": DocumentScope::Session,
                }),
                content,
            }]
        }
    })
}

/// 读单例文档的当前值，调用方只替换自己的那个键，再把**整份值**写回去。
///
/// 重复建会话与 rename 都走这里：`names` 合并不丢别的会话，`pins` / `archived` 也不被
/// `{"names": ...}` 覆盖掉。
pub(super) fn merge_session_document(
    storage: &mut Storage,
    kind: &str,
    field: &str,
    change: impl FnOnce(&mut Value),
) -> Result<Vec<StorageWrite>, SessionError> {
    let (existing_id, existing) = session_scope_document(storage, kind)?;
    let mut value = existing.unwrap_or_else(|| json!({}));
    if !value.is_object() {
        value = json!({});
    }
    let mut field_value = value.get(field).cloned().unwrap_or_else(|| json!({}));
    if !field_value.is_object() {
        field_value = json!({});
    }
    change(&mut field_value);
    value[field] = field_value;
    upsert_session_document(storage, kind, existing_id, value)
}
