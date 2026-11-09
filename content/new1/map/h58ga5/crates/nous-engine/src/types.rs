//! qrd3wh `/v1/*` 的协议类型。
//!
//! 纪律：
//! - 未知字段用 `#[serde(flatten)] extra` 收着，原样透传，**不** `deny_unknown_fields`。
//! - 这里只有引擎回的形状，没有业务规则。能否启动、是否生效，引擎都算好了，别在这里再算一遍。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// `GET /v1/sessions` 返回的整棵树。
#[derive(Debug, Clone, Deserialize)]
pub struct Sessions {
    pub items: Vec<Session>,
}

/// 会话树上的一个节点。`parent != null` = 这是一条 fork 分支。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: i64,
    pub name: String,
    pub parent: Option<SessionParent>,
    pub owner: Option<serde_json::Value>,
    /// `"provider/modelId"`，例如 `"volc-ark/deepseek-v3"`。
    pub model: Option<String>,
    /// 生效的思考档，不是请求值。
    pub thinking_level: Option<serde_json::Value>,
    pub thinking_levels: Vec<serde_json::Value>,
    pub skills: Vec<String>,
    pub template: Option<String>,
    pub read_only: bool,
    /// `"queued" | "running" | <task state kind> | null`，不是布尔。
    pub busy: Option<serde_json::Value>,
    pub last: Option<SessionLast>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionParent {
    pub id: i64,
    pub at: Option<serde_json::Value>,
}

/// 列表里那条「最后一句」的摘要。`text` 可以是 null（消息没正文时）。
#[derive(Debug, Clone, Deserialize)]
pub struct SessionLast {
    pub id: serde_json::Value,
    pub kind: String,
    pub text: Option<String>,
}

/// `GET /v1/sessions/:id/entries` 返回的历史。**新的在前**，客户端要翻正。
#[derive(Debug, Clone, Deserialize)]
pub struct Entries {
    pub items: Vec<Entry>,
    pub next: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub id: serde_json::Value,
    pub kind: String,
    /// 可为 null（比如空回复的 assistant 气泡）。
    pub text: Option<String>,
    pub parent: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// `POST /v1/sessions/:id/messages` 的响应（不传 `waitMs` 时）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Submission {
    #[serde(rename = "submissionId")]
    pub submission_id: serde_json::Value,
    pub status: String,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// `GET /v1/device`。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub self_: Option<String>,
    pub origin: Option<String>,
    pub node: Option<String>,
    pub started_at: Option<serde_json::Value>,
    pub model: Option<serde_json::Value>,
    pub faux: Option<bool>,
    pub data: Option<String>,
    pub catalog: Option<serde_json::Value>,
    pub available: Vec<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// `GET /v1/presets/:section` 的一条模板。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub name: String,
    pub summary: String,
    pub description: String,
    pub category: Vec<String>,
    pub spec: Option<serde_json::Value>,
    /// `Some(..)` = 这条自己写坏了（JSON / 形状）。
    pub spec_error: Option<String>,
    /// `false` = 盘上有、仓里没。
    pub tracked: bool,
    pub dir: String,
    /// 引用校验结果。`resolved.*.state == "invalid"` = 现在开不了。
    pub resolved: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Presets {
    pub section: String,
    pub title: Option<String>,
    pub presets: Vec<Preset>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// `POST /v1/sessions` 的请求体。
#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

/// `POST /v1/sessions` 的响应（201）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedSession {
    pub id: i64,
    pub name: String,
    pub model: Option<String>,
    pub thinking_level: Option<serde_json::Value>,
    pub skills: Vec<String>,
    pub template: Option<String>,
    pub extensions: Vec<String>,
    pub read_only: bool,
    pub preset: Option<String>,
    pub missing: Vec<String>,
    #[serde(rename = "missingExtensions", default)]
    pub missing_extensions: Vec<String>,
    #[serde(rename = "missingTemplates", default)]
    pub missing_templates: Vec<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}
