//! #72 提示词插件：磁盘侧只读扫描 + 纯函数渲染 + 7 条 `/v1/prompts*` 端点。
//!
//! 三块：
//!
//! - **解析**（[`catalog`]）：只读扫描 `PROMPT.md` / `nous.json` / `settings.json`。
//! - **渲染**（[`render`]）：纯函数，不碰盘、不碰网、不看时钟。
//! - **端点**（[`routes`]）：`GET /v1/prompts` 是上面那层投影；另外五条是编辑页的
//!   管理面，盘面规矩在 `nous_repo::entries`，这里只做 HTTP 翻译。

pub mod catalog;
pub mod render;
pub mod routes;

pub use catalog::{catalog_value, read_catalog, read_template, Catalog, PromptEntry, Template};
pub use render::render;
