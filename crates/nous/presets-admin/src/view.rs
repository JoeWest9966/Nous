//! 名册投影：一条预设 → roster 行；一段 → `GET /v1/presets/<段>` 的回包。
//!
//! 这里只做**拼装**：盘上事实（字节 / 清单 / 归档目录）由 server 读进来，判定交给
//! [`crate::resolve`]，形状交给 [`crate::shape`]。所以这一层能单测、能重放。

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::resolve::{resolve_spec, resolve_ultimate, Ctx};
use crate::section::Section;
use crate::shape::problems_for;

/// 盘上的一条：名字 / 目录 / 清单原文 / 原始字节。
#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub dir: String,
    pub body: Value,
    /// 清单文件的原始字节（sha 的来源）。
    pub bytes: Vec<u8>,
}

impl Item {
    /// 形状坏了的那条：`specError` 非空。引用坏了**不算**（那是「现在开不了」）。
    pub fn spec_error(&self, section: &Section) -> Option<String> {
        if self.body.is_null() {
            return Some(format!("{} 解析失败", section.manifest));
        }
        let problems = problems_for(section, &self.body, Some(&self.name));
        if problems.is_empty() { None } else { Some(problems.join("；")) }
    }
}

/// 一条 roster 行。字段跟 TS 内核的 `overview` 逐字对齐，另加 #60 的 `version` / `sha`。
#[allow(clippy::too_many_arguments)]
pub fn roster_row(
    item: &Item,
    section: &Section,
    ctx: &Ctx,
    tracked: bool,
    version: i64,
    order: i64,
    sha: String,
) -> Value {
    let spec_error = item.spec_error(section);
    let resolved = if spec_error.is_some() {
        Value::Null
    } else if section.is_ultimate() {
        resolve_ultimate(&item.body, ctx)
    } else {
        resolve_spec(&item.body, ctx)
    };
    let category = item
        .body
        .get("category")
        .filter(|value| value.is_array())
        .cloned()
        .unwrap_or_else(|| json!([]));
    json!({
        "name": item.name,
        "summary": item.body.get("summary").cloned().unwrap_or(Value::Null),
        "description": item.body.get("description").cloned().unwrap_or(Value::Null),
        "category": category,
        "spec": item.body,
        "specError": spec_error,
        "tracked": tracked,
        "dir": item.dir,
        "resolved": resolved,
        "version": version,
        "sha": sha,
        "order": order,
    })
}

/// 归档区里的一条（`<段>/.archive/<名>/<manifest>`）。
#[derive(Debug, Clone)]
pub struct ArchivedItem {
    pub name: String,
    pub body: Value,
    /// 归档目录的 mtime（epoch ms）；读不到就是 `None`。
    pub archived_at: Option<i64>,
}

/// 归档行。`archivedAt` 只给人看「什么时候扔进去的」。
pub fn archived_row(item: &ArchivedItem) -> Value {
    let category = item
        .body
        .get("category")
        .filter(|value| value.is_array())
        .cloned()
        .unwrap_or_else(|| json!([]));
    json!({
        "name": item.name,
        "summary": item.body.get("summary").cloned().unwrap_or(Value::Null),
        "category": category,
        "archivedAt": item.archived_at,
    })
}

