//! 发布与归档三代：git add/commit/push，按 pathspec 防搭车。

use std::path::Path;

use serde::Serialize;

use crate::error::{RepoError, Result};
use crate::git;

#[derive(Debug, Clone, Serialize)]
pub struct CommitResult {
    pub commit: String,
    pub pushed: bool,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitFile {
    pub path: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryItem {
    pub hash: String,
    pub at: String,
    pub actor: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleResult {
    pub commit: String,
    pub pushed: bool,
}

fn lines(out: &str) -> Vec<String> {
    out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect()
}

/// 内容闸：不同私库有自己的规则。失败时绝不 commit / push。
fn validate(root: &Path, paths: &[String]) -> Result<()> {
    let repo_kind = detect_kind(root);
    for path in paths {
        let full = root.join(path);
        if !full.is_file() { continue; }
        let content = std::fs::read_to_string(&full).map_err(|e| RepoError::Io(e.to_string()))?;
        let issue = match repo_kind.as_str() {
            "skills" if path.ends_with("SKILL.md") => {
                if !(content.starts_with("---\n") && content[4..].contains("\n---")) {
                    Some(("SKILL.md", "缺少 frontmatter"))
                } else {
                    let fm = content[4..].split("\n---").next().unwrap_or("");
                    let mut missing = Vec::new();
                    for field in ["name", "description"] {
                        if !fm.lines().any(|l| l.trim_start().starts_with(&format!("{field}:"))) {
                            missing.push(field);
                        }
                    }
                    if missing.is_empty() { None }
                    else { Some(("SKILL.md", &*Box::leak(format!("frontmatter 缺字段：{}", missing.join(", ")).into_boxed_str()))) }
                }
            }
            "prompts" if path.ends_with(".md") => {
                if content.trim().is_empty() { Some(("body", "正文为空")) } else { None }
            }
            "prompts" if path.ends_with("nous.json") => {
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Err(_) => Some(("nous.json", "不是合法 JSON")),
                    Ok(v) => {
                        let missing: Vec<&str> = ["name", "description", "prompt"]
                            .into_iter().filter(|k| v.get(k).is_none()).collect();
                        if missing.is_empty() { None }
                        else { Some(("nous.json", &*Box::leak(format!("缺字段：{}", missing.join(", ")).into_boxed_str()))) }
                    }
                }
            }
            "presets" if path.ends_with(".json") => {
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Err(_) => Some(("spec", "不是合法 JSON")),
                    Ok(v) => {
                        if v.get("v").is_none() { Some(("spec.v", "缺字段")) }
                        else {
                            let allowed = ["v","name","summary","description","category","trigger","params","session","enabled"];
                            let bad: Vec<String> = v.as_object().unwrap().keys()
                                .filter(|k| !allowed.contains(&k.as_str())).cloned().collect();
                            if bad.is_empty() { None }
                            else { Some(("spec", &*Box::leak(format!("不认识的键：{}", bad.join(", ")).into_boxed_str()))) }
                        }
                    }
                }
            }
            "extend" if path.ends_with("nous.json") => {
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Err(_) => Some(("nous.json", "不是合法 JSON")),
                    Ok(v) => {
                        let declared = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        let dir_name = full.parent().and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if declared != dir_name {
                            Some(("name", &*Box::leak(format!("name 必须等于目录名 {dir_name}").into_boxed_str())))
                        } else {
                            let entry = v.get("entry").and_then(|e| e.as_str()).unwrap_or("index.js");
                            if full.parent().unwrap().join(entry).is_file() { None }
                            else { Some(("entry", &*Box::leak(format!("入口文件不存在：{entry}").into_boxed_str()))) }
                        }
                    }
                }
            }
            _ => None,
        };
        if let Some((field, problem)) = issue {
            return Err(RepoError::BadRequest(format!("内容闸未过：{path} / {field} / {problem}")));
        }
    }
    Ok(())
}

fn detect_kind(root: &Path) -> String {
    let name = root.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    for kind in ["presets", "skills", "prompts", "extend"] {
        if name.contains(kind) { return kind.to_string(); }
    }
    String::new()
}

