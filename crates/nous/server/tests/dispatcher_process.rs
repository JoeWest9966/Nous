//! #73 的进程级判据：真起 nous-app 子进程，用短间隔驱动常驻节拍。
//!
//! 接缝是进程的外部面：`listening on` 行、`GET /v1/triggers` 与 durable 库里的 tick 行。
//! 这里故意不直接调 dispatcher 的库函数 —— 只验证「装配处真的起了节拍」这一件事。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn app() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nous-app"))
}

fn spawn_app(dir: &std::path::Path, port: u16) -> (Child, String) {
    let mut child = app()
        .env("NOUS_LISTEN", format!("127.0.0.1:{port}"))
        .env("NOUS_DB", dir.join("nous.db"))
        .env("NOUS_PLUGINS_DB", dir.join("nous-plugins.db"))
        .env("NOUS_DISPATCHER_INTERVAL_MS", "40")
        .env("NOUS_ORIGIN", format!("http://127.0.0.1:{port}"))
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("nous-app 子进程起得来");
    let mut reader = BufReader::new(child.stdout.take().expect("stdout piped"));
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .expect("读得到 listening on");
    assert!(
        line.starts_with("listening on "),
        "启动行应该是 listening on ...，实得：{line}"
    );
    (child, line)
}

fn get_scheduler(port: u16) -> Value {
    let body = std::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "2",
            &format!("http://127.0.0.1:{port}/v1/triggers"),
        ])
        .output()
        .expect("curl 可用");
    assert!(
        body.status.success(),
        "GET /v1/triggers 失败：{}",
        String::from_utf8_lossy(&body.stderr)
    );
    serde_json::from_slice::<Value>(&body.stdout)
        .expect("返回合法 JSON")
        .get("scheduler")
        .cloned()
        .expect("scheduler 段存在")
}

fn tick_count(dir: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open_with_flags(
        dir.join("nous.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("临时库可读");
    // `kind` 是 SQLite 里的索引编码字符串（带引号），不拿字面量猜；按 record 原文数。
    conn.query_row(
        "SELECT count(*) FROM tasks WHERE record LIKE '%nous.dispatcher.tick%'",
        [],
        |row| row.get(0),
    )
    .expect("tick 行可数")
}

fn wait_for_at_least(dir: &std::path::Path, want: i64, timeout: Duration) -> i64 {
    let deadline = Instant::now() + timeout;
    let mut seen = 0;
    while Instant::now() < deadline {
        seen = tick_count(dir);
        if seen >= want {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{} 内只看到 {seen} 条 tick，期望至少 {want}", timeout.as_secs_f64());
}

#[test]
fn production_process_runs_a_real_dispatcher_tick_loop() {
    let dir = tempfile::tempdir().unwrap();
    // 先建库，让进程启动前 lastTickAt 诚实地是 null。
    let _storage = nous_durable::Storage::open(dir.path().join("nous.db")).unwrap();
    let _store = nous_store::Store::open(dir.path().join("nous-plugins.db")).unwrap();
    assert_eq!(tick_count(dir.path()), 0);

    let port = 47_900 + (std::process::id() % 500) as u16;
    let (mut child, _line) = spawn_app(dir.path(), port);

    // 等至少两拍：证明是常驻循环，而不是启动时写死的一条。
    let seen = wait_for_at_least(dir.path(), 2, Duration::from_secs(5));
    let scheduler = get_scheduler(port);
    assert!(
        scheduler["lastTickAt"].is_i64(),
        "lastTickAt 必须从 null 变成真时刻：{scheduler}"
    );
    assert!(
        seen >= 2,
        "库里至少两条 tick：{seen}"
    );

    // 停进程后 tick 不再增长（任务随 runtime 收敛，不留孤儿）。
    child.kill().expect("子进程可杀");
    let _ = child.wait();
    std::thread::sleep(Duration::from_millis(200));
    let stopped = tick_count(dir.path());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(tick_count(dir.path()), stopped, "进程停了 tick 不许继续增长");
}
