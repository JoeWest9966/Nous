//! 宿主的配置。**三个预算数、两个硬顶、一个并发上限全部从这里来**（spec 4.4 / 4.5）——
//! 清单无权声明（`E7`）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use nous_ext_sdk::manifest::SectionDecl;

/// 一个句柄的**值**：应用层配置里的一处目录（可选：只交付列出的条目）。
///
/// 值只在配置里；扩展拿到的只有内容（spec 6.1）。
#[derive(Debug, Clone)]
pub struct HandleValue {
    pub root: PathBuf,
    /// 只交付这些相对路径。给 `None` = 整目录（排序后交付）。
    /// 出现在这里的一条 `../../etc/passwd` 必须在**代读时**就拒（EX-60）。
    pub entries: Option<Vec<String>>,
}

impl HandleValue {
    pub fn dir(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entries: None,
        }
    }

    pub fn entries(root: impl Into<PathBuf>, entries: Vec<String>) -> Self {
        Self {
            root: root.into(),
            entries: Some(entries),
        }
    }
}

/// 预算与硬顶。
#[derive(Debug, Clone)]
pub struct Limits {
    /// 整轮所有扩展 section 装配合计（毫秒）。
    pub section_budget_ms: u64,
    /// 握手 / 重载的固定上限（毫秒）。
    pub handshake_timeout_ms: u64,
    /// 单帧上限（字节，双向各自计数）。
    pub frame_bytes: usize,
    /// 单次调用输出上限（字节）。撞顶 = 崩溃。
    pub output_bytes: usize,
    /// 每扩展 RSS 上限（字节）。
    pub rss_bytes: u64,
    /// 每扩展并发调用上限；超出排队并记一条诊断。
    pub concurrency: usize,
    /// tool 单次上限（毫秒）。`None` = 默认无限，靠取消。
    pub tool_timeout_ms: Option<u64>,
    /// 一份内容的体积上限（字节）。撞顶 `truncated=true`。
    pub handle_bytes: usize,
    /// 非崩失败连续几次进段熔断。
    pub failure_threshold: u32,
    /// 同一 `(扩展, 段, 错误类)` 的诊断去重窗口（毫秒）。
    pub diag_dedup_ms: u64,
    /// 同一扩展在窗口内多次写，合并成一次装载。
    pub reload_debounce_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            section_budget_ms: 1_000,
            handshake_timeout_ms: 10_000,
            frame_bytes: 1024 * 1024,
            output_bytes: 1024 * 1024,
            rss_bytes: 256 * 1024 * 1024,
            concurrency: 16,
            tool_timeout_ms: None,
            handle_bytes: 64 * 1024,
            failure_threshold: 3,
            diag_dedup_ms: 60_000,
            reload_debounce_ms: 200,
        }
    }
}

/// 隔离参数。`enabled=false` 只给单用户/容器环境用，且必须在交付里明写降级（#27）。
#[derive(Debug, Clone)]
pub struct Isolation {
    pub enabled: bool,
    /// 扩展 uid 从这里往上分（`60001` 起，无 passwd 条目）。
    pub base_uid: u32,
    /// spawn 时显式关掉除 0/1/2 与帧管道之外的全部 fd。
    pub close_fds: bool,
    /// `environ` 白名单（键集合相等，EX-46）。
    pub env_allow: Vec<(String, String)>,
}

impl Default for Isolation {
    fn default() -> Self {
        Self {
            enabled: true,
            base_uid: 60_001,
            close_fds: true,
            env_allow: Vec::new(),
        }
    }
}

/// 宿主的全部可配置面。
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// 扩展目录的根：下面一个目录一个扩展（`nous.json` 在里面）。
    pub exts_root: PathBuf,
    /// 产物目录：`<file>` 在这里。装载前校验哈希 / triple / protocol。
    pub artifacts_dir: PathBuf,
    /// 句柄名 → 值。全库只一处（spec 6.1）。
    pub handles: BTreeMap<String, HandleValue>,
    /// 运行时数据目录（诊断、重载计数这类**可丢**的东西）。
    pub data_dir: PathBuf,
    /// 审计提交落哪个库。`None` = 只跑内存（不落提交，仅判据的载体场景会用）。
    pub db: Option<PathBuf>,
    /// 宿主兼容的 target triple 列表（不是相等，EX-50）。
    pub host_triples: Vec<String>,
    pub limits: Limits,
    pub isolation: Isolation,
}

impl RuntimeConfig {
    pub fn new(exts_root: impl Into<PathBuf>, artifacts_dir: impl Into<PathBuf>) -> Self {
        Self {
            exts_root: exts_root.into(),
            artifacts_dir: artifacts_dir.into(),
            handles: BTreeMap::new(),
            data_dir: std::env::temp_dir(),
            db: None,
            host_triples: vec!["x86_64-unknown-linux-gnu".to_string()],
            limits: Limits::default(),
            isolation: Isolation::default(),
        }
    }
}

/// 一段 section 的声明（清单里的有序项）。重导出，省得每个模块都引 SDK。
pub type Section = SectionDecl;
