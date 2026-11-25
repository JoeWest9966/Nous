//! `SshExecutionEnv`：`nous_env::FileSystem` + `nous_env::Shell` 的 SSH 实现。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use nous_env::{
    Command, Context, ContextExt, CreateDirOptions, ExecutionError, ExecutionErrorCode,
    FileContent, FileError, FileErrorCode, FileInfo, FileKind, FileSystem,
    OutputSink, RemoveOptions, Shell, ShellExecOptions, ShellExecResult, ShellStream,
};

use crate::config::Machine;
use crate::key::TemporaryKey;
use crate::mux::Mux;
use crate::remote::{self, RemoteOutput, SSH_CONNECT_FAILED};

/// 一次远端命令的默认超时：连不上由 ssh 的 `ConnectTimeout=10` 管，命令本身也设 10s。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct SshExecutionEnv {
    machine: Machine,
    cwd: String,
    /// 内嵌私钥生成的临时文件；`keyPath` 型机器走 `machine.key_path`，这里为 None。
    key: Option<TemporaryKey>,
    mux: Mux,
}

impl SshExecutionEnv {
    /// 从机器编目里的名字构造；`pem` 是可选的内嵌私钥内容（只在内存里过一手）。
    pub fn new(machine: Machine, pem: Option<&str>) -> std::io::Result<Self> {
        let cwd = machine.cwd.clone();
        let key = match pem {
            Some(pem) if !pem.is_empty() => Some(TemporaryKey::new(pem)?),
            _ => None,
        };
        Ok(Self { machine, cwd, key, mux: Mux::new() })
    }

    /// 私钥文件路径（公共接缝，测试断言 0600 与清理）。
    pub fn key_path(&self) -> Option<&Path> {
        self.key.as_ref().map(|key| key.path())
    }

    fn key_path_ref(&self) -> Option<&Path> {
        self.key_path()
    }

    fn resolved(&self, path: &str) -> String {
        remote::resolve_remote(&self.cwd, path)
    }

    async fn run_remote(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        timeout: Option<Duration>,
    ) -> Result<RemoteOutput, FileError> {
        remote::run(&self.machine, self.key_path_ref(), &self.mux, command, stdin, timeout)
            .await
            .map_err(|error| {
                FileError::new(FileErrorCode::Unknown, format!("ssh 启动失败：{error}"))
            })
    }

    fn file_error(&self, output: &RemoteOutput, path: &str) -> FileError {
        let code = remote::classify_stderr(output.status, &output.stderr);
        let message = if output.stderr.trim().is_empty() {
            format!("远端命令退出码 {}", output.status.unwrap_or(-1))
        } else {
            output.stderr.trim().to_string()
        };
        FileError::new(code, message).with_path(path)
    }
}

#[async_trait]
impl FileSystem for SshExecutionEnv {
    fn id(&self) -> &str {
        // id 里不能有密钥内容；只标识「这个远端命名空间」。
        "ssh"
    }

    fn cwd(&self) -> &Path {
        Path::new(&self.cwd)
    }

    async fn absolute_path(&self, path: &str, _context: &Context) -> Result<String, FileError> {
        Ok(self.resolved(path))
    }

    async fn join_path(&self, parts: &[&str], _context: &Context) -> Result<String, FileError> {
        Ok(parts.join("/"))
    }

