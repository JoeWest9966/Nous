//! #77 接缝 A · 上游追踪的公共 API：sources / mirror / check / pin / sync / tree / file / copy / adopt。
//!
//! 测试用两个本地临时 git 仓模拟「上游」与「我们」；唯一的远端动作是显式的
//! `refresh` / `check` / `sync`，打开概览端点只认已存在的镜像缓存，不联网。

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::error::{RepoError, Result};
use crate::files::safe_path;
use crate::git;

const CHECK_FILE: &str = ".git/nous-check.json";
const SOURCES_FILE: &str = "sources.json";

/// 一条订阅。`version` 是「认的 commit」，只由 pin / sync（及首拉基线）写。
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<SourceVersion>,
}

/// 人认的版本：commit 是主键，at 是钉住时刻（ISO-8601）。
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SourceVersion {
    pub commit: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
struct SourcesFile {
    v: u32,
    sources: Vec<Source>,
}

/// 概览的一行：三个版本（远端 / 镜像 / 认的）分开摆，不合并成一个。
#[derive(Debug, Clone, Serialize)]
pub struct UpstreamStatus {
    #[serde(flatten)]
    pub source: Source,
    pub mirror: &'static str,
    pub mirror_head: Option<String>,
    #[serde(rename = "pinned")]
    pub pinned_commit: Option<String>,
    pub newer: bool,
    pub drift: bool,
    pub bytes: u64,
    pub entries: usize,
    pub check: Option<CheckCache>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct CheckCache {
    pub head: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TreeItem {
    pub path: String,
    pub sha: String,
    pub mode: String,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpstreamFile {
    pub path: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComparedFile {
    pub path: String,
    pub state: &'static str,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdoptResult {
    pub name: String,
    pub files: Vec<String>,
    pub foot: Option<String>,
}

pub struct Upstream {
    /// 技能私库根：`sources.json` 住在这里，是内容，不进内核库。
    root: PathBuf,
    /// `data/upstream/<name>/` 镜像根：缓存，删了下次 refresh 重建。
    data: PathBuf,
}

fn clean_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 32
        || !name.as_bytes()[0].is_ascii_lowercase()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(RepoError::BadRequest(format!("上游名只认小写字母、数字、点、下划线、连字符，≤32 字：{name}")));
    }
    Ok(name)
}

fn clean_git_arg(label: &str, value: Option<&str>) -> Result<Option<String>> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => {
            if !v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b'+' | b'@' | b':' | b'~'))
                || v.contains("..")
            {
                return Err(RepoError::BadRequest(format!("{label} 不合法：{v}")));
            }
            Ok(Some(v.to_string()))
        }
    }
}

fn clean_rel(label: &str, value: &str) -> Result<String> {
    let value = value.trim().trim_matches('/');
    if value.is_empty() {
        return Ok(String::new());
    }
    for part in value.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(RepoError::BadRequest(format!("{label} 只能是仓内相对路径：{value}")));
        }
    }
    Ok(value.to_string())
}

fn validate_source(raw: &Source) -> Result<Source> {
    let name = clean_name(&raw.name)?.to_string();
    let url = raw.url.trim().to_string();
    if url.is_empty() || url.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(RepoError::BadRequest(format!("{name}: url 不能空，也不能有空白")));
    }
    let local = url.starts_with('/');
    if !local
        && !["https://", "http://", "ssh://", "git://", "file://"]
            .iter()
            .any(|p| url.starts_with(p))
        && !url.contains('@')
    {
        return Err(RepoError::BadRequest(format!("{name}: 认不出这个地址：{url}")));
    }
    let branch = clean_git_arg("branch", raw.branch.as_deref())?;
    let prefix = clean_git_arg("prefix", raw.prefix.as_deref())?;
    let version = match &raw.version {
        None => None,
        Some(v) => {
            let commit = clean_git_arg("version.commit", Some(&v.commit))?.unwrap_or_default();
            if !(7..=40).contains(&commit.len()) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(RepoError::BadRequest(format!("{name}: version 要是一个 7-40 位 commit sha")));
            }
            Some(SourceVersion { commit, at: v.at.clone() })
        }
    };
    Ok(Source { name, url, branch, prefix, version })
}

