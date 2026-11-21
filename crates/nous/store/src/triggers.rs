//! `triggers`：Nous 的「商品」。`code` 是人的身份，`id` 是机器的身份。
//!
//! 定义（字段、spec、cron、`fire_due`）在 [`crate::trigger`]；这里只做 `triggers` 表的
//! 行读写。`code` 不可变、真 `DELETE` 被拒、墓碑占着 `code` 三件事由库里的触发器保证。

use rusqlite::{params, Row};

use crate::error::{is_constraint_violation, Result, StoreError};
use crate::Store;

/// #56 的薄入参：`Trigger::new` 不认识它，只有迁移与兼容路径用。
#[derive(Debug, Clone, Copy)]
pub struct NewTrigger<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub spec: &'a str,
}

/// #56 留下的行投影（`spec` 是 JSON 原文）。新代码请用 `crate::trigger::Trigger`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub spec: String,
    pub enabled: bool,
    pub next_fire_at: Option<i64>,
    /// #73：`PATCH` 的 ETag，每次有效改动 +1。
    pub version: i64,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

impl Trigger {
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

pub fn validate_code(code: &str) -> Result<()> {
    let bytes = code.as_bytes();
    let rule = |rule: &'static str| Err(StoreError::InvalidCode { code: code.to_string(), rule });
    if bytes.len() < 3 {
        return rule("至少 3 个字符");
    }
    if bytes.len() > 64 {
        return rule("最多 64 个字符");
    }
    if !bytes[0].is_ascii_lowercase() {
        return rule("首字符必须是 a-z");
    }
    if !bytes[1..]
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
    {
        return rule("其余字符只许 a-z、0-9、_");
    }
    Ok(())
}

fn row_to_trigger(row: &Row<'_>) -> rusqlite::Result<Trigger> {
    Ok(Trigger {
        id: row.get("id")?,
        code: row.get("code")?,
        name: row.get("name")?,
        spec: row.get("spec")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        next_fire_at: row.get("next_fire_at")?,
        version: row.get("version")?,
        created_at: row.get("created_at")?,
        deleted_at: row.get("deleted_at")?,
    })
}

const COLUMNS: &str =
    "id, code, name, spec, enabled, next_fire_at, version, created_at, deleted_at";

impl Store {
    /// 兼容 #56 的薄入口：`Trigger::new` 会先校验准则，再走这里落行。
    pub fn create_trigger(&self, new: NewTrigger<'_>, now_ms: i64) -> Result<Trigger> {
        self.create_legacy_trigger(new.code, new.name, new.spec, now_ms)
    }

    pub(crate) fn create_legacy_trigger(
        &self,
        code: &str,
        name: &str,
        spec: &str,
        now_ms: i64,
    ) -> Result<Trigger> {
        validate_code(code)?;
        if let Some(deleted_at) = self.occupant(code)? {
            return Err(StoreError::CodeTaken { code: code.to_string(), deleted_at });
        }
        let inserted = self.conn.execute(
            "INSERT INTO triggers (code, name, spec, enabled, next_fire_at, created_at, deleted_at)
             VALUES (?1, ?2, ?3, 1, NULL, ?4, NULL)",
            params![code, name, spec, now_ms],
        );
        match inserted {
            Ok(_) => {}
            Err(error) if is_constraint_violation(&error) => {
                return Err(StoreError::CodeTaken { code: code.to_string(), deleted_at: None })
            }
            Err(error) => return Err(error.into()),
        }
        self.trigger(code)?.ok_or_else(|| StoreError::CodeUnknown(code.to_string()))
    }

    pub fn trigger(&self, code: &str) -> Result<Option<Trigger>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {COLUMNS} FROM triggers WHERE code = ?1"))?;
        let mut rows = stmt.query(params![code])?;
        Ok(match rows.next()? {
            Some(row) => Some(row_to_trigger(row)?),
            None => None,
        })
    }

    pub fn triggers(&self) -> Result<Vec<Trigger>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {COLUMNS} FROM triggers ORDER BY id"))?;
        let rows = stmt.query_map([], row_to_trigger)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// #57：把完整定义 JSON 写进 `triggers.spec`（建完即定型，这里只是持久化）。
    pub fn update_definition(&self, code: &str, spec: &str) -> Result<()> {
        self.touch("UPDATE triggers SET spec = ?1 WHERE code = ?2", params![spec, code], code)
    }

    pub fn rename_trigger(&self, code: &str, name: &str) -> Result<()> {
        self.touch("UPDATE triggers SET name = ?1 WHERE code = ?2", params![name, code], code)
    }

    pub fn set_schedule(&self, code: &str, enabled: bool, next_fire_at: Option<i64>) -> Result<()> {
        self.touch(
            "UPDATE triggers SET enabled = ?1, next_fire_at = ?2 WHERE code = ?3",
            params![i64::from(enabled), next_fire_at, code],
            code,
        )
    }

    pub fn delete_trigger(&self, code: &str, now_ms: i64) -> Result<()> {
        self.touch(
            "UPDATE triggers
                SET deleted_at  = COALESCE(deleted_at, ?1),
                    enabled     = 0,
                    next_fire_at = NULL
              WHERE code = ?2",
            params![now_ms, code],
            code,
        )
    }

    fn occupant(&self, code: &str) -> Result<Option<Option<i64>>> {
        let mut stmt = self.conn.prepare("SELECT deleted_at FROM triggers WHERE code = ?1")?;
        let mut rows = stmt.query(params![code])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get::<_, Option<i64>>(0)?),
            None => None,
        })
    }

    fn touch(&self, sql: &str, params: &[&dyn rusqlite::ToSql], code: &str) -> Result<()> {
        if self.conn.execute(sql, params)? == 0 {
            return Err(StoreError::CodeUnknown(code.to_string()));
        }
        Ok(())
    }

    /// #73：一次原子 PATCH。SQL 的 `WHERE version = ?` 就是乐观锁的物理保证，
    /// `version + 1` 只在这里发生 —— 别的地方都不许改这一列。
    /// 墓碑行永远不该被改：`deleted_at IS NULL` 写进 WHERE。
    pub fn patch_trigger(
        &self,
        code: &str,
        spec: &str,
        enabled: bool,
        expected_version: i64,
    ) -> Result<Option<i64>> {
        let changed = self.conn.execute(
            "UPDATE triggers SET spec = ?1, enabled = ?2, version = version + 1
              WHERE code = ?3 AND version = ?4 AND deleted_at IS NULL",
            params![spec, i64::from(enabled), code, expected_version],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        let version: i64 = self.conn.query_row(
            "SELECT version FROM triggers WHERE code = ?1",
            params![code],
            |row| row.get(0),
        )?;
        Ok(Some(version))
    }

    /// #73：PATCH 的幂等回执。命中就返回第一次的 body，调用方不再改库。
    pub fn patch_receipt(&self, code: &str, request_id: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT body FROM trigger_patch_receipts WHERE code = ?1 AND request_id = ?2",
        )?;
        let mut rows = stmt.query(params![code, request_id])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    pub fn remember_patch_receipt(
        &self,
        code: &str,
        request_id: &str,
        body: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO trigger_patch_receipts (code, request_id, body)
             VALUES (?1, ?2, ?3)",
            params![code, request_id, body],
        )?;
        Ok(())
    }
}
