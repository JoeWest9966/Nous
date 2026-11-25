//! 数据监控室五条明细端点的**夹具生成器**（dev 用，不进产物）。
//!
//! 读内核仓的 `fixtures/agent.small.sqlite`，把 `/v1/stats/{overview,conversations,
//! conversation/:id,cost,health}` 的真输出写到目录里。Desk 侧
//! `crates/nous-engine/tests/fixtures/kernel_stats/` 就是它的产物（截断后）。
//!
//! ```bash
//! cargo run -q -p data-data-manager --example dump -- fixtures/agent.small.sqlite /tmp
//! ```
//!
//! 库路径与输出目录都从命令行来：这个 example 不写死任何库文件名。
//!
//! `health` 的 `now_ms` 钉死一个固定值，避免「卡住」判定随时钟漂。

fn main() {
    let db = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("用法：cargo run -q -p data-data-manager --example dump -- <库路径> [输出目录]");
        std::process::exit(2);
    });
    let path = std::path::Path::new(&db);
    let out = std::env::args().nth(2).unwrap_or_else(|| "/tmp".to_string());
    let write = |name: &str, value: serde_json::Value| {
        std::fs::write(
            format!("{out}/{name}.json"),
            serde_json::to_string_pretty(&value).unwrap(),
        )
        .unwrap();
    };
    write("stats_overview", data_data_manager::overview(path).unwrap());
    let convs = data_data_manager::conversations(path).unwrap();
    let first = convs["list"].get(0).and_then(|r| r["id"].as_i64()).unwrap_or(1);
    write("stats_conversations", convs);
    write(
        "stats_conversation",
        data_data_manager::conversation(path, first).unwrap().unwrap(),
    );
    write("stats_cost", data_data_manager::cost(path).unwrap());
    write("stats_health", data_data_manager::health(path, 1_700_000_000_000).unwrap());
    println!("dumped to {out}, first conv id={first}");
}
