// 红：这一片先只放测试，src/lib.rs 还没有；cargo 应当因为找不到 crate 而红。
//! #48 第一片：`DocumentKind` 的编译期字段 + 校验形状。
//!
//! 接缝：documents crate 的公开类型接口（不经过 HTTP）。

use nous_documents::kinds::{presets::PresetFields, prompts::PromptFields};
use nous_documents::{DocumentKind, Issue, Level, ValidationCtx};
use serde_json::json;

#[test]
fn typed_fields_deserialize_only_their_own_schema() {
    let preset: PresetFields = serde_json::from_value(json!({
        "name": "desk", "summary": "一句话", "description": "说明书",
        "category": ["写作"], "spec": {"model": {"provider": "p"}}
    }))
    .unwrap();
    assert_eq!(preset.name, "desk");
    assert_eq!(preset.category, vec!["写作".to_string()]);

    let prompt: PromptFields = serde_json::from_value(json!({
        "name": "greet", "text": "你好", "tags": ["zh"]
    }))
    .unwrap();
    assert_eq!(prompt.text, "你好");

    // 写错字段名（或塞进别的 kind 的字段）必须在反序列化时失败 —— 字段是编译期结构体。
    let bad = serde_json::from_value::<PresetFields>(json!({
        "name": "desk", "summary": "", "description": "", "text": "不是 preset 的字段"
    }));
    assert!(bad.is_err(), "typed fields 不该吞掉陌生字段");
}

#[test]
fn validation_shape_matches_desk_preset_validation() {
    // 故意写坏：summary 空、category 空、引用不存在。
    let preset = PresetFields {
        name: "broken".into(),
        summary: "".into(),
        description: "说明".into(),
        category: vec![],
        spec: Some(json!({
            "session": "missing",
            "skills": ["known"],
        })),
        body: None,
    };
    let ctx = ValidationCtx {
        names: vec!["known".to_string()].into_iter().collect(),
    };
    let issues = preset.validate(&ctx);
    assert!(issues.iter().any(|issue| issue.path == "summary"
        && issue.level == Level::Error
        && issue.message.contains("required")));
    assert!(issues.iter().any(|issue| issue.path == "category"
        && issue.level == Level::Warning));
    let report = nous_documents::ValidationReport::of::<PresetFields>(&preset, &ctx);
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["ok"], json!(false));
    assert!(value["errors"].is_array());
    assert!(value["warnings"].is_array());
    assert!(value["resolved"].is_object());
    assert!(value["errors"][0].get("path").is_some());
    assert!(value["errors"][0].get("message").is_some());
    assert_eq!(value["resolved"]["references"]["missing"]["state"], json!("invalid"));
    assert_eq!(value["resolved"]["references"]["known"]["state"], json!("valid"));
    assert_eq!(value["resolved"]["references"]["missing"]["state"], json!("invalid"));
}

#[test]
fn prompt_kind_shares_the_same_validation_machinery() {
    let prompt = PromptFields {
        name: "greet".into(),
        text: "".into(),
        summary: None,
        tags: vec![],
    };
    let ctx = ValidationCtx::default();
    let report = nous_documents::ValidationReport::of::<PromptFields>(&prompt, &ctx);
    assert_eq!(report.ok, false);
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["errors"][0]["path"], json!("text"));
    assert!(value["resolved"].is_object());
}

fn _issue_shape(issue: Issue) -> (String, String, Level) {
    (issue.path, issue.message, issue.level)
}
