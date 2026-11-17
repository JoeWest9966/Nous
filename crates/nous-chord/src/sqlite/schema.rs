//! schema 前缀校验。上游 `sqlite/schema.ts`（9 行）**原样**，包括正则的两段语义：
//! 只能是 `[A-Za-z_][A-Za-z0-9_]*`，且不许以 `sqlite_` 开头（那是 SQLite 自己保留的）。

use super::error::{SqliteError, SqliteErrorKind};

/// 校验一个前缀能不能直接拼进 SQL 标识符。上游 `validateSqliteSchemaPrefix`。
///
/// 上游抛异常；这里是 `Result` —— 这个前缀会进 SQL 文本，**拼错就是注入**，不能默默放过。
pub fn validate_schema_prefix(prefix: &str) -> Result<(), SqliteError> {
    let mut chars = prefix.chars();
    let valid_head = chars
        .next()
        .map(|first| first.is_ascii_alphabetic() || first == '_')
        .unwrap_or(false);
    let valid_tail = chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_');
    if !valid_head || !valid_tail {
        return Err(SqliteError::new(
            SqliteErrorKind::InvalidPrefix,
            format!("invalid SQLite schema prefix {prefix:?}"),
        ));
    }
    if prefix.to_ascii_lowercase().starts_with("sqlite_") {
        return Err(SqliteError::new(
            SqliteErrorKind::InvalidPrefix,
            format!("SQLite schema prefix {prefix:?} is reserved"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_identifiers_pass() {
        for prefix in ["agent_schema", "_meta", "A1", "durable"] {
            assert!(validate_schema_prefix(prefix).is_ok(), "{prefix}");
        }
    }

    #[test]
    fn non_identifiers_are_rejected() {
        for prefix in ["", "1abc", "has space", "a-b", "a.b", "a;drop table x", "ábc"] {
            let error = validate_schema_prefix(prefix).unwrap_err();
            assert_eq!(error.kind, SqliteErrorKind::InvalidPrefix, "{prefix}");
        }
    }

    #[test]
    fn the_sqlite_underscore_prefix_is_reserved_in_any_case() {
        for prefix in ["sqlite_x", "SQLITE_X", "Sqlite_"] {
            let error = validate_schema_prefix(prefix).unwrap_err();
            assert_eq!(error.kind, SqliteErrorKind::InvalidPrefix, "{prefix}");
        }
        // `sqlite` 本身不保留 —— 保留的是 `sqlite_` 前缀。
        assert!(validate_schema_prefix("sqlite").is_ok());
    }
}
