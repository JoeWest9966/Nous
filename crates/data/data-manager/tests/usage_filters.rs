mod common;

use std::collections::BTreeSet;

use common::Db;
use data_data_manager::{GroupBy, UsageWindow};
use serde_json::Value;

#[test]
fn window_is_half_open_and_actually_filters_requests() {
    let db = Db::new("window");
    let start = 1_700_000_000_000_i64;
    for i in 0..6 {
        db.assistant(i + 1, 10, start + i as i64 * 1000, "p", "m", i, 0, false);
    }
    let full = data_data_manager::usage(&db.path, UsageWindow::default(), GroupBy::Day).unwrap();
    let sliced = data_data_manager::usage(
        &db.path,
        UsageWindow { from: Some(start + 2000), to: Some(start + 5000) },
        GroupBy::Day,
    ).unwrap();
    let full_requests = full["totals"]["requests"].as_i64().unwrap();
    let sliced_requests = sliced["totals"]["requests"].as_i64().unwrap();
    assert_eq!(full_requests, 6);
    assert_eq!(sliced_requests, 3, "[from,to) 闭开：命中 t=2000,3000,4000");
    assert!(sliced_requests < full_requests, "过滤必须真的生效（严格小于全量）");
    // 上界 exclusive：to 那一毫秒不计。
    let upper = data_data_manager::usage(
        &db.path,
        UsageWindow { from: Some(start + 5000), to: None },
        GroupBy::Day,
    ).unwrap();
    assert_eq!(upper["totals"]["requests"], serde_json::json!(1));
    std::fs::remove_file(&db.path).ok();
}

#[test]
fn model_keys_are_the_distinct_provider_model_pairs_from_entries() {
    let db = Db::new("model-keys");
    db.assistant(1, 10, 1_700_000_000_000, "volc-ark", "deepseek-v4-flash", 1, 0, false);
    db.assistant(2, 10, 1_700_000_001_000, "volc-ark", "deepseek-v4-pro", 2, 0, false);
    db.assistant(3, 11, 1_700_000_002_000, "openai", "gpt-5", 3, 0, false);
    let entries = db.entries_projection_for_keys();
    let expected: BTreeSet<String> = db.all_keys();
    assert_eq!(expected.len(), 3, "手里的去重集合先钉住");
    let report = data_data_manager::usage(&db.path, UsageWindow::default(), GroupBy::Model).unwrap();
    let actual: BTreeSet<String> = report["rows"]
        .as_array().unwrap()
        .iter()
        .map(|row| row["key"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(actual, expected, "rows[].key 集合必须等于 entries 里的 provider/model 去重");
    assert!(!entries.is_empty());
    std::fs::remove_file(&db.path).ok();
}

// 把 entries 读出来走一遍公共投影：判据要求「从 entries 去重」，不是读代码里的常量。
trait Keys {
    fn entries_projection_for_keys(&self) -> Vec<Value>;
    fn all_keys(&self) -> BTreeSet<String>;
}

impl Keys for Db {
    fn entries_projection_for_keys(&self) -> Vec<Value> {
        self.entries_projection()
    }
    fn all_keys(&self) -> BTreeSet<String> {
        self.entries_projection().iter().map(|entry| {
            let message = &entry["model"][0];
            format!("{}/{}",
                message["provider"].as_str().unwrap(),
                message["modelId"].as_str().unwrap(),
            )
        }).collect()
    }
}
