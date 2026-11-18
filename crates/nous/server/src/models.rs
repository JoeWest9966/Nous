//! 模型编目的**契约形状**：camelCase 的 wire 字段与默认模型。
//!
//! `nous-server` 不碰 provider（AGENTS.md §二），因此端点层只声明形状、不内置清单：
//! 真正的编目（几百个模型、thinking levels、窗口）由宿主进程填进来。没填时 `/v1/models`
//! 返回空列表 + `default: null`，这是一个诚实的「不知道」，不是伪造的模型表。
//!
//! `default` 的 `ModelRef` 形状与建会话入参共用，定义在 durable（会话写命令的一部分），
//! 这里直接引用，不在端点层复制第二份。

use serde::Serialize;

/// 默认空编目：`None` + `[]`，宿主填进来之前不伪造模型表。
impl Default for ModelsResponse {
    fn default() -> Self {
        Self {
            default: None,
            items: Vec::new(),
        }
    }
}

/// `GET /v1/models` 的 `items[]` 一项。字段名就是 wire 形状，不许改成 snake_case。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelItem {
    pub provider: String,
    pub model_id: String,
    pub name: String,
    pub thinking_levels: Vec<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
}

/// `GET /v1/models` 的响应（省略 `catalog` 诊断块：那是引擎私有扩展，不属于字段形状契约）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelsResponse {
    pub default: Option<crate::records::ModelRef>,
    pub items: Vec<ModelItem>,
}
