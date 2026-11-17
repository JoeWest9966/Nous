//! 契约层的值类型与 options bag。
//!
//! 全部照 `packages/durable/src/env/index.ts` 1:1（字段名 → snake_case，`?` → `Option`
//! 或有文档默认值），只有三处类型换成了 Rust 的说法：
//!
//! - `string` 路径 → `String`（**不**用 `PathBuf`：契约面要能原样跨机器传，Windows 的
//!   `\` 与 POSIX 的 `/` 由后端各自解释；`PathBuf` 一进来就绑死本机路径语义）。
//! - `Uint8Array` → `Vec<u8>`。
//! - `timeout?: number` → `Option<Duration>`（TS 那边没写单位，这里把单位钉死）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::context::Context;
use crate::error::FileError;

/// `file` | `directory` | `symlink`。后端一律用 `lstat`（不跟随 symlink）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
}

/// One directory entry's metadata.
///
/// `path` 是**被寻址的绝对路径**，不是 symlink 规范化后的路径（那是
/// [`crate::FileSystem::canonical_path`] 的事）。`mtime_ms` 是 Unix epoch 毫秒。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    pub mtime_ms: i64,
}

/// Write/append payload. TS 的 `string | Uint8Array`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    Text(String),
    Bytes(Vec<u8>),
}

impl FileContent {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            FileContent::Text(text) => text.as_bytes(),
            FileContent::Bytes(bytes) => bytes,
        }
    }

    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }
}

impl From<String> for FileContent {
    fn from(text: String) -> Self {
        FileContent::Text(text)
    }
}

impl From<&str> for FileContent {
    fn from(text: &str) -> Self {
        FileContent::Text(text.to_string())
    }
}

impl From<Vec<u8>> for FileContent {
    fn from(bytes: Vec<u8>) -> Self {
        FileContent::Bytes(bytes)
    }
}

/// One line as `read_text_lines` / `TextLineReader` return it.
///
/// `terminated` = 这一行末尾有换行。文件最后一行没有换行时它是 `false` —— 调用方
/// 靠它决定「读到的这篇是不是被截断在半行上」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    pub text: String,
    pub terminated: bool,
}

/// Line-oriented reads without loading the whole file.
#[async_trait]
pub trait TextLineReader: Send + Sync {
    /// 下 1 行；`None` = 读到文件尾。
    async fn read_line(&self, context: &Context) -> Result<Option<TextLine>, FileError>;
    async fn close(&self, context: &Context);
}

/// Positional reads from one opened regular file.
///
/// **同一路径改名字后，之前的调用仍然看见同一个文件** —— reader 持的是句柄，不是路径。
#[async_trait]
pub trait BinaryReader: Send + Sync {
    /// 打开那个文件的元信息，不是「这个路径现在指的东西」。
    async fn info(&self, context: &Context) -> Result<FileInfo, FileError>;

    /// 从 `offset` 起最多 `length` 字节；**只有到文件尾才会少**。
    async fn read(&self, offset: u64, length: usize, context: &Context)
        -> Result<Vec<u8>, FileError>;

    /// 一遍扫过整个文件，定位第 `[start_line, end_line)` 行（0 基，`end_line` 缺省 = 到结尾）；
    /// 第 `k` 行从第 `k` 个换行字节之后开始。
    async fn scan_lines(
        &self,
        options: ScanLinesOptions,
        context: &Context,
    ) -> Result<LineScan, FileError>;

    async fn close(&self, context: &Context);
}

/// `BinaryReader::scan_lines` 的范围。TS `{ startLine, endLine? }`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanLinesOptions {
    pub start_line: usize,
    pub end_line: Option<usize>,
}