/// 读 `categories.json`：`{v:1, categories:[...]}`（老形态直接是数组也收）。
pub fn vocabulary(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else { return Vec::new() };
    let list = value
        .as_array()
        .or_else(|| value.get("categories").and_then(Value::as_array));
    list.map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// 词表 ∪ 每条挂着的类别 → 去重 + 父在子前 + 同级按名字排。
///
/// TS 内核用 `localeCompare('zh-CN')`（ICU，按拼音）；Rust 侧没有 ICU，这里按码点排。
/// 差别只在中文同级的先后 —— 层级关系（父在子前）不受影响，客户端也按路径建树，不靠绝对序。
pub fn sort_categories<I, S>(names: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let unique: BTreeSet<String> = names
        .into_iter()
        .map(Into::into)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    let mut out: Vec<String> = unique.into_iter().collect();
    out.sort_by(|a, b| {
        let left: Vec<&str> = a.split('/').collect();
        let right: Vec<&str> = b.split('/').collect();
        left.cmp(&right)
    });
    out
}

/// 读 `order.json`：`{v:1, order:[...]}`（老形态 `{order:[...]}` 也收）。
pub fn order(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|value| value.get("order"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 按排序表重排：表里没有的排后面，按名字 —— 新模板稳定地待在同一头，不随刷新换位置。
pub fn apply_order<T, F>(items: Vec<T>, order: &[String], name_of: F) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    if order.is_empty() {
        return items;
    }
    let mut indexed: Vec<(usize, T)> = items
        .into_iter()
        .map(|item| {
            let name = name_of(&item);
            let at = order
                .iter()
                .position(|entry| entry == name)
                .unwrap_or(usize::MAX);
            (at, item)
        })
        .collect();
    indexed.sort_by(|a, b| match (a.0 == usize::MAX, b.0 == usize::MAX) {
        (false, false) => a.0.cmp(&b.0),
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (true, true) => name_of(&a.1).cmp(name_of(&b.1)),
    });
    indexed.into_iter().map(|(_, item)| item).collect()
}

/// `GET /v1/presets/<段>` 的整段回包。
pub fn overview(
    section: &Section,
    root: &str,
    repo: Value,
    presets: Vec<Value>,
    archived: Vec<Value>,
    categories: Vec<String>,
) -> Value {
    json!({
        "section": section.id,
        "title": section.title,
        "root": root,
        "repo": repo,
        "presets": presets,
        "archived": archived,
        "categories": categories,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::Ctx;
    use crate::section::default_sections;

    fn item(name: &str, body: Value) -> Item {
        Item {
            name: name.into(),
            dir: format!("/root/{name}"),
            bytes: br#"{}"#.to_vec(),
            body,
        }
    }

    #[test]
    fn 行里有_spec_和_resolved_和_specerror() {
        let sections = default_sections();
        let section = &sections[0];
        let good = item(
            "deep-dive",
            json!({
                "v": 1, "name": "deep-dive", "summary": "挖到底", "category": ["工程"],
                "model": { "provider": "volc-ark", "modelId": "flash" }
            }),
        );
        let row = roster_row(&good, section, &Ctx::default(), true, 3, 0, "sha".into());
        assert_eq!(row["name"], "deep-dive");
        assert_eq!(row["spec"]["summary"], "挖到底");
        assert!(row["specError"].is_null());
        assert_eq!(row["resolved"]["model"]["state"], "invalid", "编目空 → 引用失效");
        assert_eq!(row["tracked"], true);
        assert_eq!(row["version"], 3);

        let broken = item("bad", json!({ "v": 2 }));
        let row = roster_row(&broken, section, &Ctx::default(), false, 0, 0, "sha".into());
        assert!(row["specError"].is_string());
        assert!(row["resolved"].is_null(), "形状坏了不 resolve");
    }

    #[test]
    fn 终极模板段用终极解析() {
        let sections = default_sections();
        let section = &sections[1];
        let row = roster_row(
            &item("nightly", json!({ "v": 1, "name": "nightly", "summary": "s", "category": [], "trigger": "t", "params": {}, "session": "x" })),
            section,
            &Ctx::default(),
            false,
            0,
            0,
            "sha".into(),
        );
        assert_eq!(row["resolved"]["trigger"]["state"], "invalid");
        assert!(row["resolved"].get("model").is_none());
    }

    #[test]
    fn 类别父在子前且去重() {
        let sorted = sort_categories(vec!["写作/周报", "工程", "写作", "工程", "写作/周报/月"]);
        assert_eq!(sorted, vec!["写作", "写作/周报", "写作/周报/月", "工程"]);
    }

    #[test]
    fn 排序表里没有的排后面按名字() {
        let names = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        let ordered = apply_order(names, &["b".into()], |name| name.as_str());
        assert_eq!(ordered, vec!["b", "a", "c"]);
    }

    #[test]
    fn 词表与排序表两种老形态都收() {
        assert_eq!(vocabulary(Some(&json!(["a", "b"]))), vec!["a", "b"]);
        assert_eq!(
            vocabulary(Some(&json!({"v": 1, "categories": ["a"]}))),
            vec!["a"]
        );
        assert_eq!(order(Some(&json!({"order": ["x"]}))), vec!["x"]);
        assert!(order(Some(&json!({}))).is_empty());
    }
}
