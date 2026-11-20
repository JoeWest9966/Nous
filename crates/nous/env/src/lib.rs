//! `ExecutionEnv` 契约层：`FileSystem` + `Shell` 两张 trait 面 + 四处私库路径。
//!
//! ## 权威
//!
//! 形状取 `packages/durable/src/env/index.ts`（TS），**不是** `crates/rpi-tools/src/env.rs`
//! —— 理由不是「哪份更全」，是调用点数量：durable 侧 116 处、rpi-tools 侧 27 处。
//! 取 TS 形状要改 27 处，取 Rust 形状要改 116 处里的大多数，还得往 trait 上补四个它
//! 没有的能力（`openBinaryReader` 10 处、`openDirReader` 6 处、`listFiles`、`watch`）。
//! 详细对照表见 #6 票面。逐行出处见仓库根 `SOURCES.md`。
//!
//! ## 这一层不认识的东西
//!
//! - **没有 IO 实现**。本地实现、SSH / 远端实现、测试用的 InMemory 都在这个 trait 之下
//!   （AGENTS.md §二：`ExecutionEnv` 只有一份）。私库路径的解析同样是纯的：只查进程
//!   环境并给默认值，不碰盘（[`content`]）。
//! - **只有一个内部依赖：`nous-chord`**，而且只为 [`Context`]（取消 + 作用域值）。
//!   `cargo tree -p nous-env` 里出现 `nous-ai` / `nous-durable` / `nous-tools` /
//!   `reqwest` / `rusqlite` 就是契约图破了。
//! - **没有 JSONL 专用的那四样**：可复用的追加句柄（带回执）、跨进程写者租约、
//!   目录 fsync、自定义解释器（`ShellOverride`）。
//!   它们服务的是 rpi-durable 的追加日志，本库写 SQLite。台账各记一行 待提货 并写明
//!   理由，为的是以后不用再讨论一遍。
//!
//! ## 结构性改动只有一处
//!
//! 所有方法统一收 `&Context`（TS 是每方法最后一个 `context` 参数）。这个 `Context`
//! **就是 `nous-chord` 那一份**（`pub use`，不是新类型）：TS 侧只有一份，本库也只有一份。
//! 进度是挂在它上面的一个 key（[`PROGRESS_KEY`]），不是结构体字段。
//!
//! ## 三档能力
//!
//! 必需 14 个、可选 12 个，写在 [`FileSystem`] 的文档里，逐条标了。可选能力的默认实现
//! 一律 `not_supported` —— **明确的失败**，调用方必须显式降级，不许把「没有能力」当成
//! 「这里什么都没有」。
//!
//! ```
//! use nous_env::{Context, FileSystem};
//!
//! async fn read_or_empty(env: &dyn FileSystem, path: &str) -> Result<String, nous_env::FileError> {
//!     let context = Context::new();
//!     match env.read_text_file(path, &context).await {
//!         Ok(text) => Ok(text),
//!         Err(error) if error.code == nous_env::FileErrorCode::NotFound => Ok(String::new()),
//!         Err(error) => Err(error),
//!     }
//! }
//! ```

pub mod content;
pub mod context;
pub mod error;
pub mod fs;
pub mod shell;
pub mod types;

pub use content::{ContentPath, ALL, EXTEND, PRESETS, PROMPTS, SKILLS};
pub use context::{
    Context, ContextExt, ContextKey, Progress, ProgressSink, PROGRESS_KEY,
};
pub use error::{
    io_to_file_code, io_to_file_error, ExecutionError, ExecutionErrorCode, FileError,
    FileErrorCode,
};
pub use fs::{paths_change, ExecutionEnv, FileSystem};
pub use shell::Shell;
pub use types::{
    BinaryReader, ChangeSink, Command, CreateDirOptions, DirPage, DirReader, FileChange,
    FileContent, FileInfo, FileKind, FileListEntry, FileListing, FileWatcher, LineScan,
    ListFilesOptions, OpenBinaryReaderOptions, OutputSink, ReadTextLinesOptions, RemoveOptions,
    ScanLinesOptions, ShellExecOptions, ShellExecResult, ShellOutputInfo, ShellOutputSkip,
    ShellOutputWindow, ShellSpillOptions, ShellStream, TempFileOptions, TextLine, TextLineReader,
    WatchExclude, WatchMode, WatchTarget,
};
