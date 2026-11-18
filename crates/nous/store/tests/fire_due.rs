//! 判据 7：`fire_due` 纯函数 —— 四种 spec × 五个时刻，与手写期望表逐格对照。

use nous_store::trigger::{fire_due, Trigger};
use nous_store::{NewTriggerDefinition as NewTrigger, Store};

fn def<'a>(code: &'a str, cron: Option<&'a str>) -> NewTrigger<'a> {
    NewTrigger {
        code,
        name: "夜报",
        spec: Box::leak(
            serde_json::json!({ "spec_version": 1, "schedule": cron, "on": null })
                .to_string()
                .into_boxed_str(),
        ),
        template_ref: r#"{"kind":"preset","ref":"nightly"}"#,
        fires_allowed_by: r#"[{"kind":"system","id":"cron"}]"#,
        target: r#"{"kind":"new"}"#,
        policy: "serial",
    }
}

/// 2024-02-28T03:00:00Z（周三），epoch ms。
const BASE: i64 = 1_709_089_200_000;

#[test]
fn fire_due_matches_the_hand_written_table() {
    let store = Store::open_in_memory().unwrap();
    let triggers = [
        Trigger::new(def("every_minute", Some("* * * * *")), 1, Some(&store)).unwrap(),
        Trigger::new(def("daily_three", Some("0 3 * * *")), 1, Some(&store)).unwrap(),
        Trigger::new(def("no_schedule", None), 1, Some(&store)).unwrap(),
        Trigger::new(def("wednesday_three", Some("0 3 * * 3")), 1, Some(&store)).unwrap(),
    ];

    // 五列：到点 / 差 1ms / 刚好过 / 跨天 / 禁用。
    let cases: &[(i64, &[&str])] = &[
        (0, &["every_minute", "daily_three", "wednesday_three"]),
        (-1, &["every_minute"]),
        (60_000, &["every_minute"]),
        (86_400_000, &["every_minute", "daily_three", "wednesday_three"]), // 次日 03:00
        (0, &[]), // 「禁用」由下一只测试单独验（纯函数不读库）
    ];
    for (index, (offset, expected)) in cases.iter().enumerate() {
        if index == 4 {
            continue;
        }
        let now = BASE + offset;
        let mut codes: Vec<String> = fire_due(&triggers, now).into_iter().map(|due| due.code).collect();
        codes.sort();
        let mut expected = expected.to_vec();
        expected.sort();
        assert_eq!(codes, expected, "now = BASE{offset:+}: fire_due 与手写期望不一致");
        assert_eq!(fire_due(&triggers, now), fire_due(&triggers, now), "fire_due 必须是纯函数");
    }
}

#[test]
fn fire_due_skips_disabled_definitions() {
    let store = Store::open_in_memory().unwrap();
    let trigger = Trigger::new(def("daily_three", Some("0 3 * * *")), 1, Some(&store)).unwrap();
    assert_eq!(fire_due(&[trigger.clone()], BASE).len(), 1);
    store.set_schedule("daily_three", false, None).unwrap();
    let row = store.trigger("daily_three").unwrap().unwrap();
    let restored = Trigger::from_row(&row, &trigger.definition_json()).unwrap_or_else(|error| panic!("definition_json 往返失败：{error}；json = {}", trigger.definition_json()));
    assert!(!restored.enabled());
    assert!(fire_due(&[restored], BASE).is_empty(), "禁用的定义不进 due 列表");
}

#[test]
fn no_clock_import_in_store_src() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    fn walk(dir: &std::path::Path, hits: &mut usize) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { walk(&path, hits); continue; }
            if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                *hits += text.matches("SystemTime::now").count() + text.matches("Instant::now").count();
            }
        }
    }
    let mut hits = 0;
    walk(&root, &mut hits);
    assert_eq!(hits, 0, "store/src 里不许有直接时钟");
}
