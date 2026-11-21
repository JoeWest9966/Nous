//! 唯一的 git 进程调用面。所有调用都带超时与 `GIT_TERMINAL_PROMPT=0`；
//! push 失败绝不伪装成功（返回 `pushed: false` 且不回滚工作树）。

use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use crate::error::{RepoError, Result};

pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(25);

pub(crate) fn git(root: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .map_err(|e| RepoError::Io(format!("git {:?}: {e}", args.first())))?;
    Ok(output)
}

pub(crate) fn git_ok(root: &Path, args: &[&str]) -> Result<String> {
    let out = git(root, args)?;
    if !out.status.success() {
        return Err(RepoError::Io(format!(
            "git {} 失败：{}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(crate) fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = git(root, args)?;
    if !out.status.success() {
        return Err(RepoError::Io(format!(
            "git {} 失败：{}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// 仓是否配了可用的 push 目标。没有 origin → `Unknown`（wire null）。
pub fn has_push_target(root: &Path) -> bool {
    git_ok(root, &["remote"])
        .map(|out| out.lines().any(|line| !line.trim().is_empty()))
        .unwrap_or(false)
}

/// 带 25s 软超时的 push（poll 子进程，超时 kill；不把控制权交给外部 shell）。
///
/// `Unknown`（wire null）留给没有配置远端的情况；失败是 `No`，绝不报成功。
pub(crate) fn git_push(root: &Path) -> Result<crate::Pushed> {
    use std::thread;
    use std::time::Instant;

    if !has_push_target(root) {
        return Ok(crate::Pushed::Unknown);
    }

    let mut child = Command::new("git")
        .args(["-c", "http.version=HTTP/1.1", "push", "--porcelain"])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| RepoError::Io(format!("git push 启动失败：{e}")))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = child.wait_with_output().map_err(|e| RepoError::Io(e.to_string()))?;
                let _ = status;
                return Ok(if out.status.success() { crate::Pushed::Yes } else { crate::Pushed::No });
            }
            Ok(None) if started.elapsed() < GIT_TIMEOUT => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(crate::Pushed::No);
            }
            Err(e) => return Err(RepoError::Io(format!("git push 等待失败：{e}"))),
        }
    }
}
