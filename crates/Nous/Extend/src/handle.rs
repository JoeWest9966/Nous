//! 句柄代读：**值 → 内容**，路径不出内核（spec ch6）。
//!
//! 三条硬边界，每条都在这里落实：
//!
//! 1. `path` 只许相对、不许 `..`（EX-60）—— 越界在**代读时**拒并落诊断，不交给扩展再检查；
//! 2. **不跟随符号链接**（EX-67）—— `lstat` 是符号链接就**跳过**并落诊断点名；
//! 3. **截断确定性**（EX-68）—— 遍历顺序按相对路径**字符串排序**，同一上限下截断点相同。

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use nous_ext_sdk::content::{Content, ContentFile, Handles};

use crate::config::HandleValue;

/// 一次代读的结果：内容 + 代读自己产生的诊断（跳过/拒绝/截断）。
#[derive(Debug, Clone, Default)]
pub struct Delivery {
    pub handles: Handles,
    pub diagnostics: Vec<String>,
}

/// 一条路径是不是安全的**相对**路径。绝对路径、`..`、根前缀都不安全。
pub fn is_safe_relative(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.starts_with('~') {
        return false;
    }
    let mut components = Path::new(path).components();
    let mut saw_normal = false;
    for component in &mut components {
        match component {
            Component::Normal(_) => saw_normal = true,
            // CurDir 也拒：`./x` 与 `x` 是两个字符串，允许前者就等于允许拼接。
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return false
            }
        }
    }
    saw_normal
}

/// 按清单里声明的句柄**名**代读内容。缺任何一个值都是 [`crate::error::ExtError::MissingHandle`]。
pub fn deliver(
    names: &[String],
    values: &BTreeMap<String, HandleValue>,
    max_bytes: usize,
) -> Result<Delivery, String> {
    let mut delivery = Delivery::default();
    for name in names {
        let Some(value) = values.get(name) else {
            return Err(name.clone());
        };
        let mut files = Vec::new();
        match &value.entries {
            Some(entries) => {
                for entry in entries {
                    if !is_safe_relative(entry) {
                        delivery
                            .diagnostics
                            .push(format!("rejected path={entry} reason=越界"));
                        continue;
                    }
                    collect_file(&value.root, entry, &mut files, &mut delivery.diagnostics);
                }
            }
            None => {
                let mut relative = Vec::new();
                walk(&value.root, &value.root, &mut relative, &mut delivery.diagnostics);
                // 固定遍历顺序：按相对路径**字符串**排序（不按 inode、不按目录返回顺序）。
                relative.sort();
                for path in relative {
                    collect_file(&value.root, &path, &mut files, &mut delivery.diagnostics);
                }
            }
        }

        // 截断确定性：按排序后的顺序装，装不下就停；同一上限下截断点相同。
        let mut content = Content::default();
        let mut used = 0usize;
        for (path, text) in files {
            let bytes = text.len();
            if used + bytes > max_bytes {
                content.truncated = true;
                break;
            }
            used += bytes;
            content.files.push(ContentFile { path, text });
        }
        delivery.handles.insert(name.clone(), content);
    }
    Ok(delivery)
}

/// 内容的结构校验（应用层语义：Markdown 带 frontmatter 就必须有 `name`）。
///
/// 重载时新内容解析失败 → **继续用旧内容** + 一条诊断（EX-44）。这是唯一允许"用旧值"的地方：
/// 它不是造第二份真相，是"没换成功"。
pub fn validate(handles: &Handles) -> Vec<String> {
    let mut problems = Vec::new();
    for content in handles.values() {
        for file in &content.files {
            if !file.path.ends_with(".md") {
                continue;
            }
            let Some(rest) = file.text.strip_prefix("---") else {
                continue;
            };
            let Some(end) = rest.find("\n---") else {
                problems.push(format!("frontmatter 缺 name file={}", file.path));
                continue;
            };
            let frontmatter = &rest[..end];
            let has_name = frontmatter
                .lines()
                .any(|line| line.trim_start().starts_with("name:"));
            if !has_name {
                problems.push(format!("frontmatter 缺 name file={}", file.path));
            }
        }
    }
    problems
}

