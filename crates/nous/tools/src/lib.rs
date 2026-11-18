//! Built-in agent tools (read/write/edit/bash/grep/find/ls) — frozen copy of
//! `bigfish1913/pi-rust@v0.24.2` `crates/rpi-tools/src/**`, with the dependency
//! surface narrowed to `nous-ai` + `nous-env` (票 #7 / AGENTS.md §二 规则 1).
//!
//! 改动只有三类（逐文件见 `SOURCES.md`）：
//! 1. 上游 agent crate 的 use 行 → `nous_ai::{tool,types}`（符号在 #5 已搬过去）；
//! 2. 上游 plugin SDK 的两个环境变量名内联成 `&str`（见 `os_env.rs`）；
//! 3. `os_env.rs` 的 `process::resume_child`（Windows 子进程控制）未提货 ——
//!    需要子进程控制，超出本库契约面；其余部分照抄。
//!
//! `env.rs` / `error.rs` 已由 #6 抽成 `nous-env`，本 crate 不重复定义。

#![forbid(unsafe_code)]

pub mod compat;
pub mod error_compat;

pub mod file_io;
pub mod file_mutation_queue;
pub mod image;
pub mod image_processing;
pub mod in_memory;
pub mod os_env;
pub mod output_decode;
pub mod path_utils;
pub mod shell_output;
pub mod truncate;

pub mod utils {
    pub mod git;
}

pub mod tools;
pub use tools::{
    create_bash_tool, create_edit_tool, create_find_tool, create_grep_tool, create_ls_tool,
    create_powershell_tool, create_read_tool, create_write_tool, BashExecution, BashPrepare,
    BashToolOptions, ExecutionToolContext, ProcessedImage, ReadImageProcessor, ReadToolOptions,
};

pub use in_memory::{InMemoryExecutionEnv, ShellScript};

/// 组装根要能写出「工具表」这个类型，但不必为此依赖 `nous-ai`。
///
/// 重导出的是**同一个 trait**（不是包装）：`nous-ai` 是工具层的上游，
/// 端点层只经由这一层碰它，依赖图上就没有 `nous-server → nous-ai` 那条边。
pub use nous_ai::tool::AgentTool;

/// 工具名 → 工具实例。组装根用它把工具表交给轮次层，不必自己拼 `BTreeMap` 的类型。
pub type ToolMap = std::collections::BTreeMap<String, std::sync::Arc<dyn AgentTool>>;

/// 内置工具的一整张表：bash / read / grep / find / ls，工作目录是 `cwd`。
///
/// **这里是工具表唯一的构造点**：谁要装工具就调它，别在各处重复 `insert("bash", …)` ——
/// 那样每加一个工具就要改 N 个地方，且"某个入口少装了一个工具"这种 bug 不会报错。
pub fn builtin_tools(cwd: impl AsRef<std::path::Path>) -> ToolMap {
    let env = std::sync::Arc::new(OsExecutionEnv::with_cwd(cwd.as_ref().to_path_buf()));
    let context = tools::tool_context::ExecutionToolContext::new(
        env.clone(),
        Some(env as std::sync::Arc<dyn file_mutation_queue::MutatingEnv>),
    );
    let mut map: ToolMap = std::collections::BTreeMap::new();
    map.insert("bash".into(), create_bash_tool(&context, None));
    map.insert("read".into(), create_read_tool(&context, None));
    map.insert("grep".into(), create_grep_tool(&context, None));
    map.insert("find".into(), create_find_tool(&context, None));
    map.insert("ls".into(), create_ls_tool(&context, None));
    map
}
pub use crate::compat::{
    LegacyExecutionEnv as ExecutionEnv, LegacyFileSystem as FileSystem, LegacyShell as Shell,
    LegacyShellExecOptions as ShellExecOptions, ShellOutput,
};
pub use nous_env::{
    ExecutionError, ExecutionErrorCode, FileError, FileErrorCode, FileInfo, FileKind,
};
pub use os_env::{is_host_private_env, OsExecutionEnv, HOST_PRIVATE_ENV};
pub use path_utils::{normalize_tool_path, resolve_read_tool_path, resolve_tool_path};
pub use truncate::{
    format_size, split_lines_for_counting, truncate_head, truncate_line, truncate_tail,
    TruncateLineOutput, TruncationLimit, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES,
    DEFAULT_MAX_LINES, GREP_MAX_LINE_LENGTH,
};
