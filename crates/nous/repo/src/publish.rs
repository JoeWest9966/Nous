//! 发布与归档三代：git add/commit/push，按 pathspec 防搭车。

use std::path::Path;

use serde::Serialize;

use crate::error::{RepoError, Result};
use crate::git;

/// 推送结果的 wire 三态：`true`（推上去了）/ `false`（推失败）/ `null`（没有远端）。
/// 默认序列化为 `true|false|null`，不是字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pushed {
    Yes,
    No,
    #[default]
    Unknown,
}

impl Serialize for Pushed {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Pushed::Yes => serializer.serialize_bool(true),
            Pushed::No => serializer.serialize_bool(false),
            Pushed::Unknown => serializer.serialize_none(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitResult {
    pub commit: String,
    pub pushed: Pushed,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitFile {
    pub path: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitDetail {
    pub hash: String,
    pub message: String,
    pub files: Vec<CommitFile>,
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
    pub pushed: Pushed,
}

/// 「写/删 + 提交 + 尽力推」的入参：一次只碰一条路径。
#[derive(Debug, Clone)]
pub struct WriteCommit<'a> {
    pub path: &'a str,
    /// `None` = 删除这条路径。
    pub content: Option<&'a [u8]>,
    /// Some = If-Match（内容指纹不符回 409）；None = 新建或硬写。
    pub base_etag: Option<&'a str>,
    pub message: &'a str,
    /// 幂等锚：同一个 `requestId` 只提交一次，重放回 `replay: true`。
    pub request_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct WriteCommitResult {
    /// 写后的内容指纹；删除时为 None。
    pub etag: Option<String>,
    /// 真的产生了提交才有；内容没变 / 重放时为 None。
    pub commit: Option<CommitResult>,
    /// 同一 `requestId` 之前已提交过：这次一个字节都没写。
    pub replay: bool,
}

/// `find_request_id` 只看最近这么多条提交（私库是内容仓，重试不会比这更旧）。
pub const REQUEST_ID_WINDOW: usize = 1000;

fn lines(out: &str) -> Vec<String> {
    out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect()
}

/// 内容闸：不同私库有自己的规则。失败时绝不 commit / push。
/// 返回 `RepoError::Validation { field, problem }`，HTTP 层直接映射为 400 + `{field, problem}`。
fn validate(root: &Path, paths: &[String]) -> Result<()> {
    let repo_kind = detect_kind(root);
    for path in paths {
        let full = root.join(path);
        if !full.is_file() { continue; }
        let content = std::fs::read_to_string(&full).map_err(|e| RepoError::Io(e.to_string()))?;
        let issue: Option<(String, String)> = match repo_kind.as_str() {
            "skills" if path == "sources.json" => {
                let value = serde_json::from_str::<serde_json::Value>(&content);
                match value {
                    Err(_) => Some(("sources.json".into(), "不是合法 JSON".into())),
                    Ok(v) if !v.get("sources").is_some_and(|s| s.is_array()) => {
                        Some(("sources".into(), "缺 sources 数组".into()))
                    }
                    Ok(v) => {
                        let bad: Vec<String> = v["sources"].as_array().unwrap()
                            .iter()
                            .enumerate()
                            .filter(|(_, s)| {
                                s.get("name").and_then(|n| n.as_str()).unwrap_or_default().is_empty()
                                    || s.get("url").and_then(|u| u.as_str()).unwrap_or_default().is_empty()
                            })
                            .map(|(i, _)| format!("sources[{i}]"))
                            .collect();
                        if bad.is_empty() { None }
                        else { Some(("sources".into(), format!("缺 name/url：{}", bad.join(", ")))) }
                    }
                }
            }
            "skills" if path.ends_with("SKILL.md") => {
                if !(content.starts_with("---\n") && content[4..].contains("\n---")) {
                    Some(("SKILL.md".into(), "缺少 frontmatter".into()))
                } else {
                    let fm = content[4..].split("\n---").next().unwrap_or("");
                    let missing: Vec<&str> = ["name", "description"].into_iter()
                        .filter(|field| !fm.lines().any(|l| l.trim_start().starts_with(&format!("{field}:"))))
                        .collect();
                    if missing.is_empty() { None }
                    else { Some(("SKILL.md".into(), format!("frontmatter 缺字段：{}", missing.join(", ")))) }
                }
            }
            "prompts" if path.ends_with(".md") => {
                if content.trim().is_empty() { Some(("body".into(), "正文为空".into())) } else { None }
            }
            "prompts" if path.ends_with("nous.json") => {
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Err(_) => Some(("nous.json".into(), "不是合法 JSON".into())),
                    Ok(v) => {
                        let missing: Vec<&str> = ["name", "description", "prompt"]
                            .into_iter().filter(|k| v.get(k).is_none()).collect();
                        if missing.is_empty() { None }
                        else { Some(("nous.json".into(), format!("缺字段：{}", missing.join(", ")))) }
                    }
                }
            }
            "presets" if path.ends_with(".json") => {
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Err(_) => Some(("spec".into(), "不是合法 JSON".into())),
                    Ok(v) => {
                        if v.get("v").is_none() { Some(("spec.v".into(), "缺字段".into())) }
                        else {
                            let allowed = ["v","name","summary","description","category","trigger","params","session","enabled"];
                            let bad: Vec<String> = v.as_object().unwrap().keys()
                                .filter(|k| !allowed.contains(&k.as_str())).cloned().collect();
                            if bad.is_empty() { None }
                            else { Some(("spec".into(), format!("不认识的键：{}", bad.join(", ")))) }
                        }
                    }
                }
            }
            "triggers" if path.ends_with("nous.json") => trigger_manifest_issue(&full),
            "triggers" if path.ends_with("Cargo.toml") => None,
            "triggers" if path.ends_with("src/main.rs") => None,
            "extend" if path.ends_with("nous.json") => {
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Err(_) => Some(("nous.json".into(), "不是合法 JSON".into())),
                    Ok(v) => {
                        let declared = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        let dir_name = full.parent().and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if declared != dir_name {
                            Some(("name".into(), format!("name 必须等于目录名 {dir_name}")))
                        } else {
                            let entry = v.get("entry").and_then(|e| e.as_str()).unwrap_or("index.js");
                            if full.parent().unwrap().join(entry).is_file() { None }
                            else { Some(("entry".into(), format!("入口文件不存在：{entry}"))) }
                        }
                    }
                }
            }
            _ => None,
        };
        if let Some((field, problem)) = issue {
            return Err(RepoError::Validation { field, problem });
        }
    }
    Ok(())
}

/// 触发器清单的内容闸：字段齐全、类型合法、目录名对上、骨架三件在。
///
/// 只校验 `nous.json` 这一份声明；`Cargo.toml` / `src/main.rs` 的存在性一并查，
/// 因为「清单写好了但项目编不了」在触发器页上就是一条坏卡。
pub fn validate_manifest(dir: &Path) -> Option<String> {
    trigger_manifest_issue(&dir.join("nous.json"))
        .map(|(field, problem)| format!("{field}：{problem}"))
}

fn trigger_manifest_issue(full: &Path) -> Option<(String, String)> {
    const TYPES: [&str; 6] = ["epoch", "number", "string", "text", "boolean", "select"];
    let text = std::fs::read_to_string(full).ok()?;
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return Some(("nous.json".into(), format!("不是合法 JSON：{error}"))),
    };
    let object = match value.as_object() {
        Some(object) => object,
        None => return Some(("nous.json".into(), "顶层必须是对象".into())),
    };
    let dir = full.parent()?.file_name()?.to_string_lossy().into_owned();
    for field in ["name", "version", "summary"] {
        if object.get(field).and_then(|v| v.as_str()).is_none_or(str::is_empty) {
            return Some((field.into(), format!("缺字段或为空：{field}")));
        }
    }
    if object.get("name").and_then(|v| v.as_str()) != Some(dir.as_str()) {
        return Some(("name".into(), format!("name 必须等于目录名 {dir}")));
    }
    if !object.get("category").is_none_or(|v| v.as_array().is_some()) {
        return Some(("category".into(), "必须是字符串数组".into()));
    }
    let params = match object.get("params") {
        None => &Vec::new(),
        Some(serde_json::Value::Array(params)) => params,
        Some(_) => return Some(("params".into(), "必须是数组".into())),
    };
    for (i, param) in params.iter().enumerate() {
        let kind = param.get("type").and_then(|v| v.as_str()).unwrap_or_default();
        if param.get("name").and_then(|v| v.as_str()).is_none_or(str::is_empty) {
            return Some((format!("params[{i}].name"), "缺字段".into()));
        }
        if !TYPES.contains(&kind) {
            return Some((format!("params[{i}].type"), format!("不认识：{kind}")));
        }
        if kind == "select"
            && !param.get("options").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty())
        {
            return Some((format!("params[{i}].options"), "select 必须给非空 options".into()));
        }
    }
    let project = full.parent()?;
    if !project.join("Cargo.toml").is_file() {
        return Some(("Cargo.toml".into(), "缺 Cargo.toml".into()));
    }
    if !project.join("src/main.rs").is_file() {
        return Some(("src/main.rs".into(), "缺 src/main.rs".into()));
    }
    None
}

