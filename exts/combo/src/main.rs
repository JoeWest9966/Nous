//! 样例扩展 `combo`：四件能力压在一个扩展里。
//!
//! 判据只用四个小样例拆开验（坏的时候分得清是哪件），这个组合件用来证明四件能力
//! 可以共存，以及"一个扩展一个进程"不是按能力拆的。

use std::time::Duration;

use nous_ext_sdk::{run, Extension, Section, SectionInput, Tool, ToolCtx};
use serde_json::{json, Value};

struct Roster;

impl Section for Roster {
    fn key(&self) -> &'static str {
        "roster"
    }

    fn render(&self, input: &SectionInput) -> String {
        let skills = input
            .handles
            .get("skills")
            .map(|content| content.files.len())
            .unwrap_or(0);
        let prompts = input
            .handles
            .get("prompts")
            .map(|content| content.files.len())
            .unwrap_or(0);
        format!("roster: skills={skills} prompts={prompts}\n")
    }
}

struct Ping;

impl Tool for Ping {
    fn name(&self) -> &'static str {
        "ping"
    }

    fn call(&mut self, ctx: &mut ToolCtx, _args: &Value) -> Result<Value, String> {
        for index in 0..3 {
            ctx.output(&format!("ping-{index}"));
            ctx.sleep_unless_cancelled(Duration::from_millis(5));
        }
        Ok(json!({ "content": "pong" }))
    }
}

fn main() {
    run(Extension::new("combo")
        .section(Roster)
        .tool(Ping)
        .nameplates("skills", "skill:")
        .nameplates("prompts", "prompt:"));
}
