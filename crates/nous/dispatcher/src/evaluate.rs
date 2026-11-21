//! 调度判定纯函数：同一份 `(spec, now, state)` 两次调用结果必须相同。
//!
//! `spec` 是触发器自己的规则（`schedule` / `on` / `params`），`state` 是调用方带来的
//! 外部事实（是否忙、上拍是否已完成）。求值不读时钟、不碰库、不 spawn 进程。

use nous_store::parse_five_field_cron;
use serde_json::Value;

/// 触发器规则里本判定会看的那部分。`params` 不参与点火判定，原样透传给调用方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecView {
    pub enabled: bool,
    /// 五段 cron；`None` 表示这条触发器不由时钟点火。
    pub schedule: Option<String>,
    /// `on` 是事件源的 slug；时钟调度只认 `cron` / `clock`。
    pub on: Option<String>,
}

/// 判定时带进来的外部事实。纯函数不自己查任何一处真相。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StateView {
    /// 上一轮还没结束：按串行策略跳过这一拍，不排队。
    pub busy: bool,
    /// 这条规则能不能由时钟点火（许可名单里有 `cron` / `clock`）。
    pub clock_allowed: bool,
}

/// 为什么这么判。`fire` / `skip` 之外都带一句人话，写进 `lastError`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    // 正判定：消息给人看的，不拿 slug 分支。
    Schedule,
    /// `on` 指定的是别的事件源，不是时钟。
    NotClock,
    Disabled,
    /// 上拍没完，跳过而不排队。
    Busy,
    /// 没有 schedule，也没有时钟事件源。
    NoSchedule,
    /// 许可名单里没有时钟。
    NotAllowed,
    /// 外部事实不全（调用方没带来判定需要的东西）。
    MissingState,
    /// cron 写坏了；调用方把 `detail` 记到 `lastError`，其余触发器照跑。
    BadSchedule { detail: String },
}

/// 一次判定的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub fire: bool,
    pub reason: Reason,
}

impl Decision {
    fn fire(reason: Reason) -> Self {
        Self { fire: true, reason }
    }

    fn no(reason: Reason) -> Self {
        Self { fire: false, reason }
    }
}

/// 纯判定：不读时钟、不碰库、不产生副作用。
///
/// `now_ms` 是唯一的时间来源，调用方从注入的 `Clock` 取。顺序写死：
/// 先看 enabled/许可，再看 busy（忙就记 `busy`，不排队），最后才判 cron —— 这样
/// 「上一拍没完」不会因为 cron 恰好不命中而被记成别的理由。
pub fn evaluate(spec: &SpecView, now_ms: i64, state: &StateView) -> Decision {
    if !spec.enabled {
        return Decision::no(Reason::Disabled);
    }
    if !state.clock_allowed {
        return Decision::no(Reason::NotAllowed);
    }
    if state.busy {
        return Decision::no(Reason::Busy);
    }
    let cron = match spec.schedule.as_deref() {
        Some(schedule) => match parse_five_field_cron(schedule) {
            Ok(cron) => cron,
            Err(error) => {
                return Decision::no(Reason::BadSchedule { detail: error.slug().to_string() })
            }
        },
        None => {
            return match spec.on.as_deref() {
                Some(on) if on == "cron" || on == "clock" => Decision::fire(Reason::Schedule),
                Some(_) => Decision::no(Reason::NotClock),
                None => Decision::no(Reason::NoSchedule),
            }
        }
    };
    if cron.matches_ms(now_ms) {
        Decision::fire(Reason::Schedule)
    } else {
        Decision::no(Reason::NoSchedule)
    }
}

/// 判定失败的人话文案：写进触发器的 `lastError`，不给第二份规则真相。
pub fn reason_text(reason: &Reason) -> String {
    match reason {
        Reason::Schedule => "到点".to_string(),
        Reason::NotClock => "事件源不是时钟".to_string(),
        Reason::Disabled => "触发器已停用".to_string(),
        Reason::Busy => "上一拍还没完，跳过本拍".to_string(),
        Reason::NoSchedule => "这一拍没有到点".to_string(),
        Reason::NotAllowed => "点火许可里没有时钟".to_string(),
        Reason::MissingState => "判定所需的状态缺失".to_string(),
        Reason::BadSchedule { detail } => format!("schedule 写坏了：{detail}"),
    }
}

/// `params` 只做形状校验（字符串 / 数字 / 布尔），不解释语义：
/// 求值不依赖它，但 PATCH 写盘时不能把数组或对象放进来。
pub fn validate_params(params: &Value) -> bool {
    params.as_object().is_some_and(|object| {
        object.values().all(|value| {
            value.is_string() || value.is_number() || value.is_boolean() || value.is_null()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn every_minute() -> SpecView {
        SpecView {
            enabled: true,
            schedule: Some("* * * * *".to_string()),
            on: None,
        }
    }

    #[test]
    fn same_spec_now_and_state_always_yield_the_same_decision() {
        let spec = every_minute();
        let now = 1_760_000_000_000;
        let state = StateView { busy: false, clock_allowed: true };
        assert_eq!(evaluate(&spec, now, &state), evaluate(&spec, now, &state));
        assert!(evaluate(&spec, now, &state).fire);
    }

    #[test]
    fn disabled_or_busy_or_unauthorized_never_fires() {
        let mut spec = every_minute();
        let state = StateView { busy: false, clock_allowed: true };
        spec.enabled = false;
        assert_eq!(evaluate(&spec, 0, &state).reason, Reason::Disabled);
        spec.enabled = true;
        let busy = StateView { busy: true, clock_allowed: true };
        assert_eq!(evaluate(&spec, 0, &busy).reason, Reason::Busy);
        let denied = StateView { busy: false, clock_allowed: false };
        assert_eq!(evaluate(&spec, 0, &denied).reason, Reason::NotAllowed);
    }

    #[test]
    fn bad_schedule_is_a_decision_not_a_panic() {
        let spec = SpecView {
            enabled: true,
            schedule: Some("not a cron".to_string()),
            on: None,
        };
        let decision = evaluate(&spec, 0, &StateView { busy: false, clock_allowed: true });
        assert!(!decision.fire);
        assert!(matches!(decision.reason, Reason::BadSchedule { .. }));
        assert!(reason_text(&decision.reason).contains("schedule"));
    }

    #[test]
    fn params_accept_only_scalar_values() {
        assert!(validate_params(&json!({"limit": 3, "tag": "x", "dry": true})));
        assert!(!validate_params(&json!({"nested": {"a": 1}})));
        assert!(!validate_params(&json!([])));
    }
}
