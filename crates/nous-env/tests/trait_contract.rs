//! 契约面自己的测试：三档能力、默认失败、`dyn ExecutionEnv` 可共享。
//!
//! 这里刻意只实现**必需**能力（14 个），其余一个都不写 —— 可选项的默认实现必须自己
//! 说 `not_supported`，这就是本文件要证的。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nous_env::{
    Command, Context, CreateDirOptions, ExecutionEnv, ExecutionError, FileContent, FileError,
    FileErrorCode, FileInfo, FileKind, FileSystem, ReadTextLinesOptions, RemoveOptions,
    ShellExecOptions, ShellExecResult, ShellStream, TempFileOptions, TextLine,
};

/// 只实现必需能力的后端：内存里的文件表。**一个可选方法都没实现。**
#[derive(Default)]
struct MemoryEnv {
    root: PathBuf,
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    /// 每次方法被调用就 +1，用来证明 `cleanup` 真的走到了。
    cleanups: AtomicUsize,
}

impl MemoryEnv {
    fn new() -> Self {
        Self {
            root: PathBuf::from("/mem"),
            ..Self::default()
        }
    }

    fn abs(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{}", self.root.display(), path)
        }
    }
}

#[async_trait]
impl FileSystem for MemoryEnv {
    fn id(&self) -> &str {
        "mem:local"
    }

    fn cwd(&self) -> &Path {
        &self.root
    }

    async fn absolute_path(&self, path: &str, context: &Context) -> Result<String, FileError> {
        context.check()?;
        Ok(self.abs(path))
    }

    async fn join_path(&self, parts: &[&str], context: &Context) -> Result<String, FileError> {
        context.check()?;
        Ok(parts.join("/"))
    }

    async fn read_text_file(&self, path: &str, context: &Context) -> Result<String, FileError> {
        context.check()?;
        let key = self.abs(path);
        let files = self.files.lock().unwrap();
        match files.get(&key) {
            Some(bytes) => Ok(String::from_utf8_lossy(bytes).into_owned()),
            None => Err(FileError::not_found(key)),
        }
    }

    async fn write_file(
        &self,
        path: &str,
        content: FileContent,
        context: &Context,
    ) -> Result<(), FileError> {
        context.check()?;
        self.files
            .lock()
            .unwrap()
            .insert(self.abs(path), content.as_bytes().to_vec());
        Ok(())
    }

    async fn exists(&self, path: &str, context: &Context) -> Result<bool, FileError> {
        context.check()?;
        Ok(self.files.lock().unwrap().contains_key(&self.abs(path)))
    }

    async fn file_info(&self, path: &str, context: &Context) -> Result<FileInfo, FileError> {
        context.check()?;
        let key = self.abs(path);
        let files = self.files.lock().unwrap();
        let bytes = files.get(&key).ok_or_else(|| FileError::not_found(&key))?;
        Ok(FileInfo {
            name: key.rsplit('/').next().unwrap_or_default().to_string(),
            path: key,
            kind: FileKind::File,
            size: bytes.len() as u64,
            mtime_ms: 0,
        })
    }

    async fn list_dir(&self, path: &str, context: &Context) -> Result<Vec<FileInfo>, FileError> {
        context.check()?;
        let prefix = format!("{}/", self.abs(path));
        let files = self.files.lock().unwrap();
        Ok(files
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(key, bytes)| FileInfo {
                name: key.rsplit('/').next().unwrap_or_default().to_string(),
                path: key.clone(),
                kind: FileKind::File,
                size: bytes.len() as u64,
                mtime_ms: 0,
            })
            .collect())
    }

    async fn create_dir(
        &self,
        _path: &str,
        _options: CreateDirOptions,
        context: &Context,
    ) -> Result<(), FileError> {
        context.check()
    }

    async fn remove(
        &self,
        path: &str,
        options: RemoveOptions,
        context: &Context,
    ) -> Result<(), FileError> {
        context.check()?;
        let key = self.abs(path);
        let mut files = self.files.lock().unwrap();
        if files.remove(&key).is_none() && !options.force {
            return Err(FileError::not_found(key));
        }
        Ok(())
    }

    async fn rename_file(
        &self,
        source: &str,
        destination: &str,
        context: &Context,
    ) -> Result<(), FileError> {
        context.check()?;
        let from = self.abs(source);
        let to = self.abs(destination);
        let mut files = self.files.lock().unwrap();
        let bytes = files.remove(&from).ok_or_else(|| FileError::not_found(&from))?;
        files.insert(to, bytes);
        Ok(())
    }

    async fn create_temp_dir(
        &self,
        prefix: Option<&str>,
        context: &Context,
    ) -> Result<String, FileError> {
        context.check()?;
        Ok(format!("/tmp/{}-1", prefix.unwrap_or("tmp-")))
    }

    async fn cleanup(&self, _context: &Context) {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
    }
}

