//! entry 的判别键：库里有哪些 kind、怎么认、怎么取 `data`。上游 `packages/durable/src/entries.ts`。
//!
//! 上游是 34 行的 `defineEntry` + 六个常量，`Entry<D>` 靠 TS 泛型把 `data` 的形状带在令牌上。
//! Rust 里把「形状」留在调用点：`EntryKind` 只认 `kind` 字符串，取载荷用
//! [`EntryKind::data`] 现给类型参数 —— 令牌不绑死 `D`，于是同一个常量既能读
//! `pi.tool-result` 的诊断，也能读将来的新载荷。
//!
//! **`pi.system` 的 message 不是 `nous_ai::Message`**：它带 `role: "system"`，而 `nous-ai` 是
//! `rpi-ai` 的冻结拷贝，只有 user / assistant / toolResult。`docs/pitfalls-from-pi.md` 有这条，
//! 机器守卫在 `tests/records_fixture.rs`。要 `model` 就按 `Value` 透传。

use serde::de::DeserializeOwned;

use crate::types::EntryRecord;

/// 一个 entry kind。上游 `Entry<D>`：`{kind, is()}`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryKind {
    kind: &'static str,
}

impl EntryKind {
    /// 造一个 kind。空字符串上游要 `TypeError`，这里同样在构造处拒。
    pub const fn new(kind: &'static str) -> Self {
        assert!(!kind.is_empty(), "entry kind must be a non-empty string");
        Self { kind }
    }

    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    /// 这条记录是不是这个 kind。上游 `Entry.is()`。
    pub fn is(&self, entry: &EntryRecord) -> bool {
        entry.kind == self.kind
    }

    /// 取 `data` 并按给定形状解析；没有 `data` 就是 `None`。上游 `TypedEntry<D>` 的 `data`。
    ///
    /// 类型由调用点给，不由令牌给：同一个 kind 在不同用途下要的字段不一样（`pi.tool-result`
    /// 的 `data.diagnostics` 只有渲染那一层关心）。
    pub fn data<D: DeserializeOwned>(&self, entry: &EntryRecord) -> Result<Option<D>, DataError> {
        let Some(value) = entry.data.as_ref() else {
            return Ok(None);
        };
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| self.error(entry, error.to_string()))
    }

    /// 取 `data` 并要求它在：`None` 也算错。上游 `TypedEntry<D>` 的 `data` 不是可选字段。
    pub fn require_data<D: DeserializeOwned>(&self, entry: &EntryRecord) -> Result<D, DataError> {
        match self.data(entry)? {
            Some(data) => Ok(data),
            None => Err(self.error(entry, "entry has no data".to_string())),
        }
    }

    fn error(&self, entry: &EntryRecord, message: String) -> DataError {
        DataError {
            kind: self.kind,
            entry_id: entry.id,
            message,
        }
    }
}

/// `data` 解析失败。上游靠泛型在编译期排除，Rust 里只能是运行时错误 —— 带上 kind 与 id，
/// 否则一条 209 行的转录里没人知道是哪一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataError {
    pub kind: &'static str,
    pub entry_id: i64,
    pub message: String,
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "entry {} of kind {}: {}",
            self.entry_id, self.kind, self.message
        )
    }
}

impl std::error::Error for DataError {}

/// 用户输入：`model` 是 `[UserMessage]`。由提交写入。上游 `UserEntry`。
pub const USER: EntryKind = EntryKind::new("pi.user");
/// 模型结果（任何 stop reason）：`model` 是 `[AssistantMessage]`。由生成写入。上游 `AssistantEntry`。
pub const ASSISTANT: EntryKind = EntryKind::new("pi.assistant");
/// 定位提示与工具变更：`model` 是 `[SystemMessage]`，`content` 为空。上游 `SystemEntry`。
///
/// 注意它的 message 带 `role: "system"`（外加 `toolsAdded`），不能用 `nous_ai::Message` 解析。
pub const SYSTEM: EntryKind = EntryKind::new("pi.system");
/// 工具结果：`model` 的 content 末尾是渲染后的诊断块，`data` 是结构化诊断（可能为空）。
/// 上游 `ToolResultEntry`。
pub const TOOL_RESULT: EntryKind = EntryKind::new("pi.tool-result");
/// 新上下文起点：`head` 恒为 `"self"`，`model` 缺席（纯重置）或带交接文本的 `[UserMessage]`。
/// 上游 `ResetEntry`。
pub const RESET: EntryKind = EntryKind::new("pi.reset");
/// 压缩摘要：`model` 是包着摘要的 `[UserMessage]`，`head` 是第一条保留的 entry。
/// 上游 `CompactionEntry`。
pub const COMPACTION: EntryKind = EntryKind::new("pi.compaction");

/// 库里认得的全部 kind。新增 kind 时**加进这个数组**，否则 `docs/storage.md` 的统计与
/// `tests/records_fixture.rs` 的分布断言会各说各话。
pub const ALL: [EntryKind; 6] = [USER, ASSISTANT, SYSTEM, TOOL_RESULT, RESET, COMPACTION];
