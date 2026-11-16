//! 本地 SQLite 适配器：上游 `sqlite/node.ts`（232 行，`node:sqlite`）→ `rusqlite`。
//!
//! ## 换成 `rusqlite` 之后必须自己盯的两件事
//!
//! 1. **`changes()` 的语义没变**：`run_counted` 用 `Connection::execute` 的返回值，
//!    和上游 `runCounted` 同一个数（索引写、触发器改的行不算）。
//! 2. **只读打开是硬约束**（`docs/storage.md` §8.3）：`open_read_only` 传
//!    `SQLITE_OPEN_READ_ONLY`，写操作会在 SQLite 层直接报错 —— 不靠调用方自律。
//!
//! 事务的「回调里不能用数据库本身」这条上游约束，在 Rust 里由 `&mut self` 表达：
//! 事务期间数据库被借走了，调不到。

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};

use super::database::{SqliteDatabase, SqliteExecutor, SqliteRow, SqliteValue};
use super::error::{SqliteError, SqliteErrorKind};

/// 本地文件 / 内存数据库。上游 `NodeSqliteDatabase`。
pub struct LocalDatabase {
    /// `None` = 已经 `close` 过。再多一次调用就报错，不 UB。
    connection: Option<Connection>,
}

impl LocalDatabase {
    /// 读写打开（不存在就建）。**不要用它开 fixture** —— 那是只读的。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SqliteError> {
        let connection = Connection::open(path)?;
        Ok(Self::wrap(connection))
    }

    /// 只读打开。`docs/storage.md` §8.3 的机器实现：写操作在 SQLite 层就失败。
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, SqliteError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self::wrap(connection))
    }

    pub fn open_in_memory() -> Result<Self, SqliteError> {
        Ok(Self::wrap(Connection::open_in_memory()?))
    }

    fn wrap(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
        }
    }

    fn connection(&self) -> Result<&Connection, SqliteError> {
        self.connection.as_ref().ok_or_else(|| {
            SqliteError::new(SqliteErrorKind::Backend, "database is already closed")
        })
    }

    /// 建表 / 迁移用：一次跑多条语句，不带绑定。上游 `exec`。
    pub fn execute_batch(&self, sql: &str) -> Result<(), SqliteError> {
        self.connection()?.execute_batch(sql)?;
        Ok(())
    }

    /// 打开的是什么模式（诊断 / 判据用）。
    pub fn is_read_only(&self) -> Result<bool, SqliteError> {
        let value: i64 = self
            .connection()?
            .query_row("PRAGMA query_only", [], |row| row.get(0))?;
        Ok(value != 0)
    }
}

fn to_backend(value: &SqliteValue) -> rusqlite::types::Value {
    match value {
        SqliteValue::Null => rusqlite::types::Value::Null,
        SqliteValue::Integer(value) => rusqlite::types::Value::Integer(*value),
        SqliteValue::Real(value) => rusqlite::types::Value::Real(*value),
        SqliteValue::Text(value) => rusqlite::types::Value::Text(value.clone()),
        SqliteValue::Blob(value) => rusqlite::types::Value::Blob(value.clone()),
    }
}

