//! 会话树：`ImportStore` 的行 → `wire.md` 的 `GET /v1/sessions` 形状。
//!
//! ## 这一层认的是**旧引擎**的文档种类
//!
//! 名字和模型这两样在 fixture 里不住在会话记录里，而是住在文档里：
//!
//! | 想要的字段 | fixture 里的来源 | 说明 |
//! |---|---|---|
//! | `name` | `qrd3wh.sessions`（session 作用域）最新 base 的 `names` 表 | 旧引擎把全部名字放这张表里，key 是会话 id 的十进制文本 |
//! | `model` | `pi.agent`（conversation 作用域）最新 base 的 `model.provider` + `.modelId` | `"provider/modelId"` |
//! | `thinkingLevel` | 同上，`thinkingLevel` | |
//!
//! 这两个 kind 是**导入期**的知识，不是 Nous 的文档契约：Nous 自己的名字/模型走 12c 落地的
//! 文档（`pi.agent` / `pi.session`）。所以这里的常量带 `LEGACY_` 前缀，别在别处引用。
//!
//! ## 这一层**不**填的字段
//!
//! `thinkingLevels` / `skills` / `template` / `readOnly` / `archived` / `updatedAt` 在旧库里
//! 没有可读的来源：它们是生效值/墓碑，得等 12b（归档拆除）与 12c（文档契约）落地。
//! 这里给 `[]` / `null` / `false`，`wire.md` 允许这些值（`name` 给 `""` 也有明文规矩：
//! **本库不生成默认名**）。
//!
//! ## delta 不静默忽略
//!
//! 一份文档最新一条 revision 如果是 `delta`，说明有人写过增量 —— 本层还不会重放（12c），
//! 于是**明确失败**（[`StorageError::UndecodedDelta`]），不拿旧 base 冒充当前值。

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use super::import::ImportedStore;
use super::sqlite::rows::decode_indexed_string;
use crate::error::{Result, StorageError};

/// 旧引擎放会话名的文档种类（session 作用域）。
pub const LEGACY_NAMES_DOCUMENT_KIND: &str = "qrd3wh.sessions";
/// 放模型与思考档位的文档种类（conversation 作用域）。
pub const AGENT_DOCUMENT_KIND: &str = "pi.agent";
/// 放运行态（`generation.message` / `tools` …）的文档种类（conversation 作用域）。
/// 名称与 `harness::generation::LIVE_KIND` 一致；这一层是导入期的只读知识，不反向依赖 harness。
pub const LIVE_DOCUMENT_KIND: &str = "pi.live";

/// `GET /v1/sessions` 的一条。字段名照 `wire.md`（`camelCase`，`null` 照样输出）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionItem {
    pub id: i64,
    pub name: String,
    pub parent: Option<SessionParent>,
    pub owner: Option<SessionOwner>,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
    pub thinking_levels: Vec<String>,
    pub skills: Vec<String>,
    pub template: Option<String>,
    pub read_only: bool,
    pub busy: Option<String>,
    pub updated_at: Option<i64>,
    pub last: Option<SessionLast>,
    pub archived: Option<SessionArchived>,
    /// 置顶投影：从 `qrd3wh.sessions` 的 `pins.ids` 合并，不另存一份状态。
    pub pinned: bool,
}

/// `parent: {id, at}`：fork 来源与会话内的父 entry。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionParent {
    pub id: i64,
    pub at: i64,
}

/// `owner: {conversationId, taskId}`：创建者边（归属/子树中止用）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOwner {
    pub conversation_id: i64,
    pub task_id: i64,
}

/// `last: {id, kind, text}`：最后一条 entry 的摘要。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionLast {
    pub id: i64,
    pub kind: String,
    pub text: String,
}

/// `archived: {at, gen, purged}`：归档墓碑。本层永远是 `None`（12b 落地）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionArchived {
    pub at: i64,
    pub gen: String,
    pub purged: bool,
}

/// 把全部会话渲染成会话树，**新的在前**（`wire.md`）。
///
/// 没有模型编目时的入口：档位表诚实给 `[]`。
pub fn render_sessions(store: &ImportedStore) -> Result<Vec<SessionItem>> {
    render_sessions_with_levels(store, &BTreeMap::new())
}

