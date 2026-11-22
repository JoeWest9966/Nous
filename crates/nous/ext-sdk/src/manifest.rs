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

/// TS 时代（`qrd3wh`）JS 插件的清单读出来应该看见的话。
///
/// 报 `missing-field` 等于告诉人去改 JSON —— 那条路是死的：新运行时 `spawn` 的是
/// 原生产物，`index.js` 没有解释器可跑。所以先把「这是换代，不是笔误」说清楚。
pub const LEGACY_JS_REASON: &str = "TS 时代的 JS 插件（index.js + defineExtension）新运行时装载不了：
扩展已换成原生进程 + 协议握手，需按 nous-ext-sdk 重写";

/// 这份清单是不是 TS 时代 JS 插件的形状。
///
/// 三个记号里占两个才算 —— 单看 `file` 以 `.js` 结尾可能是别的东西，单看没有
/// `protocol` 可能只是漏写。两个同时成立，才不是"字段笔误"而是换了一整套装载形态。
pub fn legacy_js_shape(raw: &serde_json::Value) -> bool {
    let file_is_js = raw
        .get("file")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|file| file.ends_with(".js") || file.ends_with(".mjs"));
    let sections_are_strings = raw
        .get("sections")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| items.iter().any(serde_json::Value::is_string));
    let no_protocol = raw.get("protocol").is_none();
    [file_is_js, sections_are_strings, no_protocol]
        .iter()
        .filter(|hit| **hit)
        .count()
        >= 2
}

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

#[cfg(test)]
mod legacy_tests {
    use super::*;

    /// TS 时代（qrd3wh）真实的 `nous.json`：`file` 是 JS、`sections` 是字符串数组、
    /// 没有 `protocol`。三个记号全中 —— 必须认出来，否则报 `missing-field` 把人引去改
    /// JSON，而改 JSON 也跑不起来（没有 JS 解释器）。
    #[test]
    fn old_js_manifest_is_legacy() {
        let raw = serde_json::json!({
            "name": "skills",
            "version": "0.2.0",
            "file": "index.js",
            "entry": "createSkills",
            "sections": ["skills"],
            "default": true,
            "summary": "扫私库里的技能目录",
            "needs": { "dir": "…" },
            "category": ["工具", "Basics"]
        });
        assert!(legacy_js_shape(&raw));
    }

    /// 新清单（原生产物 + protocol + 结构化 sections）不能被误判。
    #[test]
    fn native_manifest_is_not_legacy() {
        let raw = serde_json::json!({
            "name": "skills",
            "version": "0.3.0",
            "protocol": 1,
            "file": "skills",
            "entry": "skills",
            "sections": [{ "key": "skills", "tag": "skills" }],
            "handles": ["skills"]
        });
        assert!(!legacy_js_shape(&raw));
    }

    /// 只有一个记号成立时不下结论：`.js` 可能是别的东西，缺 `protocol` 可能只是漏写。
    /// 这种该照常报 `missing-field`，别把人误导到「要重写插件」上。
    #[test]
    fn single_marker_is_not_enough() {
        let only_js = serde_json::json!({
            "name": "x", "version": "0.1.0", "protocol": 1,
            "file": "index.js", "entry": "x",
            "sections": [{ "key": "a", "tag": "a" }]
        });
        assert!(!legacy_js_shape(&only_js));

        let only_no_protocol = serde_json::json!({
            "name": "x", "version": "0.1.0",
            "file": "bin", "entry": "x",
            "sections": [{ "key": "a", "tag": "a" }]
        });
        assert!(!legacy_js_shape(&only_no_protocol));
    }
}
