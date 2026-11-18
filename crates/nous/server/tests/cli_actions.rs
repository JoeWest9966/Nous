//! D13 运维动作的进程级判据：临时 DB 上 backup → check、active-tasks、--version 自哈希。
//! D12 的拒绝启动也在这里按真进程验一次（cwd=/ → 78 + `refuse: workdir-missing`）。

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

fn app() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nous-app"))
}

fn source_db(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("source.db");
    let _storage = nous_durable::Storage::open(&path).unwrap();
    path
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_line_uses_the_running_binary_hash() {
    let output = app().arg("--version").output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let line = stdout(&output);
    assert!(
        line.starts_with("nous-app 0.1.0 (build ") && line.ends_with(")\n"),
        "unexpected version line: {line}"
    );
    let build = line
        .split("build ")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .trim();
    assert_eq!(build.len(), 8, "build 必须是 sha256 前 8 位");
    assert!(build.chars().all(|c| c.is_ascii_hexdigit()));

    // 磁盘字节 / 进程自哈希：同一份二进制必须同值。
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_nous-app")).unwrap();
    let expected = {
        let encoded = bash_sha256(&bytes);
        encoded[..8].to_string()
    };
    assert_eq!(build, expected);
}

/// 用系统 `sha256sum` 算同一个文件的哈希：这里只是测试对第三方实现的独立核对。
fn bash_sha256(bytes: &[u8]) -> String {
    use std::io::Write;
    let mut child = Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("sha256sum 必须可用");
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().next().unwrap().to_string()
}

#[test]
fn backup_then_check_prints_exactly_two_lines() {
    let dir = tempfile::tempdir().unwrap();
    let source = source_db(dir.path());
    let backup = dir.path().join("backup.db");

    let output = app()
        .arg("--backup")
        .arg(&backup)
        .env("NOUS_DB", &source)
        .output()
        .unwrap();
    assert!(output.status.success(), "backup failed: {}", stderr(&output));
    assert!(backup.exists());

    let output = app().arg("--check").arg(&backup).output().unwrap();
    assert!(output.status.success(), "check failed: {}", stderr(&output));
    let text = stdout(&output);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 2, "check 固定两行: {lines:?}");
    assert_eq!(lines[0], "quick_check=ok");
    assert!(
        lines[1].strip_prefix("schema=").unwrap().parse::<u32>().unwrap() >= 1,
        "schema 必须是数字: {lines:?}"
    );
}

#[test]
fn active_tasks_uses_terminal_gate() {
    let dir = tempfile::tempdir().unwrap();
    let source = source_db(dir.path());
    let output = app()
        .arg("--active-tasks")
        .env("NOUS_DB", &source)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "0", "新库没有未收尾的任务");
}

/// #58 判据 4：`nous-app --triggers` 与 HTTP 端点**逐字节相同**（同一函数、同一序列化）。
///
/// CLI 的 lastTickAt 恒为 null（CLI 里没有调度循环），now_ms 也随时间漂 —— 所以把
/// HTTP 侧的比对面固定在同一口径：两边都从同一份库文件出发，走同一个
/// `observation_report`，直接比函数输出与 CLI stdout 的 JSON。
#[test]
fn triggers_cli_prints_the_same_json_as_the_endpoint_reader() {
    let dir = tempfile::tempdir().unwrap();
    let durable_db = dir.path().join("nous.db");
    let _storage = nous_durable::Storage::open(&durable_db).unwrap();
    let trigger_db = dir.path().join("nous-triggers.db");
    let store = nous_store::Store::open(&trigger_db).unwrap();

    // 造一个真触发器 + 两笔账（done + skipped）。
    let trigger = nous_store::Trigger::new(
        nous_store::NewTriggerDefinition {
            code: "nightly_digest",
            name: "夜报",
            spec: r#"{"spec_version":1,"schedule":null,"on":null}"#,
            template_ref: r#"{"kind":"preset","ref":"nightly"}"#,
            fires_allowed_by: r#"[{"kind":"system","id":"cron"},{"kind":"system","id":"local"}]"#,
            target: r#"{"kind":"new"}"#,
            policy: "serial",
        },
        1_000,
        Some(&store),
    )
    .unwrap();
    store.update_definition(trigger.code(), &trigger.definition_json()).unwrap();
    store.record_fire("nightly_digest", 7_000, None).unwrap();
    store.mark_done("nightly_digest", 7_000).unwrap();
    store.record_fire("nightly_digest", 8_000, None).unwrap();
    store.mark_skipped("nightly_digest", 8_000).unwrap();

    let output = app()
        .arg("--triggers")
        .env("NOUS_DB", &durable_db)
        .env("NOUS_TRIGGER_DB", &trigger_db)
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let cli_json: Value = serde_json::from_str(stdout(&output).trim()).expect("CLI 输出必须是合法 JSON");

    // 同一份库 + 同一个函数：triggers 数组与端点读口**逐字节**相同（判据 4）。
    // scheduler 段不比：CLI 用真时钟、HTTP 侧测试用固定 0 —— 计时字段本来就随时刻漂。
    let store = nous_store::Store::open(&trigger_db).unwrap();
    let definitions = nous_server::triggers::TriggerRuntime::open_definitions_only(&store).unwrap();
    let endpoint_reader =
        nous_server::triggers::observation_report(&store, &definitions, None, 0).unwrap();
    assert_eq!(cli_json["triggers"], endpoint_reader["triggers"], "triggers 清单与端点读口相同");
    assert_eq!(cli_json["root"], endpoint_reader["root"]);
    assert_eq!(cli_json["scheduler"]["tickMs"], endpoint_reader["scheduler"]["tickMs"]);
    assert_eq!(cli_json["triggers"][0]["counts"]["done"], json!(1));
    assert_eq!(cli_json["triggers"][0]["counts"]["skipped"], json!(1));
    assert_eq!(cli_json["triggers"][0]["lastFire"]["state"], json!("skipped"));
}

#[test]
fn missing_workdir_refuses_with_78_and_slug() {
    let dir = tempfile::tempdir().unwrap();
    let source = source_db(dir.path());
    let output = app()
        .current_dir("/")
        .env("NOUS_DB", &source)
        .env_remove("NOUS_LISTEN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(78), "stderr: {}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("refuse: workdir-missing"), "{message}");
    assert!(message.contains("WorkingDirectory="), "{message}");
}

#[test]
fn refusal_exit_code_is_refused_not_generic() {
    // 与源码常量同值；判据 2b 也在 verify.sh 里做文本级核对。
    assert_eq!(nous_server::refusal::EXIT_REFUSED, 78);
}