/// 扫目录时跳过的名字：版本库、依赖、产物。
///
/// 句柄交付的是「给扩展读的内容」，不是一份文件系统快照：`/opt/Nous-skills` 里的
/// `.git` 一动就是几 MB 的 object，64 KiB 的句柄预算会被它吃光 —— 扩展拿到的
/// `files` 里一条 `SKILL.md` 都没有，看起来像「技能仓库是空的」。跳过是唯一说得通的
/// 口径，且它和「隐藏文件不给外读」不冲突。
fn skipped_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "target" | "artifacts")
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>, diagnostics: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .filter(|path| {
            let skip = path
                .file_name()
                .map(|name| skipped_dir(&name.to_string_lossy()))
                .unwrap_or(false);
            if skip {
                let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
                diagnostics.push(format!("skipped={relative} reason=not-content"));
            }
            !skip
        })
        .collect();
    names.sort();
    for path in names {
        let relative = match path.strip_prefix(root) {
            Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            // 不跟随：读到它就已经泄漏了，所以跳过 + 点名。
            diagnostics.push(format!("skipped={relative} reason=symlink"));
            continue;
        }
        if metadata.is_dir() {
            walk(root, &path, out, diagnostics);
        } else if metadata.is_file() {
            out.push(relative);
        }
    }
}

fn collect_file(root: &Path, relative: &str, out: &mut Vec<(String, String)>, diagnostics: &mut Vec<String>) {
    let path = root.join(relative);
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        diagnostics.push(format!("missing={relative}"));
        return;
    };
    if metadata.file_type().is_symlink() {
        diagnostics.push(format!("skipped={relative} reason=symlink"));
        return;
    }
    if !metadata.is_file() {
        diagnostics.push(format!("skipped={relative} reason=not-a-file"));
        return;
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => out.push((relative.to_string(), text)),
        Err(error) => diagnostics.push(format!(
            "unreadable={relative} reason={}",
            error.kind()
        )),
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nous-handle-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 句柄交付的是「给扩展读的内容」，不是文件系统快照：`.git` / `node_modules` /
    /// `target` 一律不交付。不跳的话，技能仓库的 `.git` 一动就是几 MB，64 KiB 的预算
    /// 会被它吃光 —— 扩展看到的 `files` 里一条 `SKILL.md` 都没有，像「仓库是空的」。
    #[test]
    fn skips_vcs_and_dependency_dirs() {
        let root = scratch("skip");
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join(".git/objects/pack"), "x".repeat(200)).unwrap();
        fs::create_dir_all(root.join("node_modules/dep")).unwrap();
        fs::write(root.join("node_modules/dep/index.js"), "junk").unwrap();
        fs::create_dir_all(root.join("tdd")).unwrap();
        fs::write(root.join("tdd/SKILL.md"), "---\nname: tdd\n---\n").unwrap();

        let mut values = BTreeMap::new();
        values.insert("skills".to_string(), HandleValue::dir(&root));
        let delivery = deliver(&["skills".to_string()], &values, 64 * 1024).unwrap();
        let content = &delivery.handles["skills"];
        let paths: Vec<&str> = content.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["tdd/SKILL.md"], "只交付内容文件：{paths:?}");
        assert!(!content.truncated, "跳完之后不该被截断");
        assert!(
            delivery.diagnostics.iter().any(|d| d.contains(".git") && d.contains("not-content")),
            "跳了要说一声：{:?}",
            delivery.diagnostics
        );
    }

    /// 越界路径在**代读时**就拒（EX-60）：`entries` 里写 `../x` 不许被读出来。
    #[test]
    fn rejects_escaping_entries() {
        let root = scratch("escape");
        fs::write(root.join("ok.md"), "fine").unwrap();
        let mut values = BTreeMap::new();
        values.insert(
            "skills".to_string(),
            HandleValue::entries(&root, vec!["ok.md".into(), "../etc/passwd".into()]),
        );
        let delivery = deliver(&["skills".to_string()], &values, 64 * 1024).unwrap();
        let paths: Vec<&str> = delivery.handles["skills"].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["ok.md"]);
        assert!(delivery.diagnostics.iter().any(|d| d.contains("越界")), "{:?}", delivery.diagnostics);
    }
}
