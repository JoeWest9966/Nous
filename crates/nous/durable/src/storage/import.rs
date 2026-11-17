//! 只读导入：上游（或旧引擎）写的库 → 本库的九张表。**永不写回源库。**
//!
//! ## 它存在的理由
//!
//! `docs/storage.md` §4 把两条路分开：导入器读**任意**上游库，迁移只推**我们自己**的库。
//! 这个模块就是导入器：`mode=ro` 打开、逐表读出、按 [`super::sqlite::tables`] 的规矩重算索引列、
//! **逐列与源库对齐**（对不上就报错，不猜），再整批写进一份新库。
//!
//! 「重算再对齐」这一步是**判据**，不只是搬运：如果 `tables.rs` 里哪条拆列规则写错了，
//! 重算出来的列就和 fixture 里那一列不等，导入直接失败 —— 而不是安静地导进去一份错的。
//!
//! ## 两处「旧引擎写的和上游不一样」，读的一侧要认
//!
//! | 列 | 上游 store 层写 | fixture（旧引擎）写 | 读的一侧 |
//! |---|---|---|---|
//! | `conversations.owner_conversation_id` / `owner_task_id` | 哨兵 `0` | `NULL` | 同义 |
//! | `submissions.request_id` | `""` 的编码体 `"\"\""` | `NULL` | 同义 |
//!
//! 见 [`SENTINEL_EQUIVALENTS`]。

use std::path::Path;

use nous_chord::sqlite::{SqliteDatabase, SqliteExecutor, SqliteRow, SqliteValue as Cell};

use super::sqlite::rows::document_row;
use super::sqlite::schema::SqliteSchema;
use super::sqlite::tables::{
    conversation_row, entry_row, record_id_row, revision_row, submission_row,
    task_row, DOCUMENT_COLUMNS, ENTRY_COLUMNS, SUBMISSION_COLUMNS, TASK_COLUMNS,
};
use super::SqliteStore;
use crate::error::{Result, StorageError};

/// 「哨兵值」等价规则：写的一侧用哨兵，旧库用 `NULL`，读的一侧两条都认。
///
/// 键是 `(表, 列)`，值是「写成哨兵的那个单元值」。用函数而不是 `const` 表，是因为
/// `Cell::Text` 装的是 `String`，`to_string()` 在常量里调不了。
pub fn sentinel_equivalent(table: &str, column: &str, stored: &Cell, computed: &Cell) -> bool {
    let sentinel = match (table, column) {
        ("conversations", "owner_conversation_id") | ("conversations", "owner_task_id") => {
            Cell::Integer(0)
        }
        // `""` 的索引编码体。
        ("submissions", "request_id") => Cell::Text("\"\"".to_string()),
        _ => return false,
    };
    (stored.is_null() && *computed == sentinel) || (computed.is_null() && *stored == sentinel)
}

/// 一次导出：九张表的原始行 + 两个计数器。行是**列名 → 单元值**，不解码记录体（那是调用方的事）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportedStore {
    pub schema_version: i64,
    pub next_id: String,
    pub next_seq: i64,
    pub record_ids: Vec<SqliteRow>,
    pub conversations: Vec<SqliteRow>,
    pub entries: Vec<SqliteRow>,
    pub tasks: Vec<SqliteRow>,
    pub submissions: Vec<SqliteRow>,
    pub documents: Vec<SqliteRow>,
    pub document_revisions: Vec<SqliteRow>,
}

