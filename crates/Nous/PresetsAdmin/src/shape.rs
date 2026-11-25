//! 形状闸：`preset.json` / `ultimate.json` 的**形状**（不看引用）。
//!
//! 跟 TS 内核 `presets.js` 的 `shapeProblems` / `ultimateShapeProblems` 是同一套规则
//! （`Nous-Template/index.mjs` 那两份各持一份同样的测试表，漂了会红）。
//!
//! 三条纪律：
//!  1. **未知键必须报错不能忽略** —— 拼错的键等于那条设置静默没生效。
//!  2. **三态不许合并** —— 键不在 / `null` / 字符串是三种语义，这里只验形状。
//!  3. **「引用失效」不是「写坏了」** —— 形状坏进 `specError`；引用坏由 `resolve` 报红。

use serde_json::Value;

use crate::section::{Checker, Section};

/// 会话模板的顶层键白名单。
pub const PRESET_KEYS: [&str; 13] = [
    "v",
    "name",
    "summary",
    "category",
    "description",
    "model",
    "thinkingLevel",
    "skills",
    "extensions",
    "template",
    "readOnly",
    "cwd",
    "machine",
];

/// 终极模板的键表：一条 = 触发器 + 参数 + 一条会话模板。
pub const ULTIMATE_KEYS: [&str; 9] = [
    "v",
    "name",
    "summary",
    "category",
    "description",
    "trigger",
    "params",
    "session",
    "enabled",
];

/// 思考档的顺序表（跟 pi-ai 的档位表同一份，别另立一套）。
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub const MAX_NAME: usize = 64;
pub const MAX_SUMMARY: usize = 200;
pub const MAX_DESCRIPTION: usize = 2000;

/// 预设名：小写字母 / 数字开头，只留 `. _ -`。名字 = 目录名 = 主键。
pub fn check_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("预设名不能为空".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("预设名太长（最多 {MAX_NAME} 字）：{name}"));
    }
    let mut chars = name.chars();
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let tail_ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !head_ok || !tail_ok {
        return Err(format!("预设名「{name}」不合格：小写字母 / 数字开头，只留 . _ -"));
    }
    if RESERVED.contains(&name) {
        return Err(format!("「{name}」是仓库自用的名字，不能当预设名"));
    }
    Ok(name.to_string())
}

/// 段目录下不是预设的名字（撞上它们 = 把索引 / 词表当预设编辑）。
const RESERVED: [&str; 10] = [
    "categories.json",
    "index.json",
    "index.mjs",
    "preset.json",
    "ultimate.json",
    "PRESET.md",
    "README.md",
    "AGENTS.md",
    "test",
    "node_modules",
];

