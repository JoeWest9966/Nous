//! `crates/rpi-tools/src/env.rs` 里未进 `nous-env` 契约面的那几样：本地 compat。
//!
//! #6 按 `packages/durable/src/env/index.ts` 取契约面，明确不取
//! `ShellOverride`（自定义解释器）/ `ShellOutput{stdout,stderr,exit_code}`
//! （全文捕获）/ 可复用追加句柄 / `WriterLease` / 手写 fsync。本 crate 的
//! `os_env` / `shell_output` / `in_memory` 是上游原样提货，工具层照旧用这些
//! 形状；把它们放在 crate 本地 compat 里，就不必往 `nous-env` 契约面再塞一份
//! （AGENTS.md §二：ExecutionEnv 只有一份，#6 票面）。
//!
//! `check_cancel_file` / `check_cancel_exec` 是与 `ContextExt::check` 等价的
//! `Option<&CancellationToken>` 转发，调用点上游照抄，不改逻辑。

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use nous_env::{ExecutionError, FileError, FileErrorCode};

pub fn check_cancel_file(
    cancel: Option<&CancellationToken>,
    path: Option<&str>,
) -> Result<(), FileError> {
    if let Some(token) = cancel {
        if token.is_cancelled() {
            let mut error = FileError::new(FileErrorCode::Aborted, "operation aborted");
            if let Some(p) = path {
                error = error.with_path(p);
            }
            return Err(error);
        }
    }
    Ok(())
}

pub fn check_cancel_exec(cancel: Option<&CancellationToken>) -> Result<(), ExecutionError> {
    if cancel.map(|t| t.is_cancelled()).unwrap_or(false) {
        Err(ExecutionError::aborted())
    } else {
        Ok(())
    }
}

/// `crates/rpi-tools/src/env.rs` 的 `ShellOutput`：上游 `Shell::exec` 的全文捕获返回值。
#[derive(Debug, Clone)]
pub struct ShellOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// 上游 `ShellOverride`：一个自定义解释器 + 固定参数（`nous-env` 的 `Command::Argv` 不是它）。
#[derive(Clone, Debug)]
pub struct ShellOverride {
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
}

/// 上游可复用追加句柄的回执（reader 是同一文件的生成句柄）。
#[derive(Clone)]
pub struct AppendReceipt {
    pub reader: Arc<dyn ReadFile>,
    pub offset: u64,
    pub length: usize,
}

#[async_trait]
pub trait ReadFile: Send + Sync {
    async fn read_all(&self) -> Result<Vec<u8>, FileError>;
    async fn read_range(&self, offset: u64, length: usize) -> Result<Vec<u8>, FileError>;
    async fn byte_len(&self) -> Result<Option<u64>, FileError> {
        Ok(None)
    }
}

#[async_trait]
pub trait AppendFile: Send + Sync {
    async fn append(&mut self, bytes: &[u8]) -> Result<(), FileError>;
    async fn append_with_receipt(
        &mut self,
        bytes: &[u8],
    ) -> Result<Option<AppendReceipt>, FileError> {
        self.append(bytes).await?;
        Ok(None)
    }
}


// ─────────────────────────────────────────────────────────────────────────────
// 上游 `crates/rpi-tools/src/env.rs` 的 trait 面（#6 取 TS 那份契约后，Rust 这份
// 不再住在 `nous-env`）。这里是它的**本地 compat**，只有 trait 声明，没有任何
// IO 实现：OsExecutionEnv / InMemoryExecutionEnv 的实现体是上游照抄的，落在这
// 个 trait 名下。`AGENTS.md §二` 的「ExecutionEnv 只有一份」说的是对外契约
// （`nous_env::ExecutionEnv`），这里不新增对外契约，只是让冻结的上游实现能编译。
pub use nous_env::FileContent;

/// 上游 `env.rs` 的 `FileInfo`：`path` 是 `PathBuf`（#6 的 `nous_env::FileInfo`
/// 是 `String`）。形状只在工具层内部用，不跨线。
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub name: String,
    pub path: std::path::PathBuf,
    pub kind: nous_env::FileKind,
    pub size: u64,
    pub mtime_ms: i64,
}