/// 同上，但把 `device` 编目的档位表按 `"provider/modelId"` 注入。
///
/// durable 是 kernel，不依赖 app 层的 `nous-device`；编目只以纯数据传进来。
pub fn render_sessions_with_levels(
    store: &ImportedStore,
    model_levels: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<SessionItem>> {
    let (names, pins, archived) = legacy_session_state(store)?;
    let mut items = Vec::with_capacity(store.conversations.len());
    for row in &store.conversations {
        let id = integer(row, "id")?;
        let record: Value = record_of(row)?;
        let agent = agent_state(store, id)?;
        items.push(SessionItem {
            id,
            name: names.get(&id).cloned().unwrap_or_default(),
            parent: record.get("parent").and_then(|parent| {
                Some(SessionParent {
                    id: parent.get("conversationId")?.as_i64()?,
                    at: parent.get("at")?.as_i64()?,
                })
            }),
            owner: record.get("owner").and_then(|owner| {
                Some(SessionOwner {
                    conversation_id: owner.get("conversationId")?.as_i64()?,
                    task_id: owner.get("taskId")?.as_i64()?,
                })
            }),
            model: agent.model.clone(),
            thinking_level: agent.thinking_level,
            thinking_levels: agent.model.as_ref()
                .and_then(|model| model_levels.get(model))
                .cloned()
                .unwrap_or_default(),
            skills: Vec::new(),
            template: None,
            read_only: false,
            busy: busy(store, id)?,
            updated_at: latest_entry_timestamp(store, id)?,
            last: last_entry(store, id)?,
            // 墓碑是 qrd3wh.sessions 的 archived 键；没有键就是 null。
            archived: archived.get(&id).cloned(),
            pinned: pins.iter().any(|pin| pin.parse::<i64>() == Ok(id)),
        });
    }
    // 新的在前。
    items.sort_by(|left, right| right.id.cmp(&left.id));
    // 置顶按 `pins.ids` 的先后顺序；未置顶保持原顺序（新的在前）。
    items.sort_by_key(|item| {
        pins.iter()
            .position(|pin| pin.parse::<i64>() == Ok(item.id))
            .unwrap_or(pins.len())
    });
    Ok(items)
}

/// `GET /v1/sessions/:id/entries` 的形状，**新的在前**。
///
/// `{id, kind, text, parent, model}`：
///
/// - `id` / `kind` / `text` / `parent` 是 `wire.md` 点名依赖的四个；`parent` 取 entry 记录里的
///   `head`（entry 的父指针，和会话树的 `parent` 不是一回事）。
/// - `model` 原样透传（**不裁**）：`/v1/entries` 不给时间，`snapshot.entries[].model[].timestamp`
///   是 SSE 客户端唯一的时间源（`wire.md`）。裁掉它，客户端就没法按天分组。
///
/// 没有分页：`#22` 的 snapshot 要整份历史；`?limit=` 的游标留给 `/v1/entries` 那张票。
pub fn render_entries(store: &ImportedStore, conversation_id: i64) -> Result<Vec<Value>> {
    Ok(entry_items(store, conversation_id)?
        .into_iter()
        .map(|(_, item)| item)
        .collect())
}

/// `GET /v1/sessions/:id/entries` 的一页：`(items, next)`，新的在前。
///
/// `limit` 由调用方夹到 `1..=`[`ENTRIES_MAX_LIMIT`]；`cursor` 只带 `id` 与方向（不透明，
/// 客户端原样回传）。`next` 只在后面还有东西时出现。
pub fn render_entries_page(
    store: &ImportedStore,
    conversation_id: i64,
    limit: usize,
    cursor: Option<&crate::storage::cursor::Cursor>,
) -> Result<(Vec<Value>, Option<crate::storage::cursor::Cursor>)> {
    use crate::storage::cursor::{Cursor, ScanOrder};
    let after = cursor.map(|cursor| cursor.after).unwrap_or(i64::MAX);
    let mut items: Vec<(i64, Value)> = entry_items(store, conversation_id)?
        .into_iter()
        .filter(|(id, _)| *id < after)
        .collect();
    let next = if items.len() > limit {
        items.truncate(limit);
        items
            .last()
            .map(|(id, _)| Cursor::new(*id, ScanOrder::Descending))
    } else {
        None
    };
    Ok((items.into_iter().map(|(_, item)| item).collect(), next))
}

/// `wire.md` 的 `/v1/entries` 单条上限。超了**截断不报错**。
pub const ENTRIES_MAX_LIMIT: usize = 1000;

fn entry_items(store: &ImportedStore, conversation_id: i64) -> Result<Vec<(i64, Value)>> {
    let mut items: Vec<(i64, Value)> = Vec::new();
    for row in &store.entries {
        if integer(row, "conversation_id")? != conversation_id {
            continue;
        }
        let id = integer(row, "id")?;
        let record = record_of(row)?;
        let mut item = serde_json::Map::new();
        item.insert("id".into(), Value::from(id));
        item.insert(
            "kind".into(),
            Value::from(record.get("kind").and_then(Value::as_str).unwrap_or_default()),
        );
        item.insert("text".into(), Value::from(entry_text(&record)));
        item.insert(
            "parent".into(),
            record.get("head").cloned().unwrap_or(Value::Null),
        );
        if let Some(model) = record.get("model") {
            item.insert("model".into(), model.clone());
        }
        items.push((id, Value::Object(item)));
    }
    items.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(items)
}

/// `snapshot.generation.message`：会话里正在生成的那条消息（没有就是 `None`）。
///
/// 来源是 `pi.live` 文档（conversation 作用域）最新 base 的 `generation.message`。旧 fixture
/// 的 revision 全是 `base`；最新一条是 `delta` 时 [`latest_base_content`] 会明确失败（同
/// [`render_sessions`] 的规矩：不拿旧 base 冒充当前值）。
pub fn render_live_message(store: &ImportedStore, conversation_id: i64) -> Result<Option<Value>> {
    for row in &store.documents {
        let kind = decode_indexed_string(text(row, "kind")?)?;
        if kind != LIVE_DOCUMENT_KIND || integer(row, "owner_id")? != conversation_id {
            continue;
        }
        let Some(content) = latest_base_content(store, integer(row, "id")?)? else {
            continue;
        };
        let message = content
            .get("generation")
            .and_then(|generation| generation.get("message"))
            .cloned();
        if message.is_some() {
            return Ok(message);
        }
    }
    Ok(None)
}

/// 旧引擎的 `names` 表：会话 id → 名字，外加同一份文档里的置顶 id 顺序与归档墓碑。
fn legacy_session_state(
    store: &ImportedStore,
) -> Result<(BTreeMap<i64, String>, Vec<String>, BTreeMap<i64, SessionArchived>)> {
    let mut names = BTreeMap::new();
    let mut pins = Vec::new();
    let mut archived = BTreeMap::new();
    for row in &store.documents {
        let kind = decode_indexed_string(text(row, "kind")?)?;
        if kind != LEGACY_NAMES_DOCUMENT_KIND {
            continue;
        }
        let Some(content) = latest_base_content(store, integer(row, "id")?)? else {
            continue;
        };
        if let Some(map) = content.get("names").and_then(Value::as_object) {
            for (key, value) in map {
                if let (Ok(id), Some(name)) = (key.parse::<i64>(), value.as_str()) {
                    names.insert(id, name.to_string());
                }
            }
        }
        if let Some(ids) = content
            .get("pins")
            .and_then(|pins| pins.get("ids"))
            .and_then(Value::as_array)
        {
            pins.extend(ids.iter().filter_map(Value::as_str).map(str::to_string));
        }
        if let Some(tombstones) = content.get("archived").and_then(Value::as_object) {
            for (key, value) in tombstones {
                let Ok(id) = key.parse::<i64>() else { continue };
                let Some(at) = value.get("at").and_then(Value::as_i64) else { continue };
                let Some(gen) = value.get("gen").and_then(Value::as_str) else { continue };
                let Some(purged) = value.get("purged").and_then(Value::as_bool) else { continue };
                archived.insert(
                    id,
                    SessionArchived {
                        at,
                        gen: gen.to_string(),
                        purged,
                    },
                );
            }
        }
    }
    Ok((names, pins, archived))
}

/// 一份会话的 `pi.agent` 快照：模型与思考档位。
#[derive(Default)]
struct AgentState {
    model: Option<String>,
    thinking_level: Option<String>,
}

fn agent_state(store: &ImportedStore, conversation_id: i64) -> Result<AgentState> {
    for row in &store.documents {
        let kind = decode_indexed_string(text(row, "kind")?)?;
        if kind != AGENT_DOCUMENT_KIND || integer(row, "owner_id")? != conversation_id {
            continue;
        }
        let Some(content) = latest_base_content(store, integer(row, "id")?)? else {
            continue;
        };
        let model = content.get("model").and_then(|model| {
            let provider = model.get("provider")?.as_str()?;
            let model_id = model.get("modelId")?.as_str()?;
            Some(format!("{provider}/{model_id}"))
        });
        let thinking_level = content
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(str::to_string);
        return Ok(AgentState {
            model,
            thinking_level,
        });
    }
    Ok(AgentState::default())
}

/// 一篇文档最新一条 `base` revision 的内容。
///
/// 最新 revision 是 `delta` 时**明确失败**；没有 revision 时返回 `None`（文档还没写过值）。
pub(crate) fn latest_base_content(store: &ImportedStore, document_id: i64) -> Result<Option<Value>> {
    let mut newest: Option<(i64, &str, &str)> = None;
    for row in &store.document_revisions {
        if integer(row, "document_id")? != document_id {
            continue;
        }
        let seq = integer(row, "seq")?;
        let kind = text(row, "kind")?;
        let content = text(row, "content")?;
        match newest {
            Some((current, _, _)) if seq <= current => {}
            _ => newest = Some((seq, kind, content)),
        }
    }
    let Some((_, kind, content)) = newest else {
        return Ok(None);
    };
    if kind != "base" {
        return Err(StorageError::UndecodedDelta { document_id });
    }
    Ok(Some(serde_json::from_str(content)?))
}

/// 会话的活跃任务状态（`wire.md` 的 `busy`）；没有任何活跃任务就是 `None`。
fn busy(store: &ImportedStore, conversation_id: i64) -> Result<Option<String>> {
    let mut live: Option<(i64, String)> = None;
    for row in &store.tasks {
        if integer(row, "conversation_id")? != conversation_id {
            continue;
        }
        let status = text(row, "status")?;
        if status == "terminal" {
            continue;
        }
        let id = integer(row, "id")?;
        if live.as_ref().is_none_or(|(current, _)| id > *current) {
            live = Some((id, status.to_string()));
        }
    }
    Ok(live.map(|(_, status)| status))
}

/// `updatedAt`：该会话最新 entry 的 `model[0].timestamp`（epoch 毫秒）。
///
/// 时间源是 `entries[].record`（`wire.md` 声明的唯一时间源），不是文件 mtime、
/// 也不是进程时钟。没有 entry 就是 `None` —— 诚实空值，不填 0。
fn latest_entry_timestamp(store: &ImportedStore, conversation_id: i64) -> Result<Option<i64>> {
    let mut newest: Option<(i64, i64)> = None;
    for row in &store.entries {
        if integer(row, "conversation_id")? != conversation_id {
            continue;
        }
        let id = integer(row, "id")?;
        let record = record_of(row)?;
        let timestamp = record
            .get("model")
            .and_then(Value::as_array)
            .and_then(|model| model.first())
            .and_then(|message| message.get("timestamp"))
            .and_then(Value::as_i64);
        if let Some(timestamp) = timestamp {
            if newest.as_ref().is_none_or(|(current, _)| id > *current) {
                newest = Some((id, timestamp));
            }
        }
    }
    Ok(newest.map(|(_, timestamp)| timestamp))
}

/// 会话里 id 最大的那条 entry 的摘要。
fn last_entry(store: &ImportedStore, conversation_id: i64) -> Result<Option<SessionLast>> {
    let mut newest: Option<(i64, Value)> = None;
    for row in &store.entries {
        if integer(row, "conversation_id")? != conversation_id {
            continue;
        }
        let id = integer(row, "id")?;
        if newest.as_ref().is_none_or(|(current, _)| id > *current) {
            newest = Some((id, record_of(row)?));
        }
    }
    let Some((id, record)) = newest else {
        return Ok(None);
    };
    Ok(Some(SessionLast {
        id,
        kind: record
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        text: entry_text(&record),
    }))
}

/// entry 的摘要文本：最后一条 model 消息的文本；没有就空串。
///
/// `content` 既可能是字符串（user/system），也可能是段落数组（assistant 的 `[{type:"text",text}]`）。
pub(crate) fn entry_text(record: &Value) -> String {
    let Some(messages) = record.get("model").and_then(Value::as_array) else {
        return String::new();
    };
    let Some(content) = messages.last().and_then(|message| message.get("content")) else {
        return String::new();
    };
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

pub(crate) fn record_of(row: &nous_chord::sqlite::SqliteRow) -> Result<Value> {
    Ok(serde_json::from_str(text(row, "record")?)?)
}

pub(crate) fn integer(row: &nous_chord::sqlite::SqliteRow, column: &str) -> Result<i64> {
    row.get(column)
        .and_then(|value| value.as_i64())
        .ok_or_else(|| StorageError::Shape(format!("column {column} is missing or not an integer")))
}

pub(crate) fn text<'a>(row: &'a nous_chord::sqlite::SqliteRow, column: &str) -> Result<&'a str> {
    row.get(column)
        .and_then(|value| value.as_str())
        .ok_or_else(|| StorageError::Shape(format!("column {column} is missing or not text")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_chord::sqlite::{SqliteRow, SqliteValue as Cell};
    use serde_json::json;

    fn row(pairs: Vec<(&str, Cell)>) -> SqliteRow {
        pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
    }

    fn store_with_one_delta() -> ImportedStore {
        ImportedStore {
            schema_version: 1,
            next_id: "2".into(),
            next_seq: 1,
            record_ids: vec![],
            conversations: vec![row(vec![
                ("id", Cell::Integer(2)),
                ("owner_conversation_id", Cell::Integer(0)),
                ("owner_task_id", Cell::Integer(0)),
                ("record", Cell::Text("{\"id\":2}".into())),
            ])],
            entries: vec![],
            tasks: vec![],
            submissions: vec![],
            documents: vec![row(vec![
                ("id", Cell::Integer(3)),
                ("kind", Cell::Text("\"pi.agent\"".into())),
                ("family", Cell::Integer(0)),
                ("key_value", Cell::Text("\"\"".into())),
                ("scope_kind", Cell::Text("conversation".into())),
                ("owner_id", Cell::Integer(2)),
                ("created_at", Cell::Integer(1)),
                ("retired_at", Cell::Null),
                ("record", Cell::Text("{\"id\":3}".into())),
            ])],
            document_revisions: vec![
                row(vec![
                    ("document_id", Cell::Integer(3)),
                    ("seq", Cell::Integer(1)),
                    ("kind", Cell::Text("base".into())),
                    ("version", Cell::Integer(1)),
                    ("content", Cell::Text("{\"model\":{\"provider\":\"p\",\"modelId\":\"m\"}}".into())),
                ]),
                row(vec![
                    ("document_id", Cell::Integer(3)),
                    ("seq", Cell::Integer(2)),
                    ("kind", Cell::Text("delta".into())),
                    ("version", Cell::Integer(1)),
                    ("content", Cell::Text("[]".into())),
                ]),
            ],
        }
    }

    #[test]
    fn a_delta_after_the_newest_base_is_a_loud_failure_not_a_stale_value() {
        let error = render_sessions(&store_with_one_delta()).unwrap_err();
        assert_eq!(error, StorageError::UndecodedDelta { document_id: 3 });
    }

    #[test]
    fn a_session_without_entries_or_names_renders_wire_shaped_nulls_and_empty_name() {
        let mut store = store_with_one_delta();
        // 去掉那条 delta：base 就是最新。
        store.document_revisions.pop();
        let items = render_sessions(&store).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.id, 2);
        assert_eq!(item.name, "", "没命名就是空串，本库不造默认名");
        assert_eq!(item.model.as_deref(), Some("p/m"));
        assert_eq!(item.last, None);
        assert_eq!(item.busy, None);
        assert_eq!(item.archived, None);
        assert!(item.thinking_levels.is_empty() && item.skills.is_empty());
        assert_eq!(item.template, None);
        assert!(!item.read_only);
    }

    #[test]
    fn pins_are_merged_into_session_items_and_reordered_first() {
        let mut store = store_with_one_delta();
        // 同一份 `qrd3wh.sessions` 文档同时带 names 与 pins；新的 base 取代那条 delta。
        store.documents[0].insert("kind".into(), Cell::Text("\"qrd3wh.sessions\"".into()));
        store.document_revisions = vec![row(vec![
            ("document_id", Cell::Integer(3)),
            ("seq", Cell::Integer(3)),
            ("kind", Cell::Text("base".into())),
            ("version", Cell::Integer(1)),
            ("content", Cell::Text(
                r#"{"names":{"2":"named"},"pins":{"ids":["2"],"requests":{}}}"#.into()
            )),
        ])];
        store.conversations.insert(0, row(vec![
            ("id", Cell::Integer(1)),
            ("owner_conversation_id", Cell::Integer(0)),
            ("owner_task_id", Cell::Integer(0)),
            ("record", Cell::Text("{\"id\":1}".into())),
        ]));
        let items = render_sessions(&store).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, 2, "置顶的会话排最前");
        assert!(items[0].pinned, "置顶项 pinned=true");
        assert_eq!(items[0].name, "named", "同一份文档里的 names 不能被 pins 覆盖");
        assert_eq!(items[1].id, 1);
        assert!(!items[1].pinned, "没置顶的 pinned=false");
    }

    #[test]
    fn entry_text_reads_a_string_content_or_text_parts() {
        let string_content = json!({"model": [{"role": "user", "content": "收到"}]});
        assert_eq!(entry_text(&string_content), "收到");
        let parts = json!({"model": [{"role": "assistant", "content": [{"type": "text", "text": "收到"}]}]});
        assert_eq!(entry_text(&parts), "收到");
        assert_eq!(entry_text(&json!({"kind": "pi.system"})), "");
    }
}
