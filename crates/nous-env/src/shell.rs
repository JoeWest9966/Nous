//! `Shell` —— 跑命令的契约面。
//!
//! 形状 1:1 照 `packages/durable/src/env/index.ts` 的 `Shell`。与
//! `crates/rpi-tools/src/env.rs` 那版的三处差别（都是**取 TS 那份**的结果）：
//!
//! 1. 命令可以是 `string`（走 shell）或 `string[]`（直接 exec，不经 shell）→
//!    [`Command`] 两个变体。Rust 那版只有 `&str`，argv 要靠 `ShellOverride`
//!    绕，等于两套契约并存。
//! 2. **输出只走 `on_output` 流**，返回值只有 `exitCode` + `spillPath`。Rust 那版
//!    返回 `ShellOutput{stdout, stderr, exit_code}` 全文捕获 —— 那不是这一个契约面的
//!    形状，而且全文捕获会把长输出一次性糊进内存。
//! 3. `spill` / `window` 是契约的一部分：溢出落临时文件、只留尾巴并按速率限速。
//!    这正对着我们的 SSH 流式输出 —— SSE 不吞输出的关键就在这里。
//!
//! 两条约定与 TS 一致：
//!
//! - 中止（`Context` 取消）或超时**只杀这条命令的进程**，不是整个环境。
//! - `cleanup` 杀这个环境**还在跑的所有命令**，只给它的主人关机用，绝不用于单个请求。

use async_trait::async_trait;

use crate::context::Context;
use crate::error::ExecutionError;
use crate::types::{Command, ShellExecOptions, ShellExecResult};

/// Portable shell capability.
#[async_trait]
pub trait Shell: Send + Sync {
    /// 跑一条命令。
    ///
    /// 字符串走环境的 shell；数组直接 exec `argv[0]`，其余当参数传，**不经 shell**，
    /// 所以它们原样到达程序。中止 `context` 或超时只杀这条命令的进程。
    async fn exec(
        &self,
        command: &Command,
        options: &ShellExecOptions,
        context: &Context,
    ) -> Result<ShellExecResult, ExecutionError>;

    /// 杀掉这个环境还在跑的所有命令。给主人关机用。
    async fn cleanup(&self, context: &Context);
}
