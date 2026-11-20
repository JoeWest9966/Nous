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
}

#[derive(Debug, Clone, Serialize)]
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
    Ok(RepoStatus {
        root: root.to_string_lossy().into_owned(),
        head, branch, ahead, behind, dirty, locked: false,
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

pub fn pending_diff(root: &Path, path: &str) -> Result<Diff> {
    let before = git::git_ok(root, &[&format!("HEAD:{path}")]).unwrap_or_default();
    let after = std::fs::read_to_string(root.join(path)).unwrap_or_default();
    Ok(Diff { before, after })
}

pub fn history_diff(root: &Path, name: &str, hash: &str, path: &str) -> Result<Diff> {
    let full = format!("{name}/{path}");
    let before = git::git_ok(root, &[&format!("{hash}^:{full}")]).unwrap_or_default();
    let after = git::git_ok(root, &[&format!("{hash}:{full}")]).unwrap_or_default();
    Ok(Diff { before, after })
}