#[async_trait]
pub trait LegacyFileSystem: Send + Sync {
    fn cwd(&self) -> &std::path::Path;
    async fn absolute_path(&self, path: &str, cancel: Option<&CancellationToken>) -> Result<std::path::PathBuf, FileError>;
    async fn join_path(&self, parts: &[&str], cancel: Option<&CancellationToken>) -> Result<std::path::PathBuf, FileError>;
    async fn read_text_file(&self, path: &str, cancel: Option<&CancellationToken>) -> Result<String, FileError>;
    async fn read_text_lines(
        &self,
        path: &str,
        max_lines: Option<usize>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<String>, FileError>;
    async fn read_binary_file(&self, path: &str, cancel: Option<&CancellationToken>) -> Result<Vec<u8>, FileError>;
    async fn write_file(&self, path: &str, content: nous_env::FileContent, cancel: Option<&CancellationToken>) -> Result<(), FileError>;
    async fn append_file(&self, path: &str, content: nous_env::FileContent, cancel: Option<&CancellationToken>) -> Result<(), FileError>;
    async fn truncate_file(&self, path: &str, len: u64, cancel: Option<&CancellationToken>) -> Result<(), FileError> {
        let _ = (len, cancel);
        Err(FileError::new(FileErrorCode::NotSupported, "file truncation is unavailable").with_path(path))
    }
    async fn rename_file(&self, source: &str, destination: &str, cancel: Option<&CancellationToken>) -> Result<(), FileError>;
    async fn file_info(&self, path: &str, cancel: Option<&CancellationToken>) -> Result<FileInfo, FileError>;
    async fn list_dir(&self, path: &str, cancel: Option<&CancellationToken>) -> Result<Vec<FileInfo>, FileError>;
    async fn canonical_path(&self, path: &str, cancel: Option<&CancellationToken>) -> Result<std::path::PathBuf, FileError>;
    async fn exists(&self, path: &str, cancel: Option<&CancellationToken>) -> Result<bool, FileError>;
    async fn create_dir(
        &self,
        path: &str,
        recursive: bool,
        cancel: Option<&CancellationToken>,
    ) -> Result<(), FileError>;
    async fn remove(
        &self,
        path: &str,
        recursive: bool,
        force: bool,
        cancel: Option<&CancellationToken>,
    ) -> Result<(), FileError>;
    async fn create_temp_dir(&self, prefix: Option<&str>, cancel: Option<&CancellationToken>) -> Result<std::path::PathBuf, FileError>;
    async fn create_temp_file(&self, prefix: &str, suffix: &str, cancel: Option<&CancellationToken>) -> Result<std::path::PathBuf, FileError>;
    async fn cleanup(&self);
}

#[async_trait]
pub trait LegacyShell: Send + Sync {
    async fn exec<'a>(
        &'a self,
        command: &str,
        options: LegacyShellExecOptions<'a>,
    ) -> Result<ShellOutput, ExecutionError>;
    async fn cleanup(&self);
}



/// 上游 `ShellExecOptions<'a>` 的本地形状（#6 的 `nous_env::ShellExecOptions` 是 TS 那份）。
pub struct LegacyShellExecOptions<'a> {
    pub shell: Option<ShellOverride>,
    pub cwd: Option<std::path::PathBuf>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub inherit_env: bool,
    pub timeout: Option<f64>,
    pub cancel: Option<&'a CancellationToken>,
    pub on_stdout: Option<Box<dyn FnMut(&str) + Send + 'a>>,
    pub on_stderr: Option<Box<dyn FnMut(&str) + Send + 'a>>,
}

impl<'a> Default for LegacyShellExecOptions<'a> {
    fn default() -> Self {
        Self {
            shell: None,
            cwd: None,
            env: None,
            inherit_env: true,
            timeout: None,
            cancel: None,
            on_stdout: None,
            on_stderr: None,
        }
    }
}



/// 兼容桥：`nous_env::ExecutionEnv`（#6 的 TS 形状契约）与上游的
/// `LegacyExecutionEnv`（Rust 形状）是两个不同的 trait；`MutatingEnv` / 工具
/// context 收的是后者。二者在 `nous-tools` 内只作类型标记，不额外要求方法。
pub trait LegacyExecutionEnv: LegacyFileSystem + LegacyShell {}
