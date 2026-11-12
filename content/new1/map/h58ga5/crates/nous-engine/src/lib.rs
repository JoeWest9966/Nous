//! nous-engine — qrd3wh `/v1/*` 的哑客户端。
//!
//! 三层：
//! - [`types`]：引擎回的形状（未知字段透传）。
//! - [`sse`]：SSE 帧解析。
//! - [`client`]：HTTP 转发（qrd3wh：会话/历史/流）。
//! - [`pins`]：置顶的写穿客户端（bjes6b：`pins.json` 是单源）。
//!
//! 不认业务规则，不碰 GPUI。UI 层要 16ms 合帧，那归 UI 层。

pub mod client;
pub mod pins;
pub mod resolve;
pub mod sse;
pub mod types;

pub use client::{EngineClient, EngineError};
pub use pins::{PinClient, PinError};
pub use sse::{Change, SseError, SseEvent};
