//! 数据监控室五条明细端点的**夹具生成器**（dev 用，不进产物）。
//!
//! 读内核仓的 `fixtures/agent.small.sqlite`，把 `/v1/stats/{overview,conversations,
//! conversation/:id,cost,health}` 的真输出写到目录里。Desk 侧
//! `crates/nous-engine/tests/fixtures/kernel_stats/` 就是它的产物（截断后）。
//!
//! ```bash
//! cargo run -q -p nous-stats --example dump -- /tmp
//! ```
//!
//! `health` 的 `now_ms` 钉死一个固定值，避免「卡住」判定随时钟漂。

fn main() {
    let path = std::path::Path::new("fixtures/agent.small.sqlite");
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp".to_string());
    let write = |name: &str, value: serde_json::Value| {
        std::fs::write(
            format!("{out}/{name}.json"),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
    };
    write("stats_overview", nous_stats::overview(path).unwrap());
    let convs = nous_stats::conversations(path).unwrap();
    let first = convs["list"].get(0).and_then(|r| r["id"].as_i64()).unwrap_or(1);
    write("stats_conversations", convs);
    write(
        "stats_conversation",
        nous_stats::conversation(path, first).unwrap().unwrap(),
    );
    write("stats_cost", nous_stats::cost(path).unwrap());
    write("stats_health", nous_stats::health(path, 1_700_000_000_000).unwrap());
    println!("dumped to {out}, first conv id={first}");
}
