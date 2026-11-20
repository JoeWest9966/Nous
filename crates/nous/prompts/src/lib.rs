//! #72 提示词插件：磁盘侧只读扫描 + 纯函数渲染。
//!
//! `render` 不碰盘、不碰网、不看时钟（`docs/plugins.md` 的契约）；
//! 盘上事实由 [`catalog`] 单独读，两者的接缝是一份 [`Template`] 值。

pub mod catalog;
pub mod render;

pub use catalog::{catalog_value, read_catalog, read_template, Catalog, PromptEntry, Template};
pub use render::render;