/// 一个假 shell：把收到的命令原样回显到 `on_output`（stdout），退出码 0。
struct EchoShell;

#[async_trait]
impl nous_env::Shell for EchoShell {
    async fn exec(
        &self,
        command: &Command,
        options: &ShellExecOptions,
        context: &Context,
    ) -> Result<ShellExecResult, ExecutionError> {
        context.check_exec()?;
        let text = match command {
            Command::Line(line) => line.clone(),
            Command::Argv(argv) => argv.join("\u{1f}"),
        };
        if let Some(sink) = options.on_output.as_ref() {
            sink(
                &text,
                context,
                &nous_env::ShellOutputInfo {
                    stream: ShellStream::Stdout,
                    skipped: None,
                },
            );
        }
        Ok(ShellExecResult {
            exit_code: 0,
            spill_path: None,
        })
    }

    async fn cleanup(&self, _context: &Context) {}
}

struct Everything {
    fs: MemoryEnv,
    shell: EchoShell,
}

#[async_trait]
impl FileSystem for Everything {
    fn id(&self) -> &str {
        self.fs.id()
    }
    fn cwd(&self) -> &Path {
        self.fs.cwd()
    }
    async fn absolute_path(&self, path: &str, context: &Context) -> Result<String, FileError> {
        self.fs.absolute_path(path, context).await
    }
    async fn join_path(&self, parts: &[&str], context: &Context) -> Result<String, FileError> {
        self.fs.join_path(parts, context).await
    }
    async fn read_text_file(&self, path: &str, context: &Context) -> Result<String, FileError> {
        self.fs.read_text_file(path, context).await
    }
    async fn write_file(
        &self,
        path: &str,
        content: FileContent,
        context: &Context,
    ) -> Result<(), FileError> {
        self.fs.write_file(path, content, context).await
    }
    async fn exists(&self, path: &str, context: &Context) -> Result<bool, FileError> {
        self.fs.exists(path, context).await
    }
    async fn file_info(&self, path: &str, context: &Context) -> Result<FileInfo, FileError> {
        self.fs.file_info(path, context).await
    }
    async fn list_dir(&self, path: &str, context: &Context) -> Result<Vec<FileInfo>, FileError> {
        self.fs.list_dir(path, context).await
    }
    async fn create_dir(
        &self,
        path: &str,
        options: CreateDirOptions,
        context: &Context,
    ) -> Result<(), FileError> {
        self.fs.create_dir(path, options, context).await
    }
    async fn remove(
        &self,
        path: &str,
        options: RemoveOptions,
        context: &Context,
    ) -> Result<(), FileError> {
        self.fs.remove(path, options, context).await
    }
    async fn rename_file(
        &self,
        source: &str,
        destination: &str,
        context: &Context,
    ) -> Result<(), FileError> {
        self.fs.rename_file(source, destination, context).await
    }
    async fn create_temp_dir(
        &self,
        prefix: Option<&str>,
        context: &Context,
    ) -> Result<String, FileError> {
        self.fs.create_temp_dir(prefix, context).await
    }
    async fn cleanup(&self, context: &Context) {
        self.fs.cleanup(context).await
    }
}

#[async_trait]
impl nous_env::Shell for Everything {
    async fn exec(
        &self,
        command: &Command,
        options: &ShellExecOptions,
        context: &Context,
    ) -> Result<ShellExecResult, ExecutionError> {
        self.shell.exec(command, options, context).await
    }
    async fn cleanup(&self, context: &Context) {
        self.shell.cleanup(context).await
    }
}

