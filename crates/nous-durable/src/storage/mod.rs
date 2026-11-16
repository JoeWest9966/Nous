//! 存储层：九张表的读写、提交语义、只读导入。
//!
//! ## 两条路，不许混（`docs/storage.md` §4）
//!
//! - **导入器**（[`import`]）：只读打开任意上游库 → 吐 `/v1` 形状。永不写回源库。
//! - **迁移**（[`SqliteStore::open_writer`]）：只推进**我们自己**写的库的版本，单向、无 down。
//!
//! ## 三件东西各自的活
//!
//! - [`sqlite`]：行与语句（照上游），一次提交几行几语句的问题在这里解决；
//! - [`writes`] + [`commit`]：一批写合不合法（拒绝）、怎么盖 seq、攒成什么净效果；
//! - [`storage`]：一个 `BEGIN IMMEDIATE` 事务包住上面两步，加上 id 分配器与扫描。
//!
//! 所以「打开一份上游 v2 库」在写模式下是**明确失败**（版本 2 > 我们支持的 1），这条由
//! `nous_chord::sqlite::apply_sqlite_migrations` 的 `SchemaTooNew` 保证，不是约定。

pub mod commit;
pub mod documents;
pub mod cursor;
pub mod import;
pub mod sessions;
pub mod storage;
pub mod writes;
pub mod sqlite;

use std::path::Path;

use nous_chord::sqlite::{
    apply_sqlite_migrations, LocalDatabase, SqliteError, SqliteErrorKind, SqliteExecutor,
};

use crate::error::{Result, StorageError};

pub use sqlite::migrations::{migrations, CURRENT_VERSION, WRITER};
pub use sqlite::schema::SqliteSchema;

/// 一份 nous 自己的库。写模式过了认门，只读模式没有。
pub struct SqliteStore {
    db: LocalDatabase,
    schema: SqliteSchema,
}

impl SqliteStore {
    /// 写模式打开。**三道门**按顺序：
    ///
    /// 1. 库比代码新 → `SchemaTooNew`（`nous-chord` 的迁移助手拒绝运行）；
    /// 2. 缺 `writer` 列（上游库）→ [`StorageError::WriterMismatch`]；
    /// 3. `writer != 'nous'` → 同上。
    ///
    /// 打开成功时 `PRAGMA journal_mode=WAL` + `busy_timeout=5000` 已经设好（`docs/storage.md` §7）。
    pub fn open_writer(path: impl AsRef<Path>) -> Result<Self> {
        let schema = SqliteSchema::unprefixed();
        let mut db = LocalDatabase::open(path)?;
        apply_sqlite_migrations(&mut db, &migrations(&schema), schema.table("durable_schema"))?;
        // WAL + busy_timeout：单写者的两个前提。
        db.exec("PRAGMA journal_mode=WAL")?;
        db.exec("PRAGMA busy_timeout=5000")?;

        let store = Self { db, schema };
        match store.writer()? {
            Some(writer) if writer == WRITER => Ok(store),
            found => Err(StorageError::WriterMismatch { found }),
        }
    }

    /// 只读打开。**不认门**：上游 v2 库（没有 `writer` 列）照读 —— 那是导入器的入口。
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            db: LocalDatabase::open_read_only(path)?,
            schema: SqliteSchema::unprefixed(),
        })
    }

    pub fn schema(&self) -> &SqliteSchema {
        &self.schema
    }

    pub fn db(&self) -> &LocalDatabase {
        &self.db
    }

    pub fn db_mut(&mut self) -> &mut LocalDatabase {
        &mut self.db
    }

    /// `durable_schema.writer`。上游库（v2）没有这一列 → `None`。
    pub fn writer(&self) -> Result<Option<String>> {
        match self.db.get(
            &format!("SELECT writer FROM {} WHERE singleton = 1", self.schema.table("durable_schema")),
            &[],
        ) {
            Ok(row) => Ok(row
                .and_then(|row| row.get("writer").and_then(|value| value.as_str().map(str::to_string)))),
            // 没有这一列 = 上游库。用「列不存在」的 SQLite 错误识别，而不是猜。
            Err(error) if is_missing_column(&error) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// `durable_schema.version`（上游库也有）。
    pub fn schema_version(&self) -> Result<i64> {
        let row = self
            .db
            .get(
                &format!("SELECT version FROM {} WHERE singleton = 1", self.schema.table("durable_schema")),
                &[],
            )?
            .ok_or_else(|| StorageError::Shape("durable_schema has no singleton row".into()))?;
        row.get("version")
            .and_then(|value| value.as_i64())
            .ok_or_else(|| StorageError::Shape("durable_schema.version is not an integer".into()))
    }

    /// 分配器状态：`(next_id 的十进制文本, next_seq)`。
    pub fn metadata(&self) -> Result<(String, i64)> {
        let row = self
            .db
            .get(
                &format!(
                    "SELECT next_id, next_seq FROM {} WHERE singleton = 1",
                    self.schema.table("durable_metadata")
                ),
                &[],
            )?
            .ok_or_else(|| StorageError::Shape("durable_metadata has no singleton row".into()))?;
        let next_id = row
            .get("next_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| StorageError::Shape("durable_metadata.next_id is not text".into()))?;
        let next_seq = row
            .get("next_seq")
            .and_then(|value| value.as_i64())
            .ok_or_else(|| StorageError::Shape("durable_metadata.next_seq is not an integer".into()))?;
        Ok((next_id.to_string(), next_seq))
    }
}

/// SQLite 报「no such column」时返回 true。用它区分「上游库没有 writer 列」和真正的故障。
fn is_missing_column(error: &SqliteError) -> bool {
    error.kind == SqliteErrorKind::Backend && error.message.contains("no such column")
}
