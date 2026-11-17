//! `nous.json`：能力声明，不是分发清单。
//!
//! 清单里**只有**这些字段（spec ch1.1）：`name` / `version` / `protocol` / `file` /
//! `entry` / `sections`（有序，每项带 `tag`）/ `handles`（句柄**名**）/ `summary` /
//! `category`。没有超时、没有并发、没有内核版本（`E7`），没有本机路径（EX-01）。

use serde::{Deserialize, Serialize};

/// 清单里的一项 section。**顺序就是提示词里的顺序**；`tag` 也在清单里。
///
/// 应答里的顺序与 `tag` 一律不算数（EX-15 / EX-16）—— 所以两者必须随清单走。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionDecl {
    pub key: String,
    #[serde(default)]
    pub tag: String,
}

/// `nous.json`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// **`name` 就是 API**：会话里存的是这个名字。
    pub name: String,
    pub version: String,
    /// 协议版本。与宿主不符即拒载并报双方版本（EX-06）。
    pub protocol: u32,
    /// 产物名（相对 `artifacts` 目录）。清单里不写本机路径。
    pub file: String,
    /// 入口标识。原生扩展里等于产物名，留给将来的解释型入口。
    #[serde(default)]
    pub entry: String,
    /// 有序：顺序就是提示词里的顺序。
    #[serde(default)]
    pub sections: Vec<SectionDecl>,
    /// 句柄**名**（`skills` / `prompts` / …），不是值、不是路径。
    #[serde(default)]
    pub handles: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

/// 清单不合法的一条原因。`Display` 出来的就是诊断原文（判据里 `grep -F` 用得上）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestProblem {
    /// 问题分类：`empty` / `forbidden-key` / `absolute-path` / `missing-field` / `duplicate`。
    pub kind: &'static str,
    /// 指向清单里的哪个字段；整体性问题用 `*`。
    pub field: String,
    /// 给人看的一句话。
    pub message: String,
}

impl std::fmt::Display for ManifestProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} ({})", self.kind, self.message, self.field)
    }
}

/// 四件能力全空 ⇒ 拒载，并指名"这个扩展什么都不提供"（EX-03）。
pub const EMPTY_REASON: &str = "这个扩展什么都不提供";

/// 清单里被禁止的字段（`E7`）。出现即拒载。
pub const FORBIDDEN_KEYS: [&str; 5] = ["kernel", "timeout", "concurrency", "rss", "budget"];

impl Manifest {
    /// 解析 + 结构校验。`raw` 是 JSON，因为「哪些键出现了」本身是判据（EX-05）。
    pub fn parse(raw: &serde_json::Value) -> Result<Self, Vec<ManifestProblem>> {
        let mut problems = Vec::new();
        for key in forbidden_keys(raw) {
            problems.push(ManifestProblem {
                kind: "forbidden-key",
                field: key.clone(),
                message: format!("清单不许自报 {key}（超时/并发/内核版本由宿主给）"),
            });
        }
        if let Some(path) = first_absolute_path(raw) {
            problems.push(ManifestProblem {
                kind: "absolute-path",
                field: path.clone(),
                message: format!("清单里出现了本机路径：{path}"),
            });
        }
        let manifest: Manifest = match serde_json::from_value(raw.clone()) {
            Ok(manifest) => manifest,
            Err(error) => {
                problems.push(ManifestProblem {
                    kind: "missing-field",
                    field: "*".to_string(),
                    message: format!("清单字段不完整：{error}"),
                });
                return Err(problems);
            }
        };
        if manifest.name.trim().is_empty() {
            problems.push(ManifestProblem {
                kind: "missing-field",
                field: "name".to_string(),
                message: "清单缺 name".to_string(),
            });
        }
        if manifest.protocol == 0 {
            problems.push(ManifestProblem {
                kind: "missing-field",
                field: "protocol".to_string(),
                message: "清单缺 protocol".to_string(),
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for section in &manifest.sections {
            if !seen.insert(section.key.clone()) {
                problems.push(ManifestProblem {
                    kind: "duplicate",
                    field: format!("sections.{}", section.key),
                    message: format!("section 键重复：{}", section.key),
                });
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(manifest)
    }

    /// 四件能力全空？section 与句柄都在清单里；tool / 名牌在应答里（本方法只看清单面）。
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() && self.handles.is_empty()
    }
}

/// 清单里出现过的禁止键（含嵌套）。EX-05 的载体。
pub fn forbidden_keys(raw: &serde_json::Value) -> Vec<String> {
    let mut hits = Vec::new();
    walk_keys(raw, &mut |key| {
        if FORBIDDEN_KEYS.contains(&key) {
            hits.push(key.to_string());
        }
    });
    hits.sort();
    hits.dedup();
    hits
}

fn walk_keys(value: &serde_json::Value, visit: &mut impl FnMut(&str)) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                visit(key);
                walk_keys(child, visit);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                walk_keys(item, visit);
            }
        }
        _ => {}
    }
}

/// 第一个绝对路径（字符串以 `/` 开头或以 `~` 开头）。EX-01 的载体。
pub fn first_absolute_path(raw: &serde_json::Value) -> Option<String> {
    let mut found = None;
    walk_strings(raw, &mut |text| {
        if found.is_none() && (text.starts_with('/') || text.starts_with("~/")) {
            found = Some(text.to_string());
        }
    });
    found
}

fn walk_strings(value: &serde_json::Value, visit: &mut impl FnMut(&str)) {
    match value {
        serde_json::Value::String(text) => visit(text),
        serde_json::Value::Object(map) => {
            for child in map.values() {
                walk_strings(child, visit);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                walk_strings(item, visit);
            }
        }
        _ => {}
    }
}