fn detect_kind(root: &Path) -> String {
    let name = root.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    for kind in ["presets", "skills", "prompts", "extend", "trigger"] {
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

pub fn commit_detail(root: &Path, hash: &str) -> Result<CommitDetail> {
    let files = commit_files(root, hash)?;
    let message = git::git_ok(root, &["show", "-s", "--pretty=format:%s", hash])
        .unwrap_or_default().trim().to_string();
    Ok(CommitDetail { hash: hash.to_string(), message, files })
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

fn commit_and_push(root: &Path, pathspecs: &[&str], message: &str, request_id: Option<&str>) -> Result<LifecycleResult> {
    // git mv / rm 已把变更放进 index；commit 仍按 pathspec 约束，避免无关 staged 文件搭车。
    // 注意：pathspec 里只能放「index 里有记录」的路径，否则 git 直接报未匹配。
    let subject = match request_id {
        Some(id) if !id.is_empty() => format!("{message}\n\nRequest-Id: {id}"),
        _ => message.to_string(),
    };
    let mut args = vec!["commit", "-m", &subject, "--"];
    args.extend_from_slice(pathspecs);
    git::git_ok(root, &args)?;
    let hash = git::git_ok(root, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok(LifecycleResult { commit: hash, pushed: git::git_push(root)? })
}

/// index 里有没有这个路径（未发布过的新单元 = 没有）。
fn is_tracked(root: &Path, path: &str) -> Result<bool> {
    Ok(!git::git_ok(root, &["ls-files", "--", path])?.trim().is_empty())
}

pub fn archive(root: &Path, name: &str, request_id: Option<&str>) -> Result<LifecycleResult> {
    let safe = crate::files::safe_path(root, name)?;
    if !safe.is_dir() || safe == root {
        return Err(RepoError::NotFound(format!("要归档的单元不存在：{name}")));
    }
    let archive_root = root.join(".archive");
    std::fs::create_dir_all(&archive_root).map_err(|e| RepoError::Io(e.to_string()))?;
    let target = archive_root.join(name);
    if target.exists() { return Err(RepoError::Conflict { message: format!("已归档过：{name}"), current: None, etag: None }); }
    let rel = format!(".archive/{name}");
    let message = format!("archive: {name}");
    if is_tracked(root, name)? {
        // 已跟踪：mv 会同时 stage「删除旧路径 + 新增 .archive 路径」，两条都在 index 里。
        git::git_ok(root, &["mv", name, &rel])?;
        commit_and_push(root, &[name, &rel], &message, request_id)
    } else {
        // 建完没发布的项目在 index 里一条记录都没有：`git mv` 报「源目录为空」，
        // 而且 pathspec 放 `name` 也会报未匹配 —— 直接改名 + 只提交新路径。
        std::fs::rename(&safe, &target).map_err(|e| RepoError::Io(format!("改名失败：{e}")))?;
        git::git_ok(root, &["add", "-A", "--", &rel])?;
        commit_and_push(root, &[&rel], &message, request_id)
    }
}

pub fn restore(root: &Path, name: &str, request_id: Option<&str>) -> Result<LifecycleResult> {
    let archived = format!(".archive/{name}");
    if !root.join(&archived).is_dir() {
        return Err(RepoError::NotFound(format!("没有归档单元：{name}")));
    }
    if root.join(name).exists() {
        return Err(RepoError::Conflict { message: format!("目标已存在：{name}"), current: None, etag: None });
    }
    let message = format!("restore: {name}");
    if is_tracked(root, &archived)? {
        git::git_ok(root, &["mv", &archived, name])?;
        commit_and_push(root, &[name, &archived], &message, request_id)
    } else {
        std::fs::rename(root.join(&archived), root.join(name))
            .map_err(|e| RepoError::Io(format!("改名失败：{e}")))?;
        git::git_ok(root, &["add", "-A", "--", name])?;
        commit_and_push(root, &[name], &message, request_id)
    }
}

pub fn purge(root: &Path, name: &str, request_id: Option<&str>) -> Result<LifecycleResult> {
    let archived = format!(".archive/{name}");
    if !root.join(&archived).is_dir() {
        return Err(RepoError::NotFound(format!("没有归档单元：{name}")));
    }
    let message = format!("purge: {name}");
    if is_tracked(root, &archived)? {
        git::git_ok(root, &["rm", "-r", "-q", "--", &archived])?;
        commit_and_push(root, &[&archived], &message, request_id)
    } else {
        // 归档时没进过 index 的目录：盘上删掉就行，没有提交可做。
        std::fs::remove_dir_all(root.join(&archived)).map_err(|e| RepoError::Io(format!("删除失败：{e}")))?;
        let hash = git::git_ok(root, &["rev-parse", "HEAD"])?.trim().to_string();
        Ok(LifecycleResult { commit: hash, pushed: git::git_push(root)? })
    }
}

/// 最近 [`REQUEST_ID_WINDOW`] 条提交里找 `Request-Id: <id>` trailer，命中给那条提交的 hash。
///
/// 幂等回执只认这一处：`commit` 写 trailer，`write_and_commit` 重放前查它。
pub fn find_request_id(root: &Path, request_id: &str) -> Result<Option<String>> {
    if request_id.is_empty() {
        return Ok(None);
    }
    let window = REQUEST_ID_WINDOW.to_string();
    // `%B` 是多行正文：用 0x1e 作记录分隔、0x1f 作字段分隔，不能靠换行切。
    //
    // **空仓也是合法状态**：`git log` 在「还没有任何提交」时退出码 128
    // （`your current branch 'master' does not have any commits yet`）。
    // 那不是「回执查不到」，是「根本没有历史」—— 按 `None` 处理，让第一次写能成功。
    // 踩过：用空私库验元素库的新建，PUT 直接回「git log 失败」（Desk 只看到一句红字）。
    let out = match git::git_ok(root, &["log", "--format=%H%x1f%B%x1e", "-n", &window]) {
        Ok(out) => out,
        Err(_) if !has_commits(root)? => return Ok(None),
        Err(e) => return Err(e),
    };
    let trailer = format!("Request-Id: {request_id}");
    for record in out.split('\u{1e}') {
        let Some((hash, body)) = record.split_once('\u{1f}') else { continue };
        let hash = hash.trim();
        if hash.is_empty() {
            continue;
        }
        if body.lines().any(|line| line.trim() == trailer) {
            return Ok(Some(hash.to_string()));
        }
    }
    Ok(None)
}

/// 这个仓有没有提交（空仓的 `git log` 会失败，那是状态不是错误）。
fn has_commits(root: &Path) -> Result<bool> {
    match git::git_ok(root, &["rev-parse", "--verify", "-q", "HEAD"]) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// 写 / 删一条路径，再提交它（写一次提交一次）；同一个 `requestId` 只做一次。
pub fn write_and_commit(root: &Path, input: WriteCommit<'_>) -> Result<WriteCommitResult> {
    // 幂等：这条 requestId 已经提交过 = 上次的写入就是结果，重放一个字节都不碰。
    if let Some(request_id) = input.request_id.filter(|id| !id.is_empty()) {
        if find_request_id(root, request_id)?.is_some() {
            return Ok(WriteCommitResult { etag: None, commit: None, replay: true });
        }
    }

    let path = input.path.to_string();
    // 动手之前先记下原来那份：提交失败时要用它把盘上和索引还原。
    // （提交会失败：pre-commit 钩子、磁盘满、身份没配 —— 都不是「半成功」的理由。）
    let previous = crate::files::read(root, input.path).ok().map(|read| read.content);
    let (etag, changed) = match input.content {
        Some(content) => {
            // 内容与盘上逐字节相同 = 没有改动，不制造空提交（回执里 commit 为 null）。
            let unchanged = crate::files::read(root, input.path)
                .is_ok_and(|old| old.content == content);
            let written = crate::files::write(root, crate::files::WriteFile {
                path: input.path,
                content,
                base_etag: input.base_etag,
            })?;
            (Some(written.etag), !unchanged)
        }
        None => {
            crate::files::remove(root, input.path, input.base_etag)?;
            (None, true)
        }
    };

    if !changed {
        return Ok(WriteCommitResult { etag, commit: None, replay: false });
    }
    // 工作区里其实没变化（未跟踪被 ignore / 与 HEAD 一致）时 `git commit` 会失败；
    // 先问一句 status，别把「没有实际改动」报成 500。
    let dirty = git::git_ok(root, &["status", "--porcelain", "--", &path])?;
    if dirty.trim().is_empty() {
        return Ok(WriteCommitResult { etag, commit: None, replay: false });
    }

    let commit = match commit(root, &[path.clone()], input.message, input.request_id) {
        Ok(commit) => commit,
        Err(error) => {
            // **提交失败 = 这次什么也没发生**：把内容和索引都还原回去，再把错误交出去。
            // 不还原的话，调用方收到的是错误、盘上却已经是新的那份 —— 锁最怕这个：
            // 盘上写着 `locked: true`，`git log` 里找不到谁锁的，`push` 也永远带不上去。
            restore_after_failed_commit(root, &path, previous.as_deref());
            return Err(error);
        }
    };
    Ok(WriteCommitResult { etag, commit: Some(commit), replay: false })
}

/// 提交失败之后把一条路径还原成「动手之前」的样子：索引退回，内容写回 / 删掉。
///
/// 尽力而为：**它自己的失败不盖住原来那个错误** —— 用户要看的是「为什么没提交成功」。
fn restore_after_failed_commit(root: &Path, path: &str, previous: Option<&[u8]>) {
    // 先把索引退回（`git add` 过的那一份），否则下次提交会捎上这次的半成品。
    let _ = git::git_ok(root, &["reset", "-q", "--", path]);
    let full = root.join(path);
    match previous {
        Some(bytes) => {
            if let Some(parent) = full.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&full, bytes);
        }
        None => {
            let _ = std::fs::remove_file(&full);
        }
    }
}
