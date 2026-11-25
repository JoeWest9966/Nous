//! #78：`ExecutionEnv` 的 SSH 远端实现。
//!
//! 契约面在 `nous-env`：`FileSystem` 必需 14 个方法 + `Shell` 两个方法；这里只做
//! 「把每次调用翻成一条 ssh 命令」的适配，不在本层监听、不写内核库、不进日志。
//!
//! 凭据只从环境/文件读，机器参数从编目读。私钥在内存里过一手，写进 0700 临时目录的
//! 0600 文件，交给 ssh `-i`；`cleanup` 与进程退出各删一次。
//!
//! ControlMaster 默认开（0700 临时目录里的 socket，按 user/addr/port 分开）；
//! `NOUS_SSH_MUX=0` 可关。连不上是显式错误（10 秒 `ConnectTimeout`），不是静默退回本机。

pub mod config;
pub mod dirs;
pub mod env;
pub mod key;
pub mod mux;
pub mod remote;

pub use config::{machine, Machine, MachineError, MACHINES_FILE_ENV};
pub use dirs::{remote_subdirs, RemoteDirsError};
pub use env::SshExecutionEnv;
pub use key::TemporaryKey;
pub use mux::Mux;
