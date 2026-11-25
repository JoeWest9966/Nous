//! 引用解析：这条预设**实际会跑成什么**、现在还成不成立。
//!
//! 判据全在这一层（面板 / 建会话 / 校验端点三处共用），客户端只抄 `state`，不自己拼规则。
//! 三态 `ok` / `auto` / `invalid` —— `auto` 只属于「没钉模型」那一维。
//!
//! 形状坏了的条目不 resolve（那是 `specError`，不是 `invalid`）；引用坏才是「现在开不了」。

use std::collections::BTreeSet;

use serde_json::{json, Value};

/// 编目里的一个模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelLevels {
    pub provider: String,
    pub model_id: String,
    pub thinking_levels: Vec<String>,
}

/// 触发器声明（`NOUS_TRIGGER/<code>/nous.json` 的 `params[]`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerDecl {
    pub code: String,
    pub summary: Option<String>,
    pub version: Option<i64>,
    pub params: Vec<ParamDecl>,
}

/// 触发器的一个参数声明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDecl {
    pub name: String,
    /// `epoch` / `number` / `string` / `text` / `boolean` / `select`。
    pub kind: String,
    pub label: Option<String>,
    pub required: bool,
    pub options: Vec<String>,
}

/// 解析要问的那几件事。**全是数据，不是闭包** —— 这样它能裸跑单测。
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub skills: BTreeSet<String>,
    pub prompts: BTreeSet<String>,
    pub extensions: BTreeSet<String>,
    pub models: Vec<ModelLevels>,
    /// 设备默认模型（`provider` + `model_id`）。
    pub device_model: Option<(String, String)>,
    pub triggers: Vec<TriggerDecl>,
    /// `Session-Template` 段的名册（终极模板的 `session` 引用它）。
    pub sessions: BTreeSet<String>,
}

impl Ctx {
    fn model(&self, provider: &str, model_id: &str) -> Option<&ModelLevels> {
        self.models
            .iter()
            .find(|model| model.provider == provider && model.model_id == model_id)
    }

    fn levels(&self, provider: &str, model_id: &str) -> Vec<String> {
        self.model(provider, model_id)
            .map(|model| model.thinking_levels.clone())
            .unwrap_or_default()
    }

    fn trigger(&self, code: &str) -> Option<&TriggerDecl> {
        self.triggers.iter().find(|trigger| trigger.code == code)
    }
}

