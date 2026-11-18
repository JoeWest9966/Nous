//! `fires`：每次触发一行账。**唯一键 `(trigger_code, fire_at)` 就是幂等的物理保证。**
//!
//! ## 顺序写死（`docs/decisions.md`）
//!
//! ```text
//! tick 命中 → ① 插 fires(state='pending')，唯一键去重
//!          → ② 调 durable 的 submit_message(requestId = trigger:<code>:<fire_at>)
//!             ├ 成功 → fires.state = 'done'
//!             └ 失败/崩溃 → fires 留在 'pending'；超过宽限期扫成 'orphaned' → 重放 ②（幂等，安全）
//! ```
//!
//! **反过来写不安全**：先提交后记账，会留下「durable 里有一条 entry、Nous 不知道谁触发的」。
//! 所以 ① 一定在 ② 前面，而 [`reclaim`] 只从「已经记过账的行」出发。
//!
//! ## 这一层只做两件事
//!
//! [`reclaim`] 是**纯函数**（时钟当参数，可穷举测），它只说「该干什么」；
//! 真去调 `submit_message` 的是调用方（下一张票的调度循环）。

use rusqlite::{params, Row};

use crate::error::{Result, StoreError};
use crate::Store;

/// 宽限期：`pending` 超过它就认定那次提交**没成功**（崩了，或者写库常年失败）。
///
/// 为什么是 2 分钟：② 是本地库的一次提交（毫秒级，`docs/bench-commit.md`）加上一次
/// `submit_message`。2 分钟远大于它，又短到「崩溃后下一轮就能补上」。
/// 宽限期太短 → 把还在路上的提交误判成 orphaned（重放是幂等的，所以不致命，但会多提交一次）；
/// 太长 → 崩溃后那次触发要等很久才补。
pub const GRACE_MS: i64 = 120_000;

/// `requestId` 的唯一来源：`trigger:<code>:<fire_at>`。
///
/// 幂等靠它。**不许在别处拼这个字符串** —— 拼错一个字符，durable 就会当成新消息再收一次。
pub fn request_id(code: &str, fire_at: i64) -> String {
    format!("trigger:{code}:{fire_at}")
}

/// 一笔账的三种状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireState {
    /// 记了账，还没确认提交成功。
    Pending,
    /// 提交成功了（durable 里有那条 entry）。**永不再重放**。
    Done,
    /// 超过宽限期还是 `pending`：那次提交大概失败了。可以重放。
    Orphaned,
}

impl FireState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Orphaned => "orphaned",
        }
    }

    /// 库里的字符串 → 变体。认不出来就明说（不猜）。
    pub fn parse(state: &str) -> Result<Self> {
        match state {
            "pending" => Ok(Self::Pending),
            "done" => Ok(Self::Done),
            "orphaned" => Ok(Self::Orphaned),
            other => Err(StoreError::StateUnknown { state: other.to_string() }),
        }
    }
}

/// 一行 `fires`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fire {
    pub trigger_code: String,
    pub fire_at: i64,
    pub state: FireState,
    /// `request_id(code, fire_at)` 的落盘副本（账本要能自己说清它对应哪条消息）。
    pub request_id: String,
    pub note: Option<String>,
}

/// 记账的结果。`AlreadyThere` **不是错误**：同一拍敲两次是正常的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inserted {
    New,
    AlreadyThere,
}

/// [`reclaim`] 说的「该干什么」。调用方按顺序执行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 把这条账标成 `orphaned`（先记，再重放 —— 顺序写死）。
    MarkOrphaned { trigger_code: String, fire_at: i64 },
    /// 重放 ②：`submit_message(request_id)`。幂等，重复调用安全。
    Replay { trigger_code: String, fire_at: i64, request_id: String },
}

/// 扫一遍账本，说这一轮该补什么。**纯函数**：不读时钟、不碰库、不改入参。
///
/// 三条规矩：
///
/// 1. `done` **永不重放**（重放 = 同一条消息再提交一次）；
/// 2. `pending` 在宽限期内 → 不动（可能还在路上）；超期 → 先 [`Action::MarkOrphaned`] 再重放；
/// 3. 已经是 `orphaned` 的 → 直接重放。这一条堵的是「标了 orphaned、还没重放就崩了」那个窗口：
///    只认 `pending` 的话，那条触发会永远躺在账本里没人补。
pub fn reclaim(rows: &[Fire], now_ms: i64) -> Vec<Action> {
    let mut actions = Vec::new();
    for fire in rows {
        match fire.state {
            FireState::Done => continue,
            FireState::Pending => {
                if !overdue(fire.fire_at, now_ms) {
                    continue;
                }
                actions.push(Action::MarkOrphaned {
                    trigger_code: fire.trigger_code.clone(),
                    fire_at: fire.fire_at,
                });
                actions.push(replay(fire));
            }
            FireState::Orphaned => actions.push(replay(fire)),
        }
    }
    actions
}

