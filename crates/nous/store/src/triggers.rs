//! `triggers`：Nous 的「商品」。`code` 是人的身份，`id` 是机器的身份。
//!
//! 三件事由**库**保证，不由 API 保证（`migrations.rs` 的 v1 里那两条触发器）：
//! 改 `code` 被拒、真 `DELETE` 被拒、墓碑占着 `code`（`UNIQUE`）。
//!
//! 所以本模块的写方法都很薄：验形状（[`validate_code`]）→ 一句 SQL → 读回行。
//! 逻辑多一分在这个库上就是多余的一分。

use rusqlite::{params, Row};

use crate::error::{is_constraint_violation, Result, StoreError};
use crate::Store;

/// 新建触发器要的三样。`id` / `created_at` / `enabled` / `deleted_at` 由库自己给。
#[derive(Debug, Clone, Copy)]
pub struct NewTrigger<'a> {
    /// `^[a-z][a-z0-9_]{2,63}$`，**创建后不可变**。
    pub code: &'a str,
    /// 人看的名，随便改。
    pub name: &'a str,
    /// JSON 原文。本票不解释它（触发规则是下一张票的事）。
    pub spec: &'a str,
}

/// 一行 `triggers`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub spec: String,
    pub enabled: bool,
    /// ms；`None` = 停着（禁用 / 删除）。
    pub next_fire_at: Option<i64>,
    pub created_at: i64,
    /// 墓碑时间。非空 = 删过了，但行还在、`code` 仍然被它占着。
    pub deleted_at: Option<i64>,
}

impl Trigger {
    /// 删过了吗（墓碑）。
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// `^[a-z][a-z0-9_]{2,63}$`。手写，不引 `regex`：一条形状判据不值得第二个依赖。
///
/// 为什么不是「随便什么字符串」：`code` 会出现在 `requestId`（`trigger:<code>:<fire_at>`）、
/// `actor.id`（`trigger:<code>`）、审计日志和命令行里。带空格 / 大写 / 冒号的那个字符串
/// 在四个地方各有各的转义规则 —— 总有一天对不上。
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
        created_at: row.get("created_at")?,
        deleted_at: row.get("deleted_at")?,
    })
}

const COLUMNS: &str = "id, code, name, spec, enabled, next_fire_at, created_at, deleted_at";

impl Store {
    /// 建一个触发器。`now_ms` 由调用方给（本库没有直接时钟）。
    ///
    /// 墓碑的 `code` 也算被占 —— 返回的 [`StoreError::CodeTaken`] 会带上墓碑时间。
    pub fn create_trigger(&self, new: NewTrigger<'_>, now_ms: i64) -> Result<Trigger> {
        validate_code(new.code)?;
        if let Some(deleted_at) = self.occupant(new.code)? {
            return Err(StoreError::CodeTaken { code: new.code.to_string(), deleted_at });
        }
        let inserted = self.conn.execute(
            "INSERT INTO triggers (code, name, spec, enabled, next_fire_at, created_at, deleted_at)
             VALUES (?1, ?2, ?3, 1, NULL, ?4, NULL)",
            params![new.code, new.name, new.spec, now_ms],
        );
        match inserted {
            Ok(_) => {}
            // 兜底：`UNIQUE` 是物理保证，上面那一次 SELECT 只是为了让消息带墓碑时间。
            Err(error) if is_constraint_violation(&error) => {
                return Err(StoreError::CodeTaken { code: new.code.to_string(), deleted_at: None })
            }
            Err(error) => return Err(error.into()),
        }
        self.trigger(new.code)?
            .ok_or_else(|| StoreError::CodeUnknown(new.code.to_string()))
    }

    /// 按 `code` 读一行（墓碑也读得到 —— 它就是「行还在」的证据）。
    pub fn trigger(&self, code: &str) -> Result<Option<Trigger>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {COLUMNS} FROM triggers WHERE code = ?1"))?;
        let mut rows = stmt.query(params![code])?;
        Ok(match rows.next()? {
            Some(row) => Some(row_to_trigger(row)?),
            None => None,
        })
    }

    /// 全部触发器（含墓碑），按 `id`（= 创建顺序）。
    pub fn triggers(&self) -> Result<Vec<Trigger>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {COLUMNS} FROM triggers ORDER BY id"))?;
        let rows = stmt.query_map([], row_to_trigger)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 改名。**只**改 `name`：`code` 由库里的触发器挡着。
    pub fn rename_trigger(&self, code: &str, name: &str) -> Result<()> {
        self.touch("UPDATE triggers SET name = ?1 WHERE code = ?2", params![name, code], code)
    }

    /// 改「开不开」与「下一次什么时候」—— 同一个状态的两个面，一次写完。
    /// 停止 / 禁用时 `next_fire_at` 传 `None`。
    pub fn set_schedule(&self, code: &str, enabled: bool, next_fire_at: Option<i64>) -> Result<()> {
        self.touch(
            "UPDATE triggers SET enabled = ?1, next_fire_at = ?2 WHERE code = ?3",
            params![i64::from(enabled), next_fire_at, code],
            code,
        )
    }

    /// 删 = 填墓碑：`deleted_at` 只写第一次，`next_fire_at` 清空，`enabled` 关掉。
    ///
    /// 行永远留着，`code` 永远被占着。再删一次不动墓碑时间。
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

    /// 行存在吗 → 存在时返回它的墓碑时间（`None` = 活着的）。
    fn occupant(&self, code: &str) -> Result<Option<Option<i64>>> {
        let mut stmt = self.conn.prepare("SELECT deleted_at FROM triggers WHERE code = ?1")?;
        let mut rows = stmt.query(params![code])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get::<_, Option<i64>>(0)?),
            None => None,
        })
    }

    /// 「按 code 改一行，改了 0 行就是没有这个人」。
    fn touch(&self, sql: &str, params: &[&dyn rusqlite::ToSql], code: &str) -> Result<()> {
        if self.conn.execute(sql, params)? == 0 {
            return Err(StoreError::CodeUnknown(code.to_string()));
        }
        Ok(())
    }
}
