//! skills 插件：`SKILL.md` 的盘上真相（#71）。
//!
//! 只读：扫目录、解析 frontmatter、数附件、汇总类别词表。
//! **不碰 git、不 spawn 进程、不写回私库** —— tracked 本票返回 `null`。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// 一条加载结果。`state` 只有 `ok` / `problem`；`detail` 装失败原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadState {
    pub state: String,
    pub detail: Option<String>,
}

/// 一个技能目录的盘上真相。字段名就是 wire 契约的形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    pub name: String,
    pub description: Option<String>,
    pub category: Vec<String>,
    pub hidden: bool,
    /// 本票不判定：恒 `None`（不碰 git）。
    pub tracked: Option<bool>,
    pub dir: PathBuf,
    pub file: Option<PathBuf>,
    /// 目录内除 SKILL.md 外的文件数。
    pub attachments: usize,
    pub load: LoadState,
}

/// 一次扫描的结果：条目 + 汇总类别词表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillScan {
    pub skills: Vec<SkillEntry>,
    pub categories: Vec<String>,
}

/// 扫一个私库根：每个子目录就是一条技能（隐藏目录跳过）。
pub fn scan(root: &Path) -> SkillScan {
    let mut skills = Vec::new();
    if let Ok(read_dir) = fs::read_dir(root) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            skills.push(scan_skill(&path, &name));
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    let categories = collect_categories(&skills, root);
    SkillScan { skills, categories }
}

fn scan_skill(dir: &Path, dir_name: &str) -> SkillEntry {
    let file = dir.join("SKILL.md");
    let mut skill = SkillEntry {
        name: dir_name.to_string(),
        description: None,
        category: Vec::new(),
        hidden: false,
        tracked: None,
        dir: dir.to_path_buf(),
        file: None,
        attachments: 0,
        load: LoadState { state: "ok".into(), detail: None },
    };
    skill.attachments = count_attachments(dir, 0);

    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) => {
            skill.load = LoadState {
                state: "problem".into(),
                detail: Some(format!("{}: {}", file.display(), error)),
            };
            return skill;
        }
    };
    skill.file = Some(file);

    match parse_frontmatter(&text) {
        Ok(front) => {
            skill.name = front.name.unwrap_or_else(|| dir_name.to_string());
            skill.description = front.description;
            skill.category = front.category;
            skill.hidden = front.hidden;
        }
        Err(detail) => {
            skill.load = LoadState { state: "problem".into(), detail: Some(detail) };
        }
    }
    skill
}

struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    category: Vec<String>,
    hidden: bool,
}

/// 极简 frontmatter：`---` 包裹的 `key: value`，以及 `category: [a, b]` 行内数组。
/// 第一行不是 `---` 或没有收尾 `---` = 坏。
fn parse_frontmatter(text: &str) -> Result<Frontmatter, String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err("SKILL.md 开头没有 frontmatter（两行 --- 包起来的那一段）".into());
    }
    let mut body = String::new();
    let mut closed = false;
    for line in &mut lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    if !closed {
        return Err("SKILL.md 的 frontmatter 没有收尾的 ---".into());
    }

    let mut result = Frontmatter { name: None, description: None, category: Vec::new(), hidden: false };
    for line in body.lines() {
        let Some((key, raw)) = line.split_once(':') else { continue };
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => result.name = Some(unquote(value)),
            "description" => result.description = Some(unquote(value)),
            "disable-model-invocation" => result.hidden = value.eq_ignore_ascii_case("true"),
            "category" => result.category = parse_list(value),
            _ => {}
        }
    }
    Ok(result)
}

fn parse_list(value: &str) -> Vec<String> {
    let trimmed = value.trim().trim_start_matches('[').trim_end_matches(']');
    trimmed
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| unquote(part))
        .collect()
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && ((value.starts_with('"') && value.ends_with('"')) || (value.starts_with('\'') && value.ends_with('\''))) {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

/// 目录内文件数：跳过点开头、`node_modules`、`test`；目录本身不算。
fn count_attachments(dir: &Path, depth: usize) -> usize {
    if depth > 8 {
        return 0;
    }
    let Ok(read_dir) = fs::read_dir(dir) else { return 0 };
    let mut total = 0;
    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" || name == "test" || name == "SKILL.md" {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_file() {
            total += 1;
        } else if kind.is_dir() {
            total += count_attachments(&entry.path(), depth + 1);
        }
    }
    total
}

/// `categories.json` 存在就用它的词表，不写回；否则只汇总技能身上的 category。
fn collect_categories(skills: &[SkillEntry], root: &Path) -> Vec<String> {
    let mut seen = Vec::new();
    let file = root.join("categories.json");
    if let Ok(text) = fs::read_to_string(&file) {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) {
            for item in items {
                if let Some(name) = item.as_str() {
                    if !seen.contains(&name.to_string()) {
                        seen.push(name.to_string());
                    }
                }
            }
        }
    }
    for skill in skills {
        for category in &skill.category {
            if !seen.contains(category) {
                seen.push(category.clone());
            }
        }
    }
    seen.sort();
    seen
}
