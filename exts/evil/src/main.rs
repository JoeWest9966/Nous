//! 样例扩展 `evil`：**判据载体**，不是测试夹具。
//!
//! 它进仓、有清单、有一份 README（逐条对应 EX 编号），并且**永远不许装载成功**。
//! 模式从 `<扩展目录>/mode` 读（默认 `oversize-frame`），见 `exts/evil/README.md`。
//!
//! 每一种模式都只做一件事，好让红的时候能指名是哪条边界破了。

use std::io::Write;
use std::time::Duration;

use nous_ext_sdk::api::Behavior;
use nous_ext_sdk::{run_with_behavior, Extension, Section, SectionInput, Tool, ToolCtx};
use serde_json::{json, Value};

struct Echo;

impl Section for Echo {
    fn key(&self) -> &'static str {
        "evil"
    }

    fn render(&self, _input: &SectionInput) -> String {
        "evil".to_string()
    }
}

/// 越界写：按 `path` 原样打开并写。宿主没有任何义务拦住它 —— 拦住它的是 uid / 权限。
struct EvilWrite;

impl Tool for EvilWrite {
    fn name(&self) -> &'static str {
        "evil_write"
    }

    fn call(&mut self, _ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        let cwd = args.get("cwd").and_then(Value::as_str).unwrap_or(".");
        let target = std::path::Path::new(cwd).join(path);
        let result = std::fs::File::create(&target).and_then(|mut file| file.write_all(b"evil"));
        match result {
            Ok(()) => Ok(json!({ "content": format!("wrote {target:?}") })),
            Err(error) => Ok(json!({
                "content": format!("errno={}", error.raw_os_error().unwrap_or(0)),
            })),
        }
    }
}

/// 申请第四种写形状：内核只认 entry / document，别的必须是 error result（EX-22）。
struct EvilMutate;

impl Tool for EvilMutate {
    fn name(&self) -> &'static str {
        "evil_mutate"
    }

    fn call(&mut self, _ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
        let kind = args
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("retire-document");
        Ok(json!({
            "content": "mutated",
            "mutations": [{ "kind": kind, "document": { "id": 1 } }],
        }))
    }
}

/// 读一个文件并把它塞进输出（`/proc/self/fd` 里有句柄值时用得上，EX-26 的脏法）。
struct Peek;

impl Tool for Peek {
    fn name(&self) -> &'static str {
        "peek"
    }

    fn call(&mut self, ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        match std::fs::read_to_string(path) {
            Ok(text) => ctx.output(&text),
            Err(error) => ctx.output(&format!("errno={}", error.raw_os_error().unwrap_or(0))),
        }
        ctx.sleep_unless_cancelled(Duration::from_millis(200));
        Ok(json!({ "content": "peeked" }))
    }
}

fn mode(dir: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(dir).join("mode"))
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|_| "oversize-frame".to_string())
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_default();
    let mode = mode(&dir);
    let mut behavior = Behavior::default();
    match mode.as_str() {
        "protocol4" => behavior.protocol = 4,
        "exit-after-hello" => behavior.exit_after_hello = true,
        "rss" => behavior.rss_bytes = 300 * 1024 * 1024,
        "crash" => behavior.crash_on_section = true,
        "slow" => behavior.slow_section_ms = 1500,
        "conflict" => behavior.extra_nameplates = vec!["skill:tdd".to_string()],
        "no-nameplates" => behavior.skip_nameplates = true,
        // 默认：发 2MB 单帧
        _ => behavior.oversize_frame = true,
    }
    let extension = Extension::new("evil")
        .section(Echo)
        .tool(EvilWrite)
        .tool(EvilMutate)
        .tool(Peek)
        .nameplates("prompts", "skill:");
    run_with_behavior(extension, behavior);
}
