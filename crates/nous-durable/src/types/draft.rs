//! 三个「还没进库」的形状：草稿、待建、待结。
//!
//! 它们与 [`super::records`] 的区别就一句：**没有 id**（或者 id 由 session 盖章）。
//! session 交给 storage 的永远是盖过章的记录，这三个只在 session 内部和测试里出现。
//!
//! 取值域照上游，不比记录松：`EntryDraft.head` 允许字面量 `"self"`，`SubmissionSettlement`
//! 只有 `done` / `unanswered` 两个终态。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::records::{ContextEdit, SubmissionFields};
use crate::Id;

/// session 分配 id 与任务归属**之前**的 entry 内容。上游 `EntryDraft`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDraft {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// `"self"` 表示活动上下文从**刚分配的这条 entry** 开始。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<EntryHead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<ContextEdit>>,
}

/// `EntryDraft.head` 的取值域：一条已有 entry 的 id，或字面量 `"self"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryHead {
    Id(Id),
    /// 上游字面量 `"self"`：head 指向这条草稿落地后的 id。
    SelfRef,
}

impl Serialize for EntryHead {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            EntryHead::Id(id) => serializer.serialize_i64(*id),
            EntryHead::SelfRef => serializer.serialize_str("self"),
        }
    }
}

impl<'de> Deserialize<'de> for EntryHead {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::Number(number) => number
                .as_i64()
                .map(EntryHead::Id)
                .ok_or_else(|| serde::de::Error::custom("entry head id must be an integer")),
            Value::String(text) if text == "self" => Ok(EntryHead::SelfRef),
            other => Err(serde::de::Error::custom(format!(
                "entry head must be an id or \"self\", got {other}"
            ))),
        }
    }
}

/// session 分配 id **之前**的提交记录。上游 `Omit<SubmissionRecord, "id">`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubmissionCreate {
    #[serde(flatten)]
    pub fields: SubmissionFields,
}

/// 提交的终态。上游 `SubmissionSettlement`。
///
/// 身份、`type`、`entry` 都取自它**当前**的记录，这里只放要改成什么。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum SubmissionSettlement {
    /// 回答者是 `answer` 这条 entry。
    Done { answer: Id },
    /// 没答上，`reason` 必填（`detail` 可选）。
    #[serde(rename_all = "camelCase")]
    Unanswered {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Value>,
    },
}
