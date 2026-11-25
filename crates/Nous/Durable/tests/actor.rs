//! #54：actor 值类型的判据（序列化形状 + 未知 kind 的 fail-closed 降级）。
use std::path::{Path, PathBuf};

use nous_durable::actor::{actor_of, Actor, ActorKind};
use nous_durable::storage::import::ImportedStore;
use nous_durable::storage::sessions::render_entries_page;
use serde_json::Value;
use serde_json::json;

#[test]
fn deserializing_an_unknown_or_missing_kind_falls_back_to_system_not_human() {
    let unknown: Actor = serde_json::from_value(json!({"kind": "root", "id": "x"})).unwrap();
    assert_eq!(unknown.kind, ActorKind::System, "未知 kind 不许冒充人");
    assert_eq!(unknown.id, "x");

    let missing: Actor = serde_json::from_value(json!({"id": "x"})).unwrap();
    assert_eq!(missing.kind, ActorKind::System, "缺 kind 也 fail-closed");
    assert_eq!(missing.id, "x");

    let agent: Actor = serde_json::from_value(json!({"kind": "agent", "id": "x"})).unwrap();
    assert_eq!(agent.kind, ActorKind::Agent);
    assert_eq!(agent.id, "x");

    let human: Actor = serde_json::from_value(json!({"kind": "human", "id": "x"})).unwrap();
    assert_eq!(human.kind, ActorKind::Human);
    assert_eq!(human.id, "x");
}

#[test]
fn actor_serializes_to_exactly_kind_and_id() {
    let value = serde_json::to_value(Actor::agent("opencode-1")).unwrap();
    assert_eq!(value, json!({"kind": "agent", "id": "opencode-1"}));
    let object = value.as_object().expect("actor 是 JSON 对象");
    let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["id", "kind"], "只有两个键，不许多出第三个");
}

#[test]
fn actor_of_a_legacy_or_malformed_record_is_system_unknown() {
    assert_eq!(actor_of(&json!({"kind": "pi.user"})), Actor::system("unknown"));
    assert_eq!(actor_of(&json!({"actor": null})), Actor::system("unknown"));
    assert_eq!(actor_of(&json!({"actor": 42})), Actor::system("unknown"));
    assert_eq!(actor_of(&json!({"actor": {}})), Actor::system("unknown"));
    assert_eq!(
        actor_of(&json!({"kind": "system", "id": "unknown"})),
        Actor::system("unknown"),
        "带 actor 的旧形状也要按值类型读"
    );
    assert_eq!(
        actor_of(&json!({"actor": {"kind": "agent", "id": "opencode-1"}})),
        Actor::agent("opencode-1")
    );
}

#[test]
fn a_legacy_database_still_renders_and_its_entries_read_as_system_unknown() {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/agent.small.sqlite");
    assert!(path.exists(), "小样本 fixture 应在仓里：{}", path.display());
    let store = ImportedStore::read(&path).expect("老库读得出来");

    // entries_page 渲染：分页、排序、老记录都没有 actor 字段，但绝不 panic。
    let (page, next) = render_entries_page(&store, 2, 5, None).expect("entries_page 渲染");
    assert_eq!(page.len(), 5, "第一页 5 条");
    assert!(next.is_some(), "后面还有条目时给游标");
    assert!(page.iter().all(|item| item.get("actor").is_none()), "渲染形状不变（不加 actor）");
    let (page2, _) = render_entries_page(&store, 2, 5, next.as_ref()).expect("第二页渲染");
    assert!(page2.iter().all(|item| item["id"].as_i64().unwrap() < page[4]["id"].as_i64().unwrap()));

    // 老记录里没有 actor：读侧一律给 system/unknown，不是 null、更不是 human。
    let mut seen = 0;
    for row in &store.entries {
        let Some(nous_chord::sqlite::SqliteValue::Text(text)) = row.get("record") else {
            continue;
        };
        let record: Value = serde_json::from_str(text).unwrap();
        assert!(record.get("actor").is_none(), "fixture 是字段出现之前的老库");
        assert_eq!(actor_of(&record), Actor::system("unknown"));
        seen += 1;
    }
    assert!(seen > 0, "老库里确实有 entry");
}