/// 超期 = 已经过了宽限期。`saturating_sub`：`fire_at` 在很远的过去也不 panic。
fn overdue(fire_at: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(fire_at) > GRACE_MS
}

fn replay(fire: &Fire) -> Action {
    Action::Replay {
        trigger_code: fire.trigger_code.clone(),
        fire_at: fire.fire_at,
        request_id: fire.request_id.clone(),
    }
}

fn row_to_fire(row: &Row<'_>) -> rusqlite::Result<Fire> {
    Ok(Fire {
        trigger_code: row.get("trigger_code")?,
        fire_at: row.get("fire_at")?,
        state: FireState::parse(&row.get::<_, String>("state")?)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
        request_id: row.get("request_id")?,
        note: row.get("note")?,
    })
}

const COLUMNS: &str = "trigger_code, fire_at, state, request_id, note";

impl Store {
    /// 记一笔账。`(code, fire_at)` 已经有了就是 [`Inserted::AlreadyThere`]（不报错）。
    ///
    /// **① 必须在 ② 前面**：先记账再提交，崩在中间最多是「重放一次」；
    /// 反过来会留下「durable 有 entry、Nous 不知道谁触发的」。
    pub fn record_fire(&self, code: &str, fire_at: i64, note: Option<&str>) -> Result<Inserted> {
        let changed = self.conn.execute(
            "INSERT INTO fires (trigger_code, fire_at, state, request_id, note)
             VALUES (?1, ?2, 'pending', ?3, ?4)
             ON CONFLICT (trigger_code, fire_at) DO NOTHING",
            params![code, fire_at, request_id(code, fire_at), note],
        )?;
        Ok(if changed == 1 { Inserted::New } else { Inserted::AlreadyThere })
    }

    /// 读一笔账。
    pub fn fire(&self, code: &str, fire_at: i64) -> Result<Option<Fire>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM fires WHERE trigger_code = ?1 AND fire_at = ?2"
        ))?;
        let mut rows = stmt.query(params![code, fire_at])?;
        Ok(match rows.next()? {
            Some(row) => Some(row_to_fire(row)?),
            None => None,
        })
    }

    /// 全量账本，按 `fire_at` 升序（同刻按 `trigger_code`）。
    pub fn fires(&self) -> Result<Vec<Fire>> {
        self.query_fires(&format!("SELECT {COLUMNS} FROM fires ORDER BY fire_at, trigger_code"))
    }

    /// [`reclaim`] 的输入：**还没结论**的账（`pending` + `orphaned`），`done` 不进。
    pub fn open_fires(&self) -> Result<Vec<Fire>> {
        self.query_fires(&format!(
            "SELECT {COLUMNS} FROM fires
              WHERE state IN ('pending', 'orphaned')
              ORDER BY fire_at, trigger_code"
        ))
    }

    /// 重放成功后落账。幂等：已经是 `done` 再调一次什么都不变。
    pub fn mark_done(&self, code: &str, fire_at: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE fires SET state = 'done' WHERE trigger_code = ?1 AND fire_at = ?2",
            params![code, fire_at],
        )?;
        if changed == 1 {
            return Ok(());
        }
        match self.fire(code, fire_at)? {
            Some(_) => Ok(()),
            None => Err(StoreError::FireUnknown {
                trigger_code: code.to_string(),
                fire_at,
            }),
        }
    }

    /// `pending` → `orphaned`。已经是 `done` 的不会被降级（`AND state = 'pending'`），
    /// 已经 `orphaned` 的什么都不变（幂等）。
    pub fn mark_orphaned(&self, code: &str, fire_at: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE fires SET state = 'orphaned'
              WHERE trigger_code = ?1 AND fire_at = ?2 AND state = 'pending'",
            params![code, fire_at],
        )?;
        if changed == 1 {
            return Ok(());
        }
        match self.fire(code, fire_at)? {
            Some(_) => Ok(()),
            None => Err(StoreError::FireUnknown {
                trigger_code: code.to_string(),
                fire_at,
            }),
        }
    }

    fn query_fires(&self, sql: &str) -> Result<Vec<Fire>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], row_to_fire)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
