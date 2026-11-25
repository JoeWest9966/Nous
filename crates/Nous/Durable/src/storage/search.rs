//! 搜索投影：会话名 + entry 正文 + 文档内容 → `wire.md` 的 `{items,total,truncated}`。
//!
//! 这是 kernel 的**纯投影**：输入是只读 `ImportedStore`，不认路由、不认时钟。
//! 服务端一次扫描完成，不要求客户端逐个会话拉历史。

use serde::Serialize;
use serde_json::Value;

use super::import::ImportedStore;
use super::sessions::{
    entry_text, integer, latest_base_content, record_of, text, LEGACY_NAMES_DOCUMENT_KIND,
};
use crate::error::Result;

/// 搜索结果的一条。字段与 `wire.md` 的搜索端点一一对应。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub kind: &'static str,
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub at: Value,
}

/// 命中上下文：前后各 40 个字符（`wire.md` 的搜索契约）。
const CONTEXT_CHARS: usize = 40;

/// 在一份只读库里搜会话正文。
///
/// `query` 由调用方夹过（`trim`、至少两个字符）；这一步不再做参数策略。
pub fn search_sessions(store: &ImportedStore, query: &str) -> Result<Vec<SearchItem>> {
    let needle = query.to_lowercase();
    let mut items = Vec::new();
    for row in &store.conversations {
        let id = integer(row, "id")?;
        let _record = record_of(row)?;
        let (name, updated_at) = legacy_name_and_time(store, id)?;
        let mut found: Option<String> = None;
        if contains_fold(&name, &needle) {
            found = Some(snippet(&name, &needle));
        }
        if found.is_none() {
            for entry_row in &store.entries {
                if integer(entry_row, "conversation_id")? != id {
                    continue;
                }
                let body = entry_text(&record_of(entry_row)?);
                if contains_fold(&body, &needle) {
                    found = Some(snippet(&body, &needle));
                    break;
                }
            }
        }
        if let Some(snippet) = found {
            let title = if name.is_empty() { format!("会话 {id}") } else { name };
            items.push(SearchItem {
                kind: "sessions",
                id: id.to_string(),
                title,
                snippet,
                at: updated_at,
            });
        }
    }
    Ok(items)
}

/// 在 Nous 自己的文档里搜名字（`kind/section/name` 三件）与正文。
pub fn search_documents(store: &ImportedStore, query: &str) -> Result<Vec<SearchItem>> {
    let needle = query.to_lowercase();
    let mut items = Vec::new();
    for row in &store.documents {
        let kind = super::sqlite::rows::decode_indexed_string(text(row, "kind")?)?;
        let name = super::sqlite::rows::decode_indexed_string(text(row, "key_value")?)?;
        let body = latest_base_content(store, integer(row, "id")?)?
            .as_ref()
            .map(value_text)
            .unwrap_or_default();
        let address = [kind.as_str(), name.as_str()].join(" / ");
        let from_body = contains_fold(&body, &needle);
        if from_body || contains_fold(&address, &needle) {
            let snippet = if from_body {
                snippet(&body, &needle)
            } else {
                // 正文没命中时，取包含命中词的那个地址分量，别让无关分量挤掉上下文。
                [kind.as_str(), name.as_str()]
                    .into_iter()
                    .find(|part| contains_fold(part, &needle))
                    .map(|part| snippet(part, &needle))
                    .unwrap_or_else(|| snippet(&address, &needle))
            };
            items.push(SearchItem {
                kind: "documents",
                id: format!("{kind}/{name}"),
                title: name.clone(),
                snippet,
                at: Value::Null,
            });
        }
    }
    Ok(items)
}

/// 一个会话使用了哪些扩展 / 技能：从会话记录的 `extensions` / `skills` 字段读。
pub fn usage(store: &ImportedStore, field: &str) -> Result<Value> {
    let mut map: serde_json::Map<String, Value> = serde_json::Map::new();
    for row in &store.conversations {
        let id = integer(row, "id")?;
        let record = record_of(row)?;
        let Some(names) = record.get(field).and_then(Value::as_array) else {
            continue;
        };
        for name in names.iter().filter_map(Value::as_str) {
            map.entry(name.to_string())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("usage entries are always arrays")
                .push(Value::from(id));
        }
    }
    Ok(Value::Object(map))
}

/// 会话名住在 `qrd3wh.sessions.names`（见 `sessions.rs` 的导入期说明）；本库自己的会话
/// 记录不重复存名字。`updatedAt` 旧库无来源，诚实给 `null`。
fn legacy_name_and_time(store: &ImportedStore, id: i64) -> Result<(String, Value)> {
    for row in &store.documents {
        let kind = super::sqlite::rows::decode_indexed_string(text(row, "kind")?)?;
        if kind != LEGACY_NAMES_DOCUMENT_KIND {
            continue;
        }
        if let Some(names) = latest_base_content(store, integer(row, "id")?)? {
            if let Some(name) = names.get(id.to_string()).and_then(Value::as_str) {
                return Ok((name.to_string(), Value::Null));
            }
        }
    }
    Ok((String::new(), Value::Null))
}

fn contains_fold(haystack: &str, lowercase_needle: &str) -> bool {
    haystack.to_lowercase().contains(lowercase_needle)
}

/// 命中大小写不敏感；上下文按**字符**数，不按字节切，避免把中文切成半个字符。
fn snippet(haystack: &str, lowercase_needle: &str) -> String {
    let folded = haystack.to_lowercase();
    let Some(byte_at) = folded.find(lowercase_needle) else {
        return haystack.chars().take(CONTEXT_CHARS * 2 + lowercase_needle.chars().count()).collect();
    };
    let hit_at = haystack[..byte_at].chars().count();
    let hit_chars = lowercase_needle.chars().count();
    let from = hit_at.saturating_sub(CONTEXT_CHARS);
    let to = (hit_at + hit_chars + CONTEXT_CHARS).min(haystack.chars().count());
    haystack.chars().skip(from).take(to - from).collect()
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(value_text)
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(map) => {
            // 文档名常作为 `name` 字段出现在正文 JSON 里。搜「正文」时它不是正文，
            // 否则判据 4 分不清 snippet 来自名字还是内容。
            map.iter()
                .filter(|(key, _)| !matches!(key.as_str(), "name" | "title" | "summary"))
                .map(|(_, value)| value_text(value))
                .collect::<Vec<_>>()
                .join(" ")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_keeps_forty_chars_on_each_side() {
        let text = format!("{}茄子{}", "前".repeat(40), "后".repeat(40));
        let got = snippet(&text, "茄子");
        assert_eq!(got, text);
    }

    #[test]
    fn contains_fold_is_case_insensitive() {
        assert!(contains_fold("HeLLo 世界", "hello"));
        assert!(!contains_fold("HeLLo 世界", "world"));
    }
}
