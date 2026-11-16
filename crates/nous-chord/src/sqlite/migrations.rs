//! 迁移。上游 `packages/chord/src/sqlite/migrations.ts`（43 行）。
//!
//! 三条语义一条不少：**版本从 1 连续**、**整批一个事务**、**库里的版本比代码新就拒绝运行**
//! （不降级、不猜）。版本元数据表是一行 singleton，不用 `PRAGMA user_version` ——
//! 上游这个选择有理由：同一份 SQLite 里可以放多套 schema，各自记版本。

use super::database::SqliteDatabase;
use super::error::{SqliteError, SqliteErrorKind};
use super::schema::validate_schema_prefix;

/// 一条迁移：版本号 + 语句。上游 `SqliteMigration`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteMigration {
    pub version: u32,
    pub statements: Vec<String>,
}

impl SqliteMigration {
    pub fn new(version: u32, statements: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            version,
            statements: statements.into_iter().map(Into::into).collect(),
        }
    }
}

/// 原子地跑完所有待执行的迁移，并把版本写进 `options.table`。上游 `applySqliteMigrations`。
///
/// 失败语义（与上游一致）：
///
/// - 版本不连续 / 前缀非法 → **在开事务之前**就拒绝；
/// - 某条语句失败 → 整个事务回滚，**版本不前进**（半套 schema 不会落地）；
/// - 库里的版本比 `migrations.last()` 新 → 拒绝运行，消息里带两侧版本号。
pub fn apply_sqlite_migrations<D: SqliteDatabase>(
    database: &mut D,
    migrations: &[SqliteMigration],
    table: &str,
) -> Result<(), SqliteError> {
    validate_schema_prefix(table)?;
    for (index, migration) in migrations.iter().enumerate() {
        if migration.version != index as u32 + 1 {
            return Err(SqliteError::new(
                SqliteErrorKind::NonContiguousVersions,
                format!(
                    "SQLite migrations must have contiguous versions starting at 1 (index {index} has version {})",
                    migration.version
                ),
            ));
        }
    }

    let supported = migrations.last().map(|m| m.version).unwrap_or(0);
    database.transaction(|tx| {
        tx.exec(&format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                version INTEGER NOT NULL CHECK (version >= 0)
            ) STRICT"
        ))?;
        tx.run(&format!("INSERT OR IGNORE INTO {table} (singleton, version) VALUES (1, 0)"), &[])?;
        let row = tx.get(&format!("SELECT version FROM {table} WHERE singleton = 1"), &[])?;
        let Some(row) = row else {
            return Err(SqliteError::new(
                SqliteErrorKind::MissingMetadata,
                "SQLite schema metadata is missing",
            ));
        };
        let current = row
            .get("version")
            .and_then(|value| value.as_i64())
            .ok_or_else(|| {
                SqliteError::new(SqliteErrorKind::MissingMetadata, "version column is not an integer")
            })?;
        if current > i64::from(supported) {
            return Err(SqliteError::new(
                SqliteErrorKind::SchemaTooNew,
                format!("SQLite schema version {current} is newer than supported version {supported}"),
            ));
        }
        for migration in migrations {
            if i64::from(migration.version) <= current {
                continue;
            }
            for statement in &migration.statements {
                tx.exec(statement)?;
            }
            tx.run(
                &format!("UPDATE {table} SET version = ? WHERE singleton = 1"),
                &[migration.version.into()],
            )?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::database::SqliteExecutor;
    use crate::sqlite::local::LocalDatabase;

    fn migrations() -> Vec<SqliteMigration> {
        vec![
            SqliteMigration::new(1, ["CREATE TABLE a (id INTEGER PRIMARY KEY) STRICT"]),
            SqliteMigration::new(
                2,
                [
                    "CREATE TABLE b (id INTEGER PRIMARY KEY, a_id INTEGER NOT NULL) STRICT",
                    "CREATE INDEX b_a ON b (a_id)",
                ],
            ),
        ]
    }

    #[test]
    fn migrations_apply_in_order_and_record_the_version() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        apply_sqlite_migrations(&mut db, &migrations(), "durable_schema").unwrap();
        let row = db
            .get("SELECT version FROM durable_schema WHERE singleton = 1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(row.get("version").unwrap().as_i64(), Some(2));
        // 表真的建出来了。
        let tables = db
            .all("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name", &[])
            .unwrap();
        let names: Vec<&str> = tables
            .iter()
            .map(|row| row.get("name").unwrap().as_str().unwrap())
            .collect();
        assert!(names.contains(&"a") && names.contains(&"b"), "{names:?}");
    }

    #[test]
    fn a_second_run_is_a_no_op() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        apply_sqlite_migrations(&mut db, &migrations(), "durable_schema").unwrap();
        apply_sqlite_migrations(&mut db, &migrations(), "durable_schema").unwrap();
        let row = db
            .get("SELECT version FROM durable_schema WHERE singleton = 1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(row.get("version").unwrap().as_i64(), Some(2));
    }

    #[test]
    fn a_newer_database_is_refused_not_downgraded() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        apply_sqlite_migrations(&mut db, &migrations(), "durable_schema").unwrap();
        let error =
            apply_sqlite_migrations(&mut db, &migrations()[..1], "durable_schema").unwrap_err();
        assert_eq!(error.kind, SqliteErrorKind::SchemaTooNew);
        assert!(error.message.contains('2') && error.message.contains('1'));
    }

    #[test]
    fn non_contiguous_versions_are_refused_before_any_sql_runs() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        let broken = vec![
            SqliteMigration::new(1, ["CREATE TABLE a (id INTEGER) STRICT"]),
            SqliteMigration::new(3, ["CREATE TABLE c (id INTEGER) STRICT"]),
        ];
        let error = apply_sqlite_migrations(&mut db, &broken, "durable_schema").unwrap_err();
        assert_eq!(error.kind, SqliteErrorKind::NonContiguousVersions);
        // 一条语句都没跑：连元数据表都不该存在。
        let tables = db
            .all("SELECT name FROM sqlite_master WHERE type = 'table'", &[])
            .unwrap();
        assert!(tables.is_empty(), "{tables:?}");
    }

    #[test]
    fn a_failing_statement_rolls_the_whole_batch_back() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        let broken = vec![
            SqliteMigration::new(1, ["CREATE TABLE a (id INTEGER PRIMARY KEY) STRICT"]),
            // 第二条语句是错的（表 b 不存在）→ 整批回滚，`a` 与版本都不留。
            SqliteMigration::new(2, ["INSERT INTO b (id) VALUES (1)"]),
        ];
        assert!(apply_sqlite_migrations(&mut db, &broken, "durable_schema").is_err());
        let tables = db
            .all(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                &[],
            )
            .unwrap();
        assert!(tables.is_empty(), "半写必须回滚干净：{tables:?}");
    }

    #[test]
    fn the_metadata_table_name_is_validated_as_a_prefix() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        let error = apply_sqlite_migrations(&mut db, &migrations(), "bad name").unwrap_err();
        assert_eq!(error.kind, SqliteErrorKind::InvalidPrefix);
    }
}