/// 按段的校验器验形状。`dir_name` 有值（盘上条目）时要求 `name` 等于目录名。
pub fn problems_for(
    section: &Section,
    raw: &Value,
    dir_name: Option<&str>,
) -> Vec<String> {
    match section.checker {
        Checker::Preset => shape_problems(raw, dir_name, &section.manifest),
        Checker::Ultimate => ultimate_shape_problems(raw, dir_name, &section.manifest),
    }
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// 会话模板的形状校验（不看引用）。返回问题清单，空 = 通过。
pub fn shape_problems(raw: &Value, dir_name: Option<&str>, manifest: &str) -> Vec<String> {
    let where_ = manifest;
    let mut problems = Vec::new();
    let Some(object) = raw.as_object() else {
        return vec![format!("{where_} 得是一个对象")];
    };

    for key in object.keys() {
        if !PRESET_KEYS.contains(&key.as_str()) {
            problems.push(format!(
                "{where_} 里有不认识的键「{key}」—— 不忽略它（拼错的键等于那条设置没生效）"
            ));
        }
    }
    if raw.get("v").and_then(Value::as_i64) != Some(1) {
        problems.push(format!(
            "{where_} 的 v 得是 1，收到 {}",
            raw.get("v").map(Value::to_string).unwrap_or_else(|| "null".into())
        ));
    }

    let name = raw.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if let Some(dir_name) = dir_name {
        if !name.is_empty() && name != dir_name {
            problems.push(format!(
                "{where_} 的 name 是「{name}」—— 必须等于目录名「{dir_name}」"
            ));
        }
    } else if name.is_empty() {
        problems.push(format!("{where_} 缺 name"));
    }
    if !name.is_empty() {
        if let Err(problem) = check_name(name) {
            problems.push(format!("{where_} 的 {problem}"));
        }
    }

    let summary = raw.get("summary").and_then(Value::as_str).unwrap_or("").trim();
    if summary.is_empty() {
        problems.push(format!("{where_} 的 summary 为空 —— 人靠这一行决定开不开它"));
    } else if summary.chars().count() > MAX_SUMMARY {
        problems.push(format!("{where_} 的 summary 太长（最多 {MAX_SUMMARY} 字）"));
    }

    if let Some(description) = raw.get("description") {
        match description.as_str() {
            None => problems.push(format!("{where_} 的 description 得是字符串")),
            Some(text) if text.chars().count() > MAX_DESCRIPTION => {
                problems.push(format!("{where_} 的 description 太长（最多 {MAX_DESCRIPTION} 字）"));
            }
            Some(_) => {}
        }
    }

    match raw.get("category") {
        None => problems.push(format!("{where_} 缺 category 数组（没有就写 []）")),
        Some(Value::Array(items)) => {
            if items.iter().any(|item| !item.is_string()) {
                problems.push(format!("{where_} 的 category 得是字符串数组"));
            }
        }
        Some(_) => problems.push(format!("{where_} 缺 category 数组（没有就写 []）")),
    }

    if let Some(model) = raw.get("model") {
        match model {
            Value::Object(fields) => {
                let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
                keys.sort_unstable();
                if keys != ["modelId", "provider"] {
                    problems.push(format!("{where_} 的 model 只能有 provider 和 modelId 两个键"));
                }
                if non_empty_string(fields.get("provider")).is_none() {
                    problems.push(format!("{where_} 的 model.provider 为空"));
                }
                if non_empty_string(fields.get("modelId")).is_none() {
                    problems.push(format!("{where_} 的 model.modelId 为空"));
                }
            }
            _ => problems.push(format!(
                "{where_} 的 model 得是 {{provider, modelId}} 对象，或者整个键不写（= 跟随设备默认）"
            )),
        }
    }

    if let Some(level) = raw.get("thinkingLevel") {
        let ok = level.as_str().is_some_and(|text| THINKING_LEVELS.contains(&text));
        if !ok {
            problems.push(format!(
                "{where_} 的 thinkingLevel「{}」不在 {} 里",
                level,
                THINKING_LEVELS.join(" / ")
            ));
        }
    }

    for field in ["skills", "extensions"] {
        let Some(value) = raw.get(field) else { continue };
        match value {
            Value::Array(items) => {
                if items.iter().any(|item| non_empty_string(Some(item)).is_none()) {
                    problems.push(format!("{where_} 的 {field} 里有空的 / 不是字符串的项"));
                }
            }
            _ => problems.push(format!(
                "{where_} 的 {field} 得是字符串数组（一个都不要写 []，跟随默认就整个键不写）"
            )),
        }
    }

    // 三态：键在且是字符串 = 选它；键在且是 null = 显式不要；键不在 = 跟随仓库默认。
    if let Some(template) = raw.get("template") {
        if !template.is_null() && non_empty_string(Some(template)).is_none() {
            problems.push(format!("{where_} 的 template 得是模板名，或者 null（明确不要）"));
        }
    }

    if let Some(read_only) = raw.get("readOnly") {
        if !read_only.is_boolean() {
            problems.push(format!("{where_} 的 readOnly 得是 true / false"));
        }
    }

    if let Some(cwd) = raw.get("cwd") {
        if !cwd.is_null() && non_empty_string(Some(cwd)).is_none() {
            problems.push(format!(
                "{where_} 的 cwd 得是绝对路径字符串，或者 null（明确用缺省）"
            ));
        }
    }
    if let Some(machine) = raw.get("machine") {
        if !machine.is_null() && non_empty_string(Some(machine)).is_none() {
            problems.push(format!(
                "{where_} 的 machine 得是机器名字符串，或者 null（明确用本机）"
            ));
        }
    }

    problems
}

/// 终极模板的形状校验（不看引用）。
pub fn ultimate_shape_problems(raw: &Value, dir_name: Option<&str>, manifest: &str) -> Vec<String> {
    let where_ = manifest;
    let mut problems = Vec::new();
    let Some(object) = raw.as_object() else {
        return vec![format!("{where_} 得是一个对象")];
    };

    for key in object.keys() {
        if !ULTIMATE_KEYS.contains(&key.as_str()) {
            problems.push(format!(
                "{where_} 里有不认识的键「{key}」—— 不忽略它（拼错的键等于那条设置没生效）"
            ));
        }
    }
    if raw.get("v").and_then(Value::as_i64) != Some(1) {
        problems.push(format!(
            "{where_} 的 v 得是 1，收到 {}",
            raw.get("v").map(Value::to_string).unwrap_or_else(|| "null".into())
        ));
    }

    let name = raw.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if let Some(dir_name) = dir_name {
        if !name.is_empty() && name != dir_name {
            problems.push(format!(
                "{where_} 的 name 是「{name}」—— 必须等于目录名「{dir_name}」"
            ));
        }
    } else if name.is_empty() {
        problems.push(format!("{where_} 缺 name"));
    }
    if !name.is_empty() {
        if let Err(problem) = check_name(name) {
            problems.push(format!("{where_} 的 {problem}"));
        }
    }

    let summary = raw.get("summary").and_then(Value::as_str).unwrap_or("").trim();
    if summary.is_empty() {
        problems.push(format!("{where_} 的 summary 为空 —— 人靠这一行决定开不开它"));
    } else if summary.chars().count() > MAX_SUMMARY {
        problems.push(format!("{where_} 的 summary 太长（最多 {MAX_SUMMARY} 字）"));
    }

    if let Some(description) = raw.get("description") {
        match description.as_str() {
            None => problems.push(format!("{where_} 的 description 得是字符串")),
            Some(text) if text.chars().count() > MAX_DESCRIPTION => {
                problems.push(format!("{where_} 的 description 太长（最多 {MAX_DESCRIPTION} 字）"));
            }
            Some(_) => {}
        }
    }

    match raw.get("category") {
        None => problems.push(format!("{where_} 缺 category 数组（没有就写 []）")),
        Some(Value::Array(items)) => {
            if items.iter().any(|item| !item.is_string()) {
                problems.push(format!("{where_} 的 category 得是字符串数组"));
            }
        }
        Some(_) => problems.push(format!("{where_} 缺 category 数组（没有就写 []）")),
    }

    if non_empty_string(raw.get("trigger")).is_none() {
        problems.push(format!("{where_} 缺 trigger —— 这条得挂在某个触发器上"));
    }

    match raw.get("params") {
        None => problems.push(format!(
            "{where_} 的 params 得是一个对象（键是参数名，值是参数值）"
        )),
        Some(Value::Object(params)) => {
            for (key, value) in params {
                if !param_key_ok(key) {
                    problems.push(format!(
                        "{where_} 的参数名「{key}」不合格：小写字母 / 数字开头，只留 _ -"
                    ));
                }
                if !matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
                    problems.push(format!("{where_} 的参数「{key}」的值得是字符串 / 数字 / 布尔"));
                }
            }
        }
        Some(_) => problems.push(format!(
            "{where_} 的 params 得是一个对象（键是参数名，值是参数值）"
        )),
    }

    if non_empty_string(raw.get("session")).is_none() {
        problems.push(format!(
            "{where_} 缺 session —— 得钉一条会话模板（Session-Template 段里的名字）"
        ));
    }
    if let Some(enabled) = raw.get("enabled") {
        if !enabled.is_boolean() {
            problems.push(format!("{where_} 的 enabled 得是 true / false"));
        }
    }

    problems
}

