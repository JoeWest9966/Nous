//! #75：四套私库的读写与发布（platform 层，唯一的 git 进程调用面）。
//!
//! 本 crate 自己是「能力实现」，没有 IO 替身：测试用临时真 git 仓 + 本地 bare remote
//! 作为替身；`nous-env` 的四个 `ContentPath` 只提供配置，不在这里读环境变量。

#![forbid(unsafe_code)]

mod error;
mod files;
mod git;
mod publish;
mod upstream;
mod variants;
mod views;

pub use error::{RepoError, Result};
pub use git::materialize_head_dir;
pub use files::{sha256, WriteFile, WrittenFile, MAX_FILE_BYTES};
pub use publish::{
    validate_manifest, CommitDetail, CommitFile, CommitResult, LifecycleResult, Pushed, WriteCommit,
    WriteCommitResult,
};
pub use upstream::{AdoptResult, CheckCache, ComparedFile, Source, SourceVersion, TreeItem, Upstream, UpstreamFile, UpstreamStatus};
pub use variants::{
    variants, BindVariant, BoundVariant, PruneVariant, PrunedVariant, PublishedVariant, VariantFamily,
};
pub use views::{ArchivedUnit, Diff, PendingUnit, RepoStatus};

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

    /// 同一个 `requestId` 提交过没有？有就给那条提交的 hash（幂等回执的判据）。
    ///
    /// 只看最近 [`publish::REQUEST_ID_WINDOW`] 条提交 —— 私库是内容仓，不是审计库；
    /// 一个重试进来的 `requestId` 不会比这更旧。
    pub fn find_request_id(&self, request_id: &str) -> Result<Option<String>> {
        publish::find_request_id(&self.root, request_id)
    }

    /// 「写/删 + 提交 + 尽力推」一次做完：一个 `requestId` 一个事务。
    ///
    /// 这是「写一次提交一次」的原子形态：同一个 `requestId` 重放不会二次提交；
    /// 内容与盘上一致时不制造空提交（回 `commit: None`），也不假装写了什么。
    pub fn write_and_commit(&self, input: WriteCommit<'_>) -> Result<WriteCommitResult> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        publish::write_and_commit(&self.root, input)
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

    /// HEAD 里有没有这条（前缀 = `<段>/<名>` 或 `<段>/<名>/`）。
    ///
    /// 名册的 `tracked` 判据：盘上有、仓里没 → `false`，页面画「未发布」。
    pub fn tracked_paths(&self, prefix: &str) -> Result<Vec<String>> {
        views::tracked_paths(&self.root, prefix)
    }

    /// `origin` 的 URL；没配远端给 `None`（本地单用户也是合法用法）。
    pub fn remote_url(&self) -> Result<Option<String>> {
        views::remote_url(&self.root)
    }

    pub fn pending_diff(&self, path: &str) -> Result<Diff> {
        views::pending_diff(&self.root, path)
    }

    pub fn pending_commit(&self, hash: &str) -> Result<CommitDetail> {
        publish::commit_detail(&self.root, hash)
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

    /// 归档区（`.archive/`）里的单元名。写端点早就有（archive / restore / purge），
    /// 没有读就等于「归档了就再也看不见」—— 客户端只能猜。
    pub fn archived(&self) -> Result<Vec<ArchivedUnit>> {
        views::archived(&self.root)
    }

    /// 异步写者共用的锁 future：同一仓排队，不等价于返回 429（票面允许排队）。
    pub fn write_permit(&self) -> Arc<Mutex<()>> {
        self.write_lock.clone()
    }

    // ── #76：变体（worktree + 分支 + 绑定表） ──────────────────────────────
    // 绑定表路径由调用方（server）按私库种类注入，repo 不猜数据目录。

    pub fn variants(&self, table_path: &std::path::Path) -> Result<serde_json::Value> {
        variants::variants(table_path)
    }

    pub fn bind(
        &self,
        family: VariantFamily,
        table_path: &std::path::Path,
        input: BindVariant<'_>,
    ) -> Result<BoundVariant> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        variants::bind(&self.root, table_path, family, input)
    }

    pub fn unbind(
        &self,
        family: VariantFamily,
        table_path: &std::path::Path,
        id: &str,
    ) -> Result<()> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        variants::unbind(&self.root, table_path, family, id)
    }

    pub fn switch_branch(
        &self,
        family: VariantFamily,
        table_path: &std::path::Path,
        id: &str,
        branch: &str,
    ) -> Result<String> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        variants::switch_branch(&self.root, table_path, family, id, branch)
    }

    pub fn publish(
        &self,
        family: VariantFamily,
        table_path: &std::path::Path,
        id: &str,
        message: &str,
        request_id: Option<&str>,
    ) -> Result<PublishedVariant> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        variants::publish(&self.root, table_path, family, id, message, request_id)
    }

    pub fn prune(
        &self,
        family: VariantFamily,
        table_path: &std::path::Path,
        id: &str,
        opts: PruneVariant,
    ) -> Result<PrunedVariant> {
        let _guard = self.write_lock.lock().map_err(|_| RepoError::LockPoisoned)?;
        variants::prune(&self.root, table_path, family, id, opts)
    }
}
