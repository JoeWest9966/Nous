mod common;

use std::time::Instant;

use common::Db;
use nous_stats::{GroupBy, UsageWindow};

/// 票面约束 2「先量」：190 会话 / 1710 assistant entries 的全量 day 聚合。
/// 不设硬断言（开发机 debug 抖动 + j1900 不可达）；打印实际数字供票面评论记录。
/// 阈值 150ms 是票面的硬线：真机超过就改索引/物化表（后者需 decisions.md 签字）。
#[test]
fn measure_full_day_aggregation_on_190_sessions_1710_entries() {
    let db = Db::new("perf");
    let start = 1_700_000_000_000_i64;
    let mut id = 1_i64;
    for conversation in 1..=190 {
        for n in 0..9 {
            db.assistant(
                id, conversation,
                start + (conversation * 9 + n) as i64 * 3_600_000,
                if n % 2 == 0 { "volc-ark" } else { "openai" },
                "m", 10, 1, n % 7 == 0,
            );
            id += 1;
        }
    }
    assert_eq!(id - 1, 1710);
    let t0 = Instant::now();
    let report = nous_stats::usage(&db.path, UsageWindow::default(), GroupBy::Day).unwrap();
    let elapsed = t0.elapsed();
    eprintln!(
        "[perf] 190 sessions / 1710 entries / group_by=day: {} ms, rows={}, tokens={}",
        elapsed.as_millis(),
        report["rows"].as_array().unwrap().len(),
        report["totals"]["totalTokens"],
    );
    assert_eq!(report["totals"]["requests"], serde_json::json!(1710));
    std::fs::remove_file(&db.path).ok();
}
