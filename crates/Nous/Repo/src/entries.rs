//! 资源仓管理面的纯逻辑：名字 / 路径校验、递归清单、骨架落盘、发布。
//!
//! 三个私库（技能 / 模板 / 扩展）共用**同一套文件形状**：仓根下一条目一个目录，
//! 目录里是若干文本文件。`/v1/{skills,prompts,extensions}/{files,file,create,publish}`
//! 那一组端点的 HTTP 翻译在各自的插件里，**盘面规矩只写在这里** —— 抄第二份就等于
//! 让「什么名字合法」「清单截到多少个文件」出现两处真相。
//!
//! 这里不 spawn `git`（发布走 [`Repo::commit`] / [`Repo::sync`]），也不读环境变量：
//! 仓根由调用方给。

use std::path::Path;

use serde::Serialize;
use serde_json::json;

use crate::{Repo, RepoError, Result, WriteFile, MAX_FILE_BYTES};

/// 一次列清单最多这么多文件（旧面板 spec §6.1 同一个数）：递归不设层数上限，
/// 只靠这个数刹车 —— 截到第 4 层等于「文件在盘上、面板里看不见」。
pub const MAX_FILES: usize = 200;

/// 名字最长多少字（旧面板 `checkName` 同一个数）。
pub const MAX_NAME: usize = 64;

/// 仓库根下不是条目的保留名（撞上它们 = 把 index.json 当技能编辑）。
pub const RESERVED: &[&str] = &[
    "categories.json",
    "deleted-extensions.json",
    "index.json",
    "index.mjs",
    "nous.json",
    "SKILL.md",
    "PROMPT.md",
    "README.md",
    "settings.json",
    "sources.json",
    "AGENTS.md",
    "test",
    "node_modules",
];

