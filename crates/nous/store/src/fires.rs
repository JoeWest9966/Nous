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

/// 调度循环的节拍（#58 的 `scheduler.tickMs`，毫秒）。
///
/// 为什么是 5 s：`fire_due` 按**分钟对齐**（`now.div_euclid(60_000) * 60_000`），
/// 一分钟里只要响到那一拍就永不丢触发；tick 越长，`lagMs` 分母越大、
/// 「循环卡死」的发现越晚。5 s 在两者之间。
pub const SCHEDULER_TICK_MS: i64 = 5_000;

/// 调度健康（#58）：`lagMs` 是循环「自报迟到量」。
///
/// 口径：`lag = max(0, now - lastTickAt - tickMs)` —— **下一拍该响而没响的毫秒数**。
/// 不用票面草稿的那个「对齐余数」式（`lastTickAt - tickMs*floor(...)`）：那个值在
/// 停摆时**不增长**（卡死的循环和健康循环算出来一样小），分不开「循环卡住」和
/// 「没有触发器要点火」这两种长得一模一样的情况 —— 而区分它们正是引入 lag 的目的。
pub fn scheduler_lag(now_ms: i64, last_tick_at: Option<i64>, tick_ms: i64) -> i64 {
    let Some(last_tick_at) = last_tick_at else {
        return 0; // 一拍都没响过（刚启动）：没有迟到可言，不是“无限迟到”
    };
    (now_ms - last_tick_at - tick_ms).max(0)
}

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
    /// 串行策略下上一轮还在跑，本次没有提交（不是失败，不重放）。
    Skipped,
}

impl FireState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Orphaned => "orphaned",
            Self::Skipped => "skipped",
        }
    }

    /// 库里的字符串 → 变体。认不出来就明说（不猜）。
    pub fn parse(state: &str) -> Result<Self> {
        match state {
            "pending" => Ok(Self::Pending),
            "done" => Ok(Self::Done),
            "orphaned" => Ok(Self::Orphaned),
            "skipped" => Ok(Self::Skipped),
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

/// 一个触发器的账面统计（#58）：`GROUP BY state` 的结果。
///
/// **库是唯一真相**：这个结构每次从 `fires` 表现算，没有第二份内存计数器 ——
/// 「UI 显示 12 次」和「库里 12 行」因此不可能分叉（#58 判据 1）。四个键恒在，
/// 没账就是 0，读的人不需要猜缺键是什么意思。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FireCounts {
    pub done: i64,
    pub skipped: i64,
    pub orphaned: i64,
    pub pending: i64,
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
            FireState::Skipped => continue,
        }
    }
    actions
}

/// 超期 = **超过**宽限期（严格大于：恰好 `GRACE_MS` 仍算宽限期内，`+1 ms` 才超）。
/// `saturating_sub`：`fire_at` 在很远的过去也不 panic；`fire_at` 在未来自然不超期。
fn overdue(fire_at: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(fire_at) > GRACE_MS
}

/// 重放动作的 `requestId` **现算**（`request_id(code, fire_at)`），不读行里那一份副本：
/// 账本可能被手工改过、也可能是老版本写进去的，而幂等的锚点只有一个来源。
fn replay(fire: &Fire) -> Action {
    Action::Replay {
        trigger_code: fire.trigger_code.clone(),
        fire_at: fire.fire_at,
        request_id: request_id(&fire.trigger_code, fire.fire_at),
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

    /// 一个触发器的账面统计（#58）：`GROUP BY state`，不是任何内存计数器。
    /// 库里出现四种状态之外的值会被 CHECK 拦住，这里仍走 [`FireState::parse`] 不猜。
    pub fn fire_counts(&self, code: &str) -> Result<FireCounts> {
        let mut stmt = self.conn.prepare(
            "SELECT state, count(*) FROM fires WHERE trigger_code = ?1 GROUP BY state",
        )?;
        let rows = stmt.query_map(params![code], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut counts = FireCounts::default();
        for row in rows {
            let (state, count) = row?;
            match FireState::parse(&state)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
            {
                FireState::Done => counts.done = count,
                FireState::Skipped => counts.skipped = count,
                FireState::Orphaned => counts.orphaned = count,
                FireState::Pending => counts.pending = count,
            }
        }
        Ok(counts)
    }

    /// 最近一次**有结论**的点火（#58 的 `lastFire`）：`pending` 不算 —— 它还没成也没败，
    /// 把它当「上次点火」是在替账本撒谎。一条账都没有 → `None`。
    pub fn last_resolved_fire(&self, code: &str) -> Result<Option<Fire>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM fires
              WHERE trigger_code = ?1 AND state != 'pending'
              ORDER BY fire_at DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query(params![code])?;
        Ok(match rows.next()? {
            Some(row) => Some(row_to_fire(row)?),
            None => None,
        })
    }

    /// 一个触发器最近的账，时间倒序（#58 的 `GET /v1/triggers/:code/fires`）。
    /// `limit <= 0` 回空 —— 「要 0 条」是个合法的问法，不是错误。
    pub fn recent_fires(&self, code: &str, limit: i64) -> Result<Vec<Fire>> {
        if limit <= 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM fires WHERE trigger_code = ?1 ORDER BY fire_at DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![code, limit], row_to_fire)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
        self.transition(
            "UPDATE fires SET state = 'done' WHERE trigger_code = ?1 AND fire_at = ?2",
            code,
            fire_at,
        )
    }

    /// `pending` → `orphaned`。**`done` 不会被降级**（`AND state = 'pending'` 就是这条保证：
    /// 一条消息提交成功后，任何扫描都再也碰不到它），已经 `orphaned` 的什么都不变（幂等）。
    pub fn mark_orphaned(&self, code: &str, fire_at: i64) -> Result<()> {
        self.transition(
            "UPDATE fires SET state = 'orphaned'
              WHERE trigger_code = ?1 AND fire_at = ?2 AND state = 'pending'",
            code,
            fire_at,
        )
    }

    /// 串行策略命中：`pending` → `skipped`（不进 reclaim，不重放）。
    pub fn mark_skipped(&self, code: &str, fire_at: i64) -> Result<()> {
        self.transition(
            "UPDATE fires SET state = 'skipped' WHERE trigger_code = ?1 AND fire_at = ?2 AND state = 'pending'",
            code,
            fire_at,
        )
    }

    /// 落账时已知模板缺失：直接写成 `orphaned` + note（没有 entry，不该被重放成空轮）。
    pub fn mark_orphaned_with_note(&self, code: &str, fire_at: i64, note: &str) -> Result<()> {
        self.record_fire(code, fire_at, Some(note))?;
        self.conn.execute(
            "UPDATE fires SET state = 'orphaned', note = ?3
              WHERE trigger_code = ?1 AND fire_at = ?2",
            params![code, fire_at, note],
        )?;
        Ok(())
    }

    /// 状态迁移的公共尾巴：改到 0 行要么是「已经是那个状态」（幂等，OK），要么是「没这笔账」。
    fn transition(&self, sql: &str, code: &str, fire_at: i64) -> Result<()> {
        if self.conn.execute(sql, params![code, fire_at])? == 1 {
            return Ok(());
        }
        match self.fire(code, fire_at)? {
            Some(_) => Ok(()),
            None => Err(StoreError::FireUnknown { trigger_code: code.to_string(), fire_at }),
        }
    }

    fn query_fires(&self, sql: &str) -> Result<Vec<Fire>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], row_to_fire)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
