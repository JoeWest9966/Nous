//! 五种记录 → 行，以及每张表的列序。上游 `store/tables.ts`（122 行）+ `sqlite/tables.ts` 的映射部分。
//!
//! ## 拆列规则（`docs/storage.md` §2）
//!
//! **被 `WHERE` / `ORDER BY` / 联结用到的字段才拆列，其余全进 `record`。** 所以这里的每个
//! `*_row` 都只做一件事：把记录里那几个**要被查的**字段抄成列，`record` 列原样放整条记录的 JSON。
//! 加字段默认进 `record`（不用迁移）；需要查了才升列（= 一次迁移）。
//!
//! 四张记录表的拆列清单：
//!
//! | 表 | 列 | 为什么是列 |
//! |---|---|---|
//! | `conversations` | `owner_conversation_id` / `owner_task_id` | 子树中止、子树 idle 要 `WHERE` 它们 |
//! | `entries` | `conversation_id` / `head` / `commit_seq` | 会话历史按 `(conversation_id, id DESC)` 取；`head` 有部分索引；`commit_seq` 是全局游标 |
//! | `tasks` | `kind` / `status` / `abort_requested` / `background` | 全是索引列，认门/调度/后台过滤都靠它们 |
//! | `submissions` | `request_id` / `status` | `request_id` 是幂等键（`wire.md` 的 `requestId`），`status` 有索引 |

use serde_json::Value;

use nous_chord::sqlite::SqliteValue as Cell;

use super::rows::{
    encode_indexed_string, encode_json, optional_bool, optional_i64, required_i64, required_str,
};
use crate::error::Result;

/// `conversations` 的列序（插入语句按它走）。
pub const CONVERSATION_COLUMNS: &[&str] = &["id", "owner_conversation_id", "owner_task_id", "record"];
/// `entries` 的列序。
pub const ENTRY_COLUMNS: &[&str] = &["id", "conversation_id", "head", "commit_seq", "record"];
/// `tasks` 的列序。
pub const TASK_COLUMNS: &[&str] =
    &["id", "conversation_id", "kind", "status", "abort_requested", "background", "record"];
/// `submissions` 的列序。
pub const SUBMISSION_COLUMNS: &[&str] = &["id", "conversation_id", "request_id", "status", "record"];
/// `record_ids` 的列序。
pub const RECORD_ID_COLUMNS: &[&str] = &["id", "record_type"];
/// `documents` 的列序。
pub const DOCUMENT_COLUMNS: &[&str] = &[
    "id",
    "kind",
    "family",
    "key_value",
    "scope_kind",
    "owner_id",
    "created_at",
    "retired_at",
    "record",
];
/// `document_revisions` 的列序。
pub const DOCUMENT_REVISION_COLUMNS: &[&str] =
    &["document_id", "seq", "kind", "version", "content"];

/// 一条会话记录的行。缺席的 owner 写哨兵 `0`（不是 NULL —— 见 `rows.rs` 的规矩 3）。
pub fn conversation_row(record: &Value) -> Result<Vec<Cell>> {
    let owner = record.get("owner");
    Ok(vec![
        required_i64(record, "id")?.into(),
        optional_i64(owner.unwrap_or(&Value::Null), "conversationId").unwrap_or(0).into(),
        optional_i64(owner.unwrap_or(&Value::Null), "taskId").unwrap_or(0).into(),
        encode_json(record).into(),
    ])
}

/// 一条 entry 记录的行。`commit_seq` 由提交步骤盖章，不从记录里读。
pub fn entry_row(record: &Value, commit_seq: i64) -> Result<Vec<Cell>> {
    Ok(vec![
        required_i64(record, "id")?.into(),
        required_i64(record, "conversationId")?.into(),
        optional_i64(record, "head").into(),
        Cell::Integer(commit_seq),
        encode_json(record).into(),
    ])
}

/// 一条 task 记录的行。`kind` 是索引字符串（编码过的），`status` 从 `state.status` 抄。
pub fn task_row(record: &Value) -> Result<Vec<Cell>> {
    let kind = required_str(record, "kind")?;
    let status = record
        .get("state")
        .and_then(|state| state.get("status"))
        .and_then(Value::as_str)
        .ok_or_else(|| crate::error::StorageError::Shape("task record has no state.status".into()))?;
    Ok(vec![
        required_i64(record, "id")?.into(),
        required_i64(record, "conversationId")?.into(),
        encode_indexed_string(kind).into(),
        status.into(),
        Cell::Integer(if optional_bool(record, "abortRequested") { 1 } else { 0 }),
        Cell::Integer(if optional_bool(record, "background") { 1 } else { 0 }),
        encode_json(record).into(),
    ])
}

