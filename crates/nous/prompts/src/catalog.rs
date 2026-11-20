//! 私库目录的只读扫描：`PROMPT.md` + `nous.json` + 根目录 `settings.json`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// 一条提示词模板：目录名是身份，正文与元数据分开读。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub description: Option<String>,
    pub category: Vec<String>,
    pub body: String,
}

/// 列表里的一条：目录扫描能看见的事实，`problem` 是给客户端看的诊断（空串 = 没毛病）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptEntry {
    pub name: String,
    pub description: Option<String>,
    pub category: Vec<String>,
    pub path: String,
    pub problem: String,
}

/// `GET /v1/prompts` 的投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub default: Option<String>,
    pub entries: Vec<PromptEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct NousJson {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    category: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Settings {
    #[serde(default)]
    default: Option<String>,
}

/// 只读扫描一个提示词根目录。目录不存在时回空表，不建、不写。
pub fn read_catalog(root: &Path) -> Catalog {
    let mut entries = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(root) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let manifest = path.join("nous.json");
            let prompt = path.join("PROMPT.md");
            let metadata: Option<NousJson> = std::fs::read_to_string(&manifest)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok());
            let body = std::fs::read_to_string(&prompt).ok();
            let mut problems = Vec::new();
            if metadata.is_none() {
                problems.push("缺少 nous.json".to_string());
            }
            if body.is_none() {
                problems.push("缺少 PROMPT.md".to_string());
            }
            entries.push(PromptEntry {
                name: name.clone(),
                description: metadata.as_ref().and_then(|m| m.description.clone()),
                category: metadata.map(|m| m.category).unwrap_or_default(),
                path: path.to_string_lossy().into_owned(),
                problem: problems.join("；"),
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let default = std::fs::read_to_string(root.join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .and_then(|settings| settings.default);
    Catalog { default, entries }
}

/// 读一条模板正文（给注入用）。`None` = 磁盘上没有 PROMPT.md。
pub fn read_template(root: &Path, name: &str) -> Option<Template> {
    let dir = root.join(name);
    let body = std::fs::read_to_string(dir.join("PROMPT.md")).ok()?;
    let metadata: Option<NousJson> = std::fs::read_to_string(dir.join("nous.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    Some(Template {
        name: name.to_string(),
        description: metadata.as_ref().and_then(|m| m.description.clone()),
        category: metadata.map(|m| m.category).unwrap_or_default(),
        body,
    })
}

/// 列表投影：把 `default` 指向不存在模板的错误说清楚，不静默吞成 null。
pub fn catalog_value(root: &Path) -> Value {
    let catalog = read_catalog(root);
    let mut prompts = Vec::new();
    for entry in &catalog.entries {
        prompts.push(serde_json::json!({
            "name": entry.name,
            "description": entry.description,
            "category": entry.category,
            "path": entry.path,
            "problem": if entry.problem.is_empty() { Value::Null } else { Value::String(entry.problem.clone()) },
        }));
    }
    let mut categories: BTreeMap<String, ()> = BTreeMap::new();
    for entry in &catalog.entries {
        for category in &entry.category {
            categories.insert(category.clone(), ());
        }
    }
    let mut body = serde_json::json!({
        "default": catalog.default,
        "categories": categories.keys().collect::<Vec<_>>(),
        "prompts": prompts,
    });
    if let Some(default) = catalog.default.as_deref() {
        if !catalog.entries.iter().any(|entry| entry.name == default) {
            body["problem"] = serde_json::json!(format!("settings.json 的 default 指向不存在的模板：{default}"));
        }
    }
    body
}

/// 兼容旧调用点：默认私库路径由调用方解析后传进来。
pub fn default_root() -> PathBuf {
    PathBuf::from("/opt/Nous-prompts")
}
