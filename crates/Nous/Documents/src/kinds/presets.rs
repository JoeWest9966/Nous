//! 模板（desk `Preset`）。字段来自 `NousDesk/crates/nous-engine/src/types.rs:246`。
//!
//! 故意不写 `Default`：缺字段的构造在编译期就红。`deny_unknown_fields` 让字段名写错
//! 在反序列化时红（不是默默塞进 `Value`）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{DocumentKind, Issue, ValidationCtx};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetFields {
    pub name: String,
    pub summary: String,
    pub description: String,
    pub category: Vec<String>,
    pub spec: Option<Value>,
    pub body: Option<String>,
}

impl DocumentKind for PresetFields {
    const KIND: &'static str = "presets";
    const SECTIONS: &'static [&'static str] = &["Session-Template", "Ultimate-Template"];

    fn validate(&self, _ctx: &ValidationCtx) -> Vec<Issue> {
        let mut issues = Vec::new();
        if self.name.trim().is_empty() {
            issues.push(Issue::error("name", "name is required"));
        }
        if self.summary.trim().is_empty() {
            issues.push(Issue::error("summary", "summary is required"));
        }
        if self.category.is_empty() {
            issues.push(Issue::warning("category", "no category is attached"));
        }
        issues
    }

    fn references(&self) -> Vec<String> {
        // 终极模板可能引用别的模板名（形状由 spec 决定，字段本身仍是 typed）。
        let mut names = Vec::new();
        if let Some(spec) = &self.spec {
            if let Some(session) = spec.get("session").and_then(Value::as_str) {
                names.push(session.to_string());
            }
            if let Some(skills) = spec.get("skills").and_then(Value::as_array) {
                names.extend(skills.iter().filter_map(Value::as_str).map(str::to_string));
            }
        }
        names
    }
}
