//! 样例扩展 `handle`：只吃内核代读的**内容**，一个文件句柄都没有。
//!
//! 段文本是 `prompts` 句柄内容的投影：`{count, names}`。它拿不到值、拿不到路径（EX-26/46）。

use nous_ext_sdk::{run, Extension, Section, SectionInput};

struct Prompts;

impl Section for Prompts {
    fn key(&self) -> &'static str {
        "prompts"
    }

    fn render(&self, input: &SectionInput) -> String {
        let Some(content) = input.handles.get("prompts") else {
            return "prompts: 0\n".to_string();
        };
        let mut names: Vec<&str> = content
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        names.sort_unstable();
        let mut text = format!("prompts: {}\n", names.len());
        for name in names {
            text.push_str(&format!("- {name}\n"));
        }
        if content.truncated {
            text.push_str("truncated: true\n");
        }
        text
    }
}

fn main() {
    run(Extension::new("handle").section(Prompts));
}