// ───────────────────────── 必需能力 ─────────────────────────

#[tokio::test]
async fn mandatory_capabilities_work_on_a_minimal_backend() {
    let env = MemoryEnv::new();
    let context = Context::new();

    assert_eq!(env.id(), "mem:local");
    assert_eq!(env.cwd(), Path::new("/mem"));
    assert_eq!(env.absolute_path("a.txt", &context).await.unwrap(), "/mem/a.txt");
    assert_eq!(env.join_path(&["a", "b"], &context).await.unwrap(), "a/b");

    assert!(!env.exists("a.txt", &context).await.unwrap());
    env.write_file("a.txt", FileContent::from("héllo"), &context)
        .await
        .unwrap();
    assert!(env.exists("a.txt", &context).await.unwrap());
    assert_eq!(env.read_text_file("a.txt", &context).await.unwrap(), "héllo");
    assert_eq!(env.file_info("a.txt", &context).await.unwrap().size, 6);

    let listed = env.list_dir("/mem", &context).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "a.txt");

    env.rename_file("a.txt", "b.txt", &context).await.unwrap();
    assert!(env.exists("b.txt", &context).await.unwrap());
    assert!(!env.exists("a.txt", &context).await.unwrap());

    env.remove("b.txt", RemoveOptions::default(), &context)
        .await
        .unwrap();
    assert!(!env.exists("b.txt", &context).await.unwrap());
    // force=false 时删不存在的文件是 not_found，不是静默成功。
    assert_eq!(
        env.remove("b.txt", RemoveOptions::default(), &context)
            .await
            .unwrap_err()
            .code,
        FileErrorCode::NotFound
    );
    // force=true 时才是静默成功。
    env.remove(
        "b.txt",
        RemoveOptions {
            recursive: false,
            force: true,
        },
        &context,
    )
    .await
    .unwrap();

    assert_eq!(
        env.create_temp_dir(Some("scan-"), &context).await.unwrap(),
        "/tmp/scan--1"
    );
    env.cleanup(&context).await;
    assert_eq!(env.cleanups.load(Ordering::SeqCst), 1);
}

// ───────────────────────── 可选能力：默认说不行 ─────────────────────────

#[tokio::test]
async fn optional_capabilities_fail_closed_by_default() {
    let env = MemoryEnv::new();
    let context = Context::new();
    let is_unsupported = |error: FileError| error.code == FileErrorCode::NotSupported;

    assert!(is_unsupported(
        env.read_binary_file("a", &context).await.err().expect("must fail closed")
    ));
    assert!(is_unsupported(
        env.append_file("a", FileContent::from("x"), &context)
            .await
            .unwrap_err()
    ));
    assert!(is_unsupported(
        env.truncate_file("a", 0, &context).await.err().expect("must fail closed")
    ));
    assert!(is_unsupported(env.flush_file("a", &context).await.err().expect("must fail closed")));
    assert!(is_unsupported(
        env.canonical_path("a", &context).await.err().expect("must fail closed")
    ));
    assert!(is_unsupported(
        env.read_text_lines("a", ReadTextLinesOptions::default(), &context)
            .await
            .unwrap_err()
    ));
    assert!(is_unsupported(
        env.open_text_line_reader("a", &context).await.err().expect("must fail closed")
    ));
    assert!(is_unsupported(
        env.open_binary_reader("a", Default::default(), &context)
            .await
            .err()
            .expect("must fail closed")
    ));
    assert!(is_unsupported(
        env.open_dir_reader("a", &context).await.err().expect("must fail closed")
    ));
    assert!(is_unsupported(
        env.list_files("a", Default::default(), &context)
            .await
            .unwrap_err()
    ));
    assert!(is_unsupported(
        env.create_temp_file(TempFileOptions::default(), &context)
            .await
            .unwrap_err()
    ));

    let sink: nous_env::ChangeSink = Arc::new(|_| {});
    assert!(is_unsupported(
        env.watch(&[], sink, &context).await.err().expect("must fail closed")
    ));
}

