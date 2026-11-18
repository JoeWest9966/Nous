//! D13 运维动作的进程级判据：临时 DB 上 backup → check、active-tasks、--version 自哈希。
//! D12 的拒绝启动也在这里按真进程验一次（cwd=/ → 78 + `refuse: workdir-missing`）。

use std::path::Path;
use std::process::{Command, Output};

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
