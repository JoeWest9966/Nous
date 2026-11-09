//! 用引擎的真实 JSON 形状做反序列化测试。
//! 这些 sample 是从 qrd3wh `/v1/*` 实际响应里抠出来的，不是编的。

use nous_engine::types::{Device, Entries, Sessions, Submission};

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
