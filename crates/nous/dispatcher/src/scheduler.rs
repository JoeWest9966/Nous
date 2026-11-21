//! 心跳与 sleeping 任务：节拍不自己写循环，只把「下一拍」交给注入的 `Clock::wait_until`。
//!
//! durable 的 `tasks` 表只存不跑；这里把一拍的**待办**写成一条 `sleeping` 任务
//! （跟 pi 的任务同一个表，但 dispatcher 不伪造 pi 的 task 语义）。任务记录用完整的
//! `TaskRecord` 形状，`state.status = sleeping`，`kind = nous.dispatcher.tick`，
//! `conversationId` 固定 0（调度不是任何会话的子任务）。真正的等待和推进由 Clock 完成。

use std::sync::Arc;

use nous_chord::sqlite::{SqliteExecutor, SqliteValue as Cell};
use nous_durable::harness::clock::Clock;
use nous_durable::session::{CommitOptions, Session};
use nous_durable::StorageWrite;
use serde_json::{json, Value};

/// 与 `nous-store::SCHEDULER_TICK_MS` 对齐的节拍；可在注入时钟里改（判据用自己的短 tick）。
pub const SCHEDULER_TICK_MS: u64 = nous_store::SCHEDULER_TICK_MS as u64;

/// 一拍的待办：第 n 拍、什么时候该响。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickRecord {
    pub seq: u64,
    pub due_at_ms: u64,
}

/// 从 durable 的 `sleeping` 任务行里读回待办。**库是唯一真相**，重启后还在。
pub fn tick_from_rows(rows: &[Value]) -> Option<TickRecord> {
    rows.iter()
        .filter(|row| row.get("kind").and_then(Value::as_str) == Some("nous.dispatcher.tick"))
        .filter(|row| {
            row.get("state")
                .and_then(|state| state.get("status"))
                .and_then(Value::as_str)
                == Some("sleeping")
        })
        .filter_map(|row| {
            let seq = row.get("id")?.as_i64()? as u64;
            let due_at_ms = row.get("state")?.get("wakeAt")?.as_i64()? as u64;
            Some(TickRecord { seq, due_at_ms })
        })
        .max_by_key(|record| record.seq)
}

/// 一次 tick 的两态：真跑（记心跳 + 求值）或忙则跳过（只记 skipped，不排队）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickDecision {
    Run { now_ms: u64 },
    SkippedBusy { now_ms: u64 },
}

/// 纯函数：上一拍没完成就跳过本拍，不排队、不叠加。
pub fn decide_tick(previous_running: bool, now_ms: u64) -> TickDecision {
    if previous_running {
        TickDecision::SkippedBusy { now_ms }
    } else {
        TickDecision::Run { now_ms }
    }
}

/// 调度健康：跟 `nous_store::scheduler_lag` 同口径，但节拍长度由调用方注入，
/// 这样假 Clock 的短 tick 测试不用改产品常量。
pub fn scheduler_lag_with_tick_ms(now_ms: u64, last_tick_at: Option<u64>, tick_ms: u64) -> u64 {
    match last_tick_at {
        None => 0,
        Some(last) => now_ms.saturating_sub(last).saturating_sub(tick_ms),
    }
}

/// 一条 `sleeping` 任务的 record：形状照 `TaskRecord`，状态写在 `state`（durable 不解释）。
fn sleeping_tick_record(id: i64, seq: u64, due_at_ms: u64) -> Value {
    json!({
        "id": id,
        "conversationId": 0,
        "kind": "nous.dispatcher.tick",
        "version": 1,
        "input": { "seq": seq, "wakeAt": due_at_ms },
        "background": true,
        "abortRequested": false,
        "state": { "status": "sleeping", "wakeAt": due_at_ms },
    })
}

/// 等下一拍，然后把待办写进 durable 的 `tasks` 表。
///
/// **不直接读时钟**：`now_ms = clock.now_ms()` 是唯一入口；等待也由 `clock.wait_until` 做。
/// 返回值是这批写占用的 task id，调用方据此读回同一行（库是唯一真相）。
pub async fn arm_next_tick(
    session: &Session,
    clock: &Arc<dyn Clock>,
    seq: u64,
    period_ms: u64,
) -> Result<i64, String> {
    // 醒来之后再看一眼时钟：手动时钟可能一次跳过多拍，待办要贴着真实到达的时刻写。
    let at_ms = clock.now_ms() + period_ms;
    let committed = session
        .commit(
            vec![StorageWrite::Task(sleeping_tick_record(
                seq as i64,
                seq,
                at_ms + period_ms,
            ))],
            CommitOptions::default(),
        )
        .await
        .map_err(|error| error.to_string())?;
    if committed.stored == 0 {
        return Err(format!("tick task commit refused: {:?}", committed.refused));
    }
    Ok(seq as i64)
}

/// 给判据的 SQL 读口：直接从 durable 库扫 `kind = nous.dispatcher.tick` 的 sleeping 行。
pub fn read_sleeping_ticks(storage: &nous_durable::Storage) -> Result<Vec<Value>, String> {
    let rows = storage
        .store()
        .db()
        .all(
            "SELECT record FROM tasks WHERE status = 'sleeping' ORDER BY id",
            &[],
        )
        .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .filter_map(|row| row.get("record").and_then(|cell| match cell {
            Cell::Text(text) => serde_json::from_str(&text).ok(),
            _ => None,
        }))
        .filter(|value: &Value| value.get("kind").and_then(Value::as_str) == Some("nous.dispatcher.tick"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_tick_is_skipped_not_queued() {
        assert_eq!(decide_tick(true, 60_000), TickDecision::SkippedBusy { now_ms: 60_000 });
        assert_eq!(decide_tick(false, 60_000), TickDecision::Run { now_ms: 60_000 });
    }

    #[test]
    fn lag_uses_injected_tick_and_never_goes_negative() {
        assert_eq!(scheduler_lag_with_tick_ms(0, None, 60_000), 0);
        assert_eq!(scheduler_lag_with_tick_ms(60_000, Some(0), 60_000), 0);
        assert_eq!(scheduler_lag_with_tick_ms(180_000, Some(0), 60_000), 120_000);
        assert_eq!(scheduler_lag_with_tick_ms(10, Some(90), 60_000), 0);
    }

    #[test]
    fn tick_is_read_back_from_sleeping_rows_only() {
        let rows = vec![
            json!({"id": 1, "kind": "nous.dispatcher.tick", "state": {"status": "terminal"}}),
            json!({"id": 2, "kind": "pi.generation", "state": {"status": "sleeping", "wakeAt": 9}}),
            json!({"id": 3, "kind": "nous.dispatcher.tick", "state": {"status": "sleeping", "wakeAt": 60_000}}),
        ];
        assert_eq!(tick_from_rows(&rows), Some(TickRecord { seq: 3, due_at_ms: 60_000 }));
    }
}
