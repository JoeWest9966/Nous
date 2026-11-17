//! 五个状态 + 诊断计数（spec 4.1 / 4.2）。
//!
//! **状态在提交、计数在内存**：连续失败计数、上次诊断时间都能丢；状态转换不能丢。
//!
//! 诊断本身去重：同一 `(扩展, 段, 错误类)` 在窗口内只留一条**日志**，但**去重不去计数**
//! （3 次崩必须数得出来，EX-31）。

use std::collections::BTreeMap;

/// 一个扩展的状态。只有五个，不是每个错误一个状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExtState {
    Normal,
    /// 该段失败过（保留 `shown`，零增量），继续试。
    Degraded,
    /// 该段熔断：该段不出现，其余段照常。
    TrippedSection,
    /// 进程熔断：该扩展 0 次调用。
    TrippedProcess,
    /// 人在配置里禁用（**不是失败**，不进熔断）。
    Disabled,
}

impl ExtState {
    pub fn as_str(self) -> &'static str {
        match self {
            ExtState::Normal => "normal",
            ExtState::Degraded => "degraded",
            ExtState::TrippedSection => "tripped_section",
            ExtState::TrippedProcess => "tripped_process",
            ExtState::Disabled => "disabled",
        }
    }

    /// 该扩展现在还能不能被调用。
    pub fn callable(self) -> bool {
        !matches!(self, ExtState::TrippedProcess | ExtState::Disabled)
    }

    /// 从提交里读回来的字符串。
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "normal" => ExtState::Normal,
            "degraded" => ExtState::Degraded,
            "tripped_section" => ExtState::TrippedSection,
            "tripped_process" => ExtState::TrippedProcess,
            "disabled" => ExtState::Disabled,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ExtState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一条诊断（日志里的一行）。内存里的观测，不是真相。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub extension: String,
    pub segment: String,
    pub class: String,
    pub message: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagCount {
    pub count: u32,
    pub last_logged_ms: u64,
}

/// 诊断表。`clock` 由调用方给（内核层零直接时钟，这条同样适用于应用层的可测部分）。
#[derive(Debug, Default)]
pub struct Diagnostics {
    counts: BTreeMap<(String, String, String), DiagCount>,
    log: Vec<Diagnostic>,
    dedup_ms: u64,
}

impl Diagnostics {
    pub fn new(dedup_ms: u64) -> Self {
        Self {
            counts: BTreeMap::new(),
            log: Vec::new(),
            dedup_ms,
        }
    }

    /// 记一次失败。返回这条是否真的写进了日志（false = 被去重）。
    pub fn record(
        &mut self,
        extension: &str,
        segment: &str,
        class: &str,
        message: &str,
        now_ms: u64,
    ) -> bool {
        let key = (extension.to_string(), segment.to_string(), class.to_string());
        let entry = self.counts.entry(key).or_insert(DiagCount {
            count: 0,
            last_logged_ms: 0,
        });
        entry.count += 1;
        let deduped = now_ms.saturating_sub(entry.last_logged_ms) < self.dedup_ms
            && entry.last_logged_ms != 0;
        if deduped {
            return false;
        }
        entry.last_logged_ms = now_ms;
        self.log.push(Diagnostic {
            extension: extension.to_string(),
            segment: segment.to_string(),
            class: class.to_string(),
            message: message.to_string(),
            at_ms: now_ms,
        });
        true
    }

    /// 连续失败计数（内存的；重启归零）。
    pub fn count(&self, extension: &str, segment: &str, class: &str) -> u32 {
        self.counts
            .get(&(extension.to_string(), segment.to_string(), class.to_string()))
            .map(|entry| entry.count)
            .unwrap_or(0)
    }

    /// 这个扩展所有计数之和（判据里的 `memory_count`）。
    pub fn memory_count(&self, extension: &str) -> u32 {
        self.counts
            .iter()
            .filter(|((ext, _, _), _)| ext == extension)
            .map(|(_, count)| count.count)
            .sum()
    }

    /// 某个扩展的全部（计数, 段, 错误类）。
    pub fn counts_for(&self, extension: &str) -> Vec<(u32, String, String)> {
        self.counts
            .iter()
            .filter(|((ext, _, _), _)| ext == extension)
            .map(|((_, segment, class), count)| (count.count, segment.clone(), class.clone()))
            .collect()
    }

    /// 全部日志行数（`log_lines`）。
    pub fn log_lines(&self) -> usize {
        self.log.len()
    }

    pub fn log(&self) -> &[Diagnostic] {
        &self.log
    }

    /// 该段是否已经达到段熔断阈值。
    pub fn reached(&self, extension: &str, segment: &str, threshold: u32) -> bool {
        self.counts
            .iter()
            .filter(|((ext, seg, class), _)| {
                ext == extension && seg == segment && class != "user_cancel"
            })
            .any(|(_, count)| count.count >= threshold)
    }

    /// 重载成功：清空全部计数（只有重载清全部）。
    pub fn clear(&mut self) {
        self.counts.clear();
    }

    /// 进程重启：只清「进程熔断」相关的那条，段级计数**不清**。
    pub fn clear_process(&mut self, extension: &str) {
        self.counts
            .retain(|(ext, _, class), _| !(ext == extension && class == "extension_crash"));
    }
}