impl Upstream {
    pub fn open(root: impl AsRef<Path>, data: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize().map_err(|e| RepoError::InvalidPath(e.to_string()))?;
        let data = data.as_ref().canonicalize().map_err(|e| RepoError::InvalidPath(e.to_string()))?;
        if !root.is_dir() || !data.is_dir() {
            return Err(RepoError::InvalidPath("私库根与 data 根都必须是目录".into()));
        }
        Ok(Self { root, data })
    }

    /// `sources.json` 不存在 = 没订阅任何人（不是错误）；读坏 = 400，不装作空清单。
    pub fn sources(&self) -> Result<Vec<Source>> {
        let path = self.root.join(SOURCES_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(RepoError::Io(e.to_string())),
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| RepoError::BadRequest(format!("sources.json 不是合法 JSON：{e}")))?;
        let list = value.get("sources").and_then(|v| v.as_array()).ok_or_else(|| {
            RepoError::BadRequest("sources.json 缺 sources 数组".into())
        })?;
        let mut sources = Vec::new();
        for item in list {
            let raw: Source = serde_json::from_value(item.clone())
                .map_err(|e| RepoError::BadRequest(format!("sources.json 的一条订阅不合法：{e}")))?;
            sources.push(validate_source(&raw)?);
        }
        Self::check_duplicates(&sources)?;
        Ok(sources)
    }

    /// 读整份 `sources.json`（`{v, sources}`）——GET 端点原样吐出，不吞 `v`。
    pub fn sources_doc(&self) -> Result<serde_json::Value> {
        let sources = self.sources()?;
        Ok(serde_json::json!({ "v": 1u32, "sources": sources }))
    }

    /// 整份替换。写盘不提交：加一个上游也是内容，得走 `/commit` 才进历史。
    pub fn write_sources(&self, sources: &[Source]) -> Result<Vec<Source>> {
        let sources: Vec<Source> = sources.iter().map(validate_source).collect::<Result<_>>()?;
        Self::check_duplicates(&sources)?;
        let path = safe_path(&self.root, SOURCES_FILE)?;
        let body = serde_json::to_string_pretty(&SourcesFile { v: 1, sources: sources.clone() })
            .map_err(|e| RepoError::Io(e.to_string()))?;
        let tmp = self.root.join(format!(".sources-tmp-{}", std::process::id()));
        fs::write(&tmp, format!("{body}\n")).map_err(|e| RepoError::Io(e.to_string()))?;
        fs::rename(&tmp, &path).map_err(|e| RepoError::Io(e.to_string()))?;
        Ok(sources)
    }

