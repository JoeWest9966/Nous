//! `Conversation.watch()` 的窗口语义（照抄 `packages/durable/src/harness/view.ts` 的
//! `ConversationWindow` / `resolveWindow` / `advance`，不是自创的「给全部 entries」）。
//!
//! min 默认 50，max 默认 `2 * min`。初始窗口取**最新 min 条**（或 `from` 起最多 max 条）；
//! append 后一旦超过 max，就一次 splice 砍掉最旧的多余部分，剩下 min 条，
//! 并把砍掉的最后一条 id 放进 `before`。窗口只看 `entries`  id 有序的尾部。

use serde_json::Value;

/// 默认 min。上游 `DEFAULT_WINDOW_MIN = 50`。
pub const DEFAULT_MIN: usize = 50;

/// 默认 max 倍率：max = multiplier × min。上游 `2 * min`。
pub const DEFAULT_MAX_MULTIPLIER: usize = 2;

/// 一个观察者的窗口。上游 `ConversationWindow`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationWindow {
    /// 第一条 value 从哪里开始（含），是一个 entry ID（整数，0 合法）；只影响
    /// 第一次取值，之后的 append 照收。上游 `ConversationWindow.from`。
    pub from: Option<u64>,
    /// 窗口最少保留多少条；默认 50，整数且 >= 1。
    pub min: Option<usize>,
    /// 窗口最多保留多少条；默认 `2 * min`，整数且 >= min。
    pub max: Option<usize>,
}

impl Default for ConversationWindow {
    fn default() -> Self {
        Self {
            from: None,
            min: None,
            max: None,
        }
    }
}

/// 应用默认值之后的窗口边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowBounds {
    pub from: Option<u64>,
    pub min: usize,
    pub max: usize,
}

/// 窗口参数不合法。上游抛 `RangeError`；这里返回 `Err`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowError {
    MinOutOfRange(usize),
    MaxOutOfRange { min: usize, max: usize },
}

impl std::fmt::Display for WindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowError::MinOutOfRange(min) => {
                write!(f, "window min must be an integer >= 1, got {min}")
            }
            WindowError::MaxOutOfRange { min, max } => write!(
                f,
                "window max must be an integer >= min ({min}), got {max}"
            ),
        }
    }
}

impl std::error::Error for WindowError {}

/// 应用默认值并校验。上游 `resolveWindow`。
pub fn resolve_window(window: Option<&ConversationWindow>) -> Result<WindowBounds, WindowError> {
    let min = window.and_then(|w| w.min).unwrap_or(DEFAULT_MIN);
    if min == 0 {
        return Err(WindowError::MinOutOfRange(min));
    }
    let max = window.and_then(|w| w.max).unwrap_or_else(|| min * DEFAULT_MAX_MULTIPLIER);
    if max < min {
        return Err(WindowError::MaxOutOfRange { min, max });
    }
    Ok(WindowBounds {
        from: window.and_then(|w| w.from),
        min,
        max,
    })
}

/// 一个有窗口的会话视图。上游 `ConversationView` 中本库要的 `entries` / `before`；
/// `conversation` 与 `docs` 留作 `Value`，不在此冻结它们的结构。
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationView {
    pub conversation: Value,
    pub entries: Vec<Value>,
    pub before: Option<u64>,
    pub docs: serde_json::Map<String, Value>,
}

/// 把「最新 min 条」装进窗口。上游 `loadWindow` 的第二条路径（`from` 缺席时）。
///
/// `entries` 按 id 升序给出；`next_id` 是最后一条的 id（用来算 `before`）。
pub fn initial_window(
    entries_sorted_asc: &[Value],
    bounds: WindowBounds,
    conversation: Value,
    docs: serde_json::Map<String, Value>,
) -> ConversationView {
    let (before, window_entries) = if let Some(from) = bounds.from {
        let since: Vec<Value> = entries_sorted_asc
            .iter()
            .filter(|entry| entry_id(entry).is_some_and(|id| id >= from))
            .cloned()
            .collect();
        if since.len() <= bounds.max {
            let previous = entries_sorted_asc
                .iter()
                .rev()
                .find(|entry| entry_id(entry).is_some_and(|id| id < from));
            (previous.and_then(entry_id), since)
        } else {
            newest_min(entries_sorted_asc, bounds.min)
        }
    } else {
        newest_min(entries_sorted_asc, bounds.min)
    };
    ConversationView {
        conversation,
        entries: window_entries,
        before,
        docs,
    }
}

fn newest_min(entries_sorted_asc: &[Value], min: usize) -> (Option<u64>, Vec<Value>) {
    let start = entries_sorted_asc.len().saturating_sub(min);
    let before = if start > 0 {
        entry_id(&entries_sorted_asc[start - 1])
    } else {
        None
    };
    (before, entries_sorted_asc[start..].to_vec())
}