impl ImportedStore {
    /// 只读打开 `path` 并全部读出来。源库不会被写。
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let schema = SqliteSchema::unprefixed();
        let db = nous_chord::sqlite::LocalDatabase::open_read_only(path)?;
        Self::read_from(&db, &schema)
    }

    /// 从一个执行器读全部。本库自己的库也走这条（读回来才能比对）。
    pub fn read_from(db: &dyn SqliteExecutor, schema: &SqliteSchema) -> Result<Self> {
        let metadata = db
            .get(
                &format!(
                    "SELECT next_id, next_seq FROM {} WHERE singleton = 1",
                    schema.table("durable_metadata")
                ),
                &[],
            )?
            .ok_or_else(|| StorageError::Shape("durable_metadata has no singleton row".into()))?;
        let schema_version = db
            .get(
                &format!("SELECT version FROM {} WHERE singleton = 1", schema.table("durable_schema")),
                &[],
            )?
            .and_then(|row| row.get("version").and_then(|value| value.as_i64()))
            .ok_or_else(|| StorageError::Shape("durable_schema has no version".into()))?;

        Ok(Self {
            schema_version,
            next_id: text(&metadata, "next_id")?.to_string(),
            next_seq: integer(&metadata, "next_seq")?,
            record_ids: read_table(db, schema.table("record_ids"), "id")?,
            conversations: read_table(db, schema.table("conversations"), "id")?,
            entries: read_table(db, schema.table("entries"), "id")?,
            tasks: read_table(db, schema.table("tasks"), "id")?,
            submissions: read_table(db, schema.table("submissions"), "id")?,
            documents: read_table(db, schema.table("documents"), "id")?,
            document_revisions: read_table(db, schema.table("document_revisions"), "document_id, seq")?,
        })
    }

    /// 九张表的行数（判据里对得上 fixture 的那五个数）。
    pub fn counts(&self) -> BTreeCounts {
        BTreeCounts {
            record_ids: self.record_ids.len(),
            conversations: self.conversations.len(),
            entries: self.entries.len(),
            tasks: self.tasks.len(),
            submissions: self.submissions.len(),
            documents: self.documents.len(),
            document_revisions: self.document_revisions.len(),
        }
    }

    /// 逐表把行写进 `store`。**一个事务**：要么全进，要么一条都没有。
    ///
    /// 写之前先按 [`super::sqlite::tables`] 重算索引列并与源行逐列对齐 —— 对不上就
    /// `StorageError::Shape`，不写。
    pub fn write_into(&self, store: &mut SqliteStore) -> Result<()> {
        let schema = store.schema().clone();
        let this = self.clone();
        store.db_mut().transaction(|tx| {
            // 2. 分配器台账：id → 类型，只增不复用。
            for row in &this.record_ids {
                let id = integer(row, "id")?;
                let record_type = text(row, "record_type")?;
                if !matches!(record_type, "conversation" | "entry" | "task" | "submission" | "document") {
                    return Err(StorageError::Shape(format!(
                        "unknown record_type {record_type:?} for id {id}"
                    )));
                }
                super::sqlite::statements::insert_rows(
                    tx,
                    schema.table("record_ids"),
                    &["id", "record_type"],
                    &[record_id_row(id, record_type)],
                    "",
                )?;
            }

            // 3. 四张记录表：记录体原样，索引列重算。
            write_mapped(
                tx,
                schema.table("conversations"),
                "conversations",
                &this.conversations,
                super::sqlite::tables::CONVERSATION_COLUMNS,
                |record, _row| conversation_row(record),
            )?;
            write_mapped(
                tx,
                schema.table("entries"),
                "entries",
                &this.entries,
                ENTRY_COLUMNS,
                |record, row| entry_row(record, integer(row, "commit_seq")?),
            )?;
            write_mapped(
                tx,
                schema.table("tasks"),
                "tasks",
                &this.tasks,
                TASK_COLUMNS,
                |record, _row| task_row(record),
            )?;
            write_mapped(
                tx,
                schema.table("submissions"),
                "submissions",
                &this.submissions,
                SUBMISSION_COLUMNS,
                |record, _row| submission_row(record),
            )?;
            // 4. 文档与修订。
            write_mapped(
                tx,
                schema.table("documents"),
                "documents",
                &this.documents,
                DOCUMENT_COLUMNS,
                |record, _row| document_row(record),
            )?;
            let revisions: Vec<Vec<Cell>> = this
                .document_revisions
                .iter()
                .map(|row| -> Result<Vec<Cell>> {
                    let content = text(row, "content")?;
                    let parsed: serde_json::Value = serde_json::from_str(content)?;
                    Ok(revision_row(
                        integer(row, "document_id")?,
                        integer(row, "seq")?,
                        text(row, "kind")?,
                        integer(row, "version")?,
                        &parsed,
                    ))
                })
                .collect::<Result<_>>()?;
            super::sqlite::statements::insert_rows(
                tx,
                schema.table("document_revisions"),
                &["document_id", "seq", "kind", "version", "content"],
                &revisions,
                "",
            )?;

            // 5. 计数器照搬源库：id 空间与 seq 空间都不许倒退。
            tx.run(
                &format!(
                    "UPDATE {} SET next_id = ?, next_seq = ? WHERE singleton = 1",
                    schema.table("durable_metadata")
                ),
                &[this.next_id.clone().into(), this.next_seq.into()],
            )?;
            Ok(())
        })
    }
}

/// [`ImportedStore::counts`] 的返回：七张有数据的表的行数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BTreeCounts {
    pub record_ids: usize,
    pub conversations: usize,
    pub entries: usize,
    pub tasks: usize,
    pub submissions: usize,
    pub documents: usize,
    pub document_revisions: usize,
}

fn read_table(db: &dyn SqliteExecutor, table: &str, order_by: &str) -> Result<Vec<SqliteRow>> {
    Ok(db.all(&format!("SELECT * FROM {table} ORDER BY {order_by}"), &[])?)
}

/// 一段记录表：逐行读 `record`、重算索引列、与源行对齐，然后整批插入。
fn write_mapped<F>(
    tx: &dyn SqliteExecutor,
    table: &str,
    table_key: &'static str,
    rows: &[SqliteRow],
    columns: &[&str],
    derive: F,
) -> Result<()>
where
    F: Fn(&serde_json::Value, &SqliteRow) -> Result<Vec<Cell>>,
{
    let mut derived_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let record: serde_json::Value = serde_json::from_str(text(row, "record")?)?;
        let derived = derive(&record, row)?;
        check_columns(table_key, row, columns, &derived)?;
        derived_rows.push(derived);
    }
    super::sqlite::statements::insert_rows(tx, table, columns, &derived_rows, "")?;
    Ok(())
}

