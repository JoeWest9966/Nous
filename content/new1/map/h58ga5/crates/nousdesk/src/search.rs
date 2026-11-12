//! 左栏会话搜索：命中判断、分组、高亮切片。
//!
//! 这里只有**逻辑**，不碰渲染 —— 能单测，改 UI 的时候不用跟着动。
//!
//! 跟真机上「搜什么」的约定：只搜**会话名**和**最后一条消息的预览**。
//! 不搜历史正文：那得把每条会话的 entries 全拉到本地，51 个会话就是 51 次请求，
//! 换来的只是「搜到一句不知道在哪的话」。真要做全文再单开一条路。

use chrono::{Local, NaiveDate, TimeZone as _};
use nous_engine::types::Session;

/// 命中的是哪一栏。行右侧那个小标签用（「名称」/「内容」/「名称+内容」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    Name,
    Preview,
    Both,
}

impl Where {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "名称",
            Self::Preview => "内容",
            Self::Both => "名称+内容",
        }
    }
}

/// 结果分组。**置顶跟时间无关** —— 只要置顶了，永远在最上面一组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Pinned,
    Today,
    Week,
    Older,
}

impl Group {
    /// 组头那行字。
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinned => "置顶",
            Self::Today => "今天",
            Self::Week => "7 天内",
            Self::Older => "更早",
        }
    }
}

/// 会话名 + 预览里找 `query`（大小写不敏感）。
///
/// 空查询返回 `None` —— 调用方据此走「原来的列表」，不做过滤也不分组，
/// 免得没搜的时候列表顺序突然变成一个不一样的东西。
pub fn hit(s: &Session, query: &str) -> Option<Where> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    let q = q.to_lowercase();
    let by_name = s.name.to_lowercase().contains(&q);
    let by_preview = s
        .last
        .as_ref()
        .and_then(|l| l.text.as_deref())
        .is_some_and(|t| t.to_lowercase().contains(&q));
    match (by_name, by_preview) {
        (true, true) => Some(Where::Both),
        (true, false) => Some(Where::Name),
        (false, true) => Some(Where::Preview),
        (false, false) => None,
    }
}

/// 分组：置顶 → 今天 → 7 天内 → 更早。
///
/// `today` 由调用方传进来（不是内部取 `Local::now()`），这样单测能固定住"今天"。
pub fn group_of(updated_at: Option<i64>, pinned: bool, today: NaiveDate) -> Group {
    if pinned {
        return Group::Pinned;
    }
    // `updated_at` 是 null 的时候是"不知道"，不是"很久以前" —— 但列表里总得有个位置，
    // 又不能冒充今天，所以落到「更早」。
    let Some(ms) = updated_at else {
        return Group::Older;
    };
    let Some(t) = Local.timestamp_millis_opt(ms).single() else {
        return Group::Older;
    };
    // 按**日期**差算，不按 24 小时：昨晚 23:50 和今早 00:10 差 20 分钟，但那是两天。
    let days = (today - t.date_naive()).num_days();
    match days {
        d if d <= 0 => Group::Today,
        1..=6 => Group::Week,
        _ => Group::Older,
    }
}

/// 文本里 `query` 命中的字节区间（左闭右开）。没有命中 / 空查询 → `None`。
///
/// 返回的是**原文**的下标，可以直接拿去切 `text`。
pub fn span(text: &str, query: &str) -> Option<(usize, usize)> {
    let q = query.trim();
    if q.is_empty() || text.is_empty() {
        return None;
    }
    // 小写之后再找：中文不变，英文能忽略大小写。`to_lowercase` 理论上会让极少数
    // 字符变长（土耳其语 İ 之类），所以下面切原文之前还要验一次是不是字符边界。
    let lower = text.to_lowercase();
    let at = lower.find(&q.to_lowercase())?;
    let end = at + q.len();
    // 下标必须落在原文的字符边界上，否则切片会 panic。
    if !text.is_char_boundary(at) || !text.is_char_boundary(end) {
        return None;
    }
    Some((at, end))
}