/// 从源拿到的一批追加（`WatchCall::Value` 的载荷）。
#[derive(Debug, Clone, PartialEq)]
pub struct Advance {
    /// 新追加的 entry，按 id 升序。
    pub appended: Vec<Value>,
}

/// 把一批 append 折进窗口，返回 `before` 是否变了。
///
/// 上游 `advance()` 里那段：`entries = entries.concat(appended)`，
/// 若 `entries.len() > max`，`drop = len - min`，`before = entries[drop-1].id`，
/// `entries = entries[drop..]`。一次 splice 砍回 min，不逐条丢。
pub fn apply_appends(view: &mut ConversationView, appended: &[Value], bounds: WindowBounds) -> bool {
    if appended.is_empty() {
        return false;
    }
    view.entries.extend_from_slice(appended);
    if view.entries.len() > bounds.max {
        let drop = view.entries.len() - bounds.min;
        view.before = entry_id(&view.entries[drop - 1]);
        view.entries.drain(..drop);
        return true;
    }
    false
}

/// entry 的 id。本库 records 用 camelCase 的 `id`，值是整数（上游 `EntryId` 是 number）。
pub fn entry_id(entry: &Value) -> Option<u64> {
    entry.get("id").and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entries() -> Vec<Value> {
        (1..=10)
            .map(|id| json!({ "id": id, "text": format!("e{id}") }))
            .collect()
    }

    #[test]
    fn defaults_are_fifty_and_one_hundred_not_all_entries() {
        let bounds = resolve_window(None).unwrap();
        assert_eq!(bounds.min, 50);
        assert_eq!(bounds.max, 100);
    }

    #[test]
    fn resolve_window_validates_like_upstream() {
        assert!(resolve_window(Some(&ConversationWindow {
            from: None,
            min: Some(0),
            ..Default::default()
        }))
        .is_err());
        assert!(resolve_window(Some(&ConversationWindow {
            from: None,
            min: Some(5),
            max: Some(4),
        }))
        .is_err());
        let bounds = resolve_window(Some(&ConversationWindow {
            min: Some(3),
            ..Default::default()
        }))
        .unwrap();
        assert_eq!((bounds.min, bounds.max), (3, 6));
    }

    #[test]
    fn initial_window_takes_the_newest_min_not_everything() {
        let bounds = resolve_window(Some(&ConversationWindow {
            min: Some(3),
            ..Default::default()
        }))
        .unwrap();
        let view = initial_window(&entries(), bounds, json!({}), Default::default());
        assert_eq!(view.entries.len(), 3);
        assert_eq!(view.before, Some(7));
        assert_eq!(entry_id(&view.entries[0]), Some(8));
    }

    #[test]
    fn from_selects_the_tail_from_that_id_when_at_most_max_entries_remain() {
        let bounds = resolve_window(Some(&ConversationWindow {
            from: Some(8),
            min: Some(2),
            max: Some(5),
        }))
        .unwrap();
        let view = initial_window(&entries(), bounds, json!({}), Default::default());
        assert_eq!(view.entries.len(), 3);
        assert_eq!(view.before, Some(7));
    }

    #[test]
    fn appends_beyond_max_drop_oldest_down_to_min_in_one_step() {
        let bounds = resolve_window(Some(&ConversationWindow {
            from: None,
            min: Some(2),
            max: Some(4),
        }))
        .unwrap();
        let mut view = initial_window(&entries(), bounds, json!({}), Default::default());
        assert_eq!(entry_id(&view.entries[0]), Some(9));
        let appended = vec![json!({"id": 11}), json!({"id": 12}), json!({"id": 13})];
        assert!(apply_appends(&mut view, &appended, bounds));
        // len=5 > max=4 => drop=3, 剩 min=2 条；before 是被砍掉的最后一条 (11)。
        assert_eq!(view.entries.len(), 2);
        assert_eq!(view.before, Some(11));
        assert_eq!(entry_id(&view.entries[0]), Some(12));
        assert_eq!(entry_id(&view.entries[1]), Some(13));
    }

    #[test]
    fn appends_within_max_keep_before_untouched() {
        let bounds = resolve_window(Some(&ConversationWindow {
            from: None,
            min: Some(2),
            max: Some(4),
        }))
        .unwrap();
        let mut view = initial_window(&entries(), bounds, json!({}), Default::default());
        let old_before = view.before;
        assert!(!apply_appends(&mut view, &[json!({"id": 11})], bounds));
        assert_eq!(view.before, old_before);
        assert_eq!(view.entries.len(), 3);
    }
}
