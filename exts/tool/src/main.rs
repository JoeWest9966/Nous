//! 样例扩展 `tool`：一个会流式输出、能被取消、能申请写的工具。
//!
//! 参数（模型给的 `args`）：
//!
//! | 键 | 含义 |
//! |---|---|
//! | `lines` | 输出几行（默认 5），行文本 `line-1`… |
//! | `delayMs` | 每行之间睡多久（取消在这段时间里到） |
//! | `cancelAfterMs` | 先睡这么久（宿主在这时发 cancel） |
//! | `lateOutput` | 发完 `final{cancelled}` 之后**再发一条 output**（EX-19 的载体） |
//! | `crash` | 立刻退出（`extension_crash`） |
//! | `mutations` | 原样放进 `final.result.mutations`（EX-20/21/22） |
//!
//! 接入它时内核零改动（EX-04）。

use std::time::Duration;

use nous_ext_sdk::api::{Cancel, Finished};
use nous_ext_sdk::{run, Extension, Section, SectionInput, Tool, ToolCtx};
use serde_json::{json, Value};

struct Preamble;

impl Section for Preamble {
    fn key(&self) -> &'static str {
        "tools"
    }

    fn render(&self, _input: &SectionInput) -> String {
        "Available extension tools: echo, write\n".to_string()
    }
}

struct Echo;

impl Tool for Echo {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "lines": { "type": "integer" },
                "delayMs": { "type": "integer" },
                "cancelAfterMs": { "type": "integer" },
                "lateOutput": { "type": "boolean" },
                "crash": { "type": "boolean" },
                "mutations": { "type": "array" }
            }
        })
    }

    fn call(&mut self, ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
        if args.get("crash").and_then(Value::as_bool).unwrap_or(false) {
            std::process::exit(1);
        }
        if let Some(bytes) = args.get("bytes").and_then(Value::as_u64) {
            // 撞 1MB 输出硬顶的载体：分块发（单帧必须 < 1MB，否则是**帧**超限而不是输出超限）。
            let mut sent = 0u64;
            while sent < bytes {
                let chunk = (bytes - sent).min(300_000);
                ctx.output(&"x".repeat(chunk as usize));
                sent += chunk;
            }
            return Err(Finished::as_str().to_string());
        }
        let lines = args.get("lines").and_then(Value::as_u64).unwrap_or(5);
        let delay = args.get("delayMs").and_then(Value::as_u64).unwrap_or(0);
        let late_output = args
            .get("lateOutput")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(ms) = args.get("cancelAfterMs").and_then(Value::as_u64) {
            ctx.sleep_unless_cancelled(Duration::from_millis(ms));
        }
        for index in 1..=lines {
            if ctx.cancelled() {
                break;
            }
            ctx.output(&format!("line-{index}"));
            if delay > 0 {
                ctx.sleep_unless_cancelled(Duration::from_millis(delay));
            }
        }
        if ctx.cancelled() {
            ctx.finish_cancelled();
            if late_output {
                // 取消之后还发 output：宿主必须拒掉，且不落库（EX-19）。
                ctx.output("late-output");
            }
            // 自己已经发过终帧，别再让 SDK 补一帧。
            return Err(Finished::as_str().to_string());
        }
        let mut result = json!({ "content": "done" });
        if let Some(mutations) = args.get("mutations").cloned() {
            result["mutations"] = mutations;
        }
        Ok(result)
    }
}

struct Write;

impl Tool for Write {
    fn name(&self) -> &'static str {
        "write"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "kind": { "type": "string" }
            }
        })
    }

    fn call(&mut self, _ctx: &mut ToolCtx, args: &Value) -> Result<Value, String> {
        // 申请写：两种原语（追加 entry / 写文档），终态不在申请范围（spec 7.3）。
        let kind = args
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("entry")
            .to_string();
        let mutation = if kind == "entry" {
            json!({ "kind": "entry", "entry": { "kind": "pi.tool-result", "data": { "text": "wrote" } } })
        } else {
            json!({ "kind": "document", "document": { "key": "notes", "value": { "wrote": true } } })
        };
        Ok(json!({ "content": "wrote", "mutations": [mutation] }))
    }
}

fn main() {
    // 取消的约定返回值也要被 SDK 认得（`Cancel` 只是把字面量钉在一处）。
    let _ = Cancel::as_str();
    run(Extension::new("tool")
        .section(Preamble)
        .tool(Echo)
        .tool(Write));
}
