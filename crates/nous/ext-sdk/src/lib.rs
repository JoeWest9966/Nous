//! 扩展作者面（SDK）。
//!
//! 一个扩展 = **一份清单 + 一个入口 + 一个进程**（spec ch1）。这份 crate 是扩展作者能
//! 看见的全部：
//!
//! - [`Manifest`] —— `nous.json` 的形状；只有能力声明，没有本机路径；
//! - [`frame`] —— 长度前缀 JSON 帧的编解码（单帧上限 1MB，双向各自计数）；
//! - [`content`] —— 内核代读后交过来的**内容**（`{files:[{path,text}],truncated}`）；
//! - [`api`] —— 注册 section / tool / 名牌，然后 [`api::run`]。
//!
//! 它**不依赖** `crates/kernel` / `crates/platform` / `crates/app` 任何 crate：扩展不进
//! 内核的依赖图，内核也不认识扩展的进程（spec Q7）。

pub mod api;
pub mod content;
pub mod frame;
pub mod manifest;

pub use api::{
    run, run_with_behavior, Behavior, Extension, Section, SectionInput, Tool, ToolCtx,
};
pub use content::{Content, ContentFile, Handles};
pub use frame::{read_frame, write_frame, FrameError, MAX_FRAME_BYTES, PROTOCOL};
pub use manifest::{Manifest, ManifestProblem, SectionDecl};
