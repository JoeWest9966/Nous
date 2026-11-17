//! `durable` 的语义自己写（`AGENTS.md` §一：**只借语义和坑，不借结构**）。
//!
//! 已落地：`storage`（只读导入 + 九张表的行/语句 + 会话树渲染 + 提交语义 + 文档修订重放
//! + 一致性套件）、`types`（记录与草稿的形状）、`session`（#13）。
//! 进行中：`harness`（#14）—— `json` / `partial` / `live`（类型 + 九个 draft 变更函数 +
//! `checkpoint_when`）/ `generation` / `abort` 已落；剩 8 类 `MessageChange` 的投影（`#22` 共用
//! 那张表）。
//!
//! ## 契约面（AGENTS.md §二）
//!
//! `nous-durable → chord, ai, wire, telemetry, env`。工具靠 `nous-ai::Tool` trait 注入，
//! 所以 `cargo tree -p nous-durable` 里**永远不该出现 `nous-tools`**。

pub mod error;
pub mod harness;
pub mod session;
pub mod storage;
pub mod types;

pub use error::{Result, StorageError};
pub use storage::commit::{
    CommitResult, CommitState, ConversationQuery, PendingWrites, StoredBatch, StoredChange,
    StoredDocumentChange,
};
pub use storage::cursor::{Cursor, Page, ScanOrder};
pub use storage::storage::{EntryQuery, Storage};
pub use storage::documents::{DocumentPoint, StoredDocument};
pub use storage::writes::{
    CopySource, DocumentAction, DocumentContent, Id, Refusal, Seq, StorageWrite, TableName,
};
