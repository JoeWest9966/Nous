//! **内容**：内核按句柄的值代读之后交给扩展的已解析数据。
//!
//! 形状固定（spec 3.0）：`{ files: [{ path, text }], truncated: bool }`。
//! `path` 是**相对句柄值目录**的相对路径；扩展拿不到值、也拿不到路径（`E1`）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 一份内容里的一个文件。`path` 只许相对、不含 `..`（越界在内核代读时就拒了）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentFile {
    pub path: String,
    pub text: String,
}

/// 内核代读的一份**内容**。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    pub files: Vec<ContentFile>,
    #[serde(default)]
    pub truncated: bool,
}

impl Content {
    /// 按相对路径取一个文件。
    pub fn file(&self, path: &str) -> Option<&ContentFile> {
        self.files.iter().find(|file| file.path == path)
    }

    /// 按相对路径取文本。
    pub fn text(&self, path: &str) -> Option<&str> {
        self.file(path).map(|file| file.text.as_str())
    }

    /// 第一条文件的文本（样例里"`mode` 文件"那种约定用它）。
    pub fn first_text(&self) -> Option<&str> {
        self.files.first().map(|file| file.text.as_str())
    }

    /// 所有 `*.md` 的**文件主干名**，按路径排序。名牌就靠它。
    pub fn stems(&self) -> Vec<String> {
        let mut paths: Vec<&str> = self.files.iter().map(|file| file.path.as_str()).collect();
        paths.sort_unstable();
        paths
            .into_iter()
            .filter_map(|path| {
                let name = path.rsplit('/').next().unwrap_or(path);
                let stem = name.strip_suffix(".md").unwrap_or(name);
                Some(stem.to_string())
            })
            .collect()
    }
}

/// 句柄名 → 内容。键是清单里声明的**名字**。
pub type Handles = BTreeMap<String, Content>;