fn from_backend(value: ValueRef<'_>) -> SqliteValue {
    match value {
        ValueRef::Null => SqliteValue::Null,
        ValueRef::Integer(value) => SqliteValue::Integer(value),
        ValueRef::Real(value) => SqliteValue::Real(value),
        ValueRef::Text(bytes) => SqliteValue::Text(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => SqliteValue::Blob(bytes.to_vec()),
    }
}

fn bind(params: &[SqliteValue]) -> Vec<rusqlite::types::Value> {
    params.iter().map(to_backend).collect()
}

fn query_rows(
    connection: &Connection,
    sql: &str,
    params: &[SqliteValue],
    limit: Option<usize>,
) -> Result<Vec<SqliteRow>, SqliteError> {
    let bound = bind(params);
    let mut statement = connection.prepare(sql)?;
    let columns: Vec<String> = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut rows = statement.query(rusqlite::params_from_iter(bound.iter()))?;
    let mut collected = Vec::new();
    while let Some(row) = rows.next()? {
        let mut mapped = SqliteRow::new();
        for (index, column) in columns.iter().enumerate() {
            mapped.insert(column.clone(), from_backend(row.get_ref(index)?));
        }
        collected.push(mapped);
        if limit.is_some_and(|limit| collected.len() >= limit) {
            break;
        }
    }
    Ok(collected)
}

impl SqliteExecutor for LocalDatabase {
    fn exec(&self, sql: &str) -> Result<(), SqliteError> {
        self.execute_batch(sql)
    }

    fn run(&self, sql: &str, params: &[SqliteValue]) -> Result<(), SqliteError> {
        let bound = bind(params);
        self.connection()?
            .execute(sql, rusqlite::params_from_iter(bound.iter()))?;
        Ok(())
    }

    fn run_counted(&self, sql: &str, params: &[SqliteValue]) -> Result<u64, SqliteError> {
        let bound = bind(params);
        let changed = self
            .connection()?
            .execute(sql, rusqlite::params_from_iter(bound.iter()))?;
        Ok(changed as u64)
    }

    fn get(&self, sql: &str, params: &[SqliteValue]) -> Result<Option<SqliteRow>, SqliteError> {
        Ok(query_rows(self.connection()?, sql, params, Some(1))?.into_iter().next())
    }

    fn all(&self, sql: &str, params: &[SqliteValue]) -> Result<Vec<SqliteRow>, SqliteError> {
        query_rows(self.connection()?, sql, params, None)
    }
}

/// 事务句柄。上游把 `SqliteExecutor` 的同一个面递给回调；这里也一样，
/// 只是句柄的生命周期被借用检查器钉在事务内部。
struct TransactionHandle<'a> {
    transaction: &'a rusqlite::Transaction<'a>,
}

impl SqliteExecutor for TransactionHandle<'_> {
    fn exec(&self, sql: &str) -> Result<(), SqliteError> {
        self.transaction.execute_batch(sql)?;
        Ok(())
    }

    fn run(&self, sql: &str, params: &[SqliteValue]) -> Result<(), SqliteError> {
        let bound = bind(params);
        self.transaction
            .execute(sql, rusqlite::params_from_iter(bound.iter()))?;
        Ok(())
    }

    fn run_counted(&self, sql: &str, params: &[SqliteValue]) -> Result<u64, SqliteError> {
        let bound = bind(params);
        let changed = self
            .transaction
            .execute(sql, rusqlite::params_from_iter(bound.iter()))?;
        Ok(changed as u64)
    }

    fn get(&self, sql: &str, params: &[SqliteValue]) -> Result<Option<SqliteRow>, SqliteError> {
        Ok(
            query_rows(self.transaction, sql, params, Some(1))?
                .into_iter()
                .next(),
        )
    }

    fn all(&self, sql: &str, params: &[SqliteValue]) -> Result<Vec<SqliteRow>, SqliteError> {
        query_rows(self.transaction, sql, params, None)
    }
}