    async fn read_text_file(&self, path: &str, context: &Context) -> Result<String, FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let output = self
            .run_remote(&format!("base64 -w0 < {}", remote::shq(&resolved)), None, Some(DEFAULT_TIMEOUT))
            .await?;
        if output.status != Some(0) {
            return Err(self.file_error(&output, &resolved));
        }
        use base64::Engine;
        let clean: String = output.stdout.chars().filter(|c| !c.is_whitespace()).collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(clean.as_bytes())
            .map_err(|error| FileError::new(FileErrorCode::Invalid, format!("base64: {error}")))?;
        String::from_utf8(bytes)
            .map_err(|error| FileError::new(FileErrorCode::Invalid, format!("utf8: {error}")))
    }

    async fn read_binary_file(&self, path: &str, context: &Context) -> Result<Vec<u8>, FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let output = self
            .run_remote(&format!("base64 -w0 < {}", remote::shq(&resolved)), None, Some(DEFAULT_TIMEOUT))
            .await?;
        if output.status != Some(0) {
            return Err(self.file_error(&output, &resolved));
        }
        use base64::Engine;
        let clean: String = output.stdout.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD
            .decode(clean.as_bytes())
            .map_err(|error| FileError::new(FileErrorCode::Invalid, format!("base64: {error}")))
    }

    async fn write_file(
        &self,
        path: &str,
        content: FileContent,
        context: &Context,
    ) -> Result<(), FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let parent = Path::new(&resolved)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "/".to_string());
        let command = format!(
            "mkdir -p {} && cat > {}",
            remote::shq(&parent),
            remote::shq(&resolved)
        );
        let output = self.run_remote(&command, Some(content.as_bytes()), Some(DEFAULT_TIMEOUT)).await?;
        if output.status != Some(0) {
            return Err(self.file_error(&output, &resolved));
        }
        Ok(())
    }

    async fn append_file(
        &self,
        path: &str,
        content: FileContent,
        context: &Context,
    ) -> Result<(), FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let command = format!("cat >> {}", remote::shq(&resolved));
        let output = self.run_remote(&command, Some(content.as_bytes()), Some(DEFAULT_TIMEOUT)).await?;
        if output.status != Some(0) {
            return Err(self.file_error(&output, &resolved));
        }
        Ok(())
    }

    async fn exists(&self, path: &str, context: &Context) -> Result<bool, FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let output = self
            .run_remote(&format!("test -e {} && echo yes || echo no", remote::shq(&resolved)), None, Some(DEFAULT_TIMEOUT))
            .await?;
        if output.status == Some(SSH_CONNECT_FAILED) {
            return Err(self.file_error(&output, &resolved));
        }
        Ok(output.stdout.trim() == "yes")
    }

    async fn file_info(&self, path: &str, context: &Context) -> Result<FileInfo, FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let command = format!(
            "stat -c '%F|%s|%Y' {} 2>&1; printf '\\n\\n__exit__%s' \"$?\"",
            remote::shq(&resolved)
        );
        let output = self.run_remote(&command, None, Some(DEFAULT_TIMEOUT)).await?;
        let (text, exit) = split_exit_marker(&output.stdout);
        if exit != 0 || text.is_empty() {
            return Err(self.file_error(&output, &resolved));
        }
        let mut fields = text.splitn(3, '|');
        let kind_text = fields.next().unwrap_or("");
        let size = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let mtime = fields.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        let kind = match kind_text {
            "regular file" => FileKind::File,
            "directory" => FileKind::Directory,
            "symbolic link" => FileKind::Symlink,
            _ => return Err(FileError::new(FileErrorCode::Invalid, format!("不支持的文件类型：{kind_text}"))),
        };
        Ok(FileInfo {
            name: Path::new(&resolved).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            path: resolved,
            kind,
            size,
            mtime_ms: mtime.saturating_mul(1000),
        })
    }

    async fn list_dir(&self, path: &str, context: &Context) -> Result<Vec<FileInfo>, FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let command = format!(
            "find {} -mindepth 1 -maxdepth 1 -printf '%y\\t%f\\t%s\\t%T@\\n' 2>&1; printf '\\n\\n__exit__%s' \"$?\"",
            remote::shq(&resolved)
        );
        let output = self.run_remote(&command, None, Some(DEFAULT_TIMEOUT)).await?;
        let (text, exit) = split_exit_marker(&output.stdout);
        if exit != 0 {
            return Err(self.file_error(&output, &resolved));
        }
        let mut entries = Vec::new();
        for line in text.lines() {
            let mut fields = line.splitn(4, '\t');
            let Some(kind_text) = fields.next() else { continue };
            let Some(name) = fields.next() else { continue };
            let size = fields.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let mtime = fields.next().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let kind = match kind_text {
                "f" => FileKind::File,
                "d" => FileKind::Directory,
                "l" => FileKind::Symlink,
                _ => continue,
            };
            entries.push(FileInfo {
                name: name.to_string(),
                path: format!("{}/{}", resolved.trim_end_matches('/'), name),
                kind,
                size,
                mtime_ms: (mtime * 1000.0) as i64,
            });
        }
        Ok(entries)
    }

    async fn create_dir(
        &self,
        path: &str,
        options: CreateDirOptions,
        context: &Context,
    ) -> Result<(), FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let flag = if options.recursive { "-p" } else { "" };
        let output = self
            .run_remote(&format!("mkdir {flag} {}", remote::shq(&resolved)), None, Some(DEFAULT_TIMEOUT))
            .await?;
        if output.status != Some(0) {
            return Err(self.file_error(&output, &resolved));
        }
        Ok(())
    }

    async fn remove(&self, path: &str, options: RemoveOptions, context: &Context) -> Result<(), FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let mut flags = String::new();
        if options.recursive { flags.push('r'); }
        if options.force { flags.push('f'); }
        let output = self
            .run_remote(&format!("rm -{} {}", flags, remote::shq(&resolved)), None, Some(DEFAULT_TIMEOUT))
            .await?;
        if output.status != Some(0) {
            return Err(self.file_error(&output, &resolved));
        }
        Ok(())
    }

    async fn rename_file(&self, source: &str, destination: &str, context: &Context) -> Result<(), FileError> {
        context.check()?;
        let source = self.resolved(source);
        let destination = self.resolved(destination);
        let output = self
            .run_remote(
                &format!("mv {} {}", remote::shq(&source), remote::shq(&destination)),
                None,
                Some(DEFAULT_TIMEOUT),
            )
            .await?;
        if output.status != Some(0) {
            return Err(self.file_error(&output, &source));
        }
        Ok(())
    }

    async fn create_temp_dir(&self, prefix: Option<&str>, context: &Context) -> Result<String, FileError> {
        context.check()?;
        let prefix = prefix.unwrap_or("tmp-");
        let output = self
            .run_remote(
                &format!("mktemp -d {}", remote::shq(&format!("/tmp/{prefix}.XXXXXX"))),
                None,
                Some(DEFAULT_TIMEOUT),
            )
            .await?;
        if output.status != Some(0) {
            return Err(self.file_error(&output, "/tmp"));
        }
        Ok(output.stdout.trim().to_string())
    }

    async fn cleanup(&self, _context: &Context) {
        // ssh 进程由 `kill_on_drop` 收；这里删密钥与 mux socket 目录。
        if let Some(key) = &self.key {
            crate::key::remove(key.path());
        }
        // Mux 是字段，&self 下无法 take；由 Drop 完成清理。
    }

    async fn canonical_path(&self, path: &str, context: &Context) -> Result<String, FileError> {
        context.check()?;
        let resolved = self.resolved(path);
        let command = format!(
            "realpath {} 2>&1; printf '\\n\\n__exit__%s' \"$?\"",
            remote::shq(&resolved)
        );
        let output = self.run_remote(&command, None, Some(DEFAULT_TIMEOUT)).await?;
        let (text, exit) = split_exit_marker(&output.stdout);
        if exit != 0 {
            return Err(self.file_error(&output, &resolved));
        }
        Ok(text.trim().to_string())
    }
}

