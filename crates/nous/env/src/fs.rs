//! `FileSystem` —— 便携的文件能力面。
//!
//! 形状 1:1 照 `packages/durable/src/env/index.ts`。操作**返回失败**，不抛。
//!
//! # 三档能力（写进契约，不是建议）
//!
//! **必需**（无默认实现，后端必须写）：
//! `id` `cwd` `absolute_path` `join_path` `read_text_file` `write_file` `exists`
//! `file_info` `list_dir` `create_dir` `remove` `rename_file` `create_temp_dir` `cleanup`
//!
//! **可选**（默认返回 `not_supported`，**调用方必须显式降级**，不许假设它成功）：
//! `read_binary_file` `append_file` `truncate_file` `flush_file` `canonical_path`
//! `read_text_lines` `open_text_line_reader` `open_binary_reader` `open_dir_reader`
//! `list_files` `watch` `create_temp_file`
//!
//! 判据：`not_supported` 是**明确的失败**，不是「没有就到不了」。比如
//! `open_binary_reader` 不支持时，调用方要退回 `read_text_file` 全量读，而不是假装
//! 读到了空文件。
//!
//! # 与 TS 的两处有意偏差
//!
//! - `cwd` 只读（TS 是可写属性）。trait 对象以 `Arc<dyn ExecutionEnv>` 共享，写 cwd
//!   会让并发语义变成「谁后写谁算」，而我们没有那样用的调用方。
//! - 所有方法统一收 `&Context`；TS 是每个方法最后带一个 `context` 参数 —— 顺序与语义
//!   不变，只是 Rust 的惯例把它写成引用（`&Context` 相当于 TS 的 `context` 值传递，
//!   `Context` 本身 `Clone` 且共享取消令牌）。

use std::path::Path;

use async_trait::async_trait;

use crate::context::Context;
use crate::error::{FileError, FileErrorCode};
use crate::types::{
    ChangeSink, CreateDirOptions, DirReader, FileChange, FileContent, FileInfo, FileListEntry,
    FileListing, FileWatcher, ListFilesOptions, OpenBinaryReaderOptions,
    ReadTextLinesOptions, RemoveOptions, TempFileOptions, TextLineReader, WatchTarget,
};

/// 可选能力的统一默认失败。
pub(crate) fn not_supported(method: &'static str) -> FileError {
    FileError::new(
        FileErrorCode::NotSupported,
        format!("{method} is not supported by this environment"),
    )
}

/// Portable filesystem capability.
#[async_trait]
pub trait FileSystem: Send + Sync {
    /// The file namespace: **equal ids see the same files at the same paths**, whatever
    /// their `cwd`. 每个本地 Node 环境共享一个 id；每个容器或远端主机各有自己的。
    ///
    /// 这条比看起来重要：两个环境 id 不同，就不能拿 A 算出来的路径去 B 上读。
    fn id(&self) -> &str;

    /// 这个环境的工作目录（绝对路径）。只读 —— 见模块文档。
    fn cwd(&self) -> &Path;

    /// 把 `path`（相对 cwd、`~`、`file://`、绝对路径都行）解析成被寻址的绝对路径。
    /// **纯函数**：不碰文件系统，不看取消令牌。
    async fn absolute_path(&self, path: &str, context: &Context) -> Result<String, FileError>;

    /// 拼路径。纯函数。
    async fn join_path(&self, parts: &[&str], context: &Context) -> Result<String, FileError>;

    async fn read_text_file(&self, path: &str, context: &Context) -> Result<String, FileError>;

    /// 写文件。**父目录自动递归创建。**
    async fn write_file(
        &self,
        path: &str,
        content: FileContent,
        context: &Context,
    ) -> Result<(), FileError>;

    /// `false` **只**在 `not_found` 时给出；其他错误照常抛。
    /// （TS 的实现委托给 `fileInfo`，不是 `access(F_OK)`，后端照此办理。）
    async fn exists(&self, path: &str, context: &Context) -> Result<bool, FileError>;

    /// 用 `lstat`（不跟随 symlink）。`path` 是被寻址的绝对路径，不是规范化路径。
    async fn file_info(&self, path: &str, context: &Context) -> Result<FileInfo, FileError>;

    async fn list_dir(&self, path: &str, context: &Context) -> Result<Vec<FileInfo>, FileError>;

    /// 建目录。默认 `recursive = true`。
    async fn create_dir(
        &self,
        path: &str,
        options: CreateDirOptions,
        context: &Context,
    ) -> Result<(), FileError>;

    /// 删。默认 `recursive = false`、`force = false`。
    async fn remove(
        &self,
        path: &str,
        options: RemoveOptions,
        context: &Context,
    ) -> Result<(), FileError>;

    async fn rename_file(
        &self,
        source: &str,
        destination: &str,
        context: &Context,
    ) -> Result<(), FileError>;

    /// 建临时目录。默认前缀 `tmp-`。
    async fn create_temp_dir(
        &self,
        prefix: Option<&str>,
        context: &Context,
    ) -> Result<String, FileError>;

    /// Best-effort，**永不返回失败**（杀掉自己还在跑的子进程之类）。
    async fn cleanup(&self, context: &Context);

    // ───────────────────────── 可选能力 ─────────────────────────

