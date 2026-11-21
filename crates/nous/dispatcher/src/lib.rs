//! #73 调度节拍：纯判定 + 注入时钟 + durable `sleeping` 任务。
//!
//! 这一层不认识 HTTP，也不在调度逻辑里读时钟：`Clock` 由装配方注入，时间只从
//! `now_ms` 参数进来。唯一的真实时钟实现 [`RealClock`] 给装配处注入，判据用 `ManualClock`。
//! 一次 tick 只让调用方执行一轮 `fire`，重复同一 tick 由 `fires` 的唯一键挡住。

mod evaluate;
mod scheduler;

pub use evaluate::{evaluate, reason_text, validate_params, Decision, Reason, SpecView, StateView};
pub use scheduler::{
    arm_next_tick, decide_tick, read_sleeping_ticks, scheduler_lag_with_tick_ms, tick_from_rows,
    interval_from_env, run_dispatcher, tick_history_count, RealClock, TickDecision, TickRecord,
    INTERVAL_ENV, SCHEDULER_TICK_MS,
};
