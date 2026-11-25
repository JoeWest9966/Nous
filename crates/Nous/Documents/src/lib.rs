//! `documents` 应用层模块（#48）。
//!
//! 一个 [`DocumentKind`] 实现 = 一组端点。框架只认 trait，不认具体 kind：
//! `kinds/presets.rs` 与 `kinds/prompts.rs` 各自只有字段结构体 + `validate`，
//! 没有一行 `if kind == "presets"`。路由注册表见 [`routes`]。
//!
//! ## 写路径
//!
//! 本 crate **不持有 `Storage`**。写入经 [`nous_durable::session::commands`] 的
//! `write_document` 走 `Session` 的同一把写锁、同一个事务、同一个 `ObserverRegistry`。
//! 读取由宿主/session 层提供的只读句柄完成（见 [`store`]）。

pub mod kinds;
pub mod routes;
pub mod service;
pub mod store;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// 校验问题的严重度。反序列化形状 `{path, message, level}` 由 desk 的 `PresetValidation`
/// 按 `Value` 收；这里给显式枚举，免得 spelling 漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
}

/// 一条字段级问题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub path: String,
    pub message: String,
    pub level: Level,
}

impl Issue {
    pub fn error(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { path: path.into(), message: message.into(), level: Level::Error }
    }
    pub fn warning(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { path: path.into(), message: message.into(), level: Level::Warning }
    }
}

/// 引用校验的上下文。`names` 是**同一 section 里已存在的名字全集**（由 store 提供）。
#[derive(Debug, Clone, Default)]
pub struct ValidationCtx {
    pub names: BTreeSet<String>,
}

/// 一份文档的编译期形状。`KIND` / `SECTIONS` 是端点注册表的输入；
/// `validate` 是唯一的校验实现（框架不分支）。
pub trait DocumentKind: Sized + Serialize + DeserializeOwned {
    const KIND: &'static str;
    const SECTIONS: &'static [&'static str];

    fn validate(&self, ctx: &ValidationCtx) -> Vec<Issue>;

    /// 从 JSON 值里解析 typed 字段。字段写错在这里被挡住。
    fn from_value(value: Value) -> Result<Self, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    /// 这份文档引用了哪些名字。默认不引用任何东西；引用校验交给框架。
    fn references(&self) -> Vec<String> {
        Vec::new()
    }
}

/// desk `PresetValidation` 的回包形状（snapshots/documents_validation.json）。
///
/// `errors` / `warnings` 的条目是 `{path, message}`；`resolved` 是
/// `{ "references": { <name>: { "state": "valid" | "invalid" } } }`。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationReport {
    pub ok: bool,
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
    pub resolved: Value,
}

impl ValidationReport {
    /// **一条泛型实现**：任何 `DocumentKind` 都走这里；kind 自己的 `validate` 决定 issues，
    /// 引用状态由这里按 `ctx.names` 统一算（默认空 = `resolved.references: {}`）。
    pub fn of<K: DocumentKind>(fields: &K, ctx: &ValidationCtx) -> Self {
        let issues = K::validate(fields, ctx);
        let errors: Vec<Issue> = issues
            .iter()
            .filter(|issue| issue.level == Level::Error)
            .cloned()
            .collect();
        let warnings: Vec<Issue> = issues
            .iter()
            .filter(|issue| issue.level == Level::Warning)
            .cloned()
            .collect();
        let mut references = BTreeMap::new();
        for name in K::references(fields) {
            let state = if ctx.names.contains(&name) { "valid" } else { "invalid" };
            references.insert(name, json!({ "state": state }));
        }
        Self {
            ok: errors.is_empty(),
            errors,
            warnings,
            resolved: json!({ "references": references }),
        }
    }
}
