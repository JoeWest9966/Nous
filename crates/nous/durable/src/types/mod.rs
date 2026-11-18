//! 记录与草稿的类型层。上游 `packages/durable/src/types.ts` 里真被 storage/session 用到的那部分。
//!
//! 这个模块**只放形状**（struct / enum + 序列化规矩），不放行为 —— 行为在 `storage/` 与
//! `session/` 里。上游 1,449 行的 `types.ts` 里大半是 task 引擎、watch 机制、TS 类型体操，
//! 一个都不搬；取舍清单贴在 issue #13 上，逐条带理由。
//!
//! 两条硬规矩（改动前先读）：
//!
//! 1. **序列化后的 JSON 名字与上游一字不差**。字段是 `snake_case`，`#[serde(rename_all =
//!    "camelCase")]` 负责落地；`Option` 缺席时整个键都不出现。
//! 2. **取值域是枚举**。拼错值、非法组合在反序列化时失败，不留到运行时。
//!
//! `TaskRecord.state` / `memos` / `trace` 三处故意是不透明 JSON，理由在 `docs/storage.md` §12.5。

pub mod draft;
pub mod records;

pub use draft::{EntryDraft, EntryHead, SubmissionCreate, SubmissionSettlement};
pub use records::{
    AbortReason, ContextEdit, ConversationOwner, ConversationOwnership, ConversationParent,
    ConversationRecord, DocumentFork, DocumentHistory, DocumentRecord, DocumentScope, EntryRecord,
    SubmissionFields, SubmissionRecord, SubmissionStatus, SubmissionType, TaskRecord,
    TerminalReason,
};
