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
pub mod scans;
pub mod sessions;
pub mod storage;
pub mod writes;
pub mod sqlite;

use std::path::Path;

use nous_chord::sqlite::{
    apply_sqlite_migrations, LocalDatabase, SqliteDatabase, SqliteError, SqliteErrorKind,
    SqliteExecutor, SqliteValue as Cell,
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
    /// 打开成功时 `PRAGMA journal_mode=WAL` + `busy_timeout=5000` + `synchronous=NORMAL` 已经设好
    /// （`docs/storage.md` §7；最后那条的代价在 `docs/bench-commit.md`）。
    pub fn open_writer(path: impl AsRef<Path>) -> Result<Self> {
        let schema = SqliteSchema::unprefixed();
        let mut db = LocalDatabase::open(path)?;
        apply_sqlite_migrations(&mut db, &migrations(&schema), schema.table("durable_schema"))?;
        // WAL + busy_timeout：单写者的两个前提。
        db.exec("PRAGMA journal_mode=WAL")?;
        db.exec("PRAGMA busy_timeout=5000")?;
        // `synchronous=NORMAL`：WAL 下**提交本身不 fsync**（只有 checkpoint 才 fsync）。
        // 掉电的代价是丢掉最近若干个提交，库本身不会坏 —— SQLite 文档写明的取舍。
        // 依据是数字，不是口味：1,000 次 immediate partial 提交在 ext4 上默认 FULL 要 11,200 ms
        // （每次提交 ~11 ms 全花在 fsync），NORMAL 后 1,890 ms；按真实节流（100 ms 一次，
        // 16 秒流式输出 = 160 次提交）是 110 ms。据此上游的 deferred 持久化
        // （`live.ts:236` `PROGRESS_COMMIT`）在本库**不需要**。见 `docs/bench-commit.md`。
        db.exec("PRAGMA synchronous=NORMAL")?;

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
/// 把一份**上游 v2 库的副本**领养成可写库：加 `writer` 列、版本号记成 1。
///
/// ## 这不是迁移，是「领养副本」
///
/// `docs/storage.md` §12.3：两边的版本号**不可比**。上游 v2 的最终 DDL 与本库 v1 逐字相同
/// （`tasks` 的 `sleeping` 已含在 v1，见 `sqlite/migrations.rs` 模块文档），差别只有
/// `durable_schema.writer` 这一列。所以「领养」= 补一列 + 把版本号记成我们自己的 1，
/// 不动任何表、不动任何行数据。
///
/// ## 两道守卫，缺一不可
///
/// 1. 文件必须**没有** `writer` 列（有就说明已经是别人的可写库，不许改它的认门值）；
/// 2. 版本号必须**等于 2**（上游 v2）。别的值一律拒绝 —— 猜一个不认识的库是数据损坏。
///
/// ## 只能用在副本上
///
/// 这是**破坏性**的：跑完之后那份文件在本库眼里就是 v1，真上游再打开会看到旧版本。
/// 调用方（`nous-server` 的 bin）只在显式开关下调用，且文档写明「只对 /var/tmp 的副本用」。
pub fn adopt_legacy_copy(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let schema = SqliteSchema::unprefixed();
    {
        // 先用只读连接判两道门，确认之后再开写连接 —— 不留一个「开了写但立刻退出」的窗口。
        let probe = SqliteStore::open_read_only(path)?;
        if let Some(found) = probe.writer()? {
            return Err(StorageError::WriterMismatch {
                found: Some(found),
            });
        }
        let version = probe.schema_version()?;
        if version != 2 {
            return Err(StorageError::Shape(format!(
                "legacy adoption only accepts upstream v2, found version {version}"
            )));
        }
    }
    let mut db = LocalDatabase::open(path)?;
    let table = schema.table("durable_schema");
    db.transaction(|tx| -> Result<()> {
        tx.exec(&format!("ALTER TABLE {table} ADD COLUMN writer TEXT"))?;
        // 版本记成**当前版本**：上游 v2 的表结构与本库 v2 相同（含 #38 的终态索引），
        // 若硬写 1，下次打开会重跑 v2 迁移、撞「index already exists」。
        tx.run(
            &format!(
                "UPDATE {table} SET writer = ?, version = ? WHERE singleton = 1"
            ),
            &[WRITER.into(), Cell::Integer(CURRENT_VERSION as i64)],
        )?;
        Ok(())
    })?;
    Ok(())
}

fn is_missing_column(error: &SqliteError) -> bool {
    error.kind == SqliteErrorKind::Backend && error.message.contains("no such column")
}