/// Where the selected lines are, as `BinaryReader::scan_lines` found them.
///
/// `selected_bytes` / `first_line_bytes` 是按 `new TextDecoder().decode(file)` 那套
/// 口径算的 UTF-8 长度：**文件开头的 BOM 不计入**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineScan {
    /// 整个文件的换行字节数 —— 它共有 `newlines + 1` 行。
    pub newlines: u64,
    /// 选中范围的字节区间：从第一行的开头到最后一行末尾（不含结尾那个换行）。
    /// 选到文件尾之后是「文件尾处的空区间」。
    pub start: u64,
    pub end: u64,
    /// 第一行在哪里结束：它的换行处，或者文件尾。
    pub first_line_end: u64,
    /// 最后一行从哪里开始。
    pub last_line_start: u64,
    /// 选中范围（以及它的第一行）解码后的 UTF-8 字节数。
    pub selected_bytes: u64,
    pub first_line_bytes: u64,
}

/// Pages of one directory's entries.
#[async_trait]
pub trait DirReader: Send + Sync {
    /// 最多 `max_entries` 条，按文件系统返回的顺序，从上次停下的地方继续。
    /// 元信息读之前就消失的条目会被跳过，不支持的类型也跳过。
    /// **失败或中止之后这个 reader 只能 close。**
    async fn next(&self, max_entries: usize, context: &Context) -> Result<DirPage, FileError>;
    async fn close(&self, context: &Context);
}

/// 一页目录条目。`done` = 到底了（可能和最后一批条目一起到，也可能单独来一页空的）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirPage {
    pub entries: Vec<FileInfo>,
    pub done: bool,
}

/// What `FileSystem::list_files` lists.
///
/// 默认值照 TS 的文档：`hidden` / `gitignore` 都是 `true`，`max_entries` 50 000，
/// `max_bytes` 4 MiB，`timeout` 10 s。`exclude` 里的名字**连同它下面的一切**一起跳过。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListFilesOptions {
    /// 包含 `.` 开头的名字（Windows 上还包括带隐藏属性的文件）。`.git` 永远不列。
    pub hidden: bool,
    /// 在 git 工作树里跳过被忽略的东西：本目录/子目录/父目录的 `.gitignore`、
    /// `.git/info/exclude`、全局 excludes 文件。**和 `git ls-files` 不同：已跟踪但
    /// 命中忽略规则的也会跳过。** 工作树之外 `.gitignore` 不生效。
    pub gitignore: bool,
    pub exclude: Vec<String>,
    pub max_entries: usize,
    pub max_bytes: usize,
    pub timeout: Duration,
}

impl Default for ListFilesOptions {
    fn default() -> Self {
        Self {
            hidden: true,
            gitignore: true,
            exclude: Vec::new(),
            max_entries: 50_000,
            max_bytes: 4 * 1024 * 1024,
            timeout: Duration::from_secs(10),
        }
    }
}

/// One entry of a [`FileListing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListEntry {
    /// 相对被列目录，**每个系统都用 `/` 分隔**。symlink 按 symlink 列，不跟随。
    pub path: String,
    pub kind: FileKind,
}

/// 一次 `list_files` 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListing {
    /// 深度优先，父在子前，兄弟按名字字节序。
    pub entries: Vec<FileListEntry>,
    /// 被 [`ListFilesOptions`] 里的某个上限提前截断了。
    pub truncated: bool,
}

/// A file or directory to watch. **它可能还不存在** —— 创建它也是一次变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchTarget {
    pub path: String,
    /// 监视目录下面的所有东西，不只是它这一层。符号链接不跟随。
    pub recursive: Option<bool>,
    /// 不监视也不上报的条目：`.` 开头的名字，或这里点名的名字。
    pub exclude: Option<WatchExclude>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WatchExclude {
    pub hidden: Option<bool>,
    pub names: Vec<String>,
}

/// What changed, tagged by kind.
///
/// - `paths`：这些路径本身或下面的东西可能变了（目录路径覆盖整棵子树）。
///   **可能有假阳性**；只要 watcher 还健康，真变更不会漏。
/// - `overflow`：有一段时间覆盖不可信（丢事件、重连）。**把监视范围整个重新扫一遍。**
/// - `failed`：watcher 带着 `error` 停了（比如树大到超过环境上限），之后不再有任何回调。
#[derive(Debug)]
pub enum FileChange {
    Paths { paths: Vec<String> },
    Overflow,
    Failed { error: FileError },
}

