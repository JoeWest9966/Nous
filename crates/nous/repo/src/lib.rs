//! #75：四套私库的读写与发布（platform 层，唯一的 git 进程调用面）。
//!
//! 本 crate 自己是「能力实现」，没有 IO 替身：测试用临时真 git 仓 + 本地 bare remote
//! 作为替身；`nous-env` 的四个 `ContentPath` 只提供配置，不在这里读环境变量。

#![forbid(unsafe_code)]

mod error;
mod files;
mod git;
mod publish;
mod views;

pub use error::{RepoError, Result};
pub use files::{WriteFile, WrittenFile};
pub use publish::{CommitResult, CommitFile, LifecycleResult, Pushed};
pub use views::{Diff, PendingUnit, RepoStatus};

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// 一个私库工作树。所有写操作经同一把仓根锁串行；不同 `Repo` 实例互不阻塞。
pub struct Repo {
    root: std::path::PathBuf,
    write_lock: Arc<Mutex<()>>,
    /// 归档操作的第二把锁（与写锁不交叉，统一由 `with_write` 取锁）。
    _archive_lock: Arc<Mutex<()>>,
    _locks: Arc<Mutex<HashMap<std::path::PathBuf, Arc<Mutex<()>>>>>,
}

impl Repo {
    /// 打开一个已经存在的工作树；不建仓、不建目录。
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize().map_err(|e| {
            RepoError::InvalidPath(format!("仓根不可访问：{}（{e}）", root.as_ref().display()))
        })?;
        if !root.is_dir() {
            return Err(RepoError::InvalidPath(format!("仓根不是目录：{}", root.display())));
        }
        Ok(Self {
            root,
            write_lock: Arc::new(Mutex::new(())),
            _archive_lock: Arc::new(Mutex::new(())),
            _locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 读取正文。etag 是文件 blob 的 sha256（与 git blob sha 区分，见 `files.rs`）。
    pub fn read_file(&self, path: &str) -> Result<WrittenFile> {
        files::read(&self.root, path)
    }

    pub fn write_file(&self, input: WriteFile<'_>) -> Result<WrittenFile> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        files::write(&self.root, input)
    }

    pub fn delete_file(&self, path: &str, base_etag: Option<&str>) -> Result<()> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        files::remove(&self.root, path, base_etag)
    }

    pub fn list_dir(&self, path: &str) -> Result<Vec<files::DirEntry>> {
        files::list(&self.root, path)
    }

    pub fn create_dir(&self, path: &str) -> Result<()> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        files::create_dir(&self.root, path)
    }

    pub fn commit(&self, paths: &[String], message: &str, request_id: Option<&str>) -> Result<CommitResult> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        publish::commit(&self.root, paths, message, request_id)
    }

    /// 把已 commit 未 push 的提交推上去（``sync``）。无远端返回 `Pushed::Unknown`。
    pub fn sync(&self) -> Result<Pushed> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        git::git_push(&self.root)
    }

    pub fn status(&self) -> Result<RepoStatus> {
        views::status(&self.root)
    }

    pub fn pending(&self) -> Result<Vec<PendingUnit>> {
        views::pending(&self.root)
    }

    pub fn pending_diff(&self, path: &str) -> Result<Diff> {
        views::pending_diff(&self.root, path)
    }

    pub fn pending_commit(&self, hash: &str) -> Result<Vec<CommitFile>> {
        publish::commit_files(&self.root, hash)
    }

    pub fn history(&self, name: &str, limit: usize) -> Result<Vec<publish::HistoryItem>> {
        publish::history(&self.root, name, limit)
    }

    pub fn history_diff(&self, name: &str, hash: &str, path: &str) -> Result<Diff> {
        views::history_diff(&self.root, name, hash, path)
    }

    pub fn archive(&self, name: &str, request_id: Option<&str>) -> Result<publish::LifecycleResult> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        publish::archive(&self.root, name, request_id)
    }

    pub fn restore(&self, name: &str, request_id: Option<&str>) -> Result<publish::LifecycleResult> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        publish::restore(&self.root, name, request_id)
    }

    pub fn purge(&self, name: &str, request_id: Option<&str>) -> Result<publish::LifecycleResult> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        publish::purge(&self.root, name, request_id)
    }

    /// 异步写者共用的锁 future：同一仓排队，不等价于返回 429（票面允许排队）。
    pub fn write_permit(&self) -> Arc<Mutex<()>> {
        self.write_lock.clone()
    }
}
