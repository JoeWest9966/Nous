#![forbid(unsafe_code)]

//! `sessions` 模块：会话的读投影（[`source`]）、写命令（[`commands`]）、
//! `POST /v1/sessions` 的形状（[`records`]）与三者共用的错误（[`error`]）。
//!
//! 端点（`/v1/sessions*` 那 17 条）住在 [`routes`]（Magento 2「模块自带 controller」）；
//! 宿主只实现 [`routes::Host`]。这三层先于端点搬进来 —— 它们本来就是会话的语义，
//! 留在 `server` 里会让「会话是谁的」这个问题没有答案。

pub mod commands;
pub mod error;
pub mod records;
pub mod source;
pub mod turn;

pub use error::{Result, SessionError};
pub use records::{CreateSessionRequest, CreatedSession};
pub use source::SessionSource;