pub fn commit(root: &Path, paths: &[String], message: &str, request_id: Option<&str>) -> Result<CommitResult> {
    if paths.is_empty() { return Err(RepoError::BadRequest("paths 不能为空".into())); }
    validate(root, paths)?;

    // pathspec 防搭车：`--` 之后只列请求的路径，绝不 `git add .`。
    let mut add_args = vec!["add", "--"];
    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    add_args.extend(path_refs.iter().copied());
    git::git_ok(root, &add_args)?;

    let subject = match request_id {
        Some(id) if !id.is_empty() => format!("{message}\n\nRequest-Id: {id}"),
        _ => message.to_string(),
    };
    let mut commit_args = vec!["commit", "-m", &subject, "--"];
    commit_args.extend(path_refs.iter().copied());
    git::git_ok(root, &commit_args)?;

    let hash = git::git_ok(root, &["rev-parse", "HEAD"])?;
    let hash = hash.trim().to_string();
    let pushed = git::git_push(root)?;
    let refs = lines(&git::git_ok(root, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])?);
    Ok(CommitResult { commit: hash, pushed, refs })
}

pub fn commit_files(root: &Path, hash: &str) -> Result<Vec<CommitFile>> {
    if !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(RepoError::BadRequest("hash 非法".into()));
    }
    let names = git::git_ok(root, &["show", "--pretty=format:", "--name-only", hash])?;
    let mut files = Vec::new();
    for path in lines(&names) {
        let parent = format!("{hash}^:{path}");
        let blob = format!("{hash}:{path}");
        let before = git::git_ok(root, &[&parent]).unwrap_or_default();
        let after = git::git_ok(root, &["cat-file", "-p", &blob]).unwrap_or_default();
        files.push(CommitFile { path, before, after });
    }
    Ok(files)
}

pub fn history(root: &Path, name: &str, limit: usize) -> Result<Vec<HistoryItem>> {
    let name = name.trim().trim_matches('/');
    if name.is_empty() || name.contains("..") || name.starts_with('/') {
        return Err(RepoError::BadRequest("name 非法".into()));
    }
    let format = "%H%x1f%aI%x1f%an%x1f%s";
    let out = git::git_ok(root, &["log", &format!("-{limit}"), &format!("--pretty=format:{format}"), "--", name])?;
    Ok(out.lines().filter_map(|line| {
        let parts: Vec<&str> = line.split('\x1f').collect();
        if parts.len() != 4 { return None; }
        Some(HistoryItem { hash: parts[0].into(), at: parts[1].into(), actor: parts[2].into(), message: parts[3].into() })
    }).collect())
}

fn commit_and_push(root: &Path, pathspec: &str, message: &str) -> Result<LifecycleResult> {
    // git mv 已经把目标放进 index；commit 只按 pathspec，不再 add（避免搭车）。
    let _ = pathspec; // 只提交 index 中已经 staged 的精确变化（git mv / rm 已写好 index）
    git::git_ok(root, &["commit", "-m", message])?;
    let hash = git::git_ok(root, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok(LifecycleResult { commit: hash, pushed: git::git_push(root)? })
}

pub fn archive(root: &Path, name: &str, _request_id: Option<&str>) -> Result<LifecycleResult> {
    let safe = crate::files::safe_path(root, name)?;
    if !safe.is_dir() || safe == root {
        return Err(RepoError::NotFound(format!("要归档的单元不存在：{name}")));
    }
    let archive_root = root.join(".archive");
    std::fs::create_dir_all(&archive_root).map_err(|e| RepoError::Io(e.to_string()))?;
    let target = archive_root.join(name);
    if target.exists() { return Err(RepoError::Conflict { message: format!("已归档过：{name}"), current: None, etag: None }); }
    let rel = format!(".archive/{name}");
    git::git_ok(root, &["mv", name, &rel])?;
    commit_and_push(root, ".archive", &format!("archive: {name}"))
}

pub fn restore(root: &Path, name: &str, _request_id: Option<&str>) -> Result<LifecycleResult> {
    let archived = format!(".archive/{name}");
    if !root.join(&archived).is_dir() {
        return Err(RepoError::NotFound(format!("没有归档单元：{name}")));
    }
    if root.join(name).exists() {
        return Err(RepoError::Conflict { message: format!("目标已存在：{name}"), current: None, etag: None });
    }
    git::git_ok(root, &["mv", &archived, name])?;
    commit_and_push(root, name, &format!("restore: {name}"))
}

pub fn purge(root: &Path, name: &str, _request_id: Option<&str>) -> Result<LifecycleResult> {
    let archived = format!(".archive/{name}");
    if !root.join(&archived).is_dir() {
        return Err(RepoError::NotFound(format!("没有归档单元：{name}")));
    }
    git::git_ok(root, &["rm", "-r", "-q", "--", &archived])?;
    git::git_ok(root, &["commit", "-m", &format!("purge: {name}")])?;
    let hash = git::git_ok(root, &["rev-parse", "HEAD"])?;
    let hash = hash.trim().to_string();
    Ok(LifecycleResult { commit: hash, pushed: git::git_push(root)? })
}