/// 把一段文本切成「没命中 / 命中 / 没命中」三段，给渲染层拼 TextRun 用。
///
/// 命中不了就整段一个元素 —— 调用方不用区别对待。
pub fn runs(text: &str, query: &str) -> Vec<(String, bool)> {
    let Some((at, end)) = span(text, query) else {
        return vec![(text.to_string(), false)];
    };
    let mut out = Vec::with_capacity(3);
    if at > 0 {
        out.push((text[..at].to_string(), false));
    }
    out.push((text[at..end].to_string(), true));
    if end < text.len() {
        out.push((text[end..].to_string(), false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 只造搜索关心的那几栏，其余走 serde 默认 —— 这样 `Session` 加了字段也不会把测试写死。
    fn sess(name: &str, preview: Option<&str>) -> Session {
        serde_json::from_value(json!({
            "id": 1,
            "name": name,
            "readOnly": false,
            "thinkingLevels": [],
            "skills": [],
            "last": preview.map(|t| json!({ "id": 1, "kind": "assistant", "text": t })),
        }))
        .expect("Session 形状变了？")
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 10).unwrap()
    }

    fn at_noon(days_ago: i64) -> Option<i64> {
        let d = today() - chrono::Duration::days(days_ago);
        Some(
            d.and_hms_opt(12, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis(),
        )
    }

    #[test]
    fn 空查询不算命中() {
        let s = sess("滚动条", Some("viewport_bounds"));
        assert_eq!(hit(&s, ""), None);
        assert_eq!(hit(&s, "   "), None);
    }

    #[test]
    fn 名字和预览分开认() {
        let s = sess("滚动条", Some("会话列表不再是一条线"));
        assert_eq!(hit(&s, "滚动"), Some(Where::Name));
        assert_eq!(hit(&s, "列表"), Some(Where::Preview));
        assert_eq!(hit(&s, "滚动条"), Some(Where::Name));
        assert_eq!(hit(&s, "没有这个词"), None);
    }

    #[test]
    fn 两边都命中算both() {
        let s = sess("会话搜索", Some("左栏 51 个会话了"));
        assert_eq!(hit(&s, "会话"), Some(Where::Both));
    }

    #[test]
    fn 英文不区分大小写() {
        let s = sess("SSE 心跳", None);
        assert_eq!(hit(&s, "sse"), Some(Where::Name));
        assert_eq!(hit(&s, "Sse"), Some(Where::Name));
    }

    #[test]
    fn 置顶压过时间() {
        assert_eq!(group_of(at_noon(30), true, today()), Group::Pinned);
    }

    #[test]
    fn 今天本周更早三档() {
        assert_eq!(group_of(at_noon(0), false, today()), Group::Today);
        assert_eq!(group_of(at_noon(3), false, today()), Group::Week);
        assert_eq!(group_of(at_noon(6), false, today()), Group::Week);
        assert_eq!(group_of(at_noon(7), false, today()), Group::Older);
    }

    #[test]
    fn 不知道时间落在更早() {
        assert_eq!(group_of(None, false, today()), Group::Older);
    }

    #[test]
    fn 切片只标命中的那一段() {
        assert_eq!(
            runs("会话搜索", "搜索"),
            vec![("会话".to_string(), false), ("搜索".to_string(), true)]
        );
    }

    #[test]
    fn 命中在开头或结尾时不产生空片段() {
        assert_eq!(
            runs("会话搜索", "会话"),
            vec![("会话".to_string(), true), ("搜索".to_string(), false)]
        );
        assert_eq!(
            runs("会话搜索", "搜索"),
            vec![("会话".to_string(), false), ("搜索".to_string(), true)]
        );
    }

    #[test]
    fn 没命中就整段一个元素() {
        assert_eq!(runs("滚动条", "zzz"), vec![("滚动条".to_string(), false)]);
    }

    #[test]
    fn 英文高亮保留原文大小写() {
        assert_eq!(
            runs("SSE 心跳", "sse"),
            vec![("SSE".to_string(), true), (" 心跳".to_string(), false)]
        );
    }
}