/// Where change notifications go. 对应 TS 的 `onChange: (change) => void`。
pub type ChangeSink = Arc<dyn Fn(FileChange) + Send + Sync>;

/// `native`：变更大约两秒内报到。`polling`：环境靠比对快照，因为文件系统报告不可靠
/// （网络盘、FUSE）；两次快照之间被撤销的变更可能漏掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchMode {
    Native,
    Polling,
}

/// A live watch. `stop` 之后不会再有任何回调 —— 它是幂等的。
#[async_trait]
pub trait FileWatcher: Send + Sync {
    fn mode(&self) -> WatchMode;
    async fn stop(&self, context: &Context);
}

/// `create_dir` 的 options bag。TS `{ recursive?: boolean }`，默认 `true`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateDirOptions {
    pub recursive: bool,
}

impl Default for CreateDirOptions {
    fn default() -> Self {
        Self { recursive: true }
    }
}

/// `remove` 的 options bag。TS `{ recursive?, force? }`，默认都 `false`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RemoveOptions {
    pub recursive: bool,
    pub force: bool,
}

/// `read_text_lines` 的 options bag。TS `{ maxLines? }`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadTextLinesOptions {
    /// `None` = 读到底。`Some(0)` = 空。
    pub max_lines: Option<usize>,
}

/// `open_binary_reader` 的 options bag。TS `{ noFollow? }`。
///
/// `no_follow`：路径最后一段是符号链接时判 `invalid`，不跟随；前面几段照常解析。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenBinaryReaderOptions {
    pub no_follow: bool,
}

/// `create_temp_file` 的 options bag。TS `{ prefix?, suffix? }`，默认都是空串。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TempFileOptions {
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

impl TempFileOptions {
    pub fn prefix(&self) -> &str {
        self.prefix.as_deref().unwrap_or("")
    }

    pub fn suffix(&self) -> &str {
        self.suffix.as_deref().unwrap_or("")
    }
}

/// 一次 shell 调用。TS 的 `string | readonly string[]`。
///
/// - [`Command::Line`] 走环境的 shell（`/bin/bash` 那套）。
/// - [`Command::Argv`] 直接 exec `argv[0]`，其余当参数，**不经 shell** —— 所以参数里的
///   空格、引号、`$` 都原样到达程序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Line(String),
    Argv(Vec<String>),
}

impl From<&str> for Command {
    fn from(line: &str) -> Self {
        Command::Line(line.to_string())
    }
}

impl From<String> for Command {
    fn from(line: String) -> Self {
        Command::Line(line)
    }
}

impl<const N: usize> From<[&str; N]> for Command {
    fn from(argv: [&str; N]) -> Self {
        Command::Argv(argv.iter().map(|part| part.to_string()).collect())
    }
}

/// 完整输出越过阈值后溢写到临时文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellSpillOptions {
    pub after_bytes: u64,
    /// 完整行或半行都算。
    pub after_lines: u64,
}

/// 调用方只留输出尾巴时给的窗口，以及它采样这个尾巴的频率。
///
/// 慢链路（SSH）用它：调用方本来就要丢的东西不必传，传得也不比调用方提交得快。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellOutputWindow {
    /// 尾巴留多少 UTF-8 字节。
    pub max_bytes: usize,
    /// 尾巴留多少行。
    pub max_lines: usize,
    /// 两次采样之间的最小间隔。
    pub min_interval: Duration,
    /// 每次采样还要按这个速率把调用方拖慢，避免大块输出糊脸。
    pub bytes_per_second: u64,
}

/// 输出里被环境省掉的一段，按 `on_output` 会收到的解码文本计算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellOutputSkip {
    /// 被省掉文本的 UTF-8 字节数。
    pub bytes: u64,
    /// 其中的换行（U+000A）数。
    pub newlines: u64,
    pub ends_with_newline: bool,
}

/// 这个 chunk 来自哪条流。合并了省略时，它写的是 chunk 最后一条流的名字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellStream {
    Stdout,
    Stderr,
}

