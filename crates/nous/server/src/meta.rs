//! #47 的静态元数据端点：`device` / `selection` / `skills` / `prompts` /
//! `extensions` / `capabilities`。
//!
//! 这里只做**数据装配**，没有第二份事实源：进程配置来自环境，能力声明是这张表本身。
//! 形状以 `wire.md` + `NousDesk/crates/nous-engine/src/types.rs` 为准；未知字段透传给客户端。

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::models::ModelsResponse;

/// `GET /v1/device`。`node` 是引擎版本标识（About 面板显示），`startedAt` 是 epoch 毫秒。
pub fn device(models: &ModelsResponse) -> Value {
    let default = models.default.as_ref().map(|model| {
        json!({ "provider": model.provider, "modelId": model.model_id })
    });
    json!({
        "self": std::env::var("NOUS_SELF").ok(),
        "origin": std::env::var("NOUS_ORIGIN").ok(),
        "node": env!("CARGO_PKG_VERSION"),
        "startedAt": started_at(),
        "model": default,
        "faux": false,
        "data": std::env::var("NOUS_DATA").ok(),
        "catalog": Value::Null,
        "available": models.items.iter().map(|item| {
            format!("{}/{}", item.provider, item.model_id)
        }).collect::<Vec<_>>(),
    })
}

fn started_at() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// `GET /v1/selection`：当前没有选中的会话，诚实回 `null`（客户端 0 处依赖）。
pub fn selection() -> Value {
    json!({ "id": std::env::var("NOUS_SELECTION").ok().and_then(|s| s.parse::<i64>().ok()) })
}

/// `GET /v1/skills`：目录不存在就是空表，不伪造内容、不 404。
pub fn skills() -> Value {
    let root = std::env::var("NOUS_SKILLS_ROOT").ok().map(PathBuf::from);
    let entries = root
        .as_ref()
        .and_then(|root| std::fs::read_dir(root).ok())
        .map(|read_dir| {
            read_dir
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_dir())
                .map(|entry| {
                    json!({
                        "name": entry.file_name().to_string_lossy().into_owned(),
                        "description": Value::Null,
                        "category": Value::Array(vec![]),
                        "hidden": false,
                        "tracked": false,
                        "dir": entry.path().to_string_lossy().into_owned(),
                        "file": Value::Null,
                        "load": { "state": "ok" },
                    })
                })
                .collect::<Vec<Value>>()
        })
        .unwrap_or_default();
    json!({
        "root": root.map(|root| root.to_string_lossy().into_owned()),
        "categories": [],
        "skills": entries,
    })
}

/// `GET /v1/prompts`：#48 前先与 skills 同构的空表。
pub fn prompts() -> Value {
    let root = std::env::var("NOUS_PROMPTS_ROOT").unwrap_or_default();
    json!({
        "default": Value::Null,
        "categories": [],
        "prompts": [],
        "root": if root.is_empty() { Value::Null } else { Value::String(root) },
    })
}

/// `GET /v1/extensions`：扩展运行时冻结，清单为空。
pub fn extensions() -> Value {
    json!({ "items": [] })
}

/// `GET /v1/capabilities`：一个发行版一张清单。desk 按它显示/隐藏功能面。
pub fn capabilities() -> Value {
    json!({ "modules": [
        {
            "name": "sessions",
            "enabled": true,
            "endpoints": [
                "GET /v1/sessions",
                "POST /v1/sessions",
                "GET /v1/sessions/:id/entries",
                "GET /v1/sessions/:id/events",
                "POST /v1/sessions/:id/messages",
                "POST /v1/sessions/:id/abort",
                "POST /v1/sessions/:id/fork",
                "POST /v1/sessions/:id/rename"
            ]
        },
        {
            "name": "presets",
            "enabled": false,
            "reason": "document 模块未落地（#47）"
        },
        { "name": "archive", "enabled": false, "reason": "A 阶段关闭" },
        { "name": "combos", "enabled": false, "reason": "A 阶段关闭" },
        { "name": "triggers", "enabled": false, "reason": "A 阶段关闭" },
        { "name": "fs", "enabled": false, "reason": "宿主目录浏览的安全面另议" },
        {
            "name": "extensions",
            "enabled": true,
            "endpoints": ["GET /v1/extensions"]
        }
    ]})
}