/// 逐列对齐：源行的值与重算出来的值必须相等（哨兵见 [`sentinel_equivalent`]）。
fn check_columns(table: &str, row: &SqliteRow, columns: &[&str], derived: &[Cell]) -> Result<()> {
    for (index, column) in columns.iter().enumerate() {
        let Some(stored) = row.get(*column) else {
            return Err(StorageError::Shape(format!("{table} row has no column {column}")));
        };
        let computed = &derived[index];
        if equivalent(table, column, stored, computed) {
            continue;
        }
        // `record` 列对的是**值的相等**，不是文本：键序与转义都可能不同（我们重新编码过一次）。
        if *column == "record" {
            if let (Some(stored_text), Some(computed_text)) = (stored.as_str(), computed.as_str()) {
                let stored_value: serde_json::Value = serde_json::from_str(stored_text)?;
                let computed_value: serde_json::Value = serde_json::from_str(computed_text)?;
                if stored_value == computed_value {
                    continue;
                }
            }
        }
        return Err(StorageError::Shape(format!(
            "{table}.{column}: stored {stored:?} but the record says {computed:?}"
        )));
    }
    Ok(())
}

/// 两个单元值是否同义（含哨兵/`NULL` 的等价）。
fn equivalent(table: &str, column: &str, stored: &Cell, computed: &Cell) -> bool {
    if stored == computed {
        return true;
    }
    sentinel_equivalent(table, column, stored, computed)
}

fn integer(row: &SqliteRow, column: &str) -> Result<i64> {
    row.get(column)
        .and_then(|value| value.as_i64())
        .ok_or_else(|| StorageError::Shape(format!("column {column} is missing or not an integer")))
}

fn text<'a>(row: &'a SqliteRow, column: &str) -> Result<&'a str> {
    row.get(column)
        .and_then(|value| value.as_str())
        .ok_or_else(|| StorageError::Shape(format!("column {column} is missing or not text")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(pairs: Vec<(&str, Cell)>) -> SqliteRow {
        pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
    }

    #[test]
    fn an_absent_owner_in_the_old_engine_is_the_same_as_the_sentinel_zero() {
        let stored = row(vec![
            ("id", Cell::Integer(2)),
            ("owner_conversation_id", Cell::Null),
            ("owner_task_id", Cell::Null),
            ("record", Cell::Text("{\"id\":2}".into())),
        ]);
        let derived = conversation_row(&json!({"id": 2})).unwrap();
        check_columns("conversations", &stored, super::super::sqlite::tables::CONVERSATION_COLUMNS, &derived)
            .unwrap();
    }

    #[test]
    fn an_absent_request_id_in_the_old_engine_is_the_same_as_the_encoded_empty_string() {
        let record = "{\"id\":10,\"conversationId\":2,\"type\":\"input\",\"status\":\"done\"}";
        let stored = row(vec![
            ("id", Cell::Integer(10)),
            ("conversation_id", Cell::Integer(2)),
            ("request_id", Cell::Null),
            ("status", Cell::Text("done".into())),
            ("record", Cell::Text(record.into())),
        ]);
        let derived =
            submission_row(&json!({"id": 10, "conversationId": 2, "type": "input", "status": "done"}))
                .unwrap();
        check_columns("submissions", &stored, SUBMISSION_COLUMNS, &derived).unwrap();
    }

    #[test]
    fn a_column_that_disagrees_with_the_record_is_a_loud_failure() {
        // 源库说 status=done，记录说 running —— 不许「以记录为准」悄悄改。
        let stored = row(vec![
            ("id", Cell::Integer(11)),
            ("conversation_id", Cell::Integer(2)),
            ("kind", Cell::Text("\"pi.generation\"".into())),
            ("status", Cell::Text("done".into())),
            ("abort_requested", Cell::Integer(0)),
            ("background", Cell::Integer(0)),
            ("record", Cell::Text("{}".into())),
        ]);
        let derived = task_row(&json!({
            "id": 11, "conversationId": 2, "kind": "pi.generation", "state": {"status": "running"}
        }))
        .unwrap();
        let error = check_columns("tasks", &stored, TASK_COLUMNS, &derived).unwrap_err();
        assert!(error.to_string().contains("tasks.status"), "{error}");
    }

    #[test]
    fn a_sentinel_we_do_not_know_about_is_not_equivalent_to_null() {
        let stored = row(vec![("owner_conversation_id", Cell::Null)]);
        // `conversations.owner_task_id` 的哨兵是 0，这里传 7 不该被当成等价。
        assert!(!equivalent(
            "conversations",
            "owner_task_id",
            &stored["owner_conversation_id"],
            &Cell::Integer(7)
        ));
    }
}