/// `on_output` 的第三个参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellOutputInfo {
    pub stream: ShellStream,
    /// 紧跟在这个 chunk 之前的省略量，**只在给了 `window` 时出现**。
    pub skipped: Option<ShellOutputSkip>,
}

/// `on_output` 回调。TS `(text, context, info) => void`；每收到一段就调一次，
/// 原始、无界、不节流。**两条流各自解码**，所以跨 chunk 的半个字符不会坏。
pub type OutputSink = Arc<dyn Fn(&str, &Context, &ShellOutputInfo) + Send + Sync>;

/// `Shell::exec` 的 options bag。TS `ShellExecOptions`。
pub struct ShellExecOptions {
    pub cwd: Option<String>,
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// 默认 `true`。`false` 时**只**用 `env`（不带进程环境，也不带 shell 的基础环境）。
    pub inherit_env: bool,
    /// 超过就杀这条命令（只杀它自己的进程）。`None` = 不限。
    pub timeout: Option<Duration>,
    pub on_output: Option<OutputSink>,
    pub spill: Option<ShellSpillOptions>,
    pub window: Option<ShellOutputWindow>,
}

impl Default for ShellExecOptions {
    fn default() -> Self {
        Self {
            cwd: None,
            env: None,
            inherit_env: true,
            timeout: None,
            on_output: None,
            spill: None,
            window: None,
        }
    }
}

impl std::fmt::Debug for ShellExecOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellExecOptions")
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field("inherit_env", &self.inherit_env)
            .field("timeout", &self.timeout)
            .field("has_on_output", &self.on_output.is_some())
            .field("spill", &self.spill)
            .field("window", &self.window)
            .finish()
    }
}

/// TS 的 `{ exitCode, spillPath? }`。
///
/// **注意**：输出不在这里 —— 它走 [`ShellExecOptions::on_output`]。这是 durable 版的
/// 形状（`crates/rpi-tools/src/env.rs` 的 `ShellOutput{stdout,stderr,exit_code}` 是
/// `packages/agent` 的旧形状，本库不取）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellExecResult {
    pub exit_code: i32,
    /// 越过溢写阈值时，完整原始输出所在的临时文件。
    pub spill_path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_files_options_defaults_match_the_ts_docs() {
        let options = ListFilesOptions::default();
        assert!(options.hidden);
        assert!(options.gitignore);
        assert_eq!(options.max_entries, 50_000);
        assert_eq!(options.max_bytes, 4 * 1024 * 1024);
        assert_eq!(options.timeout, Duration::from_secs(10));
        assert!(options.exclude.is_empty());
    }

    #[test]
    fn options_bag_defaults_match_the_ts_docs() {
        assert!(CreateDirOptions::default().recursive);
        let remove = RemoveOptions::default();
        assert!(!remove.recursive && !remove.force);
        assert_eq!(ReadTextLinesOptions::default().max_lines, None);
        assert!(!OpenBinaryReaderOptions::default().no_follow);
        let temp = TempFileOptions::default();
        assert_eq!(temp.prefix(), "");
        assert_eq!(temp.suffix(), "");
        let exec = ShellExecOptions::default();
        assert!(exec.inherit_env, "inheritEnv 默认 true");
        assert!(exec.on_output.is_none() && exec.spill.is_none() && exec.window.is_none());
    }

    #[test]
    fn command_distinguishes_shell_line_from_argv() {
        assert_eq!(Command::from("ls -la"), Command::Line("ls -la".into()));
        assert_eq!(Command::from(String::from("ls")), Command::Line("ls".into()));
        assert_eq!(
            Command::from(["ls", "-la", "/tmp"]),
            Command::Argv(vec!["ls".into(), "-la".into(), "/tmp".into()])
        );
    }

    #[test]
    fn file_content_exposes_bytes_without_copying() {
        assert_eq!(FileContent::from("hé").as_bytes(), "hé".as_bytes());
        assert_eq!(FileContent::from(vec![1u8, 2]).as_bytes(), &[1u8, 2]);
        assert!(FileContent::from("").is_empty());
        assert_eq!(FileContent::from("abc").len(), 3);
    }
}
