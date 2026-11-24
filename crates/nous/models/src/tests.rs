//! 纯函数测试：解析 / 合并 / 投影 / 密钥引用形状。不读盘、不联网。

use nous_elements::ElementItem;
use serde_json::{json, Value};

use crate::{
    find_group, load, merge_into, parse_key_ref, resolve_key, KeyLookup, KeyRef, KeyShape, GROUP_ID,
};

fn element(id: &str, fields: Value) -> ElementItem {
    ElementItem { id: id.to_string(), fields, etag: format!("etag-{id}") }
}

fn group(packages: &[&str], extra: Value) -> ElementItem {
    let mut fields = json!({
        "id": GROUP_ID,
        "name": "Nous 模型入口",
        "packages": packages,
    });
    if let (Value::Object(base), Value::Object(extra)) = (&mut fields, extra) {
        for (key, value) in extra {
            base.insert(key, value);
        }
    }
    element(GROUP_ID, fields)
}

fn provider(id: &str, provider: &str, models: Value) -> ElementItem {
    element(
        id,
        json!({
            "id": id,
            "name": provider,
            "kind": "provider",
            "provider": provider,
            "baseUrl": "http://127.0.0.1:1/v1",
            "api": "openai-completions",
            "apiKey": "$TEST_KEY",
            "models": models,
        }),
    )
}

#[test]
fn one_group_one_provider_projects_items_and_default() {
    let group = group(
        &["provider-wb2api"],
        json!({ "default": "wb2api/cn:fast" }),
    );
    let package = provider(
        "provider-wb2api",
        "wb2api",
        json!([
            { "id": "cn:fast", "name": "Fast", "contextWindow": 256000, "maxTokens": 32000 },
            { "id": "cn:auto" },
        ]),
    );

    let snapshot = load(&group, &[package]);

    assert_eq!(snapshot.state, "ok", "{snapshot:?}");
    assert_eq!(snapshot.group.as_deref(), Some(GROUP_ID));
    assert_eq!(snapshot.items.len(), 2);
    assert_eq!(
        snapshot.items[0],
        crate::ModelItem {
            provider: "wb2api".into(),
            model_id: "cn:fast".into(),
            name: "Fast".into(),
            thinking_levels: vec!["off".into()],
            context_window: Some(256000),
            max_tokens: Some(32000),
        }
    );
    // 没有 name 的模型用 id 顶上，不许留空。
    assert_eq!(snapshot.items[1].name, "cn:auto");
    assert_eq!(
        snapshot.default,
        Some(crate::ModelRef { provider: "wb2api".into(), model_id: "cn:fast".into() })
    );
    assert_eq!(snapshot.providers.len(), 1);
    assert_eq!(snapshot.providers[0].package, "provider-wb2api");
    assert_eq!(snapshot.providers[0].base_url.as_deref(), Some("http://127.0.0.1:1/v1"));
    assert_eq!(snapshot.providers[0].key, KeyShape::Env);
    assert!(snapshot.missing.is_empty());
    assert!(snapshot.problems.is_empty());
}

#[test]
fn provider_key_falls_back_to_id_prefix() {
    let package = element(
        "provider-quickproxy",
        json!({ "id": "provider-quickproxy", "name": "Quick", "kind": "provider", "models": [] }),
    );
    let snapshot = load(&group(&["provider-quickproxy"], json!({})), &[package]);
    assert_eq!(snapshot.providers[0].provider, "quickproxy");
    // 没写 apiKey 就是 missing，不是明文、不是空串。
    assert_eq!(snapshot.providers[0].key, KeyShape::Missing);
    assert_eq!(snapshot.state, "missing");
    assert!(snapshot.problems.is_empty());
}

#[test]
fn model_override_merges_by_keys_and_replaces_arrays() {
    let base = provider(
        "provider-suanli",
        "suanli",
        json!([{
            "id": "qwen-vl-max",
            "name": "Qwen VL",
            "contextWindow": 32000,
            "input": ["text"],
            "compat": { "imageInput": "none", "keep": true },
            "thinkingLevelMap": { "off": null },
        }]),
    );
    let overlay = element(
        "qwen-vl-max",
        json!({
            "id": "qwen-vl-max",
            "name": "Qwen-VL-Max（覆盖）",
            "kind": "model",
            "provider": "suanli",
            "modelId": "qwen-vl-max",
            "contextWindow": 131072,
            "input": ["text", "image"],
            "compat": { "imageInput": "data-url" },
            "thinkingLevelMap": { "off": null, "low": "low" },
        }),
    );
    let snapshot = load(&group(&["provider-suanli", "qwen-vl-max"], json!({})), &[base, overlay]);

    assert_eq!(snapshot.state, "ok", "{snapshot:?}");
    let item = &snapshot.items[0];
    assert_eq!(item.context_window, Some(131072));
    assert_eq!(item.thinking_levels, vec!["off".to_string(), "low".to_string()]);
    // 覆盖包的 name 不压到模型名上（身份键不参与合并）。
    assert_eq!(item.name, "Qwen VL");
    assert_eq!(
        snapshot.packages.iter().find(|p| p.id == "qwen-vl-max").map(|p| p.state),
        Some("ok")
    );
}

