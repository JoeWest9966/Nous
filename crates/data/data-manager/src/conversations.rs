//! `GET /v1/stats/conversations` 与 `GET /v1/stats/conversation/:id` 的读口。
//!
//! 六屏里的「会话树」和「回放」：一条会话一行（entries / tokens / toolCalls / 起止），
//! 以及一条会话的明细（entries 全表 + tasks + documents + submissions + **prompt 重建**）。
//!
//! 形状对齐 TS 面板 `d6s4v1` 的 `conversationList` / `conversationDetail` / `promptAt`：
//! Desk 的 `StatsConvRow` / `StatsReplay` 当初就是照这些建的，复原就是让它们重新拿到同样的字段。
//! 明细一律现从 `conversations` / `entries` / `tasks` / `documents` / `submissions` 读，
//! **不落快照、不写库**。

use std::path::Path;

use nous_chord::sqlite::{SqliteExecutor, SqliteRow, SqliteValue};
use serde_json::{json, Value};

use crate::util;
use crate::Result;

/// 会话树里每会话最多扫多少条 entry 来算 token / 标题（面板同值）。
const ENTRIES_SCAN_LIMIT: i64 = 20_000;
/// 回放里一条会话最多带多少条 entry（面板同值）。
const REPLAY_LIMIT: i64 = 400;

/// 从一行 entries 读出 id / conv / seq / head / record。
fn read_entry(row: &SqliteRow, record: Value) -> Value {
    let id = row.get("id").and_then(SqliteValue::as_i64).unwrap_or(0);
    let conv = row
        .get("conversation_id")
        .and_then(SqliteValue::as_i64)
        .unwrap_or(0);
    let seq = row
        .get("commit_seq")
        .and_then(SqliteValue::as_i64)
        .unwrap_or(0);
    let head = row.get("head").and_then(SqliteValue::as_i64);
    let bytes = row
        .get("record")
        .and_then(SqliteValue::as_str)
        .map(|text| text.len() as i64)
        .unwrap_or(0);
    shape_value(id, conv, seq, head, bytes, record)
}