    async fn read_binary_file(
        &self,
        _path: &str,
        _context: &Context,
    ) -> Result<Vec<u8>, FileError> {
        Err(not_supported("read_binary_file"))
    }

    async fn append_file(
        &self,
        _path: &str,
        _content: FileContent,
        _context: &Context,
    ) -> Result<(), FileError> {
        Err(not_supported("append_file"))
    }

    /// 把文件截/扩到**正好 `size` 字节**。
    ///
    /// **这就是 TS 的语义：可缩可扩。**（`crates/rpi-tools/src/env.rs` 那版是「只缩不涨
    /// 且默认拒绝」，本库不取那版 —— 它服务的是 rpi-durable 的 JSONL 追加日志，我们写 SQLite。）
    async fn truncate_file(
        &self,
        _path: &str,
        _size: u64,
        _context: &Context,
    ) -> Result<(), FileError> {
        Err(not_supported("truncate_file"))
    }

    /// 把文件和「取回它能读」所需的元数据刷到后端。内存后端可以是空操作，
    /// 但**调用方不许把它当成「已经落盘」的证据**。
    async fn flush_file(
        &self,
        _path: &str,
        _context: &Context,
    ) -> Result<(), FileError> {
        Err(not_supported("flush_file"))
    }

    /// 跟随 symlink 的规范化。后端可能给 `not_found` / `not_supported`，
    /// 调用方（变更队列）要能退回绝对路径。
    async fn canonical_path(
        &self,
        _path: &str,
        _context: &Context,
    ) -> Result<String, FileError> {
        Err(not_supported("canonical_path"))
    }

    async fn read_text_lines(
        &self,
        _path: &str,
        _options: ReadTextLinesOptions,
        _context: &Context,
    ) -> Result<Vec<String>, FileError> {
        Err(not_supported("read_text_lines"))
    }

    /// 不把整个文件读进内存的逐行读。
    async fn open_text_line_reader(
        &self,
        _path: &str,
        _context: &Context,
    ) -> Result<Box<dyn TextLineReader>, FileError> {
        Err(not_supported("open_text_line_reader"))
    }

    /// 开一个普通文件做有界定位读。目录给 `is_directory`，其他非普通文件给 `invalid`。
    async fn open_binary_reader(
        &self,
        _path: &str,
        _options: OpenBinaryReaderOptions,
        _context: &Context,
    ) -> Result<Box<dyn crate::types::BinaryReader>, FileError> {
        Err(not_supported("open_binary_reader"))
    }

    async fn open_dir_reader(
        &self,
        _path: &str,
        _context: &Context,
    ) -> Result<Box<dyn DirReader>, FileError> {
        Err(not_supported("open_dir_reader"))
    }

    /// 目录下面（任意深度）的文件、目录、符号链接，给补全索引之类用。
    ///
    /// **现在列不出来时必须给 `not_supported`**（比如老版本的远端 daemon），
    /// 让调用方退回自己走一遍 —— 不许返回空列表冒充「这里什么都没有」。
    async fn list_files(
        &self,
        _path: &str,
        _options: ListFilesOptions,
        _context: &Context,
    ) -> Result<FileListing, FileError> {
        Err(not_supported("list_files"))
    }

    /// 报告文件和目录的变更。
    ///
    /// 返回的 watcher 一旦存在，**覆盖就已经建立**：先 watch 再去读取的宿主不会漏掉
    /// 加载过程中发生的变更。覆盖有多可靠看 [`FileWatcher::mode`]。
    async fn watch(
        &self,
        _targets: &[WatchTarget],
        _on_change: ChangeSink,
        _context: &Context,
    ) -> Result<Box<dyn FileWatcher>, FileError> {
        Err(not_supported("watch"))
    }

    async fn create_temp_file(
        &self,
        _options: TempFileOptions,
        _context: &Context,
    ) -> Result<String, FileError> {
        Err(not_supported("create_temp_file"))
    }
}

/// `ExecutionEnv extends FileSystem, Shell`。TS 也一样 —— 它只是个 marker，
/// 组合两种能力，本身不加任何方法。
///
/// 工具收到 `Arc<dyn ExecutionEnv>`，`FileSystem` 与 `Shell` 的方法交替用。
pub trait ExecutionEnv: FileSystem + crate::shell::Shell {}

impl<T: FileSystem + crate::shell::Shell + ?Sized> ExecutionEnv for T {}

/// 把 `list_dir` 的结果折成 `list_files` 的一种朴素实现，给只想实现必需能力的后端用。
///
/// 它是**非递归、不过滤隐藏文件、不过 gitignore** 的降级版 —— 真要那些语义就实现
/// `list_files`。留着这个函数是为了让「可选能力不支持」的降级路径有唯一写法。
pub fn list_dir_as_flat_listing(entries: Vec<FileInfo>, truncated: bool) -> FileListing {
    FileListing {
        entries: entries
            .into_iter()
            .map(|info| FileListEntry {
                path: info.name,
                kind: info.kind,
            })
            .collect(),
        truncated,
    }
}

/// 生成一次 `FileChange::Paths`。方便后端与测试构造回调载荷。
pub fn paths_change(paths: Vec<String>) -> FileChange {
    FileChange::Paths { paths }
}
