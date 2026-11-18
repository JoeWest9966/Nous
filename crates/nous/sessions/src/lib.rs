#![forbid(unsafe_code)]
//! `nous-sessions` —— app 层的 pi 本体：一轮怎么跑、终态怎么落。
//!
//! 内核三件（契约 / 装载器 / 调度）在 `durable` / `chord` / `wire`；这里只**用**它们：
//! 轮次编排、工具循环、一次提交。宿主配置（provider、工具表）由组装根传进来。
pub mod clock;
pub mod turn;
