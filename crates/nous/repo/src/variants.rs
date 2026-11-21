//! #76：技能 / 扩展变体（worktree + 分支 + 绑定表）。
//!
//! 绑定表只是本地账：TS 同形 `{v:1, variants:{family:{id:{root,branch}}}}`。
//! 变体是本仓自己的 worktree + 分支；主干（main/master/HEAD）永远不许动。
//! git 进程调用沿用 `crate::git`（唯一调用面）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{RepoError, Result};
use crate::git;
use crate::publish::Pushed;

const TABLE_VERSION: u32 = 1;
pub const FORBIDDEN_BRANCHES: &[&str] = &["main", "master", "HEAD"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantFamily {
    Skill,
    Extension,
}

impl VariantFamily {
    fn key(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Extension => "extension",
        }
    }

    fn branch_prefix(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Extension => "extension",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindVariant<'a> {
    pub id: &'a str,
    /// 要从主干工作区拷进变体的相对路径（技能 / 扩展单元目录）。
    /// 省略时拷 `id` 目录（不存在就只建空 worktree）。
    pub paths: &'a [&'a str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneVariant {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantBinding {
    pub root: PathBuf,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantTable {
    pub v: u32,
    pub variants: BTreeMap<String, BTreeMap<String, VariantBinding>>,
}

impl Default for VariantTable {
    fn default() -> Self {
        Self { v: TABLE_VERSION, variants: BTreeMap::new() }
    }
}

impl VariantTable {
    fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(path).map_err(|e| RepoError::Io(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| RepoError::Io(format!("绑定表损坏：{e}")))
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| RepoError::Io(e.to_string()))?;
        }
        let tmp = path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| RepoError::Io(e.to_string()))?;
        std::fs::write(&tmp, bytes).map_err(|e| RepoError::Io(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| RepoError::Io(e.to_string()))
    }

    fn get(&self, family: VariantFamily, id: &str) -> Result<&VariantBinding> {
        self.variants
            .get(family.key())
            .and_then(|rows| rows.get(id))
            .ok_or_else(|| RepoError::NotFound(format!("变体未绑定：{family_key}/{id}", family_key = family.key())))
    }
}

/// `bind` 的结果：worktree 根、分支名、绑定 id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundVariant {
    pub id: String,
    pub root: PathBuf,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishedVariant {
    pub commit: String,
    pub pushed: Pushed,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrunedVariant {
    pub unmerged: bool,
    pub remote_branch: bool,
}

/// 绑定 id 允许 slug：拒绝 `..` 与路径分隔符，防 worktree 逃到仓外。
fn check_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.starts_with('.')
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.trim() != id
    {
        return Err(RepoError::BadRequest(format!("变体 id 非法：{id:?}")));
    }
    Ok(())
}

fn branch_of(family: VariantFamily, id: &str) -> String {
    format!("{}/{}", family.branch_prefix(), id)
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    git::git_ok(root, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok()
}

fn local_branch(root: &Path) -> String {
    git::git_ok(root, &["symbolic-ref", "--short", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn checkout(root: &Path, branch: &str) -> Result<()> {
    if local_branch(root) == branch {
        return Ok(());
    }
    git::git_ok(root, &["checkout", branch]).map(|_| ())
}

/// 把未发布的工作区改动拷进变体 worktree，并在变体分支提交。
fn carry_unpublished(root: &Path, variant_root: &Path, paths: &[&str], id: &str) -> Result<()> {
    let mut copied = Vec::new();
    for rel in paths {
        let from = crate::files::safe_path(root, rel)?;
        let to = crate::files::safe_path(variant_root, rel)?;
        if !from.exists() {
            continue;
        }
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| RepoError::Io(e.to_string()))?;
            }
            std::fs::copy(&from, &to).map_err(|e| RepoError::Io(e.to_string()))?;
        }
        copied.push((*rel).to_string());
    }
    if copied.is_empty() {
        return Ok(());
    }
    let mut add = vec!["add", "--"];
    add.extend(copied.iter().map(String::as_str));
    git::git_ok(variant_root, &add)?;
    let staged = git::git_ok(variant_root, &["diff", "--cached", "--name-only"]).unwrap_or_default();
    if staged.trim().is_empty() {
        return Ok(());
    }
    let message = format!("bind: carry unpublished {id}");
    let mut commit = vec!["commit", "-qm", &message, "--"];
    commit.extend(copied.iter().map(String::as_str));
    git::git_ok(variant_root, &commit)?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| RepoError::Io(e.to_string()))?;
    for entry in std::fs::read_dir(from).map_err(|e| RepoError::Io(e.to_string()))? {
        let entry = entry.map_err(|e| RepoError::Io(e.to_string()))?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| RepoError::Io(e.to_string()))?;
        }
    }
    Ok(())
}

fn worktree_root(root: &Path, id: &str) -> PathBuf {
    let parent = root.parent().unwrap_or(root);
    parent.join(format!("{}-{id}", root.file_name().unwrap_or_default().to_string_lossy()))
}

pub fn variants(table_path: &Path) -> Result<serde_json::Value> {
    let table = VariantTable::load(table_path)?;
    serde_json::to_value(table).map_err(|e| RepoError::Io(e.to_string()))
}

