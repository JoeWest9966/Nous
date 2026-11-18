//! `POST /v1/sessions` 的请求 / 响应形状。
//!
//! 这里只放形状：真正的写由 `Session::create_conversation` 完成（和别的写共享一把锁、共享
//! `ObserverRegistry` 广播）。端点层负责把请求结构拼成一份 config `Value` 传进去，再把
//! 带 id 的返回值还原成 201 响应。durable 不认识这里的键名，也不反向依赖本 crate。

use serde::Deserialize;
use serde_json::json;

/// 默认模型 / 请求里 `model` 的形状（camelCase 落在 wire 上）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRef {
    pub provider: String,
    pub model_id: String,
}

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

impl CreateSessionRequest {
    /// 拼成一份会话配置 `Value` 交给 durable；显式 `template: null` 必须留着，
    /// 它是「明确不要」，不该跟「没提」合并。
    pub fn into_config(self) -> serde_json::Value {
        let template_explicit = self.template.is_some();
        let mut config = json!({
            "name": self.name.unwrap_or_default(),
            "model": self.model,
            "thinkingLevel": self.thinking_level,
            "skills": self.skills,
            "template": self.template.and_then(|value| value),
            "extensions": self.extensions,
            "cwd": self.cwd,
            "machine": self.machine,
        });
        if template_explicit && config["template"].is_null() {
            config["template"] = serde_json::Value::Null;
        }
        config
    }
}

/// 201 响应：请求里生效的配置 + 找不到的东西（由调用方如实告诉用户）。
#[derive(Debug, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedSession {
    pub id: i64,
    pub name: String,
    pub model: Option<ModelRef>,
    pub thinking_level: Option<String>,
    pub skills: Vec<String>,
    pub template: Option<String>,
    pub extensions: Vec<String>,
    pub cwd: Option<String>,
    pub machine: Option<String>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub missing: Vec<String>,
    #[serde(default)]
    pub missing_extensions: Vec<String>,
    #[serde(default)]
    pub missing_templates: Vec<String>,
}
