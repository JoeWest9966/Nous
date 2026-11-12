//! 会话内跳转的**纯逻辑**：把一会话的 entries 切成「一轮一轮」，再按天分组。
//!
//! 这一层不碰 GPUI、不碰 HTTP，只吃 `Entry` + 引擎 snapshot 里带来的时间戳，
//! 所以能拿数字直接钉住单测。面板块那条左列叫「目录，不是清单」——
//! 这里也一样：**不产生任何状态、不存盘、不写回引擎**。
//!
//! 「一轮」= 一条 `pi.user` 到下一个 `pi.user` 之间夹的所有东西（回答、工具输出）。
//! 时间戳来自 SSE 首帧 `snapshot.entries[].model[].timestamp`；引擎没给就是 `None`，
//! 一律归到「更早」，**不拿文件时间、不拿当天日期冒充**。

use std::collections::HashMap;

use chrono::{Datelike as _, Local, TimeZone as _};
use nous_engine::types::Entry;

/// 目录里的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavRound {
    /// 第几轮（目录序号 - 1）。
    pub index: usize,
    /// 这轮的用户消息在 `entries` 里的下标（已翻正，旧的在前面）。
    pub entry_index: usize,
    /// 用户问了什么（已压成一行）。
    pub text: String,
    /// 引擎给的时刻，epoch ms。**没有就是没有**。
    pub at: Option<i64>,
    /// 这一轮里 `pi.tool-result` 的条数。
    pub tools: usize,
    /// 这一轮里回答含代码围栏的条数。
    pub codes: usize,
    /// 这一轮在读水位之后（引擎没给水位就不标）。
    pub after_read: bool,
}

/// 一组（一天，或者「更早」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavDay {
    pub label: String,
    pub rounds: Vec<NavRound>,
}

/// 主视图留最近几条。**这是 9 号方案的根本**：聊天区永远短。
pub const KEEP: usize = 12;

/// 主视图从哪个 entry 开始。
pub fn main_from(len: usize) -> usize {
    len.saturating_sub(KEEP)
}

/// 把「你问过的」按天分组。
///
/// `times` 是「entry id → epoch ms」，来自 SSE snapshot；查不到的就是 `None`。
/// `read_at` / `has_unread` 决定要不要画红点：**没未读就一个都不标**，
/// 水位的真相在引擎那边（见 `UnreadState`），客户端不数数。
pub fn build_nav(
    entries: &[Entry],
    times: &HashMap<String, i64>,
    read_at: Option<i64>,
    has_unread: bool,
) -> Vec<NavDay> {
    let mut rounds: Vec<NavRound> = Vec::new();
    let mut cur: Option<NavRound> = None;

    for (i, e) in entries.iter().enumerate() {
        if e.kind == "pi.user" {
            if let Some(done) = cur.take() {
                rounds.push(done);
            }
            let id = entry_key(&e.id);
            let at = times.get(&id).copied();
            let text = one_line(e.text.as_deref().unwrap_or(""));
            // 空正文的用户消息也占一轮（引擎会给「有 entry 没内容」，坑.md 7）——
            // 目录里显示成「（空消息）」，别让它凭空消失。
            cur = Some(NavRound {
                index: rounds.len(),
                entry_index: i,
                text: if text.is_empty() { "（空消息）".into() } else { text },
                at,
                tools: 0,
                codes: 0,
                after_read: has_unread
                    && read_at.is_some()
                    && at.is_some_and(|t| t > read_at.unwrap_or(i64::MAX)),
            });
            continue;
        }
        if let Some(r) = cur.as_mut() {
            if e.kind == "pi.tool-result" {
                r.tools += 1;
            } else if e.text.as_deref().is_some_and(|t| t.contains("```")) {
                r.codes += 1;
            }
        }
    }
    if let Some(done) = cur {
        rounds.push(done);
    }

    // 按时间顺序分组，相邻的同Label合成一组；「更早」永远排最前。
    let now = Local::now().timestamp_millis();
    let mut out: Vec<NavDay> = Vec::new();
    let mut earlier: Vec<NavRound> = Vec::new();
    for r in rounds {
        match r.at {
            None => earlier.push(r),
            Some(at) => {
                let label = day_label(Some(at), now);
                match out.last_mut() {
                    Some(g) if g.label == label => g.rounds.push(r),
                    _ => out.push(NavDay {
                        label,
                        rounds: vec![r],
                    }),
                }
            }
        }
    }
    if !earlier.is_empty() {
        out.insert(0, NavDay {
            label: "更早".into(),
            rounds: earlier,
        });
    }
    out
}

/// 这一刻在哪一组。`None` = 「更早」，**不是**今天。
pub fn day_label(at: Option<i64>, now: i64) -> String {
    let Some(ms) = at else { return "更早".into() };
    let (Some(t), Some(n)) = (
        Local.timestamp_millis_opt(ms).single(),
        Local.timestamp_millis_opt(now).single(),
    ) else {
        return "更早".into();
    };
    let days = (n.date_naive() - t.date_naive()).num_days();
    match days {
        0 => "今天".into(),
        1 => "昨天".into(),
        d if (2..7).contains(&d) => format!("{d} 天前"),
        _ if t.year() == n.year() => format!("{}月{}日", t.month(), t.day()),
        _ => format!("{}/{}/{}", t.year(), t.month(), t.day()),
    }
}

