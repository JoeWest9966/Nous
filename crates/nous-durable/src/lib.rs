//! `durable` 的语义自己写（`AGENTS.md` §一：**只借语义和坑，不借结构**）。
//!
//! 已落地：`storage`（只读导入 + 九张表的行/语句 + 会话树渲染）。
//! 待落地：提交语义（12b）、文档修订（12c）、conformance（12d）、session、harness。
//!
//! ## 契约面（AGENTS.md §二）
//!
//! `nous-durable → chord, ai, wire, telemetry, env`。工具靠 `nous-ai::Tool` trait 注入，
//! 所以 `cargo tree -p nous-durable` 里**永远不该出现 `nous-tools`**。

pub mod error;
pub mod storage;

pub use error::{Result, StorageError};
