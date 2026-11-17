//! 会话语义层。上游 `packages/durable/src/session/**`，只借语义、不借结构。
//!
//! 这一层回答一个问题：**一次提交把什么变成了什么** —— 会话的可见转录、活动上下文、
//! 提交（submission）的生命周期，以及提交后谁该知道。它不碰传输（`nous-server`，#22）、
//! 不碰模型调用（`harness`，#14）、不碰工具（`nous-tools`）。
//!
//! ## 与上游的结构差别（有意为之）
//!
//! - 上游把事务回调（`Tx`）交给调用方，回调里再读写；本库的事务封在
//!   [`Storage::commit`](crate::Storage) 里，调用方给的是**一批写**（`12b` 的
//!   [`StorageWrite`](crate::StorageWrite)）。`Tx` 不进 Rust 的公开面。
//! - 上游 `session/` 有 2,105 行的 `session.ts` + 1,300 行的 `transaction.ts` + 缓存的
//!   `cached-storage.ts`。缓存是性能件（#18），本层先不搬；语义对得上再谈缓存。
//!
//! ## 现在有什么
//!
//! - [`entries`] —— 六个 entry 判别键（`pi.user` … `pi.compaction`）；
//! - [`publish`] —— 提交发布面：`CommitPublication` / `CommitChange` 与观察者名单。
//!
//! `Session` 本身（`commit` / `close` / `closed`）在 #13 的 commit ③。

pub mod entries;
pub mod publish;

pub use entries::EntryKind;
pub use publish::{
    CommitChange, CommitObservation, CommitObserver, CommitPublication, DocumentCommitChange,
    ObserverRegistry,
};