    fn check_duplicates(sources: &[Source]) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        for source in sources {
            if !seen.insert(source.name.as_str()) {
                return Err(RepoError::BadRequest(format!("sources.json 里 {} 出现了两次", source.name)));
            }
        }
        Ok(())
    }

    fn source(&self, name: &str) -> Result<Source> {
        let name = clean_name(name)?;
        self.sources()?
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| RepoError::BadRequest(format!("没有订阅「{name}」这个上游")))
    }

    fn mirror(&self, name: &str) -> Result<PathBuf> {
        let root = self.data.join("upstream").join(clean_name(name)?);
        if root.join(".git").is_dir() {
            Ok(root)
        } else {
            Err(RepoError::BadRequest(format!("{name}: 还没拉过镜像 —— 先 refresh")))
        }
    }

    /// 只拉已有镜像；没有就 clone 一份带 sparse checkout 的工作树。不改 `version`。
    pub fn refresh(&self, name: &str) -> Result<String> {
        let source = self.source(name)?;
        let dir = self.data.join("upstream").join(&source.name);
        if !dir.join(".git").is_dir() {
            if dir.exists() {
                fs::remove_dir_all(&dir).map_err(|e| RepoError::Io(e.to_string()))?;
            }
            fs::create_dir_all(dir.parent().unwrap()).map_err(|e| RepoError::Io(e.to_string()))?;
            git::git_ok(&self.data, &["clone", "--quiet", "--depth=1", "--no-checkout", &source.url, dir.to_str().unwrap()])?;
        }
        let branch = {
            let candidates = [source.branch.as_deref(), Some("main"), Some("master")];
            let mut found = None;
            for candidate in candidates.into_iter().flatten() {
                if git::git_ok(&dir, &["show-ref", "--verify", &format!("refs/remotes/origin/{candidate}")]).is_ok() {
                    found = Some(candidate.to_string());
                    break;
                }
            }
            found.ok_or_else(|| RepoError::BadRequest(format!("{}: 镜像里没有可用分支", source.name)))?
        };
        git::git_ok(&dir, &["fetch", "--quiet", "--prune", "origin", "+refs/heads/*:refs/remotes/origin/*"])?;
        git::git_ok(&dir, &["checkout", "--quiet", "--force", &branch])?;
        git::git_ok(&dir, &["reset", "--hard", "--quiet", &format!("origin/{branch}")])?;
        self.sparse(&dir, &source)?;
        // 首拉顺手记基线：这是唯一一次非 pin 写 version。
        if source.version.is_none() {
            let head = self.head(&dir, &source)?;
            self.write_version(&source.name, &head)?;
        }
        self.head(&dir, &source)
    }

    fn sparse(&self, dir: &Path, source: &Source) -> Result<()> {
        let head = self.head(dir, source)?;
        let out = git::git_ok(dir, &["ls-tree", "-r", "-z", &head])?;
        let mut homes = std::collections::BTreeSet::new();
        for row in out.split('\0').filter(|r| !r.is_empty()) {
            let path = row.split_once('\t').map(|(_, p)| p).unwrap_or(row);
            if !path.ends_with("/SKILL.md") {
                continue;
            }
            let home = &path[..path.len() - "/SKILL.md".len()];
            if home.is_empty() || home.split('/').any(|p| p.starts_with('.')) {
                continue;
            }
            if let Some(prefix) = &source.prefix {
                if home != prefix && !home.starts_with(&format!("{prefix}/")) {
                    continue;
                }
            }
            homes.insert(home.to_string());
        }
        let mut args = vec!["sparse-checkout", "set", "--no-cone"];
        let patterns: Vec<String> = if homes.is_empty() {
            vec!["/.nous-no-skills/".into()]
        } else {
            homes.iter().map(|h| format!("/{h}/")).collect()
        };
        args.extend(patterns.iter().map(String::as_str));
        git::git_ok(dir, &args)?;
        Ok(())
    }

    fn head(&self, dir: &Path, source: &Source) -> Result<String> {
        let mut refs = vec!["HEAD".to_string()];
        if let Some(branch) = &source.branch {
            refs.insert(0, format!("refs/heads/{branch}"));
        }
        for reference in refs {
            if let Ok(out) = git::git_ok(dir, &["rev-parse", &format!("{reference}^{{commit}}")]) {
                let head = out.trim().to_string();
                if head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Ok(head);
                }
            }
        }
        Err(RepoError::BadRequest(format!("{}: 找不到镜像 HEAD", source.name)))
    }

    fn write_version(&self, name: &str, commit: &str) -> Result<String> {
        let mut sources = self.sources()?;
        let target = sources.iter_mut().find(|s| s.name == name).ok_or_else(|| {
            RepoError::BadRequest(format!("没有订阅「{name}」这个上游"))
        })?;
        target.version = Some(SourceVersion {
            commit: commit.to_string(),
            at: chrono_like_now(),
        });
        self.write_sources(&sources)?;
        Ok(commit.to_string())
    }

    /// `pin` 是日常唯一写 `version` 的入口（首拉基线除外）。
    pub fn pin(&self, name: &str, commit: Option<&str>) -> Result<String> {
        let source = self.source(name)?;
        let commit = match clean_git_arg("commit", commit)? {
            Some(commit) => commit.to_lowercase(),
            None => self.head(&self.mirror(name)?, &source)?,
        };
        if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(RepoError::BadRequest("pin 的 commit 必须是 40 位 sha".into()));
        }
        self.write_version(name, &commit)
    }

    /// `check` 只问 refs 并写缓存，不下载任何对象、不改镜像文件。
    pub fn check(&self, name: Option<&str>) -> Result<Vec<CheckCache>> {
        let sources = match name {
            Some(name) => vec![self.source(name)?],
            None => self.sources()?,
        };
        let mut out = Vec::new();
        for source in sources {
            let dir = self.mirror(&source.name)?;
            let raw = git::git_ok(&self.root, &["ls-remote", &source.url])?;
            let head = remote_head(&raw, source.branch.as_deref()).ok_or_else(|| {
                RepoError::BadRequest(format!("{}: 远端一个 ref 都没读出来", source.name))
            })?;
            let cache = CheckCache { head, at: chrono_like_now() };
            let path = dir.join(CHECK_FILE);
            fs::write(&path, serde_json::to_string_pretty(&cache).unwrap_or_default())
                .map_err(|e| RepoError::Io(e.to_string()))?;
            out.push(cache);
        }
        Ok(out)
    }

    fn check_cache(&self, name: &str) -> Option<CheckCache> {
        let dir = self.data.join("upstream").join(name).join(CHECK_FILE);
        fs::read_to_string(dir).ok().and_then(|s| serde_json::from_str(&s).ok())
    }

    /// `sync` = refresh + pin 到远端 HEAD（一步；唯一与 pin 同样写 version 的入口）。
    pub fn sync(&self, name: &str) -> Result<String> {
        self.refresh(name)?;
        self.pin(name, None)
    }

    /// 概览：远端 HEAD 来自 check 缓存；镜像 HEAD 现算；bytes / entries 只在镜像里现算。
    /// 单条订阅的树坏了不该让整个面板白屏 —— 用 `status_with_errors` 把它记进 errors。
    pub fn status(&self) -> Result<Vec<UpstreamStatus>> {
        let (rows, errors) = self.status_with_errors();
        if let Some(err) = errors.first() {
            return Err(RepoError::BadRequest(format!(
                "{}: {}",
                err["source"].as_str().unwrap_or("*"),
                err["error"].as_str().unwrap_or_default(),
            )));
        }
        Ok(rows)
    }

    /// 逐条采集概览；每条订阅独立兜底，坏的那条进 `errors`，其余照常显示。
    /// 打开面板只读盘，不联网。
    pub fn status_with_errors(&self) -> (Vec<UpstreamStatus>, Vec<serde_json::Value>) {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        let sources = match self.sources() {
            Ok(s) => s,
            Err(e) => {
                errors.push(serde_json::json!({ "source": "*", "error": e.to_string() }));
                return (out, errors);
            }
        };
        for source in sources {
            let dir = self.data.join("upstream").join(&source.name);
            let ready = dir.join(".git").is_dir();
            let head = if ready { self.head(&dir, &source).ok() } else { None };
            let pinned = source.version.as_ref().map(|v| v.commit.as_str());
            let check = if ready { self.check_cache(&source.name) } else { None };
            let (bytes, entries) = if ready {
                (
                    dir_size(&dir),
                    match self.tree(&source.name, "") {
                        Ok(t) => t.len(),
                        Err(e) => {
                            errors.push(serde_json::json!({ "source": source.name, "error": e.to_string() }));
                            0
                        }
                    },
                )
            } else {
                (0, 0)
            };
            // drift 是「镜像里看到的已经不是我们认的那一版」：HEAD 动过，或工作区有手改。
            let dirty = if ready {
                git::git_ok(&dir, &["status", "--porcelain"])
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            } else {
                false
            };
            out.push(UpstreamStatus {
                mirror: if ready { "ready" } else { "missing" },
                mirror_head: head.clone(),
                pinned_commit: pinned.map(str::to_string),
                newer: check.as_ref().is_some_and(|c| pinned.is_some_and(|p| c.head != p)),
                drift: head.as_ref().is_some_and(|h| pinned.is_some_and(|p| h != p)) || dirty,
                bytes,
                entries,
                check,
                source,
            });
        }
        (out, errors)
    }

    fn tree(&self, name: &str, path: &str) -> Result<Vec<TreeItem>> {
        let source = self.source(name)?;
        let dir = self.mirror(name)?;
        let head = self.head(&dir, &source)?;
        let path = clean_rel("path", path)?;
        let spec = if path.is_empty() { head.clone() } else { format!("{head}:{path}") };
        let out = git::git_ok(&dir, &["ls-tree", "-r", "-l", &spec])?;
        let mut items = Vec::new();
        for row in out.lines().filter(|l| !l.trim().is_empty()) {
            let (left, name_part) = match row.split_once('\t') {
                Some(v) => v,
                None => continue,
            };
            let fields: Vec<&str> = left.split_whitespace().collect();
            if fields.len() < 4 {
                continue;
            }
            items.push(TreeItem {
                mode: fields[0].into(),
                path: format!("{}{}", if path.is_empty() { String::new() } else { format!("{path}/") }, name_part),
                sha: fields[2].into(),
                size: fields[3].parse().unwrap_or(0),
            });
        }
        items.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(items)
    }

    pub fn tree_items(&self, name: &str, path: &str) -> Result<Vec<TreeItem>> {
        self.tree(name, path)
    }

    pub fn file(&self, name: &str, path: &str) -> Result<UpstreamFile> {
        let source = self.source(name)?;
        let dir = self.mirror(name)?;
        let head = self.head(&dir, &source)?;
        let path = clean_rel("path", path)?;
        if path.is_empty() {
            return Err(RepoError::BadRequest("file 的 path 不能空".into()));
        }
        let text = git::git_ok(&dir, &["show", &format!("{head}:{path}")])?;
        Ok(UpstreamFile { path, text })
    }

    /// 本地正文与上游 blob sha 逐个比：`same` / `diff` / `missing`。
    pub fn compare(&self, name: &str, path: &str) -> Result<Vec<ComparedFile>> {
        let source = self.source(name)?;
        let home = clean_rel("path", path)?;
        if home.is_empty() {
            return Err(RepoError::BadRequest("compare 的 path 不能空".into()));
        }
        let adopted = self.find_adopted(&source.name, &home);
        let local_dir = adopted.unwrap_or_else(|| PathBuf::from(Path::new(&home).file_name().unwrap_or_default()));
        let mut out = Vec::new();
        for item in self.tree(&source.name, &home)? {
            let rel = item.path.strip_prefix(&format!("{home}/")).unwrap_or(&item.path);
            let local = self.root.join(&local_dir).join(rel);
            let state = if !local.is_file() {
                "missing"
            } else {
                let rel_to_root = local.strip_prefix(&self.root).unwrap_or(&local);
                let local_sha = git::git_ok(
                    &self.root,
                    &["hash-object", "--no-filters", "--", local_to_str(rel_to_root)],
                )
                .unwrap_or_default();
                if local_sha.trim() == item.sha { "same" } else { "diff" }
            };
            out.push(ComparedFile { path: rel.to_string(), state, size: item.size });
        }
        Ok(out)
    }

    fn find_adopted(&self, source: &str, home: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(&self.root).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            let foot = fs::read_to_string(path.join("nous.json")).ok().and_then(|text| {
                serde_json::from_str::<serde_json::Value>(&text).ok()
            }).and_then(|v| v.get("upstream").and_then(|s| s.as_str()).map(str::to_string));
            if foot.as_deref().is_some_and(|f| f.starts_with(&format!("{source}/{home}@"))) {
                return Some(path);
            }
        }
        None
    }

    /// `adopt` 写 foot 标记（`copy` 只写 `copiedFrom`，不写 foot）。
    pub fn adopt(&self, name: &str, path: &str, to: Option<&str>, foot: bool) -> Result<AdoptResult> {
        let source = self.source(name)?;
        let home = clean_rel("path", path)?;
        if home.is_empty() {
            return Err(RepoError::BadRequest("path 不能空".into()));
        }
        let target_name = match to {
            Some(to) => clean_name(to)?.to_string(),
            None => Path::new(&home).file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
        };
        if target_name.is_empty() {
            return Err(RepoError::BadRequest("目标技能名不能空".into()));
        }
        let target = self.root.join(&target_name);
        if target.exists() {
            return Err(RepoError::Conflict {
                message: format!("{target_name}/ 已经在了 —— 换个名字或先归档本地这条"),
                current: None,
                etag: None,
            });
        }
        let items = self.tree(&source.name, &home)?;
        let commit = self.head(&self.mirror(&source.name)?, &source)?;
        let skill_sha = git::git_ok(&self.mirror(name)?, &["rev-parse", &format!("{commit}:{home}")])
            .unwrap_or_else(|_| commit.clone());
        let skill_sha = skill_sha.trim().to_string();
        fs::create_dir_all(&target).map_err(|e| RepoError::Io(e.to_string()))?;
        let mut written = Vec::new();
        for item in items {
            let rel = item.path.strip_prefix(&format!("{home}/")).unwrap_or(&item.path);
            let rel_path = Path::new(rel);
            if rel_path.components().any(|c| !matches!(c, Component::Normal(_))) {
                return Err(RepoError::BadRequest(format!("上游路径不合法：{rel}")));
            }
            let dir = self.mirror(name)?;
            let content = git::git_bytes(&dir, &["cat-file", "blob", &item.sha])?;
            let out = target.join(rel);
            fs::create_dir_all(out.parent().unwrap()).map_err(|e| RepoError::Io(e.to_string()))?;
            fs::write(&out, content).map_err(|e| RepoError::Io(e.to_string()))?;
            written.push(rel.to_string());
        }
        let mark = format!("{}/{}@{}", source.name, home, skill_sha);
        let mut manifest = serde_json::Map::new();
        manifest.insert("name".into(), serde_json::Value::String(target_name.clone()));
        if foot {
            manifest.insert("upstream".into(), serde_json::Value::String(mark.clone()));
        } else {
            manifest.insert("copiedFrom".into(), serde_json::Value::String(format!("{}/{}", source.name, home)));
        }
        fs::write(
            target.join("nous.json"),
            format!("{}\n", serde_json::to_string_pretty(&serde_json::Value::Object(manifest)).unwrap_or_default()),
        )
        .map_err(|e| RepoError::Io(e.to_string()))?;
        Ok(AdoptResult {
            name: target_name,
            files: written,
            foot: foot.then_some(mark),
        })
    }
}

fn remote_head(raw: &str, branch: Option<&str>) -> Option<String> {
    let mut fallback = None;
    let mut symbolic_head = None;
    let mut branch_head = None;
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let Some(sha) = parts.next() else { continue };
        let reference = match parts.next() {
            Some(r) => r,
            None => continue,
        };
        if reference == "HEAD" {
            symbolic_head = Some(sha.to_string());
        }
        if branch.is_some_and(|b| reference == format!("refs/heads/{b}")) {
            branch_head = Some(sha.to_string());
        }
        if fallback.is_none() && reference.starts_with("refs/heads/") {
            fallback = Some(sha.to_string());
        }
    }
    branch_head.or(symbolic_head).or(fallback)
}

fn local_to_str(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                } else if meta.is_dir() {
                    total += dir_size(&path);
                }
            }
        }
    }
    total
}

/// 只给 version.at 用：进程内不需要时钟抽象（platform 层允许），但格式保持 ISO-8601。
fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // 不引 chrono：用 civil-from-days 把秒折成 UTC 日历时间，够写一个人能读的时刻。
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem / 60) % 60, rem % 60)
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + i64::from(m <= 2), m, d)
}