/// 旧面板 `checkName` 的同一套规矩：名字就是目录名、就是会话里的键。
///
/// 只认小写字母 / 数字 / 点 / 下划线 / 连字符，且以字母或数字开头；保留名不许用。
pub fn check_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(RepoError::BadRequest("名字不能为空".into()));
    }
    if name.chars().count() > MAX_NAME {
        return Err(RepoError::BadRequest(format!(
            "名字太长（最多 {MAX_NAME} 字）：{name}"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or_default();
    let valid = first.is_ascii_lowercase() || first.is_ascii_digit();
    let rest = name
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
    if !valid || !rest {
        return Err(RepoError::BadRequest(format!(
            "名字只认小写字母、数字、点、下划线、连字符，且以字母或数字开头：{name}"
        )));
    }
    if RESERVED.contains(&name) {
        return Err(RepoError::BadRequest(format!(
            "「{name}」是仓库自己的文件名，不能当条目名"
        )));
    }
    Ok(name.to_string())
}

/// 条目目录里的相对路径：不许绝对路径、不许 `..`、不许反斜杠 —— 串到别的条目里
/// 也算「逃出这一条」（[`crate::files::safe_path`] 只保证不出仓根）。
pub fn check_path(raw: &str) -> Result<String> {
    let path = raw.trim();
    if path.is_empty() {
        return Err(RepoError::BadRequest("path 不能为空".into()));
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(RepoError::BadRequest(format!(
            "路径必须是条目目录里的相对路径：{path}"
        )));
    }
    if path.split('/').any(|part| part == ".." || part == ".") {
        return Err(RepoError::BadRequest(format!("路径里有 .. 或 .：{path}")));
    }
    Ok(path.to_string())
}

/// 清单里的一行：相对路径 + 字节数（面板据此画树、判断要不要给预览）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EntryFile {
    pub path: String,
    pub size: u64,
}

/// `GET /v1/{kind}/files`：递归清单，正文与附件一起给（不按扩展名筛）。
///
/// 跳过点开头的文件与 `node_modules`；符号链接一律不进列表（面板是编辑器，
/// 不是任意路径的读口）；超过 [`MAX_FILE_BYTES`] 的也不列（读它必然失败）。
pub fn list_entry_files(repo: &Repo, name: &str) -> Result<Vec<EntryFile>> {
    require_entry(repo, name, "条目")?;
    let mut out = Vec::new();
    collect(&repo.root().join(name), "", &mut out);
    Ok(out)
}

fn collect(dir: &Path, rel: &str, out: &mut Vec<EntryFile>) {
    if out.len() >= MAX_FILES {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if kind.is_dir() {
            collect(&entry.path(), &child, out);
            continue;
        }
        // 符号链接一律不进列表：面板是编辑器，不是任意路径的读口。
        if !kind.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES as u64 {
            continue;
        }
        out.push(EntryFile { path: child, size: meta.len() });
    }
}

/// 条目在不在（是目录才算）。不在 → 404，**不是**空清单：
/// 「这条没了」和「这条一个文件都没有」是两件事。
pub fn require_entry(repo: &Repo, name: &str, label: &str) -> Result<()> {
    if !repo.root().join(name).is_dir() {
        return Err(RepoError::NotFound(format!("仓库里没有这个{label}：{name}")));
    }
    Ok(())
}

/// 一条新条目要落的那两个文件：主文件（`SKILL.md` / `PROMPT.md`）+ `nous.json`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skeleton {
    pub primary: &'static str,
    pub primary_text: String,
    pub summary: String,
    pub category: Vec<String>,
}

impl Skeleton {
    /// 技能骨架：`description` 走 JSON 字符串字面量 —— 引号 / 反斜杠都被转义，
    /// frontmatter 不会被一句话里的 `---` 撕开。
    pub fn skill(name: &str, description: &str) -> Self {
        let quoted = serde_json::to_string(description).unwrap_or_else(|_| "\"\"".to_string());
        Self {
            primary: "SKILL.md",
            primary_text: format!("---\nname: {name}\ndescription: {quoted}\n---\n\n# {name}\n"),
            summary: String::new(),
            category: Vec::new(),
        }
    }

    /// 模板骨架：正文可给；不给就落一个 `# <name>` 标题。
    pub fn prompt(name: &str, summary: &str, body: &str) -> Self {
        let text = if body.trim().is_empty() {
            format!("# {name}\n")
        } else {
            body.to_string()
        };
        Self {
            primary: "PROMPT.md",
            primary_text: text,
            summary: summary.to_string(),
            category: Vec::new(),
        }
    }

    pub fn with_category(mut self, category: Vec<String>) -> Self {
        self.category = category;
        self
    }

    /// `nous.json` 的正文（`{name, summary, category}`，缩进 + 尾换行）。
    pub fn manifest(&self, name: &str) -> String {
        let manifest = json!({
            "name": name,
            "summary": self.summary,
            "category": self.category,
        });
        serde_json::to_string_pretty(&manifest)
            .map(|text| text + "\n")
            .unwrap_or_else(|_| "{}\n".to_string())
    }
}

/// `POST /v1/{kind}/create`：落骨架两个文件，**不提交**。
///
/// 先写点开头的暂存目录再 `rename` —— 半截目录不许落到条目根里
/// （落一半就失败的话，面板会看见一条缺 `nous.json` 的「技能」）。
/// 已存在 → 409：换个名字，或者直接去改它。
pub fn create_entry(repo: &Repo, name: &str, skeleton: &Skeleton) -> Result<()> {
    let landing = repo.root().join(name);
    if landing.exists() {
        return Err(RepoError::Conflict {
            message: format!("{name}/ 已经在了 —— 换个名字，或者直接去改它"),
            current: None,
            etag: None,
        });
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let staging = repo.root().join(format!(".new-{name}-{stamp}"));
    std::fs::create_dir(&staging).map_err(|e| RepoError::Io(e.to_string()))?;
    let landed = (|| {
        std::fs::write(staging.join(skeleton.primary), &skeleton.primary_text)
            .map_err(|e| RepoError::Io(e.to_string()))?;
        std::fs::write(staging.join("nous.json"), skeleton.manifest(name))
            .map_err(|e| RepoError::Io(e.to_string()))?;
        std::fs::rename(&staging, &landing).map_err(|e| RepoError::Io(e.to_string()))
    })();
    if landed.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    landed
}

/// 发布的结果：要么提交了一条，要么只是把已提交的推上去。
///
/// 形状与 [`crate::CommitResult`] **逐字一致**（`{commit, pushed, refs}`）：
/// 只推没提交那条分支回 `commit: null` —— 客户端按同一个形状读，不认第二种。
#[derive(Debug, Clone, Serialize)]
pub struct Published {
    /// 提交的 hash；只推没提交时是 `null`。
    pub commit: Option<String>,
    pub pushed: crate::Pushed,
    pub refs: Vec<String>,
}

/// `POST /v1/{kind}/publish`：把私库里 dirty 的路径提交（并尝试 push）。
///
/// 自己不拼 `git add`：先问 [`Repo::pending`] 拿 dirty 路径，再交给 [`Repo::commit`]
/// —— 内容闸与 pathspec 防搭车都在那一层。
///
/// 只有「已提交没推」时再 commit 一次会撞 `nothing to commit`，所以那条分支只推。
/// 两样都没有 → 400（「没有待发布的改动」是调用方的操作问题，不是仓库坏了）。
pub fn publish_entry(repo: &Repo, message: &str) -> Result<Published> {
    let units = repo.pending()?;
    let mut paths: Vec<String> = Vec::new();
    let mut unpushed = false;
    for unit in &units {
        if unit.state == "unpushed" {
            unpushed = true;
            continue;
        }
        for path in &unit.paths {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
    }

    if !paths.is_empty() {
        let result = repo.commit(&paths, message, None)?;
        return Ok(Published {
            commit: Some(result.commit),
            pushed: result.pushed,
            refs: result.refs,
        });
    }
    if unpushed {
        let pushed = repo.sync()?;
        return Ok(Published { commit: None, pushed, refs: Vec::new() });
    }
    Err(RepoError::BadRequest("没有待发布的改动".into()))
}

/// 读一个文本文件：二进制 / 非 UTF-8 直接 400（面板不编辑二进制）。
pub fn read_text(repo: &Repo, path: &str) -> Result<(String, String)> {
    let read = repo.read_file(path)?;
    if read.content.contains(&0) {
        return Err(RepoError::BadRequest(format!("{path} 是二进制，这一页不编辑它")));
    }
    let text = String::from_utf8(read.content)
        .map_err(|_| RepoError::BadRequest(format!("{path} 不是 UTF-8 文本")))?;
    Ok((text, read.etag))
}

/// 写一个文本文件：只落盘、不提交。`base` 不符时回 409。
pub fn write_text(repo: &Repo, path: &str, text: &str, base: Option<&str>) -> Result<String> {
    let written = repo.write_file(WriteFile {
        path,
        content: text.as_bytes(),
        base_etag: base,
    })?;
    Ok(written.etag)
}