/// 一条 submission 记录的行。缺席的 `requestId` 写哨兵 `""` 的编码体。
pub fn submission_row(record: &Value) -> Result<Vec<Cell>> {
    let request_id = record.get("requestId").and_then(Value::as_str);
    let status = required_str(record, "status")?;
    Ok(vec![
        required_i64(record, "id")?.into(),
        required_i64(record, "conversationId")?.into(),
        encode_indexed_string(request_id.unwrap_or("")).into(),
        status.into(),
        encode_json(record).into(),
    ])
}

/// 一条 `record_ids` 的行：id 与它属于哪种记录（五型一个命名空间）。
pub fn record_id_row(id: i64, record_type: &str) -> Vec<Cell> {
    vec![id.into(), record_type.into()]
}

/// 一条 revision 的行。`content` 是**内容**的 JSON 文本（`base` 是整份值，`delta` 是一批 op）。
pub fn revision_row(document_id: i64, seq: i64, kind: &str, version: i64, content: &Value) -> Vec<Cell> {
    vec![
        document_id.into(),
        seq.into(),
        kind.into(),
        version.into(),
        encode_json(content).into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_conversation_row_uses_zero_for_an_absent_owner() {
        assert_eq!(
            conversation_row(&json!({"id": 14})).unwrap(),
            vec![
                Cell::Integer(14),
                Cell::Integer(0),
                Cell::Integer(0),
                Cell::Text("{\"id\":14}".into()),
            ]
        );
        let owned = conversation_row(&json!({"id": 15, "owner": {"conversationId": 14, "taskId": 22}}))
            .unwrap();
        assert_eq!(owned[1], Cell::Integer(14));
        assert_eq!(owned[2], Cell::Integer(22));
    }

    #[test]
    fn an_entry_row_carries_the_commit_seq_stamped_by_the_step() {
        let record = json!({"id": 9, "conversationId": 2, "kind": "pi.user"});
        let row = entry_row(&record, 3).unwrap();
        assert_eq!(row[0], Cell::Integer(9));
        assert_eq!(row[1], Cell::Integer(2));
        assert_eq!(row[2], Cell::Null, "head 缺席就是 NULL");
        assert_eq!(row[3], Cell::Integer(3));
        assert_eq!(row[4], Cell::Text(encode_json(&record)));
        // 缺 conversationId 是明确失败。
        assert!(entry_row(&json!({"id": 9}), 1).is_err());
    }

    #[test]
    fn a_task_row_encodes_its_kind_and_copies_the_state_status() {
        let record = json!({
            "id": 11, "conversationId": 2, "kind": "pi.generation",
            "background": false, "abortRequested": true,
            "state": {"status": "running"}
        });
        let row = task_row(&record).unwrap();
        assert_eq!(row[2], Cell::Text("\"pi.generation\"".into()));
        assert_eq!(row[3], Cell::Text("running".into()));
        assert_eq!(row[4], Cell::Integer(1), "abortRequested 抄成 1");
        assert_eq!(row[5], Cell::Integer(0), "background 抄成 0");
        assert!(task_row(&json!({"id": 11, "conversationId": 2, "kind": "pi.generation"})).is_err());
    }

    #[test]
    fn a_submission_row_uses_the_encoded_empty_string_when_the_request_id_is_absent() {
        let record = json!({"id": 10, "conversationId": 2, "type": "input", "status": "done"});
        let row = submission_row(&record).unwrap();
        assert_eq!(row[2], Cell::Text("\"\"".into()));
        let with_request = submission_row(&json!({
            "id": 10, "conversationId": 2, "status": "queued", "requestId": "req-1"
        }))
        .unwrap();
        assert_eq!(with_request[2], Cell::Text("\"req-1\"".into()));
    }

    #[test]
    fn a_revision_row_keeps_the_content_as_json_text() {
        let row = revision_row(3, 3536, "base", 1, &json!({}));
        assert_eq!(row[0], Cell::Integer(3));
        assert_eq!(row[1], Cell::Integer(3536));
        assert_eq!(row[2], Cell::Text("base".into()));
        assert_eq!(row[3], Cell::Integer(1));
        assert_eq!(row[4], Cell::Text("{}".into()));
    }
}
