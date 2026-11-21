mod common;

use common::{hand_totals, Db};
use nous_stats::{GroupBy, UsageWindow};
use serde_json::{json, Value};

fn as_i64(value: &Value) -> i64 {
    value.as_i64().or_else(|| value.as_f64().map(|n| n as i64)).unwrap_or(0)
}

#[test]
fn day_totals_equal_the_hand_sum_from_entries_projection() {
    let db = Db::new("totals");
    // 190 会话不必造：判据要的是「真扫库 + 自己累加」，不是行数；多会话 + 多模型覆盖分组。
    db.assistant(1, 10, 1_700_000_000_000, "volc-ark", "deepseek-v4-flash", 12, 3, false);
    db.assistant(2, 10, 1_700_000_001_000, "volc-ark", "deepseek-v4-flash", 20, 4, true);
    db.assistant(3, 11, 1_708_640_000_000, "openai", "gpt-5", 7, 1, false);
    db.assistant(4, 11, 0, "openai", "gpt-5", 999, 999, false); // 无时间戳，不进任何桶

    let entries = db.entries_projection();
    let (tokens, cost, requests, errors) = hand_totals(&entries);
    assert_eq!((tokens, cost, requests, errors), (39, 8, 3, 1), "手算自己先钉住");

    let report = nous_stats::usage(&db.path, UsageWindow::default(), GroupBy::Day).unwrap();
    let totals = &report["totals"];
    assert_eq!(as_i64(&totals["totalTokens"]), tokens);
    assert_eq!(as_i64(&totals["costTotal"]), cost, "cost={:?}", totals);
    assert_eq!(as_i64(&totals["requests"]), requests);
    assert_eq!(as_i64(&totals["errors"]), errors);
    // group_by=day 的 rows 也是同一组四元组，逐行和必须回到 totals。
    let rows = report["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "两条不同 UTC 日");
    let mut sum = (0, 0, 0, 0);
    for row in rows {
        sum.0 += as_i64(&row["totalTokens"]);
        sum.1 += as_i64(&row["costTotal"]);
        sum.2 += as_i64(&row["requests"]);
        sum.3 += as_i64(&row["errors"]);
    }
    assert_eq!(sum, (tokens, cost, requests, errors));
    assert_eq!(rows[0]["key"], json!("2023-11-14"));
    std::fs::remove_file(&db.path).ok();
}
