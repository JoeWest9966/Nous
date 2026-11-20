//! #72 判据 1/2/3：真读 PROMPT.md + settings.default，render 是纯函数。

use std::collections::BTreeMap;
use std::fs;

use nous_prompts::{catalog_value, read_template, render};
use serde_json::json;

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("nous-prompts-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn listing_reports_the_missing_prompt_file_on_the_second_template() {
    let root = temp_root("listing");
    fs::create_dir_all(root.join("complete")).unwrap();
    fs::write(root.join("complete/PROMPT.md"), "你是一名助手").unwrap();
    fs::write(root.join("complete/nous.json"), json!({"description": "完整模板", "category": ["写作"]}).to_string()).unwrap();
    fs::create_dir_all(root.join("broken")).unwrap();

    let body = catalog_value(&root);
    let items = body["prompts"].as_array().expect("prompts 是数组");
    assert_eq!(items.len(), 2, "两个目录两条：{body}");
    let broken = items.iter().find(|item| item["name"] == json!("broken")).expect("broken 在列表里");
    let problem = broken["problem"].as_str().unwrap_or_default();
    assert!(problem.contains("PROMPT.md"), "缺文件要说清是哪个：{problem}");
}

#[test]
fn settings_default_names_a_missing_template_instead_of_returning_null() {
    let root = temp_root("default");
    fs::write(root.join("settings.json"), json!({"default": "ghost"}).to_string()).unwrap();
    let body = catalog_value(&root);
    assert_eq!(body["default"], json!("ghost"), "配置错了也要如实回名字");
    let problem = body["problem"].as_str().unwrap_or_default();
    assert!(problem.contains("不存在"), "要点名配置指向不存在的模板：{problem}");
}

#[test]
fn render_is_pure_and_deterministic() {
    let mut vars = BTreeMap::new();
    vars.insert("name".to_string(), "小 nous".to_string());
    let body = "你好 {{name}}，请保持 {{name}} 的风格。";
    let first = render(body, &vars);
    let second = render(body, &vars);
    assert_eq!(first, second, "同输入两次调用逐字相同");
    assert_eq!(first, "你好 小 nous，请保持 小 nous 的风格。", "变量被替换");
    assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/render.rs").exists());
    let source = fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/render.rs")).unwrap();
    for needle in ["std::fs", "std::env", "SystemTime"] {
        assert!(!source.contains(needle), "render.rs 不许出现 {needle}");
    }
    let _ = read_template(&root_never_read(), "x");
}

fn root_never_read() -> std::path::PathBuf {
    std::path::PathBuf::from("/nonexistent-render-test")
}
