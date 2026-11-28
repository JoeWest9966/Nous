//! 触发器插件：定义面（库里的行 + 点火账 + 调度健康）与工程面（盘上的 cargo 项目）。
//!
//! 搬家票（`docs/magento2-migration.md` §3.4）的地九站。搬家前后**行为一字未改**：
//! 六步点火、观测读口、发布闸、归档三代、类别词表都是原样搬过来的，只把
//! 「宿主怎么给它句柄」换成了 [`routes::Host`]。
//!
//! 分层的唯一判据是**谁拥有状态**：
//!
//! - [`runtime`]：点火与观测的领域层。不认 axum，只认 [`nous_routes::HttpRefusal`]。
//! - [`catalog`]：`/v1/triggers*` 11 个 handler（库面）。
//! - [`projects`]：`/v1/trigger-projects*` 15 个 handler（盘面）。
//! - [`routes`]：26 条声明 + 两条宿主能力。
//!
//! 为什么库面和盘面不能在同一个文件里、也不能合成一条宿主能力：它们操作的是**两种
//! 不同的东西** —— 一行定义和一棵源码目录。合成一个「触发器宿主」大对象，等于说
//! 「有点火库的人自然也拿到了 git 写锁」，那是假的。

pub mod catalog;
pub mod projects;
pub mod refusal;
pub mod routes;
pub mod runtime;

/// 搬出去的读口也要能直接调：CLI `nous-app --triggers` 与 `GET /v1/triggers`
/// 是**同一个函数、同一份序列化**（#58 判据 4），所以它在 crate 根上必须是公开的。
pub use runtime::{observation_report, FireOutcome, TemplateLookup, TriggerRuntime};