/// 一条 entry 的展示形状（对齐面板 `shapeEntry`）。
fn shape_value(id: i64, conv: i64, seq: i64, head: Option<i64>, bytes: i64, record: Value) -> Value {
    let model = record
        .get("model")
        .and_then(Value::as_array)
        .and_then(|items| items.first());
    let model_id = model
        .and_then(|m| m.get("model").or_else(|| m.get("modelId")))
        .and_then(Value::as_str);
    let kind = record.get("kind").and_then(Value::as_str).unwrap_or("");
    let role = model.and_then(|m| m.get("role")).and_then(Value::as_str).unwrap_or("");
    let content = model
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let text: String = content
        .iter()
        .filter_map(|part| {
            let ty = part.get("type").and_then(Value::as_str);
            if ty == Some("text") || ty == Some("thinking") {
                part.get("text")
                    .or_else(|| part.get("thinking"))
                    .and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let flat = match model.and_then(|m| m.get("content")) {
        Some(Value::String(text)) => text.clone(),
        _ => text,
    };

    let tool_calls: Vec<Value> = content
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("toolCall"))
        .map(|part| {
            json!({
                "id": part.get("id").and_then(Value::as_str).unwrap_or(""),
                "tool": part.get("name").and_then(Value::as_str).unwrap_or(""),
                "args": part.get("arguments").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    let usage = model.and_then(|m| m.get("usage"));
    let num = |value: Option<&Value>| {
        value
            .and_then(Value::as_i64)
            .or_else(|| value.and_then(Value::as_f64).map(|n| n as i64))
            .unwrap_or(0)
    };
    let usage_json = usage.map(|u| {
        json!({
            "input": num(u.get("input")),
            "output": num(u.get("output")),
            "cacheRead": num(u.get("cacheRead")),
            "cacheWrite": num(u.get("cacheWrite")),
            "reasoning": num(u.get("reasoning")),
            "totalTokens": num(u.get("totalTokens")),
            "cost": num(u.get("cost").and_then(|c| c.get("total"))),
        })
    });
    let tool_result = role == "toolResult";

    json!({
        "id": id,
        "conv": conv,
        "seq": seq,
        "head": head,
        "kind": kind,
        "bytes": bytes,
        "at": num(model.and_then(|m| m.get("timestamp"))),
        "role": role,
        "model": model_id,
        "provider": model.and_then(|m| m.get("provider")).and_then(Value::as_str),
        "tokens": num(usage.and_then(|u| u.get("totalTokens"))),
        "cost": num(usage.and_then(|u| u.get("cost")).and_then(|c| c.get("total"))),
        "usage": usage_json,
        "tool": if tool_result {
            model.and_then(|m| m.get("toolName")).and_then(Value::as_str).map(str::to_owned)
        } else {
            None
        },
        "toolCalls": tool_calls,
        "isError": model.and_then(|m| m.get("isError")).and_then(Value::as_bool).unwrap_or(false),
        "ms": model.and_then(|m| m.get("durationMs")).and_then(Value::as_i64),
        "stopReason": model.and_then(|m| m.get("stopReason")).and_then(Value::as_str),
        "toolsAdded": model
            .and_then(|m| m.get("toolsAdded"))
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(|item| item.get("name").and_then(Value::as_str)).collect::<Vec<_>>())
            .unwrap_or_default(),
        "chars": flat.chars().count() as i64,
        "text": clamp(&flat.split_whitespace().collect::<Vec<_>>().join(" "), 400),
    })
}

/// 读一条会话（或全部）的 entries 展示行。`limit=0` 表示不限制；只取倒序前 `limit` 条再翻正。
fn read_entries(db: &dyn SqliteExecutor, conv: Option<i64>, limit: i64) -> Vec<Value> {
    let (sql, bind): (String, Vec<SqliteValue>) = match conv {
        Some(id) => (
            format!(
                "SELECT id, conversation_id, head, commit_seq, record FROM entries \
                 WHERE conversation_id = ?1 {}",
                if limit > 0 { format!("ORDER BY id DESC LIMIT {limit}") } else { String::from("ORDER BY id") }
            ),
            vec![SqliteValue::Integer(id)],
        ),
        None => (
            format!(
                "SELECT id, conversation_id, head, commit_seq, record FROM entries \
                 ORDER BY id DESC {}",
                if limit > 0 { format!("LIMIT {limit}") } else { String::new() }
            ),
            Vec::new(),
        ),
    };
    let rows = db.all(&sql, &bind).unwrap_or_default();
    let mut items: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            let text = row.get("record").and_then(SqliteValue::as_str)?;
            let record = util::parse_record(text)?;
            Some(read_entry(row, record))
        })
        .collect();
    // 查询是倒序（新的在前），翻正成时间序（面板 `entriesOf` / `allEntries` 也是 `.reverse()`）。
    items.reverse();
    items
}

/// `GET /v1/stats/conversations`：会话树一行一条。
pub fn conversations(durable_db: &Path) -> Result<Value> {
    let db = util::open_read_only(durable_db)?;
    let rows = util::all(
        &db,
        "SELECT id, owner_conversation_id, owner_task_id FROM conversations ORDER BY id",
    )?;
    let entries = read_entries(&db, None, ENTRIES_SCAN_LIMIT);

    // conv id → 它的 entry 行。
    let mut by_conv: std::collections::BTreeMap<i64, Vec<&Value>> = std::collections::BTreeMap::new();
    for entry in &entries {
        let conv = entry.get("conv").and_then(Value::as_i64).unwrap_or(0);
        by_conv.entry(conv).or_default().push(entry);
    }

    let mut list = Vec::new();
    let mut totals_entries = 0i64;
    let mut totals_tokens = 0i64;
    let mut totals_cost = 0i64;
    for row in &rows {
        let id = util::col_i64(row, "id");
        let owner_conv = row
            .get("owner_conversation_id")
            .and_then(SqliteValue::as_i64);
        let owner_task = row.get("owner_task_id").and_then(SqliteValue::as_i64);
        let items = by_conv.get(&id).cloned().unwrap_or_default();

        let title = items
            .iter()
            .find(|entry| entry.get("role").and_then(Value::as_str) == Some("user"))
            .and_then(|entry| entry.get("text").and_then(Value::as_str))
            .map(|text| clamp(text, 80))
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "(没有用户输入)".to_string());
        let tokens: i64 = items.iter().map(|e| e.get("tokens").and_then(Value::as_i64).unwrap_or(0)).sum();
        let cost: i64 = items.iter().map(|e| e.get("cost").and_then(Value::as_i64).unwrap_or(0)).sum();
        let tool_calls: i64 = items
            .iter()
            .map(|e| e.get("toolCalls").and_then(Value::as_array).map(Vec::len).unwrap_or(0) as i64)
            .sum();
        let mut models: Vec<String> = items
            .iter()
            .filter_map(|e| e.get("model").and_then(Value::as_str).map(str::to_owned))
            .collect();
        models.sort();
        models.dedup();
        let first_at = items.first().and_then(|e| e.get("at").and_then(Value::as_i64)).unwrap_or(0);
        let last_at = items.last().and_then(|e| e.get("at").and_then(Value::as_i64)).unwrap_or(0);

        totals_entries += items.len() as i64;
        totals_tokens += tokens;
        totals_cost += cost;

        list.push(json!({
            "id": id,
            "ownerConv": owner_conv,
            "ownerTask": owner_task,
            "entries": items.len() as i64,
            "tokens": tokens,
            "cost": cost,
            "toolCalls": tool_calls,
            "models": models,
            "firstAt": first_at,
            "lastAt": last_at,
            "title": title,
        }));
    }

    Ok(json!({
        "count": list.len() as i64,
        "list": list,
        "totals": { "entries": totals_entries, "tokens": totals_tokens, "cost": totals_cost },
    }))
}

/// `GET /v1/stats/conversation/:id`：一条会话的明细，含 prompt 重建。
///
/// 会话不存在时返回 `Ok(None)`，端点层映射成 404（不假装空会话）。
pub fn conversation(durable_db: &Path, id: i64) -> Result<Option<Value>> {
    let db = util::open_read_only(durable_db)?;
    let head = util::get(
        &db,
        &format!(
            "SELECT id, owner_conversation_id, owner_task_id FROM conversations WHERE id = {id}"
        ),
    )?;
    let Some(head) = head else { return Ok(None) };
    let owner_conv = head.get("owner_conversation_id").and_then(SqliteValue::as_i64);
    let owner_task = head.get("owner_task_id").and_then(SqliteValue::as_i64);

    let entries = read_entries(&db, Some(id), REPLAY_LIMIT);

    // tasks：这一条会话自己的任务。
    let tasks: Vec<Value> = util::all(
        &db,
        &format!("SELECT id, kind, status, record FROM tasks WHERE conversation_id = {id} ORDER BY id"),
    )
    .unwrap_or_default()
    .iter()
    .map(|row| {
        let record = row
            .get("record")
            .and_then(SqliteValue::as_str)
            .and_then(util::parse_record)
            .unwrap_or(Value::Null);
        let started = record.get("startedAt").and_then(Value::as_i64).unwrap_or(0);
        let ended = record.get("endedAt").and_then(Value::as_i64);
        let outcome = record
            .get("state")
            .and_then(|state| state.get("outcome"))
            .and_then(|outcome| outcome.get("status"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        json!({
            "id": util::col_i64(row, "id"),
            "kind": util::unquote(&util::col_str(row, "kind")),
            "status": util::col_str(row, "status"),
            "startedAt": started,
            "endedAt": ended,
            "ms": ended.unwrap_or(0) - started,
            "conv": id,
            "outcome": outcome,
        })
    })
    .collect();

    // documents：会话作用域（带 revision 数与字节）。
    let documents: Vec<Value> = util::all(
        &db,
        &format!(
            "SELECT d.id, d.kind, d.key_value, d.created_at, d.retired_at, \
             (SELECT count(*) FROM document_revisions r WHERE r.document_id = d.id) AS revisions, \
             (SELECT coalesce(sum(length(r.content)), 0) FROM document_revisions r WHERE r.document_id = d.id) AS bytes \
             FROM documents d WHERE d.scope_kind = 'conversation' AND d.owner_id = {id} ORDER BY d.kind"
        ),
    )
    .unwrap_or_default()
    .iter()
    .map(|row| {
        json!({
            "id": util::col_i64(row, "id"),
            "kind": util::unquote(&util::col_str(row, "kind")),
            "key": util::col_str(row, "key_value"),
            "revisions": util::col_i64(row, "revisions"),
            "bytes": util::col_i64(row, "bytes"),
            "createdAt": util::col_i64(row, "created_at"),
            "retired": !row.get("retired_at").map(SqliteValue::is_null).unwrap_or(true),
        })
    })
    .collect();

    // submissions：这一条会话的提交。
    let submissions: Vec<Value> = util::all(
        &db,
        &format!("SELECT id, status, request_id FROM submissions WHERE conversation_id = {id} ORDER BY id"),
    )
    .unwrap_or_default()
    .iter()
    .map(|row| {
        json!({
            "id": util::col_i64(row, "id"),
            "status": util::col_str(row, "status"),
            "requestId": row.get("request_id").and_then(SqliteValue::as_str),
        })
    })
    .collect();

    let prompt = prompt_from(&entries);
    Ok(Some(json!({
        "id": id,
        "ownerConv": owner_conv,
        "ownerTask": owner_task,
        "entries": entries,
        "tasks": tasks,
        "documents": documents,
        "submissions": submissions,
        "prompt": prompt,
    })))
}

/// 「此刻模型看到了什么」：把 entries 摊成一条消息流（对齐面板 `promptAt`）。
///
/// 用的就是库里存的那一条条 record，只补两个数：真实 input tokens（那条 assistant 自己报的）
/// 和本条 entry 自己的体积。
fn prompt_from(entries: &[Value]) -> Value {
    let mut messages = Vec::new();
    let mut chars = 0i64;
    let mut tool_chars = 0i64;
    for entry in entries {
        let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
        let role = entry.get("role").and_then(Value::as_str).unwrap_or("");
        let entry_chars = entry.get("chars").and_then(Value::as_i64).unwrap_or(0);
        chars += entry_chars;
        if kind == "pi.system" {
            tool_chars += entry_chars;
        }
        let model = entry.get("model").and_then(Value::as_str);
        let tool = entry.get("tool").and_then(Value::as_str);
        let label = match role {
            "user" => "user".to_string(),
            "assistant" => format!("assistant · {}", model.unwrap_or("?")),
            "toolResult" => format!("tool-result · {}", tool.unwrap_or("")),
            "system" => {
                let tools = entry
                    .get("toolsAdded")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let shown: Vec<String> = tools
                    .iter()
                    .take(6)
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect();
                format!("system · {} 个工具描述（{}）", tools.len(), shown.join(" "))
            }
            _ => kind.to_string(),
        };
        messages.push(json!({
            "id": entry.get("id").cloned().unwrap_or(Value::Null),
            "label": label,
            "kind": kind,
            "bytes": entry.get("bytes").cloned().unwrap_or(json!(0)),
            "chars": entry_chars,
            "tokens": entry.get("tokens").cloned().unwrap_or(json!(0)),
            "cost": entry.get("cost").cloned().unwrap_or(json!(0)),
            "head": entry.get("head").map(|value| !value.is_null()).unwrap_or(false),
            "text": entry.get("text").cloned().unwrap_or(json!("")),
        }));
    }
    let messages_count = messages.len() as i64;
    let real_input = entries
        .last()
        .and_then(|entry| entry.get("usage"))
        .and_then(|usage| usage.get("input"))
        .and_then(Value::as_i64)
        .or_else(|| {
            entries
                .iter()
                .rev()
                .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("pi.assistant"))
                .and_then(|entry| entry.get("tokens"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(0);
    json!({
        "messages": messages,
        "totals": {
            "messages": messages_count,
            "chars": chars,
            "toolChars": tool_chars,
            "approxTokens": (chars as f64 / 3.6).round() as i64,
            "realInputTokens": real_input,
        },
    })
}

fn clamp(text: &str, max: usize) -> String {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    clean_truncate(&cleaned, max)
}

fn clean_truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
