//! 迁移链：**我们自己的**版本表 + 顺序 SQL。
//!
//! ## 与 durable 各走各的
//!
//! `nous-durable` 的版本表叫 `durable_schema`，版本数到 2；这里叫 `schema_version`，从 1 起。
//! 两张表名字不同、版本号各自数 —— 这不是洁癖，是判据 2 的靶子：**借用对方的版本表**，
//! 删掉一边的库就会让另一边读成 `SchemaTooNew`，或者干脆跳到对方那一版。
//!
//! ## 版本只增不减
//!
//! 一条迁移一个事务（`BEGIN IMMEDIATE`）：DDL + 版本号落盘，要么都成，要么都不成。
//! 没有 down —— 回退 = 拿备份回来重放（`Store::backup_to` 就是干这个的）。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::error::{Result, StoreError};

/// 这一版支持的 schema 版本 = 迁移链最后一条。
pub const CURRENT_VERSION: u32 = 2;

/// 我们的版本表。**不许**改成 `durable_schema`（那是别人家的）。
pub const VERSION_TABLE: &str = "schema_version";

/// 迁移链。版本号必须从 1 连续排到 [`CURRENT_VERSION`]，跳号就是改坏了。
pub fn steps() -> Vec<(u32, &'static str)> {
    vec![(1, V1), (2, V2)]
}

/// 第一版的两张表 + 三条库级规矩。
///
/// `triggers` / `fires` 的列**逐字**照票面（#56）。多出来的两个触发器不是列、不是表，
/// 是判据 5 的物理保证：`code` 不可变、行不许真删。放在这里而不是只写在 Rust API 里，
/// 是因为「我们的 API 没提供这个方法」对拿到裸连接的人毫无约束。
const V1: &str = r#"
CREATE TABLE triggers (
  id          INTEGER PRIMARY KEY,
  code        TEXT NOT NULL UNIQUE,       -- ^[a-z][a-z0-9_]{2,63}$，创建后不可变
  name        TEXT NOT NULL,              -- 人看的名，可以随便改
  spec        TEXT NOT NULL,              -- JSON：触发规则，本票不解释它
  enabled     INTEGER NOT NULL DEFAULT 1,
  next_fire_at INTEGER,                   -- ms；停止/禁用时为 NULL
  created_at  INTEGER NOT NULL,
  deleted_at  INTEGER                     -- 墓碑：删了就填，行永远留着
) STRICT;

-- code 是人的身份：requestId、actor.id（trigger:<code>）、审计、你脑子里的印象都挂着它。
-- 复用 = 让历史记录指向另一个触发器，那是事故。
CREATE TRIGGER triggers_code_immutable
BEFORE UPDATE OF code ON triggers
BEGIN
  SELECT RAISE(ABORT, 'triggers.code 不可变（requestId / actor.id / 审计都挂着它）');
END;

-- 删除 = 填 deleted_at。真 DELETE 会让 code 空出来被复用 —— 同一条事故。
CREATE TRIGGER triggers_tombstone_only
BEFORE DELETE ON triggers
BEGIN
  SELECT RAISE(ABORT, 'triggers 只许留墓碑（填 deleted_at），不许真删');
END;

CREATE TABLE fires (
  trigger_code TEXT NOT NULL,
  fire_at      INTEGER NOT NULL,
  state        TEXT NOT NULL CHECK (state IN ('pending','done','orphaned','skipped')),
  request_id   TEXT NOT NULL,             -- trigger:<code>:<fire_at>
  note         TEXT,
  PRIMARY KEY (trigger_code, fire_at)      -- 幂等的物理保证：同一次触发只记一行
) STRICT;
"#;

/// #60：模板引擎内修订。盘上的 `preset.json` 是内容真相，这里是每次 PUT / rollback
/// 的原文与来路；`UNIQUE(section, name, n)` 是「第 n 版只有一份」的物理保证，
/// `request_id` 给幂等回执留锚（同一 requestId 重放不增 n）。跟私库 git 历史（#75）无关。
const V2: &str = r#"
CREATE TABLE presets_revisions (
  section    TEXT NOT NULL,
  name       TEXT NOT NULL,
  n          INTEGER NOT NULL,
  sha        TEXT NOT NULL,
  body       TEXT NOT NULL,
  actor      TEXT NOT NULL,
  at         INTEGER NOT NULL,
  request_id TEXT NOT NULL,
  PRIMARY KEY (section, name, n)
) STRICT;
CREATE INDEX presets_revisions_by_request ON presets_revisions (section, name, request_id);

-- 写端点的幂等回执：同一 requestId 重放必须吐回第一次的回执（version 不涨）。
CREATE TABLE preset_write_receipts (
  section    TEXT NOT NULL,
  name       TEXT NOT NULL,
  kind       TEXT NOT NULL,
  request_id TEXT NOT NULL,
  body       TEXT NOT NULL,
  PRIMARY KEY (section, name, kind, request_id)
) STRICT;
"#;

/// 建版本表 → 读版本 → 按链推进。可重复调用（第二次什么都不做）。
pub fn apply(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {VERSION_TABLE} (
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
             version   INTEGER NOT NULL
         ) STRICT;"
    ))?;

    let found: Option<u32> = conn
        .query_row(
            &format!("SELECT version FROM {VERSION_TABLE} WHERE singleton = 1"),
            [],
            |row| row.get(0),
        )
        .optional()?;

    let mut current = match found {
        Some(version) => version,
        None => {
            // 空库：先落一行 0，让后面的 UPDATE 有一个明确的对象。
            conn.execute(
                &format!("INSERT INTO {VERSION_TABLE} (singleton, version) VALUES (1, 0)"),
                [],
            )?;
            0
        }
    };

    if current > CURRENT_VERSION {
        return Err(StoreError::SchemaTooNew { found: current, supported: CURRENT_VERSION });
    }

    for (version, sql) in steps() {
        if version <= current {
            continue;
        }
        if version != current + 1 {
            return Err(StoreError::MigrationChain { expected: current + 1, found: version });
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        tx.execute(
            &format!("UPDATE {VERSION_TABLE} SET version = ?1 WHERE singleton = 1"),
            [version],
        )?;
        tx.commit()?;
        current = version;
    }

    Ok(())
}