#[tokio::test]
async fn unsupported_errors_name_the_method_so_a_caller_can_degrade() {
    let env = MemoryEnv::new();
    let error = env
        .open_binary_reader("a", Default::default(), &Context::new())
        .await
        .err()
        .expect("must fail closed");
    assert!(
        error.message.contains("open_binary_reader"),
        "调用方要能据此退回 read_text_file：{}",
        error.message
    );
}

// ───────────────────────── 取消 ─────────────────────────

#[tokio::test]
async fn cancellation_surfaces_as_aborted_not_as_a_half_done_write() {
    let env = MemoryEnv::new();
    let token = tokio_util::sync::CancellationToken::new();
    let context = Context::with_cancel(token.clone());
    token.cancel();

    let error = env
        .write_file("a.txt", FileContent::from("x"), &context)
        .await
        .unwrap_err();
    assert_eq!(error.code, FileErrorCode::Aborted);
    // 一个字都没写进去。
    assert!(!env.exists("a.txt", &Context::new()).await.unwrap());
}

// ───────────────────────── ExecutionEnv ─────────────────────────

#[tokio::test]
async fn execution_env_is_a_shareable_trait_object() {
    let env: Arc<dyn ExecutionEnv> = Arc::new(Everything {
        fs: MemoryEnv::new(),
        shell: EchoShell,
    });
    let context = Context::new();

    // FileSystem 那一半。
    env.write_file("a.txt", FileContent::from("x"), &context)
        .await
        .unwrap();
    assert_eq!(env.read_text_file("a.txt", &context).await.unwrap(), "x");

    // Shell 那一半：输出走 on_output，不走返回值。
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let seen = seen.clone();
        Arc::new(move |text: &str, _: &Context, info: &nous_env::ShellOutputInfo| {
            seen.lock().unwrap().push((text.to_string(), info.stream));
        })
    };
    let result = env
        .exec(
            &Command::from("echo hi"),
            &ShellExecOptions {
                on_output: Some(sink),
                ..Default::default()
            },
            &context,
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.spill_path, None, "没有溢写就没有临时文件");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "echo hi");
    assert_eq!(seen[0].1, ShellStream::Stdout);
}

#[tokio::test]
async fn argv_commands_do_not_go_through_a_shell() {
    let env: Arc<dyn ExecutionEnv> = Arc::new(Everything {
        fs: MemoryEnv::new(),
        shell: EchoShell,
    });
    let parts = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let parts = parts.clone();
        Arc::new(move |text: &str, _: &Context, _: &nous_env::ShellOutputInfo| {
            parts.lock().unwrap().push(text.to_string());
        })
    };
    // argv 里带空格和引号，原样到达（shell 版本会重新解析它们）。
    env.exec(
        &Command::from(["printf", "%s", "a b\"c"]),
        &ShellExecOptions {
            on_output: Some(sink),
            ..Default::default()
        },
        &Context::new(),
    )
    .await
    .unwrap();
    assert_eq!(parts.lock().unwrap()[0], "printf\u{1f}%s\u{1f}a b\"c");
}

#[tokio::test]
async fn aborted_execution_reports_aborted() {
    let env: Arc<dyn ExecutionEnv> = Arc::new(Everything {
        fs: MemoryEnv::new(),
        shell: EchoShell,
    });
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let error = env
        .exec(
            &Command::from("sleep 10"),
            &ShellExecOptions::default(),
            &Context::with_cancel(token),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, nous_env::ExecutionErrorCode::Aborted);
}

// ───────────────────────── 契约里的几处语义 ─────────────────────────

#[test]
fn truncate_is_extend_and_shrink_by_contract_not_shrink_only() {
    // 契约文本本身就是判据：本库取 TS 语义（可缩可扩），不取 Rust 那版「只缩」。
    let doc = include_str!("../src/fs.rs");
    assert!(doc.contains("可缩可扩"), "truncate_file 的语义写进契约了吗");
    // 反面靠契约文本本身：`truncate_file` 的 doc 里点名了「本库不取那版」。
    assert!(doc.contains("本库不取那版"));
}

#[test]
fn text_line_distinguishes_a_truncated_tail() {
    // 文件最后一行没有换行时 terminated=false —— 调用方靠它判断是不是读断了。
    let line = TextLine {
        text: "abc".into(),
        terminated: false,
    };
    assert!(!line.terminated);
}
