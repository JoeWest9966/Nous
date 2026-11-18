//! `qrd3wh.sessions` 的会话记录（名字 / 配置）写入。
//!
//! 这一层只写 durable 的文档；模板 / 扩展的字面值在这里存成原始 JSON —— 它们由扩展和
//! 仓库决定，端点层不解释。语义是：
//!
//! - `name` 省略 = 空串；
//! - `template: null` 是**一条显式记录**，不是省略；
//! - `extensions` 只做 add（与 `wire.md` 的 ⚠ 一致：本层没有减的语义）。

use nous_durable::types::DocumentScope;
use nous_durable::{DocumentContent, Id, Storage, StorageWrite};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Result, ServerError};
use crate::models::ModelRef;

/// `POST /v1/sessions` 的请求体。字段全 camelCase；snake_case 不报错但静默不生效，
/// 所以这里显式写 `rename_all`，让字段表成为唯一权威。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub model: Option<ModelRef>,
    #[serde(default)]
    pub thinking_level: Option<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    /// 三态：`Some(null)` = 明确不要，`None` = 用仓库默认（本层不读仓库，视作无）。
    #[serde(default)]
    pub template: Option<Option<String>>,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
}

/// 201 响应：请求里生效的配置 + 找不到的东西（由调用方如实告诉用户）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedSession {
    pub id: Id,
    pub name: String,
    pub model: Option<ModelRef>,
    pub thinking_level: Option<String>,
    pub skills: Vec<String>,
    pub template: Option<String>,
    pub extensions: Vec<String>,
    pub cwd: Option<String>,
    pub machine: Option<String>,
    pub read_only: bool,
    pub missing: Vec<String>,
    pub missing_extensions: Vec<String>,
    pub missing_templates: Vec<String>,
}

/// 写一条新会话 + 它的名字/配置文档。
pub fn create_session(
    storage: &mut Storage,
    request: CreateSessionRequest,
) -> Result<CreatedSession> {
    // 会话 id 先铸：201 回的就是它；名字/配置文档另取一个 id，不跟会话抢号。
    let id = storage.mint_id().map_err(|e| ServerError::Internal(e.to_string()))?;
    let mut writes = vec![StorageWrite::Conversation(json!({ "id": id }))];

    let template_explicit = request.template.is_some();
    let created = CreatedSession {
        id,
        name: request.name.unwrap_or_default(),
        model: request.model,
        thinking_level: request.thinking_level,
        skills: request.skills,
        template: request.template.flatten(),
        extensions: request.extensions,
        cwd: request.cwd,
        machine: request.machine,
        read_only: false,
        missing: Vec::new(),
        missing_extensions: Vec::new(),
        missing_templates: Vec::new(),
    };
    let mut config = json!({
        "name": created.name,
        "model": created.model,
        "thinkingLevel": created.thinking_level,
        "skills": created.skills,
        "template": created.template,
        "extensions": created.extensions,
        "cwd": created.cwd,
        "machine": created.machine,
    });
    // 上面 json! 借用了 created 的字段（serde 序列化按引用），下面继续用 created 作返回值。
    // 显式 null 必须留着：它是「不要模板」，不该跟「没提」合并。
    if template_explicit && created.template.is_none() {
        config["template"] = Value::Null;
    }

    let document_id = storage.mint_id().map_err(|e| ServerError::Internal(e.to_string()))?;
    writes.push(StorageWrite::DocumentCreate {
        id: document_id,
        record: json!({
            "id": document_id,
            "kind": "qrd3wh.sessions",
            "scope": DocumentScope::Session,
        }),
        content: DocumentContent::Base {
            version: 1,
            value: json!({ "names": { id.to_string(): config["name"] }, "config": config }),
        },
    });
    let committed = storage
        .commit(&[writes], |_, _| {})
        .map_err(|error| ServerError::Internal(error.to_string()))?;
    if let Some(refusal) = committed.refused {
        return Err(ServerError::BadRequest(refusal.to_string()));
    }

    Ok(created)
}
