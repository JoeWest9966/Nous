//! 用引擎的真实 JSON 形状做反序列化测试。
//! 这些 sample 是从 qrd3wh `/v1/*` 实际响应里抠出来的，不是编的。

use nous_engine::types::{Device, Entries, Forked, ReadReceipt, Sessions, Submission, Unread};

#[test]
fn parse_sessions() {
    // GET /v1/sessions 的真实响应（截断到一条）。
    let json = r#"{
      "items": [{
        "id": 2,
        "name": "真模型",
        "parent": null,
        "owner": null,
        "model": "volc-ark/deepseek-v4-flash-260425",
        "thinkingLevel": "off",
        "thinkingLevels": ["off","minimal","low","medium","high"],
        "skills": [],
        "template": null,
        "readOnly": false,
        "busy": null,
        "last": {"id": 411, "kind": "pi.assistant", "text": null}
      }]
    }"#;

    let s: Sessions = serde_json::from_str(json).unwrap();
    assert_eq!(s.items.len(), 1);
    let item = &s.items[0];
    assert_eq!(item.id, 2);
    assert_eq!(item.name, "真模型");
    assert_eq!(item.read_only, false);
    assert_eq!(item.thinking_level.as_ref().unwrap(), "off");
    assert_eq!(item.last.as_ref().unwrap().kind, "pi.assistant");
    assert_eq!(item.last.as_ref().unwrap().text, None);
    // 未知字段掉进 extra，不炸。
    assert!(item.extra.is_empty() || item.extra.contains_key("whatever"));
}

#[test]
fn parse_entries() {
    let json = r#"{
      "items": [
        {"id": 411, "kind": "pi.assistant", "text": "收到。不", "parent": null},
        {"id": 409, "kind": "pi.assistant", "text": null, "parent": null},
        {"id": 410, "kind": "pi.user", "text": "你好", "parent": null}
      ],
      "next": null
    }"#;

    let e: Entries = serde_json::from_str(json).unwrap();
    assert_eq!(e.items.len(), 3);
    assert_eq!(e.items[0].kind, "pi.assistant");
    assert_eq!(e.items[0].text.as_deref(), Some("收到。不"));
    // 坑 7：有 entry 没正文时 text 是 null，解析不许炸。
    assert_eq!(e.items[1].text, None);
    assert_eq!(e.items[2].text.as_deref(), Some("你好"));
}

#[test]
fn parse_device() {
    let json = r#"{
      "self": "/p/demo/engine",
      "origin": "http://127.0.0.1:4790",
      "node": "v22.23.2",
      "startedAt": 1794131034132,
      "model": {"provider": "volc-ark", "modelId": "deepseek-v4-flash-260425"},
      "faux": false,
      "data": "/home/user/nous/content/demo/map/engine/data",
      "catalog": {"base": "https://catalog.example.com"},
      "available": ["Qoder/Auto"]
    }"#;

    let d: Device = serde_json::from_str(json).unwrap();
    assert_eq!(d.faux, Some(false));
    assert_eq!(d.available.len(), 1);
}

#[test]
fn parse_submission() {
    let json = r#"{"submissionId": 42, "status": "queued"}"#;
    let s: Submission = serde_json::from_str(json).unwrap();
    assert_eq!(s.status, "queued");
}

/// `updatedAt` 三种状态都要活下来：有值 / null / 整个字段没来。
/// null 不是 0，也不是「刚刚」—— 老会话就是空的，别替它编时间。
#[test]
fn parse_session_updated_at() {
    let json = r#"{
      "items": [
        {"id": 2, "name": "有戳", "parent": null, "readOnly": false, "busy": null,
         "thinkingLevels": [], "skills": [], "updatedAt": 1794222429712, "last": null},
        {"id": 14, "name": "空戳", "parent": null, "readOnly": false, "busy": null,
         "thinkingLevels": [], "skills": [], "updatedAt": null, "last": null},
        {"id": 44, "name": "没这字段", "parent": {"id": 14, "at": 24}, "readOnly": false,
         "busy": null, "thinkingLevels": [], "skills": [], "last": null}
      ]
    }"#;

    let s: Sessions = serde_json::from_str(json).unwrap();
    assert_eq!(s.items[0].updated_at, Some(1794222429712));
    assert_eq!(s.items[1].updated_at, None);
    assert_eq!(s.items[2].updated_at, None);
    assert_eq!(s.items[2].parent.as_ref().unwrap().id, 14);
}

