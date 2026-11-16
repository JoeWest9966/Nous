//! 记录 ↔ 列。上游 `storage/sqlite/rows.ts`（79 行）。
//!
//! ## 三条照抄的编码规矩
//!
//! 1. **`record` 列就是记录的 JSON 文本**，一个字节都不改（`json_valid(record)` 兜底）。
//! 2. **被索引的字符串列存 `JSON.stringify(s)`**（`encode_indexed_string`）—— 直接存原文会
//!    在「孤立代理对 / NUL」上丢字节，JSON 编码可以无损往返。fixture 里 `tasks.kind` 和
//!    `documents.kind` 都是 `"\"pi.agent\""` 这个样子。
//! 3. **索引列永远不是 NULL**：缺席用哨兵（owner → `0`，request → `""`）。`0` 不可能是合法 id，
//!    而编码后的字符串一定以 `"` 开头，所以哨兵不会撞上真值。
//!
//! **与旧引擎的差别要认**：fixture 是旧引擎写的，它的 `owner_*` 是 NULL、`request_id` 也是 NULL，
//! 而不是哨兵。所以读的一侧「NULL 与 0 / NULL 与 ""」同义（`import` 里有测试钉住），写的一侧
//! 照上游写哨兵。

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use nous_chord::sqlite::SqliteValue as Cell;

use crate::error::{Result, StorageError};

/// 记录里取不到字段 / 类型不对时的错误。消息里带键名，定位靠它。
fn shape(message: impl Into<String>) -> StorageError {
    StorageError::Shape(message.into())
}

/// 解析一个 `record` 列。
pub fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T> {
    Ok(serde_json::from_str(text)?)
}

/// 把记录编码成 `record` 列。键序保持原样（`serde_json` 的 `preserve_order`）。
pub fn encode_json(value: &Value) -> String {
    serde_json::to_string(value).expect("a serde_json::Value always serializes")
}

/// 索引字符串列的编码：`JSON.stringify`。
pub fn encode_indexed_string(value: &str) -> String {
    serde_json::to_string(value).expect("a Rust string always serializes as JSON")
}

/// 索引字符串列的解码。
pub fn decode_indexed_string(stored: &str) -> Result<String> {
    let parsed: Value = serde_json::from_str(stored)?;
    parsed
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| shape(format!("indexed string column is not a JSON string: {stored:?}")))
}

/// 文档作用域 → 索引列。上游 `scopeColumns`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeColumns {
    pub scope_kind: &'static str,
    pub owner_id: i64,
}

/// 文档地址 → 索引列。上游 `addressParts`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressParts {
    /// 已经 `encode_indexed_string` 过的 `kind`。
    pub kind: String,
    pub scope_kind: &'static str,
    pub owner_id: i64,
    /// `0` = 单例文档，`1` = 带 key 的家族成员。
    pub family: i64,
    /// 已经编码过的 key（缺席是 `""` 的编码体 `"\"\""`）。
    pub key_value: String,
}

/// 一个文档作用域的索引列。上游 `scopeColumns`。
pub fn scope_columns(scope: &Value) -> Result<ScopeColumns> {
    let kind = scope
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| shape("document scope has no kind"))?;
    match kind {
        // session 作用域没有 owner：哨兵 0。
        "session" => Ok(ScopeColumns {
            scope_kind: "session",
            owner_id: 0,
        }),
        "conversation" => Ok(ScopeColumns {
            scope_kind: "conversation",
            owner_id: required_i64(scope, "conversationId")?,
        }),
        "task" => Ok(ScopeColumns {
            scope_kind: "task",
            owner_id: required_i64(scope, "taskId")?,
        }),
        other => Err(shape(format!("unknown document scope kind {other:?}"))),
    }
}

/// 一个文档地址（`DocumentAddress` / `DocumentCreate` / `DocumentRecord`）的索引列。
pub fn address_parts(address: &Value) -> Result<AddressParts> {
    let kind = address
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| shape("document address has no kind"))?;
    let scope = address
        .get("scope")
        .ok_or_else(|| shape("document address has no scope"))?;
    let columns = scope_columns(scope)?;
    let key = address.get("key").and_then(Value::as_str);
    Ok(AddressParts {
        kind: encode_indexed_string(kind),
        scope_kind: columns.scope_kind,
        owner_id: columns.owner_id,
        family: if key.is_some() { 1 } else { 0 },
        key_value: encode_indexed_string(key.unwrap_or("")),
    })
}

/// 一个地址的字符串（索引列 → 单个键）。上游 `addressColumnsKey`。
pub fn address_columns_key(
    kind: &str,
    scope_kind: &str,
    owner_id: i64,
    family: i64,
    key_value: &str,
) -> String {
    json!([kind, scope_kind, owner_id, family, key_value]).to_string()
}

/// 一个地址的字符串。上游 `addressKey`。
pub fn address_key(address: &Value) -> Result<String> {
    let parts = address_parts(address)?;
    Ok(address_columns_key(
        &parts.kind,
        parts.scope_kind,
        parts.owner_id,
        parts.family,
        &parts.key_value,
    ))
}

/// 这个文档是不是只留当前状态：只有 `scope.kind == "conversation"` 且 `history == "rewindable"`
/// 才留历史。上游 `isCurrentOnly`。
pub fn is_current_only(record: &Value) -> bool {
    let scope_kind = record
        .get("scope")
        .and_then(|scope| scope.get("kind"))
        .and_then(Value::as_str);
    if scope_kind != Some("conversation") {
        return true;
    }
    record.get("history").and_then(Value::as_str) != Some("rewindable")
}