impl SqliteDatabase for LocalDatabase {
    fn transaction<T, E, F>(&mut self, work: F) -> std::result::Result<T, E>
    where
        F: FnOnce(&dyn SqliteExecutor) -> std::result::Result<T, E>,
        E: From<SqliteError>,
    {
        let connection = self.connection.as_mut().ok_or_else(|| {
            E::from(SqliteError::new(
                SqliteErrorKind::Backend,
                "database is already closed",
            ))
        })?;
        // `BEGIN IMMEDIATE`：立刻拿写锁，不和别的进程空转（docs/storage.md §7）。
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| E::from(SqliteError::from(error)))?;
        let handle = TransactionHandle {
            transaction: &transaction,
        };
        match work(&handle) {
            Ok(value) => {
                transaction.commit().map_err(|error| {
                    E::from(SqliteError::new(SqliteErrorKind::Transaction, error.to_string()))
                })?;
                Ok(value)
            }
            Err(error) => {
                // 显式回滚：错误原样返回（回滚失败也不掩盖它）。
                let _ = transaction.rollback();
                Err(error)
            }
        }
    }

    fn close(&mut self) -> Result<(), SqliteError> {
        match self.connection.take() {
            Some(connection) => {
                connection.close().map_err(|(_, error)| SqliteError::from(error))?;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path() -> Option<std::path::PathBuf> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/agent.sqlite");
        path.exists().then_some(path)
    }

    #[test]
    fn in_memory_round_trips_every_value_kind() {
        let db = LocalDatabase::open_in_memory().unwrap();
        db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, data BLOB, gone TEXT) STRICT")
            .unwrap();
        db.run(
            "INSERT INTO t (id, name, score, data, gone) VALUES (?, ?, ?, ?, ?)",
            &[
                SqliteValue::Integer(1),
                "one".into(),
                SqliteValue::Real(1.5),
                vec![0u8, 1, 2].into(),
                SqliteValue::Null,
            ],
        )
        .unwrap();

        let row = db
            .get("SELECT id, name, score, data, gone FROM t WHERE id = ?", &[1i64.into()])
            .unwrap()
            .unwrap();
        assert_eq!(row.get("id").unwrap().as_i64(), Some(1));
        assert_eq!(row.get("name").unwrap().as_str(), Some("one"));
        assert_eq!(row.get("score").unwrap().as_f64(), Some(1.5));
        assert_eq!(row.get("data").unwrap().as_blob(), Some(&[0u8, 1, 2][..]));
        assert!(row.get("gone").unwrap().is_null());
        // 主键冲突 → 后端错误，不 panic。
        assert_eq!(
            db.run("INSERT INTO t (id) VALUES (?)", &[1i64.into()])
                .unwrap_err()
                .kind,
            SqliteErrorKind::Backend
        );
    }

    #[test]
    fn run_counted_reports_the_rows_sqlite_changed() {
        let db = LocalDatabase::open_in_memory().unwrap();
        db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY) STRICT").unwrap();
        db.exec("INSERT INTO t (id) VALUES (1), (2), (3)").unwrap();
        assert_eq!(
            db.run_counted("UPDATE t SET id = id + 10 WHERE id >= ?", &[2i64.into()])
                .unwrap(),
            2
        );
        // 命中 0 行也算成功。
        assert_eq!(
            db.run_counted("DELETE FROM t WHERE id = ?", &[999i64.into()])
                .unwrap(),
            0
        );
    }

    #[test]
    fn a_failing_transaction_leaves_nothing_behind() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY) STRICT").unwrap();
        let error = db
            .transaction(|tx| {
                tx.run("INSERT INTO t (id) VALUES (1)", &[])?;
                Err::<(), _>(SqliteError::new(SqliteErrorKind::Backend, "boom"))
            })
            .unwrap_err();
        assert_eq!(error.message, "boom");
        let rows = db.all("SELECT id FROM t", &[]).unwrap();
        assert!(rows.is_empty(), "整批回滚");
    }

    #[test]
    fn a_committed_transaction_is_visible_afterwards() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY) STRICT").unwrap();
        db.transaction(|tx| -> std::result::Result<(), SqliteError> {
            tx.run("INSERT INTO t (id) VALUES (1)", &[])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(db.all("SELECT id FROM t", &[]).unwrap().len(), 1);
    }

    #[test]
    fn a_closed_database_reports_instead_of_panicking() {
        let mut db = LocalDatabase::open_in_memory().unwrap();
        db.close().unwrap();
        assert_eq!(
            db.run("SELECT 1", &[]).unwrap_err().kind,
            SqliteErrorKind::Backend
        );
        // 再关一次是幂等的。
        db.close().unwrap();
    }

    /// 判据：**能只读打开真实 fixture 并列出 9 张表**（证明这不是只有类型定义）。
    #[test]
    fn the_fixture_opens_read_only_and_lists_nine_tables() {
        let Some(path) = fixture_path() else {
            eprintln!("skipping: fixtures/agent.sqlite 不在（它永不入库）");
            return;
        };
        let db = LocalDatabase::open_read_only(&path).unwrap();
        let tables: Vec<String> = db
            .all(
                "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
                &[],
            )
            .unwrap()
            .into_iter()
            .filter_map(|row| row.get("name").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        assert_eq!(
            tables,
            vec![
                "conversations",
                "document_revisions",
                "documents",
                "durable_metadata",
                "durable_schema",
                "entries",
                "record_ids",
                "submissions",
                "tasks",
            ],
            "fixture 的 9 张表"
        );
        // 只读打开 = 真只读：写直接失败，而且**库没被改**。
        assert_eq!(
            db.run("CREATE TABLE nope (id INTEGER)", &[]).unwrap_err().kind,
            SqliteErrorKind::Backend
        );
        let conversations = db
            .get("SELECT COUNT(*) AS n FROM conversations", &[])
            .unwrap()
            .unwrap();
        assert_eq!(conversations.get("n").unwrap().as_i64(), Some(189));
        let entries = db
            .get("SELECT COUNT(*) AS n FROM entries", &[])
            .unwrap()
            .unwrap();
        assert_eq!(entries.get("n").unwrap().as_i64(), Some(209));
    }
}
