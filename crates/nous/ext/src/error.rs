//! 失败形状。`Display` 出来的文本就是诊断原文 —— 判据用 `grep -F` 盯的就是这些字。
//!
//! 失败分三种（spec 4.0）：**明确错误** / **超时** / **崩溃**。它们走两条不同的阶梯，
//! 不许合并成"超时"。

use std::fmt;

/// 一次调用的失败类。段级阶梯只吃非崩的两种。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailureClass {
    /// 明确返回错误。
    Error,
    /// 超时 / 没回终帧。
    Timeout,
    /// 进程死 / 撞硬顶 / 帧超限。
    Crash,
    /// 被取消。**不是失败**，不进熔断。
    Cancelled,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureClass::Error => "error",
            FailureClass::Timeout => "timeout",
            FailureClass::Crash => "extension_crash",
            FailureClass::Cancelled => "user_cancel",
        }
    }

    /// 崩走进程级阶梯，其余（除取消）走段级阶梯。
    pub fn is_crash(self) -> bool {
        matches!(self, FailureClass::Crash)
    }
}

impl fmt::Display for FailureClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 装载 / 调用的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtError {
    /// 清单缺字段 / 字段非法。
    Manifest(Vec<nous_ext_sdk::manifest::ManifestProblem>),
    /// 清单里声明了句柄，而应用层配置里没有这个值（EX-02）。
    MissingHandle(String),
    /// 四件能力全空（EX-03）。
    EmptyExtension,
    /// 协议版本不符（EX-06）。旧进程不动。
    ProtocolMismatch { host: u32, ext: u32 },
    /// 哈希不符（EX-48）。
    HashMismatch { expected: String, actual: String },
    /// target triple 不在兼容列表里（EX-50）。
    TripleNotCompatible { triple: String, accepted: Vec<String> },
    /// 产物的来源被白名单拒了（EX-75，默认关）。
    ProvenanceRejected { built_from: String },
    /// 名牌冲突（EX-59）。后装的拒载，先装的照常工作。
    NameplateConflict {
        name: String,
        winner: String,
        loser: String,
    },
    /// 扩展进程起不来。
    Spawn(String),
    /// 扩展进程在握手期就没了。
    Handshake(String),
    /// 调用撞硬顶。
    HardLimit {
        class: FailureClass,
        detail: String,
    },
    /// 调用被取消。
    Cancelled(u64),
    /// 扩展没有这个能力。
    UnknownTool(String),
    /// 装载失败但旧的继续服务（EX-41）。
    ReloadFailed(String),
    /// 清单是 TS 时代（`qrd3wh`）JS 插件的形状（`index.js` + `defineExtension`）。
    ///
    /// 这不是「字段写错了」，是**装载形态换代**：新运行时只 spawn 原生进程、走协议
    /// 握手，`index.js` 没有解释器可跑。报成 `missing-field` 会把人引去改 JSON —— 那条路
    /// 是死的。所以单独一个变体，把话说到位。
    LegacyJsExtension,
}

impl fmt::Display for ExtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtError::Manifest(problems) => {
                let text: Vec<String> = problems.iter().map(|item| item.to_string()).collect();
                write!(f, "清单不合法：{}", text.join("；"))
            }
            ExtError::LegacyJsExtension => write!(
                f,
                "legacy-js: {}",
                nous_ext_sdk::manifest::LEGACY_JS_REASON
            ),
            ExtError::MissingHandle(name) => write!(f, "missing_handle={name}"),
            ExtError::EmptyExtension => {
                write!(f, "reason={}", nous_ext_sdk::manifest::EMPTY_REASON)
            }
            ExtError::ProtocolMismatch { host, ext } => {
                write!(f, "protocol_host={host} protocol_ext={ext}")
            }
            ExtError::HashMismatch { expected, actual } => {
                write!(f, "reason=哈希不符 expected={expected} actual={actual}")
            }
            ExtError::TripleNotCompatible { triple, accepted } => write!(
                f,
                "triple={triple} accepted={}",
                accepted.join(",")
            ),
            ExtError::ProvenanceRejected { built_from } => {
                write!(f, "reason=来源不在白名单 builtFrom={built_from}")
            }
            ExtError::NameplateConflict {
                name,
                winner,
                loser,
            } => write!(f, "conflict={name} winner={winner} loser={loser}"),
            ExtError::Spawn(message) => write!(f, "spawn: {message}"),
            ExtError::Handshake(message) => write!(f, "handshake: {message}"),
            ExtError::HardLimit { class, detail } => {
                write!(f, "reason={class} {detail}")
            }
            ExtError::Cancelled(call) => write!(f, "reason=user_cancel call={call}"),
            ExtError::UnknownTool(name) => write!(f, "reason=unknown tool {name}"),
            ExtError::ReloadFailed(message) => write!(f, "reason=重载失败 {message}"),
        }
    }
}

impl std::error::Error for ExtError {}

pub type ExtResult<T> = Result<T, ExtError>;