#[test]
fn overlay_without_target_is_reported_not_swallowed() {
    let base = provider("provider-a", "a", json!([{ "id": "m1" }]));
    let overlay = element(
        "ghost",
        json!({ "id": "ghost", "name": "找不到目标", "kind": "model", "provider": "a", "modelId": "nope" }),
    );
    let snapshot = load(&group(&["provider-a", "ghost"], json!({})), &[base, overlay]);

    assert_eq!(snapshot.state, "problem");
    assert_eq!(snapshot.missing, vec!["nous-models/a/nope".to_string()]);
    assert_eq!(
        snapshot.packages.iter().find(|p| p.id == "ghost").map(|p| p.state),
        Some("missing")
    );
}

#[test]
fn missing_package_keeps_the_rest_of_the_catalog() {
    let base = provider("provider-a", "a", json!([{ "id": "m1" }]));
    let snapshot = load(&group(&["provider-a", "provider-gone"], json!({})), &[base]);

    assert_eq!(snapshot.items.len(), 1, "缺一个包不许把别的都吞了");
    assert_eq!(snapshot.missing, vec!["nous-models/provider-gone".to_string()]);
    assert_eq!(snapshot.packages.len(), 2);
    // 缺包不算 problem：编目还是能用的。
    assert_eq!(snapshot.state, "ok");
}

#[test]
fn default_outside_the_catalog_is_a_problem() {
    let base = provider("provider-a", "a", json!([{ "id": "m1" }]));
    let snapshot = load(&group(&["provider-a"], json!({ "default": "a/m2" })), &[base]);

    assert_eq!(snapshot.state, "problem");
    assert_eq!(snapshot.default, None);
    assert_eq!(snapshot.problems[0].field, "default");
    assert!(snapshot.problems[0].problem.contains("a/m2"));
}

#[test]
fn default_accepts_the_package_id_prefix_but_reports_the_provider_key() {
    let base = provider("provider-wb2api", "wb2api", json!([{ "id": "cn:fast" }]));
    let snapshot = load(
        &group(&["provider-wb2api"], json!({ "default": "provider-wb2api/cn:fast" })),
        &[base],
    );
    assert_eq!(snapshot.state, "ok", "{snapshot:?}");
    assert_eq!(
        snapshot.default,
        Some(crate::ModelRef { provider: "wb2api".into(), model_id: "cn:fast".into() })
    );
}

#[test]
fn plaintext_key_is_only_reported_as_a_shape() {
    let mut fields = json!({
        "id": "provider-x", "name": "X", "kind": "provider",
        "models": [{ "id": "m" }],
    });
    fields["apiKey"] = json!("sk-live-abcdef123456");
    let package = element("provider-x", fields);

    let snapshot = load(&group(&["provider-x"], json!({})), &[package]);
    assert_eq!(snapshot.providers[0].key, KeyShape::Plaintext);

    let rendered = serde_json::to_string(&snapshot).unwrap();
    assert!(!rendered.contains("sk-live"), "明文不许出现在任何响应里：{rendered}");
}

#[test]
fn unknown_kind_is_a_problem_and_is_listed() {
    let odd = element("someth", json!({ "id": "someth", "name": "?", "kind": "prompt" }));
    let snapshot = load(&group(&["someth"], json!({})), &[odd]);

    assert_eq!(snapshot.state, "problem");
    assert_eq!(snapshot.packages[0].state, "problem");
    assert_eq!(snapshot.problems[0].field, "kind");
}

#[test]
fn duplicate_reference_is_reported_once() {
    let base = provider("provider-a", "a", json!([{ "id": "m1" }]));
    let snapshot = load(&group(&["provider-a", "provider-a"], json!({})), &[base]);

    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.problems.len(), 1);
    assert_eq!(snapshot.problems[0].field, "packages");
}

