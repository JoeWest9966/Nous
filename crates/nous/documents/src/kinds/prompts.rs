//! 提示词（kind `prompts`）。字段同样是编译期结构体，走同一条 `DocumentKind` 实现。

use serde::{Deserialize, Serialize};

use crate::{DocumentKind, Issue, ValidationCtx};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptFields {
    pub name: String,
    pub text: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
}

impl DocumentKind for PromptFields {
    const KIND: &'static str = "prompts";
    const SECTIONS: &'static [&'static str] = &["system", "user"];

    fn validate(&self, _ctx: &ValidationCtx) -> Vec<Issue> {
        let mut issues = Vec::new();
        if self.name.trim().is_empty() {
            issues.push(Issue::error("name", "name is required"));
        }
        if self.text.trim().is_empty() {
            issues.push(Issue::error("text", "text is required"));
        }
        if self.tags.is_empty() {
            issues.push(Issue::warning("tags", "no tags are attached"));
        }
        issues
    }
}