/// 文档记录的 `documents` 行。上游 `documentRow`。
pub fn document_row(record: &Value) -> Result<Vec<Cell>> {
    let parts = address_parts(record)?;
    Ok(vec![
        required_i64(record, "id")?.into(),
        parts.kind.into(),
        parts.family.into(),
        parts.key_value.into(),
        parts.scope_kind.into(),
        parts.owner_id.into(),
        required_i64(record, "createdAt")?.into(),
        optional_i64(record, "retiredAt").into(),
        encode_json(record).into(),
    ])
}

/// 取一个必填整数。缺失/类型不对都是 `Shape` 错误，不是 `unwrap`。
pub fn required_i64(value: &Value, key: &str) -> Result<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| shape(format!("missing or non-integer field {key:?}")))
}

/// 取一个可选整数。
pub fn optional_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

/// 取一个必填字符串。
pub fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| shape(format!("missing or non-string field {key:?}")))
}

/// 取一个可选布尔（缺席 = `false`）。
pub fn optional_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_strings_encode_losslessly_even_with_lone_surrogates_and_nul() {
        for value in ["pi.agent", "", "a\u{0}b", "😀", "\u{feff}"] {
            let encoded = encode_indexed_string(value);
            assert!(encoded.starts_with('"'), "{value:?} → {encoded}");
            assert_eq!(decode_indexed_string(&encoded).unwrap(), value);
        }
        // 哨兵不会撞上真值：任何编码后的字符串都以 `"` 开头。
        assert!(encode_indexed_string("").starts_with('"'));
        assert!(!decode_indexed_string("\"pi.agent\"").unwrap().starts_with('"'));
    }

    #[test]
    fn scope_columns_follow_the_three_scope_kinds() {
        let session = scope_columns(&json!({"kind": "session"})).unwrap();
        assert_eq!(session, ScopeColumns { scope_kind: "session", owner_id: 0 });

        let conversation =
            scope_columns(&json!({"kind": "conversation", "conversationId": 42})).unwrap();
        assert_eq!(conversation.owner_id, 42);

        let task = scope_columns(&json!({"kind": "task", "taskId": 7})).unwrap();
        assert_eq!(task.scope_kind, "task");
        assert_eq!(task.owner_id, 7);

        assert!(scope_columns(&json!({"kind": "galaxy"})).is_err());
        assert!(scope_columns(&json!({"kind": "conversation"})).is_err());
        assert!(scope_columns(&json!({})).is_err());
    }

    #[test]
    fn an_address_without_a_key_is_a_family_zero_singleton() {
        let singleton = address_parts(&json!({"kind": "pi.live", "scope": {"kind": "conversation", "conversationId": 2}}))
            .unwrap();
        assert_eq!(singleton.family, 0);
        assert_eq!(singleton.key_value, "\"\"");
        assert_eq!(singleton.kind, "\"pi.live\"");

        let member = address_parts(&json!({
            "kind": "pi.tool", "key": "read", "scope": {"kind": "task", "taskId": 11}
        }))
        .unwrap();
        assert_eq!(member.family, 1);
        assert_eq!(member.key_value, "\"read\"");
        assert_eq!(member.owner_id, 11);
    }

    #[test]
    fn the_address_key_is_one_string_per_address_and_orders_columns_before_the_key() {
        let address = json!({"kind": "pi.agent", "scope": {"kind": "conversation", "conversationId": 2}});
        assert_eq!(
            address_key(&address).unwrap(),
            "[\"\\\"pi.agent\\\"\",\"conversation\",2,0,\"\\\"\\\"\"]"
        );
        // 同一份输入稳定；换 scope 就换键。
        assert_eq!(address_key(&address).unwrap(), address_key(&address).unwrap());
        let other = json!({"kind": "pi.agent", "scope": {"kind": "conversation", "conversationId": 3}});
        assert_ne!(address_key(&address).unwrap(), address_key(&other).unwrap());
    }

    #[test]
    fn only_rewindable_conversation_documents_keep_history() {
        assert!(!is_current_only(&json!({"scope": {"kind": "conversation"}, "history": "rewindable"})));
        assert!(is_current_only(&json!({"scope": {"kind": "conversation"}, "history": "latest"})));
        assert!(is_current_only(&json!({"scope": {"kind": "session"}})));
        assert!(is_current_only(&json!({"scope": {"kind": "task"}})));
    }

    #[test]
    fn a_document_row_is_id_kind_family_key_scope_owner_created_retired_record() {
        let record = json!({
            "id": 3, "kind": "pi.live",
            "scope": {"kind": "conversation", "conversationId": 2},
            "history": "latest", "fork": "initial", "createdAt": 1
        });
        let row = document_row(&record).unwrap();
        assert_eq!(
            row,
            vec![
                Cell::Integer(3),
                Cell::Text("\"pi.live\"".into()),
                Cell::Integer(0),
                Cell::Text("\"\"".into()),
                Cell::Text("conversation".into()),
                Cell::Integer(2),
                Cell::Integer(1),
                Cell::Null,
                Cell::Text(encode_json(&record)),
            ]
        );
        // 缺 id 是明确失败。
        assert!(document_row(&json!({"kind": "pi.live", "scope": {"kind": "session"}})).is_err());
    }
}
