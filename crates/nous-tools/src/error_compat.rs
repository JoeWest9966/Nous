//! `crates/rpi-tools/src/error.rs` 的兼容出口：已经由 #6 抽成 `nous-env`，
//! 这里只把同名符号 re-export 回去，让下游 `use nous_tools::error_compat::…` 的上游路径
//! 仍然可以编译。零逻辑，零新类型（票 #7）。

pub use nous_env::{
    io_to_file_code, io_to_file_error, ExecutionError, ExecutionErrorCode, FileError, FileErrorCode,
};
