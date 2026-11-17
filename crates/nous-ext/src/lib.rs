//! 扩展运行时（应用层）。
//!
//! spec：`docs/spec/ext-runtime.md`。这一层拥有扩展的**进程**（spec Q7）：内核只认 trait，
//! 不认识进程。全仓进程创建点恰好两个，扩展子进程的那一处在 [`spawn`]。
//!
//! | 模块 | 负责 |
//! |---|---|
//! | [`config`] | 三个预算数、两个硬顶、一个并发上限、隔离参数 |
//! | [`manifest`] | 清单装载 + 哈希 / triple / protocol 校验 |
//! | [`handle`] | 值 → 内容的三条硬边界 |
//! | [`spawn`] | 一扩展一进程、fd 表、uid、帧通道 |
//! | [`nameplate`] | 具名槽位与冲突 |
//! | [`state`] | 五个状态 + 诊断去重 |
//! | [`runtime`] | 监管：装配、调用、熔断、重载、禁用 |
//! | [`audit`] | 四种事件落提交、结果与 mutation 同一次提交 |
//! | [`install`] | 原子切换与产物来源 |

pub mod audit;
pub mod config;
pub mod error;
pub mod handle;
pub mod install;
pub mod manifest;
pub mod nameplate;
pub mod runtime;
pub mod sha256;
pub mod spawn;
pub mod state;

pub use config::{HandleValue, Isolation, Limits, RuntimeConfig};
pub use error::{ExtError, ExtResult, FailureClass};
pub use install::Installer;
pub use manifest::{Artifact, ArtifactMeta, LoadedManifest};
pub use runtime::{PushHook, Runtime, StateCommit, ToolOutcome, Turn};
pub use state::ExtState;
