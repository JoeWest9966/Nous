//! pending / status / diff 的读视图。counts 从 git 现算，不在内存留第二份事实。

use std::path::Path;

use serde::Serialize;

use crate::error::Result;
use crate::git;

#[derive(Debug, Clone, Serialize)]
pub struct RepoStatus {
    pub root: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub dirty: bool,
    pub locked: bool,
    /// `origin` 的 URL；没配远端给 `None`（本地单用户也是合法用法）。
    /// 与 `PresetsAdmin::repo_view` 同口径 —— 客户端靠它决定「在浏览器打开」这个按钮画不画。
    pub remote: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUnit {
    pub name: String,
    pub state: String,
    pub ahead_commits: usize,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diff {
    pub before: String,
    pub after: String,
}


/// 已 commit 未 push 的条数。bare remote / 未设 upstream 时回退到 origin 的远端跟踪引用。
fn ahead_count(root: &Path) -> usize {
    for range in ["@{upstream}..HEAD", "HEAD..refs/remotes/origin/main", "HEAD..refs/remotes/origin/master"] {
        if let Ok(out) = git::git_ok(root, &["rev-list", "--count", range]) {
            if let Ok(n) = out.trim().parse() { return n; }
        }
    }
    0
}

fn unpushed_names(root: &Path) -> String {
    for range in ["@{upstream}..HEAD", "refs/remotes/origin/main..HEAD", "refs/remotes/origin/master..HEAD"] {
        if let Ok(out) = git::git_ok(root, &["log", "--name-only", "--pretty=format:", range]) {
            if !out.trim().is_empty() { return out; }
        }
    }
    String::new()
}

fn unit_of(path: &str) -> String {
    path.split('/').next().unwrap_or(path).to_string()
}

pub fn status(root: &Path) -> Result<RepoStatus> {
    let head = git::git_ok(root, &["rev-parse", "HEAD"]).ok().map(|s| s.trim().to_string());
    let branch = git::git_ok(root, &["symbolic-ref", "--short", "HEAD"]).ok().map(|s| s.trim().to_string());
    let ahead = ahead_count(root);
    let behind = git::git_ok(root, &["rev-list", "--count", "HEAD..@{upstream}"])
        .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let dirty = git::git_ok(root, &["status", "--porcelain"]).map(|s| !s.trim().is_empty()).unwrap_or(false);
    // 读不到 remote 不是错误：没配 origin 和「git 命令炸了」在这里都是「没有远端」。
    let remote = remote_url(root).ok().flatten();
    Ok(RepoStatus {
        root: root.to_string_lossy().into_owned(),
        head, branch, ahead, behind, dirty, locked: false, remote,
    })
}

/// tracked 未提交 / 未跟踪 / 已提交没推（`aheadCommits`）三种状态，按单元（顶层目录）分组。
pub fn pending(root: &Path) -> Result<Vec<PendingUnit>> {
    let porcelain = git::git_ok(root, &["status", "--porcelain"])?;
    let ahead = ahead_count(root);
    let mut units: std::collections::BTreeMap<String, PendingUnit> = Default::default();
    for line in porcelain.lines() {
        let path = line.get(3..).unwrap_or("");
        let path = path.split(" -> ").last().unwrap_or(path);
        if path.is_empty() { continue; }
        let name = unit_of(path);
        let state = if line.starts_with("??") { "untracked" } else { "tracked" };
        let entry = units.entry(name.clone()).or_insert_with(|| PendingUnit {
            name: name.clone(), state: state.to_string(), ahead_commits: 0, paths: Vec::new(),
        });
        if entry.state == "untracked" && state == "tracked" { entry.state = state.to_string(); }
        entry.paths.push(path.to_string());
    }
    if ahead > 0 {
        let names = unpushed_names(root);
        let mut seen = std::collections::BTreeSet::new();
        for line in names.lines() {
            let name = unit_of(line);
            if name.is_empty() { continue; }
            seen.insert(name.clone());
            let entry = units.entry(name.clone()).or_insert_with(|| PendingUnit {
                name: name.clone(), state: "unpushed".into(), ahead_commits: 0, paths: Vec::new(),
            });
            entry.state = "unpushed".into();
            entry.ahead_commits = ahead;
            entry.paths.push(line.to_string());
        }
        if seen.is_empty() {
            units.insert("<repo>".into(), PendingUnit {
                name: "<repo>".into(), state: "unpushed".into(), ahead_commits: ahead, paths: Vec::new(),
            });
        }
    }
    let mut out: Vec<_> = units.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// HEAD 里带这个前缀的路径。前缀按**段内口径**给（`<段>/<名>`），
/// 名册用它算 `tracked`：盘上有、仓里没 → 未发布。
pub fn tracked_paths(root: &Path, prefix: &str) -> Result<Vec<String>> {
    let prefix = prefix.trim().trim_matches('/');
    if prefix.is_empty() || prefix.contains("..") {
        return Err(crate::error::RepoError::BadRequest(format!("前缀非法：{prefix}")));
    }
    let needle = format!("{prefix}/");
    let out = git::git_ok(root, &["ls-files", "--", prefix])?;
    Ok(out
        .lines()
        .filter(|line| *line == prefix || line.starts_with(&needle))
        .map(str::to_string)
        .collect())
}

/// `origin` 的 URL；没配远端给 `None`（本地单用户也是合法用法）。
pub fn remote_url(root: &Path) -> Result<Option<String>> {
    let out = git::git_ok(root, &["remote", "get-url", "origin"])?;
    let url = out.trim();
    Ok((!url.is_empty()).then(|| url.to_string()))
}

pub fn pending_diff(root: &Path, path: &str) -> Result<Diff> {
    let safe = crate::files::safe_path(root, path)?;
    let before = git::git_ok(root, &[&format!("HEAD:{path}")]).unwrap_or_default();
    let after = std::fs::read_to_string(safe).unwrap_or_default();
    Ok(Diff { before, after })
}

pub fn history_diff(root: &Path, name: &str, hash: &str, path: &str) -> Result<Diff> {
    for part in [name, path] {
        let part = part.trim().trim_matches('/');
        if part.is_empty() || part.contains("..") || part.starts_with('/') {
            return Err(crate::RepoError::BadRequest("history/diff 的 name/path 非法".into()));
        }
    }
    if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(crate::RepoError::BadRequest("hash 非法".into()));
    }
    let full = format!("{name}/{path}");
    let before_ref = format!("{hash}^:{full}");
    let after_ref = format!("{hash}:{full}");
    let before = git::git_ok(root, &["cat-file", "-p", &before_ref]).unwrap_or_default();
    let after = git::git_ok(root, &["cat-file", "-p", &after_ref]).unwrap_or_default();
    Ok(Diff { before, after })
}

/// 归档区里的一个单元。**只有名字**：`.archive/` 里没有清单、没有版本 ——
/// 想知道它是什么，恢复回来再读。多编的字段就是又一份会说谎的账。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedUnit {
    pub name: String,
}

/// `GET /v1/repos/{repo}/archive` 的投影：`.archive/` 下的目录名，按名字排序。
///
/// 点开头的跳过（`.git` 之类混进来过一次），不是目录的也跳过 —— 归档物一定是目录，
/// 恢复端点要的是 `git mv .archive/<name> <name>`，散落文件恢复不了。
pub fn archived(root: &Path) -> Result<Vec<ArchivedUnit>> {
    let dir = root.join(".archive");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|error| crate::RepoError::Io(error.to_string()))? {
        let entry = entry.map_err(|error| crate::RepoError::Io(error.to_string()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name() else { continue };
        let name = name.to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        out.push(ArchivedUnit { name });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
mod archived_tests {
    use super::*;

    fn 临时仓() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repo-archived-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".archive").join("b")).expect("mkdir b");
        std::fs::create_dir_all(dir.join(".archive").join("a")).expect("mkdir a");
        std::fs::create_dir_all(dir.join(".archive").join(".inner")).expect("mkdir .inner");
        std::fs::write(dir.join(".archive").join("散件.md"), "x").expect("write");
        dir
    }

    #[test]
    fn 只列目录_按名字排序_点开头的跳过() {
        let dir = 临时仓();
        let names: Vec<String> = archived(&dir).expect("archived").into_iter().map(|u| u.name).collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 没有归档区就是空表不是错误() {
        let dir = std::env::temp_dir().join(format!("repo-noarchive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(archived(&dir).expect("archived").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