/// 目录里两行截断的话。换行/多余空白压成一个空格。
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect()
}

/// entry id 的形状是 `serde_json::Value`（引擎有数字也有字符串的可能），统一成字符串。
pub fn entry_key(id: &serde_json::Value) -> String {
    match id {
        serde_json::Value::String(s) => s.clone(),
        _ => id.to_string(),
    }
}

/// 浮窗里给的一行摘要：这轮的第一句回答。
pub fn answer_preview(entries: &[Entry], round: &NavRound, groups: &[NavDay]) -> Option<String> {
    let end = groups
        .iter()
        .flat_map(|g| g.rounds.iter())
        .find(|r| r.index == round.index + 1)
        .map(|r| r.entry_index)
        .unwrap_or(entries.len());
    let text = entries[round.entry_index..end]
        .iter()
        .find(|e| e.kind == "pi.assistant")
        .and_then(|e| e.text.as_deref())
        .unwrap_or("");
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.is_empty() {
        None
    } else {
        Some(one_line(strip_md(line)))
    }
}

/// 预览只要纯文字：粗体星号、行内反引号、列表前缀都去掉。
fn strip_md(s: &str) -> &str {
    let s = s.trim_start_matches(['#', '>', '-', '*', ' ']);
    let s = s.trim_end_matches('*');
    s
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use serde_json::json;

    fn e(id: i64, kind: &str, text: &str) -> Entry {
        Entry {
            id: json!(id),
            kind: kind.into(),
            text: if text.is_empty() { None } else { Some(text.into()) },
            parent: None,
            extra: HashMap::new(),
        }
    }

    /// 三轮、两个工具、一个代码块、一条没时间 —— 切得对不对。
    #[test]
    fn 轮次按_user_切_标记按内容数() {
        let es = vec![
            e(1, "pi.user", "第一句"),
            e(2, "pi.assistant", "回答"),
            e(3, "pi.tool-result", "输出"),
            e(4, "pi.user", "第二句\n带换行"),
            e(5, "pi.assistant", "```rust\nfn a(){}\n```"),
            e(6, "pi.user", "第三句"),
        ];
        let times = HashMap::from([("1".into(), 1_700_000_000_000), ("4".into(), 1_700_000_000_001)]);
        let days = build_nav(&es, &times, Some(1_700_000_000_000), true);
        // 没时间的第三轮被排到最前（「更早」组），所以按轮次号找，不按下标。
        let mut all: Vec<_> = days.iter().flat_map(|g| g.rounds.iter()).collect();
        all.sort_by_key(|r| r.index);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].text, "第一句");
        assert_eq!(all[0].tools, 1);
        assert_eq!(all[1].text, "第二句 带换行");
        assert_eq!(all[1].codes, 1);
        assert_eq!(all[2].at, None);
    }

    /// 没给时间戳的归「更早」，且排最前 —— **不许算成今天**。
    #[test]
    fn 没时间就归更早_不编日期() {
        let now = Local::now().timestamp_millis();
        let es = vec![e(1, "pi.user", "老"), e(2, "pi.user", "新")];
        let times = HashMap::from([("2".into(), now)]);
        let days = build_nav(&es, &times, None, false);
        assert_eq!(days[0].label, "更早");
        assert_eq!(days[1].label, "今天");
        assert_eq!(day_label(None, now), "更早");
        assert_eq!(day_label(Some(now - 86_400_000), now), "昨天");
    }

    /// 没有未读就一个红点都不标；水位是引擎给的，客户端不数数。
    #[test]
    fn 没未读不标红点() {
        let es = vec![e(1, "pi.user", "问"), e(2, "pi.assistant", "答")];
        let times = HashMap::from([("1".into(), 1_700_000_000_000)]);
        let a = build_nav(&es, &times, Some(1), false);
        assert!(a.iter().all(|g| g.rounds.iter().all(|r| !r.after_read)));
        let b = build_nav(&es, &times, Some(1), true);
        assert!(b[0].rounds[0].after_read);
        let c = build_nav(&es, &times, None, true);
        assert!(!c[0].rounds[0].after_read, "没水位就别猜");
    }

    /// 主视图永远只留最近 KEEP 条。
    #[test]
    fn 主视图起点() {
        assert_eq!(main_from(3), 0);
        assert_eq!(main_from(12), 0);
        assert_eq!(main_from(96), 84);
    }

    #[test]
    fn 空正文的用户消息仍占一轮() {
        let es = vec![e(1, "pi.user", ""), e(2, "pi.assistant", "答")];
        let days = build_nav(&es, &HashMap::new(), None, false);
        assert_eq!(days[0].rounds[0].text, "（空消息）");
    }

    #[test]
    fn 浮窗摘要取回答首行() {
        let es = vec![
            e(1, "pi.user", "问"),
            e(2, "pi.assistant", "**结论**：能上。\n第二行"),
            e(3, "pi.user", "再问"),
        ];
        let days = build_nav(&es, &HashMap::new(), None, false);
        let mut all: Vec<_> = days.iter().flat_map(|g| g.rounds.iter()).collect();
        all.sort_by_key(|r| r.index);
        let p = answer_preview(&es, all[0], &days).unwrap();
        assert!(p.contains("结论"), "{p:?}");
    }
}