/// `GET /v1/unread` 是**平铺的 map**，不是 `{items: []}`。形状认错 = 角标全灭。
#[test]
fn parse_unread_flat_map() {
    let json = r#"{
      "2": {"unread": 0, "readAt": 1794222457741},
      "14": {"unread": 3, "readAt": null}
    }"#;

    let u: Unread = serde_json::from_str(json).unwrap();
    assert_eq!(u.count(2), 0);
    assert_eq!(u.count(14), 3);
    assert_eq!(u.of(2).unwrap().read_at, Some(1794222457741));
    // readAt 为 null ≠ 读到过，只是一个没被推过的水位。
    assert_eq!(u.of(14).unwrap().read_at, None);
    // 表里没有的会话按 0 算，别 panic。
    assert_eq!(u.count(999), 0);
}

#[test]
fn parse_read_receipt() {
    let json = r#"{"id": 514, "readAt": 1794222568258, "unread": 0}"#;
    let r: ReadReceipt = serde_json::from_str(json).unwrap();
    assert_eq!(r.id, 514);
    assert_eq!(r.unread, 0);
    assert_eq!(r.read_at, Some(1794222568258));
}

#[test]
fn parse_forked() {
    let json = r#"{"id": 296, "at": 50}"#;
    let f: Forked = serde_json::from_str(json).unwrap();
    assert_eq!(f.id, 296);
    assert_eq!(f.at, Some(serde_json::json!(50)));
}

// ── 建会话那一页的五份清单（`/v1/models` `/v1/skills` `/v1/prompts` `/v1/machines` `/v1/fs/dirs`）

#[test]
fn parse_models() {
    // GET /v1/models：242 个模型，截断到两条。`default` 是盘上那只眼指着的。
    let json = r#"{
      "default": {"provider": "volc-ark", "modelId": "deepseek-v4-flash-260425"},
      "items": [
        {"provider":"volc-ark","modelId":"deepseek-v4-flash-260425",
         "name":"deepseek-v4-flash-260425",
         "thinkingLevels":["off","minimal","low","medium","high"],
         "contextWindow":128000,"maxTokens":8192},
        {"provider":"Qoder","modelId":"GLM-5.3","name":"GLM-5.3",
         "thinkingLevels":["off","high"],"contextWindow":180000,"maxTokens":8192}
      ]
    }"#;
    let m: nous_engine::types::Models = serde_json::from_str(json).unwrap();
    assert_eq!(m.items.len(), 2);
    assert_eq!(
        m.default_key().as_deref(),
        Some("volc-ark/deepseek-v4-flash-260425")
    );
    assert_eq!(m.items[1].provider, "Qoder");
    assert_eq!(m.items[1].levels(), vec!["off", "high"]);
    // 不给 thinkingLevels 的模型也要能建：退成只有 off。
    let only = r#"{"provider":"x","modelId":"y"}"#;
    let one: nous_engine::types::Model = serde_json::from_str(only).unwrap();
    assert_eq!(one.levels(), vec!["off"]);
}

#[test]
fn parse_skills() {
    // GET /v1/skills：`load.state != ok` 的画出来但不让勾；`hidden` 的干脆不列。
    let json = r#"{
      "root": "/opt/Nous-skills",
      "categories": ["Basics","Basics/Code"],
      "skills": [
        {"name":"code-review","path":"code-review","description":"审查从某个点以来的改动",
         "hidden":false,"category":["Basics/Code"],"load":{"state":"ok"},"tracked":true},
        {"name":"broken","path":"broken","hidden":false,"category":[],
         "load":{"state":"missing"},"tracked":false},
        {"name":"internal","path":"internal","hidden":true,"category":[],
         "load":{"state":"ok"},"tracked":true}
      ]
    }"#;
    let s: nous_engine::types::Skills = serde_json::from_str(json).unwrap();
    assert_eq!(s.skills.len(), 3);
    assert!(s.skills[0].loadable());
    assert!(!s.skills[1].loadable());
    assert!(s.skills[2].hidden);
    assert_eq!(s.categories.len(), 2);
}

#[test]
fn parse_prompts() {
    // GET /v1/prompts：形状照面板的消费方写的（`prompts[]` + `default`）。
    let json = r#"{
      "default": "审查一段改动",
      "categories": ["Basics"],
      "prompts": [
        {"name":"审查一段改动","description":"标准和规格两轴","category":["Basics"],
         "load":{"state":"ok"}},
        {"name":"坏掉的","load":{"state":"error"}}
      ]
    }"#;
    let p: nous_engine::types::Prompts = serde_json::from_str(json).unwrap();
    assert_eq!(p.default.as_deref(), Some("审查一段改动"));
    assert_eq!(p.prompts.len(), 2);
    assert!(p.prompts[0].loadable());
    assert!(!p.prompts[1].loadable());
    // 没 description 的不能炸。
    assert_eq!(p.prompts[1].description, None);
}