#[test]
fn broken_reference_type_is_ignored_instead_of_panicking() {
    let snapshot = load(&group(&[], json!({ "packages": ["x", 3, null] })), &[]);
    // `packages` 的类型闸在 `nous-elements` 里；这里只保证不 panic、诚实空表。
    assert_eq!(snapshot.state, "missing");
    assert!(snapshot.items.is_empty());
    assert_eq!(snapshot.group.as_deref(), Some(GROUP_ID));
}

#[test]
fn find_group_only_matches_the_convention_id() {
    let items = [element("frontend", json!({ "id": "frontend", "name": "前端" })), group(&[], json!({}))];
    assert_eq!(find_group(&items).map(|item| item.id.as_str()), Some(GROUP_ID));
    assert!(find_group(&items[..1]).is_none());
}

#[test]
fn thinking_levels_prefer_explicit_then_map_then_reasoning() {
    let base = provider(
        "provider-a",
        "a",
        json!([
            { "id": "explicit", "thinkingLevels": ["off", "high"] },
            { "id": "mapped", "thinkingLevelMap": { "high": "high", "low": null, "medium": "medium" } },
            { "id": "reasoning", "reasoning": true },
            { "id": "plain" },
        ]),
    );
    let snapshot = load(&group(&["provider-a"], json!({})), &[base]);

    let levels: Vec<_> = snapshot
        .items
        .iter()
        .map(|item| (item.model_id.as_str(), item.thinking_levels.clone()))
        .collect();
    assert_eq!(levels[0].1, vec!["off".to_string(), "high".to_string()]);
    // 规范顺序，不信 JSON 里的书写顺序。
    assert_eq!(levels[1].1, vec!["low".to_string(), "medium".to_string(), "high".to_string()]);
    assert_eq!(
        levels[2].1,
        vec!["off".to_string(), "low".to_string(), "medium".to_string(), "high".to_string()]
    );
    assert_eq!(levels[3].1, vec!["off".to_string()]);
}

#[test]
fn merge_is_deep_for_objects_and_replaces_everything_else() {
    let mut base = json!({ "a": { "b": 1, "c": [1, 2] }, "keep": true });
    merge_into(&mut base, &json!({ "a": { "c": [3], "d": null } }));
    assert_eq!(base, json!({ "a": { "b": 1, "c": [3], "d": null }, "keep": true }));
}

#[test]
fn key_refs_parse_three_shapes_and_reject_plaintext() {
    assert_eq!(parse_key_ref(Some("$WB2API_API_KEY")), KeyRef::Env("WB2API_API_KEY".into()));
    assert_eq!(parse_key_ref(Some("!op read op://x")), KeyRef::Cmd("op read op://x".into()));
    assert_eq!(parse_key_ref(Some("secret:wb2api")), KeyRef::Secret("wb2api".into()));
    assert_eq!(parse_key_ref(Some("sk-live-123")), KeyRef::Plaintext);
    assert_eq!(parse_key_ref(Some("   ")), KeyRef::Missing);
    assert_eq!(parse_key_ref(None), KeyRef::Missing);
}

struct FakeLookup {
    env: Option<&'static str>,
    out: &'static str,
    secret: Option<&'static str>,
}

impl KeyLookup for FakeLookup {
    fn env(&self, _name: &str) -> Option<String> {
        self.env.map(str::to_string)
    }
    fn run(&self, _command: &str) -> Result<String, String> {
        Ok(self.out.to_string())
    }
    fn secret(&self, _id: &str) -> Option<String> {
        self.secret.map(str::to_string)
    }
}

#[test]
fn resolve_key_covers_all_shapes() {
    let lookup = FakeLookup { env: Some("k-env"), out: "k-cmd\n", secret: Some("k-secret") };
    assert_eq!(resolve_key(&KeyRef::Env("X".into()), &lookup).as_deref(), Ok("k-env"));
    assert_eq!(resolve_key(&KeyRef::Cmd("echo".into()), &lookup).as_deref(), Ok("k-cmd\n"));
    assert_eq!(resolve_key(&KeyRef::Secret("s".into()), &lookup).as_deref(), Ok("k-secret"));
    assert!(resolve_key(&KeyRef::Missing, &lookup).is_err());
    assert!(resolve_key(&KeyRef::Plaintext, &lookup).is_err());

    let empty = FakeLookup { env: None, out: "", secret: None };
    assert!(resolve_key(&KeyRef::Env("X".into()), &empty).is_err());
}
