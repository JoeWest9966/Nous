mod common;

use std::path::PathBuf;

use common::Db;
use serde_json::{json, Value};

#[test]
fn db_report_uses_the_same_check_reader_as_the_cli() {
    let durable = Db::new("db");
    let durable_path = &durable.path;
    // 插件库用 Store::open 建（它自己的迁移链），schema 由它自己报。
    let plugins_path: PathBuf = std::env::temp_dir().join(format!(
        "nous-stats62-db-plugins-{}.sqlite", std::process::id()
    ));
    let _ = std::fs::remove_file(&plugins_path);
    let store = nous_store::Store::open(&plugins_path).unwrap();
    drop(store);

    let report = nous_stats::db(durable_path, &plugins_path).unwrap();
    let d = &report["durable"];
    let p = &report["plugins"];
    assert_eq!(d["quickCheck"], json!("ok"));
    assert_eq!(p["quickCheck"], json!("ok"));
    // 与 `nous-app --check` 的同一个函数、同一个 schema 值。
    assert_eq!(d["schema"], json!(nous_durable::ops::check(durable_path).unwrap()));
    assert_eq!(p["schema"], json!(nous_store::Store::open(&plugins_path).unwrap().schema_version().unwrap()));
    assert!(d["bytes"].as_i64().unwrap() > 0);
    assert!(p["bytes"].as_i64().unwrap() > 0);
    assert_eq!(d["rows"]["entries"], json!(0));
    assert_eq!(p["rows"]["triggers"], json!(0));
    std::fs::remove_file(&plugins_path).ok();
    std::fs::remove_file(durable_path).ok();
}

#[test]
fn all_three_endpoints_are_zero_or_empty_on_a_fresh_db() {
    let db = Db::new("empty");
    let missing_plugins = db.path.with_file_name("nous-stats62-never-exists.sqlite");
    let _ = std::fs::remove_file(&missing_plugins);

    let usage = nous_stats::usage(&db.path, Default::default(), nous_stats::GroupBy::Day).unwrap();
    assert_eq!(usage["totals"]["totalTokens"], json!(0));
    assert_eq!(usage["totals"]["costTotal"], json!(0.0));
    assert_eq!(usage["totals"]["requests"], json!(0));
    assert_eq!(usage["totals"]["errors"], json!(0));
    assert_eq!(usage["rows"], json!([]));

    let tasks = nous_stats::tasks(&db.path).unwrap();
    assert_eq!(tasks["active"], json!(0));
    assert!(tasks["byState"].as_object().unwrap().values().all(|v| v == &json!(0)));

    let dbs = nous_stats::db(&db.path, &missing_plugins).unwrap();
    assert_eq!(dbs["durable"]["quickCheck"], json!("ok"));
    assert_eq!(dbs["plugins"]["schema"], json!(0), "没有插件库按空库报，不 404/500");
    let rows: &Value = &dbs["durable"]["rows"];
    assert!(rows.as_object().unwrap().values().all(|v| v == &json!(0)));
    std::fs::remove_file(&db.path).ok();
}