#[test]
fn parse_machines_and_dirs() {
    let m: nous_engine::types::Machines = serde_json::from_str(
        r#"{"at":1794236685874,"machines":[
             {"name":"j1900","user":"root","port":22,
              "ips":[{"addr":"100.126.95.105","label":"tailscale"}],
              "addr":"100.126.95.105","note":"小主机","tags":["board"]}]}"#,
    )
    .unwrap();
    assert_eq!(m.machines.len(), 1);
    assert_eq!(m.machines[0].port, Some(22));
    assert_eq!(m.machines[0].ips[0].label.as_deref(), Some("tailscale"));

    // `path` 为空串 = 起点，不是「没有路径」。
    let d: nous_engine::types::Dirs = serde_json::from_str(
        r#"{"path":"","dirs":[{"name":"/opt/Nous/content/new1/map","path":"/opt/Nous/content/new1/map"}]}"#,
    )
    .unwrap();
    assert_eq!(d.path.as_deref(), Some(""));
    assert_eq!(d.dirs.len(), 1);
}

#[test]
fn create_session_body_is_camel_case() {
    // 这几个键是给引擎吃的：`thinkingLevel` / `cwd` / `machine` / `template: null`。
    // 写错一个不会报错，只会**悄悄不生效** —— 所以钉在测试里。
    let body = nous_engine::types::CreateSession {
        name: Some("试试".into()),
        model: Some(serde_json::json!({"provider":"volc-ark","modelId":"x"})),
        thinking_level: Some(serde_json::json!("medium")),
        template: Some(None),
        cwd: Some("/opt/Nous/content/new1/map".into()),
        machine: Some("j1900".into()),
        ..Default::default()
    };
    let v = serde_json::to_value(&body).unwrap();
    assert_eq!(v["thinkingLevel"], "medium");
    assert_eq!(v["cwd"], "/opt/Nous/content/new1/map");
    assert_eq!(v["machine"], "j1900");
    assert!(v.get("thinking_level").is_none());
    // `template` 总是发，null 也是有效值 = 明确不要。
    assert!(v.as_object().unwrap().contains_key("template"));
    assert!(v["template"].is_null());
    // 没给的字段不出现（省掉 = 引擎用默认）。
    assert!(v.get("skills").is_none());
    assert!(v.get("presets").is_none());
}

#[test]
fn parse_presets_with_categories_and_repo() {
    // GET /v1/presets/Session-Template 的真实响应（2026-11-09 实测，条目截到一条）。
    // 两个字段是通讯录要用的：`categories`（含零计数类别，引擎已排序）+ `repo`（这一段的仓状态）。
    let json = r#"{
      "section": "Session-Template",
      "title": "会话模板",
      "root": "/opt/Nous-Template/Session-Template",
      "repo": { "head": "9e4ccd1300c60f154e80fac8d5e5327edc54e87d", "dirty": true,
                "ahead": 0, "behind": 0,
                "remote": "https://github.com/JoeWest9966/Nous-Template", "error": null },
      "presets": [{
        "name": "weekly-report",
        "summary": "把一周的提交、聊天、会议整理成一份能直接发出去的周报",
        "description": "周五收工用。",
        "category": ["写作", "写作/周报"],
        "spec": { "v": 1, "template": "demo-weekly" },
        "specError": null,
        "tracked": true,
        "dir": "/opt/Nous-Template/Session-Template/weekly-report",
        "resolved": { "template": { "state": "ok" } }
      }],
      "archived": [],
      "categories": ["工程", "写作", "写作/周报", "Basics"]
    }"#;

    let p: nous_engine::types::Presets = serde_json::from_str(json).unwrap();
    assert_eq!(p.section, "Session-Template");
    assert_eq!(p.title.as_deref(), Some("会话模板"));
    // 顺序照引擎给的，客户端不重排；零计数（Basics）也得在。
    assert_eq!(p.categories, ["工程", "写作", "写作/周报", "Basics"]);
    let repo = p.repo.as_ref().expect("repo 块不该丢");
    assert_eq!(repo.head.as_deref().map(|h| &h[..7]), Some("9e4ccd1"));
    assert!(repo.dirty);
    assert!(repo.error.is_none());
    // `archived` / `root` 这类没型号的键进 extra，不炸。
    assert!(p.extra.contains_key("archived"));
    let item = &p.presets[0];
    assert_eq!(item.category, ["写作", "写作/周报"]);
    assert_eq!(item.spec.as_ref().unwrap()["template"], "demo-weekly");
}

#[test]
fn parse_presets_without_repo_or_categories() {
    // 老引擎 / 仓读不开时这两个键可能整块缺席 —— 不许因此解析失败。
    let json = r#"{"section":"Ultimate-Template","title":"终极模板","presets":[]}"#;
    let p: nous_engine::types::Presets = serde_json::from_str(json).unwrap();
    assert!(p.categories.is_empty());
    assert!(p.repo.is_none());
    assert!(p.presets.is_empty());
}