impl Drop for SshExecutionEnv {
    fn drop(&mut self) {
        // Drop 里只做同步清理（无 IO 运行时也能跑）。
        if let Some(key) = self.key.take() {
            drop(key);
        }
        self.mux.cleanup();
    }
}

#[async_trait]
impl Shell for SshExecutionEnv {
    async fn exec(
        &self,
        command: &Command,
        options: &ShellExecOptions,
        context: &Context,
    ) -> Result<ShellExecResult, ExecutionError> {
        context.check_exec()?;
        let cwd = options.cwd.as_deref().map(|cwd| self.resolved(cwd)).unwrap_or_else(|| self.cwd.clone());
        let body = match command {
            Command::Line(line) => line.clone(),
            Command::Argv(argv) => argv.iter().map(|part| remote::shq(part)).collect::<Vec<_>>().join(" "),
        };
        let remote_command = format!(
            "cd {} 2>/dev/null || exit 126; {}",
            remote::shq(&cwd),
            body
        );
        let timeout = options.timeout.or(Some(DEFAULT_TIMEOUT));
        let output = remote::run(&self.machine, self.key_path_ref(), &self.mux, &remote_command, None, timeout)
            .await
            .map_err(|error| ExecutionError::new(ExecutionErrorCode::SpawnError, error.to_string()))?;
        if let Some(sink) = &options.on_output {
            emit_output(sink, &output.stdout, context, ShellStream::Stdout);
            emit_output(sink, &output.stderr, context, ShellStream::Stderr);
        }
        match output.status {
            Some(126) => Err(ExecutionError::new(ExecutionErrorCode::SpawnError, format!("工作目录不存在：{cwd}"))),
            Some(SSH_CONNECT_FAILED) => Err(ExecutionError::new(
                ExecutionErrorCode::SpawnError,
                format!("ssh 连不上 {}:{}", self.machine.addr, self.machine.port),
            )),
            Some(code) => Ok(ShellExecResult { exit_code: code, spill_path: None }),
            None => Err(ExecutionError::new(ExecutionErrorCode::Timeout, "ssh 命令超时")),
        }
    }

    async fn cleanup(&self, _context: &Context) {
        // 密钥/mux 在 Drop 里同步清理；这里不额外阻塞。
    }
}

fn emit_output(sink: &OutputSink, text: &str, context: &Context, stream: ShellStream) {
    if text.is_empty() {
        return;
    }
    use nous_env::ShellOutputInfo;
    sink(text, context, &ShellOutputInfo { stream, skipped: None });
}

/// 拆出远端命令尾部 `__exit__N` 标记：stat/realpath 非零时 stdout 空、stderr 才有话说。
fn split_exit_marker(stdout: &str) -> (&str, i32) {
    let trimmed = stdout.trim_end();
    if let Some(position) = trimmed.rfind("__exit__") {
        let (text, tail) = trimmed.split_at(position);
        let exit = tail["__exit__".len()..].trim().parse().unwrap_or(1);
        return (text.trim_end_matches('\n'), exit);
    }
    (trimmed, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_marker_splits_even_when_stdout_is_empty() {
        assert_eq!(split_exit_marker("\n\n__exit__1"), ("", 1));
        assert_eq!(split_exit_marker("/tmp/x\n\n__exit__0"), ("/tmp/x", 0));
    }
}
