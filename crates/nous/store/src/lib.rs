//! Nous 自己的库（`nous.db`）：**触发器**与**触发账**，外加一条自己的迁移链（#56）。
//!
//! 触发器是**我们自己开发的插件**造出来的实体，跟 pi 无关 —— 就像 Magento 2 里「商品」是
//! Magento 的实体、存在 Magento 的库。`nous-durable` 的语义（session / entry / document /
//! submission / task）里**不该存在触发器这个概念**，所以这里是第二个库，不是一个新表。
//!
//! ## 为什么是两个文件
//!
//! WAL 下 **`ATTACH` 跨库事务不原子**（SQLite 文档：只有非 WAL 才保证）。所以「一个事务写两个
//! 库」这条路不存在 —— 不是我们不想用，是没有。能用的只有**幂等 + 补偿**：
//! [`fires`] 的唯一键去重，[`reclaim`] 把超期的 `pending` 扫成 `orphaned` 再重放。
//! 也正因为这条路不存在，本 crate 里不允许出现 `ATTACH`（`scripts/check-separation.sh` 判据 7）。
//!
//! ## 三条硬规矩（都能红）
//!
//! 1. **不依赖 `nous-durable`**：`Cargo.toml` 的依赖节里只有 `rusqlite`（判据 1）。
//!    反方向同理：`nous-durable/src` 里 `grep trigger_code|triggers|fires|plugin_snapshot|nous_store` == 0。
//! 2. **零直接时钟**：`src/` 里没有直接取 now（`std::time` 的两种 `::now()`）——
//!    时间一律由调用方传（`now_ms: i64`），为了能穷举测（判据 6）。
//! 3. **没有 `ATTACH`**（判据 7，理由见上）。
//!
//! ## 本票的 Rust 面只是**判据的载体**
//!
//! 票面写「不做触发器 API」：没有 HTTP 端点、没有调度循环、没有 UI（都是下一张）。
//! 这里的方法只为判据 3/4/5 存在（记账、读回、改状态、删成墓碑），调用方今天是测试 —
//! 谁要给触发器接上端点，那是下一张票的事。
//!
//! ## 薄接口（给 #52 用，本票只提供、不接线）
//!
//! [`Store::open`]（自带迁移）· [`Store::backup_to`]（让 `deploy.sh` 不用碰 rusqlite 细节）·
//! [`Store::schema_version`]（给 `--check` 的 `schema_plugins=`）。

mod error;
pub mod fires;
mod migrations;
pub mod trigger;
pub mod triggers;

use std::path::Path;

use rusqlite::Connection;

pub use error::{Result, StoreError};
pub use fires::{
    reclaim, request_id, scheduler_lag, Action, Fire, FireCounts, FireState, Inserted,
    SCHEDULER_TICK_MS, GRACE_MS,
};
pub use migrations::{CURRENT_VERSION, VERSION_TABLE};
pub use trigger::{
    ensure_code_immutable, fire_due, parse_five_field_cron, DueFire, FireActor, NewTrigger as NewTriggerDefinition,
    Policy, Spec, Target, TemplateRef, Trigger, TriggerError,
};
pub use triggers::NewTrigger;

/// 一份 `nous.db`。**单写者**：跟 durable 一样是 WAL + `busy_timeout`，两库互不相干。
pub struct Store {
    conn: Connection,
}

impl Store {
    /// 打开（不存在就建），设好这条连接自己的 pragma，然后把迁移推到 [`CURRENT_VERSION`]。
    ///
    /// **不复用 durable 的连接，也不复用它的 pragma**：两个进程级的库各有各的假设，
    /// 借用对方的连接等于把两边的超时、journal 模式、事务边界绑在一起。
    ///
    /// 库比代码新 → [`StoreError::SchemaTooNew`]（拒绝打开，别让写入落在看不懂的表结构上）。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        // WAL + busy_timeout：单写者的两个前提（与 `docs/storage.md` §7 同一组数）。
        // `synchronous=NORMAL`：WAL 下提交本身不 fsync，掉电丢最近若干笔提交，库不会坏。
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             PRAGMA synchronous=NORMAL;",
        )?;
        migrations::apply(&mut conn)?;

        let store = Self { conn };
        let found = store.schema_version()?;
        if found != CURRENT_VERSION {
            return Err(StoreError::SchemaVersion { found, supported: CURRENT_VERSION });
        }
        Ok(store)
    }

    /// 内存库：同一个 `Store`，零文件 IO（`docs/plugins.md` §2 给 `platform` 层的「无 IO 替身」）。
    ///
    /// 迁移、约束、`reclaim` 的输入输出都走同一份代码，只有落盘那一步没有；
    /// 判据里需要「不碰盘也能验」的地方用它。WAL 在内存库里不生效（SQLite 的 `journal_mode`
    /// 只能是 `memory`），所以「WAL 真的开了」那条判据必须用真文件。
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA synchronous=NORMAL;")?;
        migrations::apply(&mut conn)?;
        Ok(Self { conn })
    }

    /// 库的 schema 版本（`--check` 的 `schema_plugins=` 要它）。
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.conn.query_row(
            &format!("SELECT version FROM {VERSION_TABLE} WHERE singleton = 1"),
            [],
            |row| row.get(0),
        )?)
    }

    /// 整库备份到一个**还不存在**的文件（`VACUUM INTO`：一个原子的快照，不复制 WAL 边车）。
    ///
    /// 目标已存在 → [`StoreError::BackupTargetExists`]：**不覆盖上一份**。上一份也是备份，
    /// 悄悄盖掉它等于「备份看上去在，其实只剩最新的那一份」。
    pub fn backup_to(&self, path: &Path) -> Result<()> {
        if path.exists() {
            return Err(StoreError::BackupTargetExists { path: path.display().to_string() });
        }
        let target = path.to_str().ok_or_else(|| {
            StoreError::BackupTargetExists { path: path.display().to_string() }
        })?;
        self.conn.execute("VACUUM INTO ?1", [target])?;
        Ok(())
    }

    /// 裸连接。给判据和将来的维护脚本用；**生产路径走上面的方法**。
    ///
    /// 拿到它去 `DELETE FROM triggers` 或改 `code` 会被库里的触发器拒绝 —— 这是有意的：
    /// 「我们的 API 没提供这个方法」挡不住任何人，库里的规则才挡得住。
    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

impl std::fmt::Debug for Store {
    /// 打印版本与文件，不打印连接（连接里没有值得看的东西，`Connection` 也没实现 `Debug`）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("schema_version", &self.schema_version().ok())
            .finish()
    }
}