fn param_key_ok(key: &str) -> bool {
    let mut chars = key.chars();
    let head_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    head_ok
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn good() -> Value {
        json!({
            "v": 1,
            "name": "deep-dive",
            "summary": "挖到底",
            "category": ["工程"],
            "model": { "provider": "volc-ark", "modelId": "x" },
            "thinkingLevel": "medium",
            "skills": ["deep-dive"],
            "template": "smart-ass"
        })
    }

    #[test]
    fn 形状对的没有一条问题() {
        assert!(shape_problems(&good(), Some("deep-dive"), "preset.json").is_empty());
    }

    #[test]
    fn 未知键必须报错() {
        let mut value = good();
        value["thinking"] = json!("medium"); // 拼错的键
        let problems = shape_problems(&value, Some("deep-dive"), "preset.json");
        assert!(problems.iter().any(|p| p.contains("不认识的键「thinking」")), "{problems:?}");
    }

    #[test]
    fn 三态都合法() {
        for template in [json!(null), json!("smart-ass")] {
            let mut value = good();
            value["template"] = template;
            assert!(shape_problems(&value, Some("deep-dive"), "preset.json").is_empty());
        }
        let mut value = good();
        value.as_object_mut().unwrap().remove("template");
        assert!(shape_problems(&value, Some("deep-dive"), "preset.json").is_empty());
    }

    #[test]
    fn 名字必须等于目录名() {
        let problems = shape_problems(&good(), Some("other"), "preset.json");
        assert!(problems.iter().any(|p| p.contains("必须等于目录名")), "{problems:?}");
    }

    #[test]
    fn 模型只能两个键() {
        let mut value = good();
        value["model"] = json!({ "provider": "a", "modelId": "b", "x": 1 });
        let problems = shape_problems(&value, Some("deep-dive"), "preset.json");
        assert!(problems.iter().any(|p| p.contains("只能有 provider 和 modelId")), "{problems:?}");
    }

    #[test]
    fn 思考档必须在表里() {
        let mut value = good();
        value["thinkingLevel"] = json!("最高");
        let problems = shape_problems(&value, Some("deep-dive"), "preset.json");
        assert!(problems.iter().any(|p| p.contains("thinkingLevel")), "{problems:?}");
    }

    #[test]
    fn 终极模板形状() {
        let good_ultimate = json!({
            "v": 1, "name": "nightly", "summary": "每晚跑", "category": [],
            "trigger": "single-shot", "params": { "epoch": 1800000060 }, "session": "scratch"
        });
        assert!(ultimate_shape_problems(&good_ultimate, Some("nightly"), "ultimate.json").is_empty());

        let mut bad = good_ultimate.clone();
        bad["params"] = json!({ "Epoch": [1] });
        let problems = ultimate_shape_problems(&bad, Some("nightly"), "ultimate.json");
        assert!(problems.iter().any(|p| p.contains("参数名")), "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("值得是")), "{problems:?}");
    }

    #[test]
    fn 名字闸() {
        assert!(check_name("deep-dive").is_ok());
        assert!(check_name("a.b_c-d").is_ok());
        assert!(check_name("Deep").is_err());
        assert!(check_name("-x").is_err());
        assert!(check_name("").is_err());
        assert!(check_name("preset.json").is_err());
        assert!(check_name(&"a".repeat(65)).is_err());
    }
}
