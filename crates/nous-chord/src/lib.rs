//! `chord` 的语义层：`context` / `json` / `delta` / `sqlite` 已落地；
//! `output` / `watch` 在 #19（harness 才用）。
//!
//! 来源与规则见仓库根 `AGENTS.md` §一（chord 只取六个目录）与 `SOURCES.md`。
//!
//! ## 依赖红线
//!
//! `cargo tree -p nous-chord` 只该有 `serde_json` / `tokio-util` / `rusqlite`。
//! 出现 `nous-ai` / `nous-durable` / `nous-tools` 就是契约图破了（AGENTS.md §二）。
//! `nous-env` 反过来依赖本 crate 的 [`context::Context`] —— 全库只有这一份。

pub mod context;
pub mod delta;
pub mod json;
pub mod sqlite;
