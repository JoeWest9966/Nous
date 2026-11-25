//! ssh 命令原语：POSIX 引用、连接参数、远端命令执行、stderr → FileError 分类。
//!
//! 用 `ssh` CLI（尊重宿主 `~/.ssh/config` / known_hosts），零新依赖；`BatchMode=yes`
//! + `ConnectTimeout=10` 保证连不上是 10 秒内的显式错误，不是挂住。

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::config::Machine;
use crate::mux::Mux;

/// POSIX 单引号：`'` → `'\''`；`~` 留给远端 shell 展开成 `$HOME`。
pub fn shq(value: &str) -> String {
    if value == "~" {
        return "$HOME".to_string();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return format!("$HOME/{}", rest.replace('\'', "'\\''"));
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 远端路径解析（POSIX，不借宿主路径语义）。`~` 不展开，留给远端 shell。
pub fn resolve_remote(cwd: &str, path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        return path.to_string();
    }
    if path.starts_with('/') {
        return normalize_posix(path);
    }
    if cwd == "~" || cwd.starts_with("~/") {
        return format!("{}/{}", cwd.trim_end_matches('/'), path);
    }
    normalize_posix(&format!("{}/{}", cwd.trim_end_matches('/'), path))
}

fn normalize_posix(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => { parts.pop(); }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

/// ssh 连接参数（顺序稳定，判据按序列比对）。
pub fn ssh_args(machine: &Machine, key_path: Option<&std::path::Path>, mux: &Mux) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        machine.port.to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=10".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
    ];
    if let Some(path) = key_path {
        args.push("-i".to_string());
        args.push(path.display().to_string());
        args.push("-o".to_string());
        args.push("IdentitiesOnly=yes".to_string());
    } else if let Some(path) = &machine.key_path {
        args.push("-i".to_string());
        args.push(path.clone());
        args.push("-o".to_string());
        args.push("IdentitiesOnly=yes".to_string());
    }
    args.extend(mux.ssh_args(machine));
    args.push(format!("{}@{}", machine.user, machine.addr));
    args
}

/// 一条远端命令的结果（ssh 自己的退出码 + 两段输出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// 连不上时 ssh 退出码 255；10s ConnectTimeout 保证不会挂住。
pub const SSH_CONNECT_FAILED: i32 = 255;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 跑一条远端命令。文件原语强制 `LC_ALL=C`（`stat -c %F` 的措辞随 locale 变）。
pub async fn run(
    machine: &Machine,
    key_path: Option<&std::path::Path>,
    mux: &Mux,
    command: &str,
    stdin: Option<&[u8]>,
    timeout: Option<Duration>,
) -> std::io::Result<RemoteOutput> {
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_args(machine, key_path, mux))
        .arg(format!("LC_ALL=C {command}"))
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    if let Some(bytes) = stdin {
        use tokio::io::AsyncWriteExt;
        if let Some(mut handle) = child.stdin.take() {
            handle.write_all(bytes).await?;
        }
    }
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let status = if let Some(limit) = timeout {
        match tokio::time::timeout(limit, child.wait()).await {
            Ok(result) => result?,
            Err(_) => {
                child.kill().await.ok();
                return Ok(RemoteOutput {
                    status: None,
                    stdout: String::new(),
                    stderr: format!("ssh command timed out after {}s", limit.as_secs()),
                });
            }
        }
    } else {
        child.wait().await?
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    if let Some(pipe) = &mut stdout_pipe {
        use tokio::io::AsyncReadExt;
        pipe.read_to_end(&mut stdout).await?;
    }
    if let Some(pipe) = &mut stderr_pipe {
        use tokio::io::AsyncReadExt;
        pipe.read_to_end(&mut stderr).await?;
    }
    Ok(RemoteOutput {
        status: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// 把远端 stderr 的常见措辞映射成 `FileErrorCode`，避免 500 掩盖可诊断错误。
pub fn classify_stderr(status: Option<i32>, stderr: &str) -> nous_env::FileErrorCode {
    use nous_env::FileErrorCode;
    if status == Some(SSH_CONNECT_FAILED) || status.is_none() {
        return FileErrorCode::Unknown;
    }
    let text = stderr.to_ascii_lowercase();
    if text.contains("no such file") || text.contains("cannot stat") {
        FileErrorCode::NotFound
    } else if text.contains("permission denied") || text.contains("not permitted") {
        FileErrorCode::PermissionDenied
    } else if text.contains("not a directory") {
        FileErrorCode::NotDirectory
    } else if text.contains("is a directory") {
        FileErrorCode::IsDirectory
    } else {
        FileErrorCode::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_single_quotes_and_leaves_tilde_to_remote_shell() {
        assert_eq!(shq("a'b"), "'a'\\''b'");
        assert_eq!(shq("~"), "$HOME");
        assert_eq!(shq("~/x"), "$HOME/x");
        assert_eq!(shq("/tmp/x"), "'/tmp/x'");
    }

    #[test]
    fn remote_paths_never_borrow_host_path_semantics() {
        assert_eq!(resolve_remote("~", "b"), "~/b");
        assert_eq!(resolve_remote("~/dir", "b"), "~/dir/b");
        assert_eq!(resolve_remote("/a/../b", "c"), "/b/c");
    }
}
