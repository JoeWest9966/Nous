//! 段清单：`<仓根>/sections.json`。
//!
//! 段 ID **就是文件夹名** —— 目录名 = git 路径 = URL 段 = 会话里的前缀，只有一份事实源。
//! 每段自带清单文件名与校验器（`preset.json` / `preset` vs `ultimate.json` / `ultimate`），
//! 所以「终极模板」不靠段名硬编码来认。

use serde::Deserialize;

/// 一条段的形状校验器。`preset` = 会话模板，`ultimate` = 终极模板。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checker {
    Preset,
    Ultimate,
}

impl Checker {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preset => "preset",
            Self::Ultimate => "ultimate",
        }
    }
}

/// 一段的盘上事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub id: String,
    pub title: String,
    /// 这一段的清单文件名（`preset.json` / `ultimate.json`）。
    pub manifest: String,
    pub checker: Checker,
}

impl Section {
    /// 终极模板段（组合开关 / 触发器引用只活在它里面）。
    pub fn is_ultimate(&self) -> bool {
        self.checker == Checker::Ultimate
    }
}

/// 没有 `sections.json` 时的兜底：老仓按目录名认段。
///
/// 这两段是本内核的既有真相（`combos` / `triggers` 都按 `Ultimate-Template` 认），
/// 所以缺清单时也不退回「单段空 id」—— 那会让现有数据根凭空少一段。
pub fn default_sections() -> Vec<Section> {
    vec![
        Section {
            id: "Session-Template".into(),
            title: "会话模板".into(),
            manifest: "preset.json".into(),
            checker: Checker::Preset,
        },
        Section {
            id: "Ultimate-Template".into(),
            title: "终极模板".into(),
            manifest: "ultimate.json".into(),
            checker: Checker::Ultimate,
        },
    ]
}

#[derive(Debug, Deserialize)]
struct SectionsFile {
    v: i64,
    sections: Vec<SectionEntry>,
}

#[derive(Debug, Deserialize)]
struct SectionEntry {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    manifest: Option<String>,
    #[serde(default)]
    checker: Option<String>,
}

/// 解析 `sections.json` 的原文。`None` = 文件不存在（走兜底）。
///
/// 坏清单是**错**不是空表：段名写错会让一整段凭空消失，静默降级比报错更难查。
pub fn read_sections(text: Option<&str>) -> Result<Vec<Section>, String> {
    let Some(text) = text else { return Ok(default_sections()) };
    let file: SectionsFile = serde_json::from_str(text)
        .map_err(|error| format!("sections.json 不是合法 JSON: {error}"))?;
    if file.v != 1 {
        return Err(format!("sections.json 的 v 得是 1，收到 {}", file.v));
    }
    if file.sections.is_empty() {
        return Err("sections.json 缺 sections 数组".into());
    }
    let mut out = Vec::with_capacity(file.sections.len());
    for entry in file.sections {
        let id = entry.id.trim().to_string();
        if id.is_empty() || id.contains('/') || id.starts_with('.') {
            return Err(format!("sections.json 里的段 id「{id}」不合格：只能是文件夹名"));
        }
        let checker = match entry.checker.as_deref() {
            None | Some("preset") => Checker::Preset,
            Some("ultimate") => Checker::Ultimate,
            Some(other) => {
                return Err(format!("段「{id}」的 checker「{other}」不认识（只有 preset / ultimate）"));
            }
        };
        out.push(Section {
            title: entry.title.unwrap_or_else(|| id.clone()),
            manifest: entry.manifest.unwrap_or_else(|| "preset.json".into()),
            id,
            checker,
        });
    }
    Ok(out)
}

/// 按 id 找一段。认不出来就是 `None` —— 不猜、不退回默认段。
pub fn find<'a>(sections: &'a [Section], id: &str) -> Option<&'a Section> {
    sections.iter().find(|section| section.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 没有清单走两段兜底() {
        let sections = read_sections(None).unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[1].manifest, "ultimate.json");
        assert!(sections[1].is_ultimate());
    }

    #[test]
    fn 清单决定清单文件名与校验器() {
        let sections = read_sections(Some(
            r#"{"v":1,"sections":[
                {"id":"A","title":"甲","manifest":"p.json","checker":"preset"},
                {"id":"B","manifest":"u.json","checker":"ultimate"}
            ]}"#,
        ))
        .unwrap();
        assert_eq!(find(&sections, "B").unwrap().manifest, "u.json");
        assert!(find(&sections, "B").unwrap().is_ultimate());
        assert_eq!(find(&sections, "A").unwrap().title, "甲");
        assert!(find(&sections, "C").is_none());
    }

    #[test]
    fn 坏清单报错不静默() {
        assert!(read_sections(Some("{")).is_err());
        assert!(read_sections(Some(r#"{"v":2,"sections":[{"id":"A"}]}"#)).is_err());
        assert!(read_sections(Some(r#"{"v":1,"sections":[]}"#)).is_err());
        assert!(read_sections(Some(r#"{"v":1,"sections":[{"id":"a/b"}]}"#)).is_err());
        assert!(read_sections(Some(r#"{"v":1,"sections":[{"id":"A","checker":"x"}]}"#)).is_err());
    }
}
