//! `wire.md` 的端点层：把 `/v1/*` 真正接上 HTTP 与 SSE。
//!
//! 权威是仓库根的 `wire.md`（宪法）。这一层**不做语义**，只做三件事：
//!
//! 1. 把 `nous_durable` 的投影结果（会话树 / 消息变更 / entry）搬成 HTTP 响应体；
//! 2. 把 `nous_wire` 的事件类型序列化成 SSE 帧；
//! 3. 合帧：16 ms 定时器读一次缓冲，不按事件逐条 flush（`wire.md` 的合帧铁律）。
//!
//! ## 契约面（AGENTS.md §二）
//!
//! `nous-server → durable, wire, telemetry`，且**只有这个 crate** 允许依赖 HTTP 框架
//! （axum）。`nous-durable` 永久不许出现 `axum|hyper|tower-http|actix*|warp` —— 语义层
//! 不绑传输，以后换 stdio / UDS 不改 durable。判据是 `scripts/verify.sh` 4e。
//!
//! 本 crate **不依赖 `nous-ai`**：生成走 `nous_durable::session::Session`，这一层不碰
//! provider，也不许在测试里出现 `FauxProvider`。
#![forbid(unsafe_code)]

pub mod app;
pub mod auth;
pub mod ext_admin;
pub mod clock;
pub mod coalesce;
pub mod commands;
pub mod combos;
pub mod combos_actions;
pub mod error;
pub mod hub;
pub mod meta;
pub mod records;
pub mod observer;
pub mod presets;
pub mod project;
pub mod routes;
pub mod repos;
pub mod runtime;
pub mod source;
pub mod stats;
pub mod triggers;
pub mod upstream;

pub use app::AppState;
pub use coalesce::{Framing, FRAME_INTERVAL};
pub use commands::{CommandSource, DurableCommands, ForkResult, SubmitResult};
pub use error::{Result, ServerError};
pub use hub::Hub;
pub use observer::SessionObserver;
pub use nous_device::ModelRef;
pub use records::{CreateSessionRequest, CreatedSession};
pub use routes::router;
pub use source::{ImportedSource, SearchItem, SessionSource};
pub mod version;
pub mod refusal;
pub mod workdir;
