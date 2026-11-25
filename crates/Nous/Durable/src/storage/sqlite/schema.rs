//! 九张表与十七个索引的**名字**（上游 `storage/sqlite/schema.ts`，63 行）。
//!
//! 名字是**不可变历史**：迁移与运行期查询都从这张表渲染。改名 = 加新条目，不能改旧值 ——
//! 否则历史迁移会去操作一个不存在的对象。
//!
//! 前缀可选（上游允许 `agent_` 之类给一份库里的多套 schema 分家）。本库默认无前缀，
//! 因为一个文件就是一份库；前缀能力留着，是因为 fixture 之外的库可能不是我们放的。

use nous_chord::sqlite::{validate_schema_prefix, SqliteError};

/// 表名。上游 `TABLE_NAMES`。顺序照抄（导出时便于对数）。
pub const TABLE_NAMES: &[(&str, &str)] = &[
    ("durable_schema", "durable_schema"),
    ("durable_metadata", "durable_metadata"),
    ("record_ids", "record_ids"),
    ("conversations", "conversations"),
    ("entries", "entries"),
    ("tasks", "tasks"),
    ("submissions", "submissions"),
    ("documents", "documents"),
    ("document_revisions", "document_revisions"),
];

/// 索引名。上游 `INDEX_NAMES`。
pub const INDEX_NAMES: &[(&str, &str)] = &[
    ("conversations_by_owner_conversation", "conversations_by_owner_conversation"),
    ("conversations_by_owner_task", "conversations_by_owner_task"),
    ("entries_by_conversation", "entries_by_conversation"),
    ("entry_heads_by_conversation", "entry_heads_by_conversation"),
    ("entry_terminals_by_conversation", "entry_terminals_by_conversation"),
    ("tasks_by_status", "tasks_by_status"),
    ("tasks_by_conversation", "tasks_by_conversation"),
    ("tasks_by_kind", "tasks_by_kind"),
    ("tasks_by_abort_requested", "tasks_by_abort_requested"),
    ("tasks_by_background", "tasks_by_background"),
    ("submissions_by_request", "submissions_by_request"),
    ("submissions_by_conversation", "submissions_by_conversation"),
    ("submissions_by_status", "submissions_by_status"),
    ("documents_by_address", "documents_by_address"),
    ("documents_by_scope", "documents_by_scope"),
    ("documents_by_scope_kind", "documents_by_scope_kind"),
    ("document_revisions_by_kind", "document_revisions_by_kind"),
];

/// 一套渲染好的标识符。上游 `SqliteSchema`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteSchema {
    pub prefix: String,
    tables: Vec<(String, String)>,
    indexes: Vec<(String, String)>,
}

impl SqliteSchema {
    /// 无前缀的一套（本库的默认）。
    pub fn unprefixed() -> Self {
        Self {
            prefix: String::new(),
            tables: TABLE_NAMES
                .iter()
                .map(|(key, name)| (key.to_string(), name.to_string()))
                .collect(),
            indexes: INDEX_NAMES
                .iter()
                .map(|(key, name)| (key.to_string(), name.to_string()))
                .collect(),
        }
    }

    /// 带前缀的一套。前缀先过 `validate_schema_prefix` —— 它会拼进 SQL 文本。
    pub fn with_prefix(prefix: &str) -> std::result::Result<Self, SqliteError> {
        validate_schema_prefix(prefix)?;
        Ok(Self {
            prefix: prefix.to_string(),
            tables: TABLE_NAMES
                .iter()
                .map(|(key, name)| (key.to_string(), format!("{prefix}{name}")))
                .collect(),
            indexes: INDEX_NAMES
                .iter()
                .map(|(key, name)| (key.to_string(), format!("{prefix}{name}")))
                .collect(),
        })
    }

    /// 渲染好的表名。`key` 是 [`TABLE_NAMES`] 的左列。
    ///
    /// 键写错是**开发期错误**（拼错的名字会变成一条读写不存在的表的 SQL），所以这里 panic
    /// 而不是返回 `Result`：调用点全是常量字面量，`#[test]` 里逐条覆盖。
    pub fn table(&self, key: &str) -> &str {
        lookup(&self.tables, key)
    }

    /// 渲染好的索引名。
    pub fn index(&self, key: &str) -> &str {
        lookup(&self.indexes, key)
    }
}

fn lookup<'a>(entries: &'a [(String, String)], key: &str) -> &'a str {
    entries
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, name)| name.as_str())
        .unwrap_or_else(|| panic!("unknown SQLite identifier {key:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_present_exactly_once() {
        let schema = SqliteSchema::unprefixed();
        for (key, name) in TABLE_NAMES {
            assert_eq!(schema.table(key), *name);
        }
        for (key, name) in INDEX_NAMES {
            assert_eq!(schema.index(key), *name);
        }
        // 九张表、十七个索引 —— 与 `docs/storage.md` §10.1 一致（`entry_heads` 与
        // `entry_terminals` 都是部分索引，不是表；后者是 #38 为终态热路径加的）。
        assert_eq!(TABLE_NAMES.len(), 9);
        assert_eq!(INDEX_NAMES.len(), 17);
    }

    #[test]
    fn a_prefix_applies_to_every_identifier_and_is_validated() {
        let schema = SqliteSchema::with_prefix("agent_").unwrap();
        assert_eq!(schema.table("entries"), "agent_entries");
        assert_eq!(schema.index("entries_by_conversation"), "agent_entries_by_conversation");
        assert_eq!(SqliteSchema::with_prefix("bad name").unwrap_err().kind,
            nous_chord::sqlite::SqliteErrorKind::InvalidPrefix);
    }

    #[test]
    #[should_panic(expected = "unknown SQLite identifier")]
    fn a_misspelled_key_is_a_development_error() {
        SqliteSchema::unprefixed().table("entrie");
    }
}