fn non_empty(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn model_ref(spec: &Value) -> Option<(String, String)> {
    let model = spec.get("model")?;
    let provider = non_empty(model.get("provider"))?;
    let model_id = non_empty(model.get("modelId"))?;
    Some((provider.to_string(), model_id.to_string()))
}

fn template_mode(spec: &Value, ctx: &Ctx) -> Value {
    let Some(template) = spec.get("template") else {
        return json!({ "state": "ok", "ref": null, "value": null, "mode": "default" });
    };
    if template.is_null() {
        return json!({ "state": "ok", "ref": null, "value": null, "mode": "none" });
    }
    let name = template.as_str().unwrap_or("").trim();
    if name == "auto" {
        // 唯一被规范化掉的魔法值：等于「跟随默认」，也就是键缺席。
        return json!({ "state": "ok", "ref": null, "value": null, "mode": "default" });
    }
    let state = if ctx.prompts.contains(name) { "ok" } else { "invalid" };
    json!({ "state": state, "ref": name, "value": name, "mode": "picked" })
}

/// `cwd` / `machine` 三态：键不写 = 跟随默认，`null` = 显式不要，字符串 = 钉死。
fn pinned_mode(spec: &Value, key: &str) -> Value {
    match spec.get(key) {
        None => json!({ "state": "ok", "ref": null, "value": null, "mode": "default" }),
        Some(Value::Null) => json!({ "state": "ok", "ref": null, "value": null, "mode": "none" }),
        Some(Value::String(text)) => {
            json!({ "state": "ok", "ref": text, "value": text, "mode": "picked" })
        }
        Some(_) => json!({ "state": "invalid", "ref": null, "value": null, "mode": "picked" }),
    }
}

/// 面板要的那份「这条预设实际会跑成什么」。
///
/// `resolved.*.state` 的三种值：
///   `ok`      —— 钉住了 / 走默认，两种都成立；
///   `auto`    —— 只有 `model` 有：没钉模型，跟设备默认走；
///   `invalid` —— 引用失效，现在开不了。
pub fn resolve_spec(spec: &Value, ctx: &Ctx) -> Value {
    let picked = model_ref(spec);

    let model = match &picked {
        Some((provider, model_id)) => {
            let state = if ctx.model(provider, model_id).is_some() { "ok" } else { "invalid" };
            json!({
                "state": state,
                "ref": format!("{provider}/{model_id}"),
                "value": format!("{provider}/{model_id}"),
                "thinkingLevels": ctx.levels(provider, model_id),
            })
        }
        None => {
            let (value, levels) = match &ctx.device_model {
                Some((provider, model_id)) => (
                    Some(format!("{provider}/{model_id}")),
                    ctx.levels(provider, model_id),
                ),
                None => (None, Vec::new()),
            };
            json!({
                "state": "auto",
                "ref": null,
                "value": value,
                "thinkingLevels": levels,
            })
        }
    };

    let wanted = non_empty(spec.get("thinkingLevel"));
    let thinking = match wanted {
        None => json!({ "state": "ok", "ref": null, "value": null }),
        Some(level) => {
            let levels: Vec<String> = model
                .get("thinkingLevels")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            // 钉了模型但档位不在它那几档：**不改值**，报红 —— 静默夹档正是要防的那种谎。
            let ok = picked.is_none() || levels.is_empty() || levels.iter().any(|item| item == level);
            if ok {
                json!({ "state": "ok", "ref": level, "value": level })
            } else {
                json!({ "state": "invalid", "ref": level, "value": null })
            }
        }
    };

    let rows = |list: Option<&Value>, check: &dyn Fn(&str) -> bool| -> Vec<Value> {
        list.and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|name| {
                        let state = if check(name) { "ok" } else { "invalid" };
                        json!({ "state": state, "ref": name })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let skills = rows(spec.get("skills"), &|name| ctx.skills.contains(name));
    // 名牌不是扩展：`prompt:<名>` / `skill:<名>` 有各自的字段，混进来一定是写错了。
    let extensions = rows(spec.get("extensions"), &|name| {
        !name.contains(':') && ctx.extensions.contains(name)
    });

    json!({
        "name": spec.get("name").cloned().unwrap_or(Value::Null),
        "model": model,
        "thinking": thinking,
        "skills": skills,
        "extensions": extensions,
        "template": template_mode(spec, ctx),
        "readOnly": spec.get("readOnly").and_then(Value::as_bool).unwrap_or(false),
        "cwd": pinned_mode(spec, "cwd"),
        "machine": pinned_mode(spec, "machine"),
        // 「键在不在」的八个事实，给面板画 chip 用（`默认` / `都不要` / `N 条`）。
        "absence": {
            "skills": !spec.as_object().is_some_and(|o| o.contains_key("skills")),
            "extensions": !spec.as_object().is_some_and(|o| o.contains_key("extensions")),
            "template": !spec.as_object().is_some_and(|o| o.contains_key("template")),
            "model": !spec.as_object().is_some_and(|o| o.contains_key("model")),
            "thinkingLevel": !spec.as_object().is_some_and(|o| o.contains_key("thinkingLevel")),
            "readOnly": !spec.as_object().is_some_and(|o| o.contains_key("readOnly")),
            "cwd": !spec.as_object().is_some_and(|o| o.contains_key("cwd")),
            "machine": !spec.as_object().is_some_and(|o| o.contains_key("machine")),
        },
    })
}

/// 引用校验（给人看的错误清单）。跟 [`resolve_spec`] 是同一套判据的两种出口：
/// 一个给图/开关，一个给编辑框的红字。
pub fn reference_problems(spec: &Value, ctx: &Ctx) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if let Some(skills) = spec.get("skills").and_then(Value::as_array) {
        for name in skills.iter().filter_map(Value::as_str) {
            if !ctx.skills.contains(name) {
                errors.push(format!("技能「{name}」没装（NOUS_SKILLS 里没有它）"));
            }
        }
    }
    if let Some(extensions) = spec.get("extensions").and_then(Value::as_array) {
        for name in extensions.iter().filter_map(Value::as_str) {
            if name.contains(':') {
                errors.push(format!("「{name}」是名牌，不是扩展 —— 模板写 template，技能写 skills"));
            } else if !ctx.extensions.contains(name) {
                errors.push(format!("扩展「{name}」没装（registry 里没有它）"));
            }
        }
    }
    if let Some(template) = spec.get("template").and_then(Value::as_str) {
        if template != "auto" && !ctx.prompts.contains(template) {
            errors.push(format!("模板「{template}」不在 NOUS_PROMPTS 里"));
        }
    }

    let picked = model_ref(spec);
    if let Some((provider, model_id)) = &picked {
        if ctx.model(provider, model_id).is_none() {
            errors.push(format!("模型「{provider}/{model_id}」不在编目里"));
        }
    }
    if let Some(level) = non_empty(spec.get("thinkingLevel")) {
        match &picked {
            None => warnings.push(format!(
                "档位「{level}」会按设备默认模型重新夹（这条没钉模型）"
            )),
            Some((provider, model_id)) => {
                let levels = ctx.levels(provider, model_id);
                if !levels.is_empty() && !levels.iter().any(|item| item == level) {
                    errors.push(format!(
                        "档位「{level}」不在 {model_id} 支持的档位里（{}）",
                        if levels.is_empty() { "无".to_string() } else { levels.join(" / ") }
                    ));
                }
            }
        }
    }

    (errors, warnings)
}

fn param_value_problem(param: &ParamDecl, value: &Value) -> Option<String> {
    match param.kind.as_str() {
        "epoch" => match value.as_i64() {
            Some(seconds) if seconds >= 0 => None,
            _ => Some("得是 UTC 秒（正整数）".into()),
        },
        "number" => match value.as_f64() {
            Some(number) if number.is_finite() => None,
            _ => Some("得是数字".into()),
        },
        "boolean" => value.is_boolean().then_some(()).map_or(Some("得是 true / false".into()), |_| None),
        "select" => match value.as_str() {
            None => Some("得是字符串".into()),
            Some(text) if !param.options.is_empty() && !param.options.iter().any(|o| o == text) => {
                Some(format!("只能是 {}", param.options.join(" / ")))
            }
            Some(_) => None,
        },
        _ => value
            .is_string()
            .then_some(())
            .map_or(Some("得是字符串".into()), |_| None),
    }
}

/// 终极模板的解析：触发器在不在、参数跟它的声明对不对得上、会话模板还在不在。
pub fn resolve_ultimate(spec: &Value, ctx: &Ctx) -> Value {
    let code = spec.get("trigger").and_then(Value::as_str).unwrap_or("");
    let trigger = ctx.trigger(code);
    let schema = trigger.map(|trigger| trigger.params.as_slice()).unwrap_or(&[]);
    let given = spec.get("params").and_then(Value::as_object);

    let params: Vec<Value> = schema
        .iter()
        .map(|param| {
            let value = given.and_then(|params| params.get(&param.name));
            match value {
                None => json!({
                    "name": param.name,
                    "state": if param.required { "missing" } else { "default" },
                    "value": null,
                    "label": param.label.clone().unwrap_or_else(|| param.name.clone()),
                }),
                Some(value) => {
                    let problem = param_value_problem(param, value);
                    json!({
                        "name": param.name,
                        "state": if problem.is_some() { "invalid" } else { "ok" },
                        "value": value,
                        "label": param.label.clone().unwrap_or_else(|| param.name.clone()),
                        "problem": problem,
                    })
                }
            }
        })
        .collect();

    // 参数名对不上了（触发器改了声明）—— 跟「值不合格」分开说：那是去改**触发器**。
    let unknown: Vec<String> = given
        .map(|params| {
            params
                .keys()
                .filter(|key| !schema.iter().any(|param| &param.name == *key))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let session = spec.get("session").and_then(Value::as_str).unwrap_or("");
    json!({
        "name": spec.get("name").cloned().unwrap_or(Value::Null),
        "trigger": {
            "state": if trigger.is_some() { "ok" } else { "invalid" },
            "ref": spec.get("trigger").cloned().unwrap_or(Value::Null),
            "value": trigger.and_then(|trigger| trigger.summary.clone()),
            "version": trigger.and_then(|trigger| trigger.version),
            "params": params,
            "unknown": unknown,
        },
        "session": {
            "state": if ctx.sessions.contains(session) { "ok" } else { "invalid" },
            "ref": spec.get("session").cloned().unwrap_or(Value::Null),
        },
        "enabled": spec.get("enabled").and_then(Value::as_bool).unwrap_or(true),
    })
}

/// 终极模板的引用错误（给新建 / 写入闸用）：存一条引用已经坏掉的行 = 造一条红卡。
pub fn ultimate_reference_problems(spec: &Value, ctx: &Ctx) -> Vec<String> {
    let mut errors = Vec::new();
    let code = spec.get("trigger").and_then(Value::as_str).unwrap_or("");
    match ctx.trigger(code) {
        None => errors.push(format!("触发器「{code}」不在 NOUS_TRIGGER 里")),
        Some(trigger) => {
            let given = spec.get("params").and_then(Value::as_object);
            for param in &trigger.params {
                let Some(value) = given.and_then(|params| params.get(&param.name)) else {
                    if param.required {
                        errors.push(format!("触发器「{code}」要参数「{}」，这条没给", param.name));
                    }
                    continue;
                };
                if let Some(problem) = param_value_problem(param, value) {
                    errors.push(format!("参数「{}」：{problem}", param.name));
                }
            }
            for key in given.map(|params| params.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
                if !trigger.params.iter().any(|param| param.name == key) {
                    // 触发器改了参数名，这条还留着旧的 —— 报出来，让人去改**它**。
                    errors.push(format!(
                        "触发器「{code}」没有参数「{key}」了 —— 参数名变了，这条得改"
                    ));
                }
            }
        }
    }
    let session = spec.get("session").and_then(Value::as_str).unwrap_or("");
    if !ctx.sessions.contains(session) {
        errors.push(format!("会话模板「{session}」不在 Session-Template 里"));
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Ctx {
        Ctx {
            skills: ["deep-dive".to_string()].into_iter().collect(),
            prompts: ["smart-ass".to_string()].into_iter().collect(),
            extensions: ["probe".to_string()].into_iter().collect(),
            models: vec![ModelLevels {
                provider: "volc-ark".into(),
                model_id: "flash".into(),
                thinking_levels: vec!["off".into(), "medium".into()],
            }],
            device_model: Some(("volc-ark".into(), "flash".into())),
            triggers: vec![TriggerDecl {
                code: "single-shot".into(),
                summary: Some("一次".into()),
                version: Some(3),
                params: vec![ParamDecl {
                    name: "epoch".into(),
                    kind: "epoch".into(),
                    label: Some("什么时候".into()),
                    required: true,
                    options: vec![],
                }],
            }],
            sessions: ["scratch".to_string()].into_iter().collect(),
        }
    }

    fn spec() -> Value {
        json!({
            "v": 1, "name": "deep-dive", "summary": "挖到底", "category": [],
            "model": { "provider": "volc-ark", "modelId": "flash" },
            "thinkingLevel": "medium",
            "skills": ["deep-dive", "gone"],
            "extensions": ["probe", "prompt:x"],
            "template": "smart-ass",
        })
    }

    #[test]
    fn 七维状态() {
        let resolved = resolve_spec(&spec(), &ctx());
        assert_eq!(resolved["model"]["state"], "ok");
        assert_eq!(resolved["thinking"]["state"], "ok");
        assert_eq!(resolved["skills"][0]["state"], "ok");
        assert_eq!(resolved["skills"][1]["state"], "invalid");
        assert_eq!(resolved["extensions"][0]["state"], "ok");
        assert_eq!(resolved["extensions"][1]["state"], "invalid", "名牌不是扩展");
        assert_eq!(resolved["template"]["state"], "ok");
        assert_eq!(resolved["template"]["mode"], "picked");
    }

    #[test]
    fn 没钉模型就是auto() {
        let mut value = spec();
        value.as_object_mut().unwrap().remove("model");
        value.as_object_mut().unwrap().remove("thinkingLevel");
        let resolved = resolve_spec(&value, &ctx());
        assert_eq!(resolved["model"]["state"], "auto");
        assert_eq!(resolved["model"]["value"], "volc-ark/flash");
        assert_eq!(resolved["thinking"]["state"], "ok");
    }

    #[test]
    fn 档位不在模型那几档就报红且不改值() {
        let mut value = spec();
        value["thinkingLevel"] = json!("high");
        let resolved = resolve_spec(&value, &ctx());
        assert_eq!(resolved["thinking"]["state"], "invalid");
        assert_eq!(resolved["thinking"]["ref"], "high");
        assert!(resolved["thinking"]["value"].is_null());
    }

    #[test]
    fn 三态展开成四种可读形状() {
        let mut value = spec();
        value.as_object_mut().unwrap().remove("template");
        assert_eq!(resolve_spec(&value, &ctx())["template"]["mode"], "default");
        value["template"] = json!(null);
        assert_eq!(resolve_spec(&value, &ctx())["template"]["mode"], "none");
        value["template"] = json!("auto");
        assert_eq!(resolve_spec(&value, &ctx())["template"]["mode"], "default");
        value["template"] = json!("gone");
        let resolved = resolve_spec(&value, &ctx());
        assert_eq!(resolved["template"]["mode"], "picked");
        assert_eq!(resolved["template"]["state"], "invalid");
    }

    #[test]
    fn 引用错误清单() {
        let (errors, warnings) = reference_problems(&spec(), &ctx());
        assert!(errors.iter().any(|e| e.contains("技能「gone」")), "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("是名牌")), "{errors:?}");
        assert!(warnings.is_empty(), "{warnings:?}");

        let mut value = spec();
        value.as_object_mut().unwrap().remove("model");
        let (_, warnings) = reference_problems(&value, &ctx());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn 终极模板解析() {
        let ultimate = json!({
            "v": 1, "name": "nightly", "summary": "每晚", "category": [],
            "trigger": "single-shot", "params": { "epoch": 1800000060, "gone": 1 },
            "session": "scratch"
        });
        let resolved = resolve_ultimate(&ultimate, &ctx());
        assert_eq!(resolved["trigger"]["state"], "ok");
        assert_eq!(resolved["trigger"]["version"], 3);
        assert_eq!(resolved["trigger"]["params"][0]["state"], "ok");
        assert_eq!(resolved["trigger"]["unknown"], json!(["gone"]));
        assert_eq!(resolved["session"]["state"], "ok");
        assert_eq!(resolved["enabled"], true);
    }

    #[test]
    fn 终极模板引用错误() {
        let ultimate = json!({
            "v": 1, "name": "nightly", "summary": "每晚", "category": [],
            "trigger": "nope", "params": {}, "session": "gone"
        });
        let errors = ultimate_reference_problems(&ultimate, &ctx());
        assert!(errors.iter().any(|e| e.contains("不在 NOUS_TRIGGER")), "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("不在 Session-Template")), "{errors:?}");
    }
}
