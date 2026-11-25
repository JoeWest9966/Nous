//! `archive` 应用层模块（#67）：会话归档 —— 世代文件 + 墓碑 + 打捞。
//!
//! 三条端点（[`routes`]）自带 handler（Magento 2 那种模块自带 controller）：
//! 宿主只注入三条命令（[`routes::Host`]），插件不碰 `Storage`、不开连接、不看时钟。
//!
//! ## 为什么是「命令」而不是「库路径」
//!
//! 归档不是只读：它要在活库写锁下写 gen 文件、再提交墓碑，失败还要按方向补偿
//! （archive 失败删复印件；restore 失败**保留**复印件）。这套语义住在
//! `nous_durable::session::commands`，插件只声明「我要 archive / archives / restore 三件事」。
//!
//! ## 判据的接缝
//!
//! - 插件内：`routes.rs` 的 5 条（声明逐字 / 400 回显 / 409 不被改写 / item 形状 / body 校验）；
//! - 宿主级：`crates/Nous/Server/tests/archive_endpoints.rs`（真库 + 真 Router 往返）；
//! - 仓库级：`scripts/check-plugins.sh`（清单四处同源）、`meta::endpoints()`（注册表并集）。

pub mod routes;