pub fn bind(
    root: &Path,
    table_path: &Path,
    family: VariantFamily,
    input: BindVariant<'_>,
) -> Result<BoundVariant> {
    check_id(input.id)?;
    let id = input.id;
    let branch = branch_of(family, id);
    let target = worktree_root(root, id);
    let mut table = VariantTable::load(table_path)?;
    if table.variants.get(family.key()).is_some_and(|rows| rows.contains_key(id)) {
        return Err(RepoError::Conflict { message: format!("变体已绑定：{family_key}/{id}", family_key = family.key()), current: None, etag: None });
    }

    if target.exists() {
        // unbind 后树还在；重绑是「认领」同一棵 worktree，不是再建一份。
        checkout(&target, &branch)?;
    } else if branch_exists(root, &branch) {
        let path = target.to_string_lossy().into_owned();
        git::git_ok(root, &["worktree", "add", &path, &branch])?;
    } else {
        let path = target.to_string_lossy().into_owned();
        git::git_ok(root, &["worktree", "add", "-b", &branch, &path])?;
    }

    let fallback = [id];
    let paths = if input.paths.is_empty() { &fallback[..] } else { input.paths };
    carry_unpublished(root, &target, paths, id)?;

    table
        .variants
        .entry(family.key().to_string())
        .or_default()
        .insert(id.to_string(), VariantBinding { root: target.clone(), branch: branch.clone() });
    table.save(table_path)?;
    Ok(BoundVariant { id: id.to_string(), root: target, branch })
}

pub fn unbind(root: &Path, table_path: &Path, family: VariantFamily, id: &str) -> Result<()> {
    check_id(id)?;
    let mut table = VariantTable::load(table_path)?;
    let _ = table.get(family, id)?;
    table
        .variants
        .get_mut(family.key())
        .and_then(|rows| rows.remove(id))
        .ok_or_else(|| RepoError::NotFound(format!("变体未绑定：{id}")))?;
    table.save(table_path)?;
    let _ = root;
    Ok(())
}

pub fn switch_branch(
    root: &Path,
    table_path: &Path,
    family: VariantFamily,
    id: &str,
    branch: &str,
) -> Result<String> {
    check_id(id)?;
    if FORBIDDEN_BRANCHES.contains(&branch) || branch == local_branch(root) {
        return Err(RepoError::BadRequest(format!("变体分支不许是主干或 HEAD：{branch}")));
    }
    let table = VariantTable::load(table_path)?;
    let binding = table.get(family, id)?;
    if branch_exists(root, branch) {
        checkout(&binding.root, branch)?;
    } else {
        git::git_ok(&binding.root, &["checkout", "-b", branch])?;
    }
    let mut table = table;
    table
        .variants
        .get_mut(family.key())
        .and_then(|rows| rows.get_mut(id))
        .expect("binding 刚取到")
        .branch = branch.to_string();
    table.save(table_path)?;
    Ok(branch.to_string())
}

pub fn publish(
    root: &Path,
    table_path: &Path,
    family: VariantFamily,
    id: &str,
    message: &str,
    request_id: Option<&str>,
) -> Result<PublishedVariant> {
    check_id(id)?;
    let _ = root;
    let table = VariantTable::load(table_path)?;
    let binding = table.get(family, id)?;
    checkout(&binding.root, &binding.branch)?;
    let subject = match request_id {
        Some(r) if !r.is_empty() => format!("{message}\n\nRequest-Id: {r}"),
        _ => message.to_string(),
    };
    let dirty = git::git_ok(&binding.root, &["status", "--porcelain"]).unwrap_or_default();
    if dirty.trim().is_empty() {
        return Err(RepoError::BadRequest("变体没有待发布的改动".into()));
    }
    git::git_ok(&binding.root, &["add", "-A", "--"])?;
    git::git_ok(&binding.root, &["commit", "-qm", &subject])?;
    let hash = git::git_ok(&binding.root, &["rev-parse", "HEAD"])?.trim().to_string();
    // 变体分支没有 upstream，裸 push 会失败；显式推同名远端分支。
    let refspec = format!("HEAD:refs/heads/{}", binding.branch);
    let pushed = if git::has_push_target(&binding.root) {
        match git::git_ok(&binding.root, &["push", "--porcelain", "origin", &refspec]) {
            Ok(_) => Pushed::Yes,
            Err(_) => Pushed::No,
        }
    } else {
        Pushed::Unknown
    };
    Ok(PublishedVariant { commit: hash, pushed, branch: binding.branch.clone() })
}

pub fn prune(
    root: &Path,
    table_path: &Path,
    family: VariantFamily,
    id: &str,
    opts: PruneVariant,
) -> Result<PrunedVariant> {
    check_id(id)?;
    let mut table = VariantTable::load(table_path)?;
    let binding = table.get(family, id)?.clone();

    let unmerged = !git::git_ok(root, &["merge-base", "--is-ancestor", &binding.branch, "HEAD"])
        .is_ok();
    let remote_branch = git::git_ok(root, &["ls-remote", "--exit-code", "--heads", "origin", &binding.branch]).is_ok();
    if unmerged && !opts.force {
        return Err(RepoError::Conflict {
            message: format!("变体 {id} 有未合并的提交，删除要 force:true 二次确认"),
            current: None,
            etag: None,
        });
    }
    git::git_ok(root, &["worktree", "remove", "--force", binding.root.to_string_lossy().as_ref()])?;
    git::git_ok(root, &["branch", "-D", &binding.branch])?;
    if remote_branch {
        let _ = git::git_ok(root, &["push", "origin", "--delete", &binding.branch]);
    }
    table
        .variants
        .get_mut(family.key())
        .and_then(|rows| rows.remove(id));
    table.save(table_path)?;
    Ok(PrunedVariant { unmerged, remote_branch })
}

