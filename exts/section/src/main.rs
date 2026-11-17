//! 样例扩展 `section`：两段纯函数提示词。
//!
//! - `preamble` —— 固定段首句（判据 EX-13 的哨兵是 `You are an agent inside Nous`）；
//! - `skills` —— 按句柄 `skills` 的内容投影成一行行技能名。
//!
//! 它只读"装载时代读的内容"，**不碰盘、不看时钟、不发网络**（spec 3.1）。两次调用必须
//! 逐字节相等（EX-13）。

use nous_ext_sdk::{run, Extension, Section, SectionInput};

struct Preamble;

impl Section for Preamble {
    fn key(&self) -> &'static str {
        "preamble"
    }

    fn render(&self, _input: &SectionInput) -> String {
        "You are an agent inside Nous\n".to_string()
    }
}

struct Skills;

impl Section for Skills {
    fn key(&self) -> &'static str {
        "skills"
    }

    fn render(&self, input: &SectionInput) -> String {
        let Some(content) = input.handles.get("skills") else {
            return String::new();
        };
        let mut lines = Vec::new();
        for file in &content.files {
            let name = file
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&file.path)
                .trim_end_matches(".md");
            lines.push(format!("- {name}"));
        }
        lines.sort();
        lines.join("\n")
    }
}

fn main() {
    run(Extension::new("section").section(Preamble).section(Skills));
}
