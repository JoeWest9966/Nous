//! 文档在某个点上的值：base + 其后的 delta 重放一次（上游 `SqliteStorage.materializeDocument`
//! 与 chord 的 `loadContent`）。
//!
//! ## 为什么不是「读最新 revision 就行」
//!
//! `document_revisions` 里存的是 **base 的整份值** 与 **delta 的操作数组**（`docs/storage.md` §4）。
//! 一条 delta 相对的是**它上面最近的 base**，不是上一条 revision。所以一次读取是：
//!
//! 1. 找 `seq <= 点` 的最新 base；
//! 2. 取 `base.seq < seq <= 点` 的所有 revision，按 seq 升序；
//! 3. 每条必须是 `delta` 且 `version == base.version` —— 否则那是一次跨版本的接续，
//!    按规矩必须有新 base 兜着（[`StorageError::VersionCrossing`]）；
//! 4. 用 [`nous_chord::delta`] 逐条 apply。
//!
//! ## 点与在世
//!
//! [`DocumentPoint::Current`] 看最新状态；[`DocumentPoint::At`] 看某个 seq 上的状态。
//! 只留最新态的文档（`is_current_only`）在非 current 的点上**明确失败**，不悄悄给最新值 ——
//! 给了就等于伪造历史。

use serde_json::Value;

use nous_chord::delta::apply::{apply_immutable_trusted, TrustedOps};
use nous_chord::delta::codec::Decoder;
use nous_chord::sqlite::{SqliteExecutor, SqliteValue as Cell};

use super::sqlite::rows::{is_current_only, parse_json};
use super::sqlite::schema::SqliteSchema;
use super::writes::{Id, Seq};
use crate::error::{Result, StorageError};

/// 读文档的哪个点。上游 `DocumentPoint = Seq | "current"`。
///
/// 序列化形状就是上游那个联合：`At(seq)` → `seq`，`Current` → `"current"`（字符串，不是
/// `{"kind":"current"}`）。`CommitPublication` 里的 `document.copy` 带着它过线，所以必须在这
/// 一层就对得上，不能在拼 publication 的地方临时凑一个。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentPoint {
    /// 最新状态。
    Current,
    /// 某个 seq 上的状态（含那个 seq 的 revision）。
    At(Seq),
}

impl serde::Serialize for DocumentPoint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            DocumentPoint::Current => serializer.serialize_str("current"),
            DocumentPoint::At(seq) => serializer.serialize_i64(*seq),
        }
    }
}

impl<'de> serde::Deserialize<'de> for DocumentPoint {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        match <serde_json::Value as serde::Deserialize>::deserialize(deserializer)? {
            serde_json::Value::Number(number) => number
                .as_i64()
                .map(DocumentPoint::At)
                .ok_or_else(|| serde::de::Error::custom("document point seq must be an integer")),
            serde_json::Value::String(text) if text == "current" => Ok(DocumentPoint::Current),
            other => Err(serde::de::Error::custom(format!(
                "document point must be a seq or \"current\", got {other}"
            ))),
        }
    }
}

impl DocumentPoint {
    /// 查询用的上界：`At(seq)` 就是 `seq`，`Current` 是「没有上界」。
    fn upper(self) -> i64 {
        match self {
            DocumentPoint::Current => i64::MAX,
            DocumentPoint::At(seq) => seq,
        }
    }
}

/// 一份文档在某个点上的值。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredDocument {
    /// 文档记录本身（`documents.record` 那一列）。
    pub record: Value,
    /// 值来自哪个 base 的版本。
    pub version: i64,
    /// 重放后的整份值。
    pub value: Value,
    /// 这个 base 之后已经叠了几条 delta（读的人靠它决定「该写新的 base 了吗」）。
    pub deltas_since_base: usize,
}

/// 文档在 `at` 上是否在世（左闭右开的生命周期）。上游 `isAliveAt`。
pub fn alive_at(record: &Value, at: DocumentPoint) -> bool {
    let created = record.get("createdAt").and_then(Value::as_i64);
    let retired = record.get("retiredAt").filter(|value| !value.is_null()).and_then(Value::as_i64);
    match at {
        DocumentPoint::Current => retired.is_none(),
        DocumentPoint::At(seq) => {
            let Some(created) = created else { return false };
            created <= seq && retired.map(|retired| seq < retired).unwrap_or(true)
        }
    }
}

/// 读 `id` 在 `at` 上的值：不存在 / 不在世给 `Ok(None)`，形状不对给 `Err`。
pub(crate) fn materialize(
    executor: &dyn SqliteExecutor,
    schema: &SqliteSchema,
    id: Id,
    at: DocumentPoint,
) -> Result<Option<StoredDocument>> {
    let Some(row) = executor.get(
        &format!("SELECT record FROM {} WHERE id = ?", schema.table("documents")),
        &[Cell::Integer(id)],
    )?
    else {
        return Ok(None);
    };
    let record = parse_json(
        row.get("record")
            .and_then(Cell::as_str)
            .ok_or_else(|| StorageError::Shape("document row has no record".to_string()))?,
    )?;
    // 只留最新态的文档没有历史可读：给最新的值就是伪造。
    if at != DocumentPoint::Current && is_current_only(&record) {
        return Err(StorageError::HistoryNotRetained { document_id: id });
    }
    if !alive_at(&record, at) {
        return Ok(None);
    }
    let upper = at.upper();
    let base = executor
        .get(
            &format!(
                "SELECT seq, version, content FROM {} WHERE document_id = ? AND kind = 'base' AND seq <= ?
                 ORDER BY seq DESC LIMIT 1",
                schema.table("document_revisions")
            ),
            &[Cell::Integer(id), Cell::Integer(upper)],
        )?
        .ok_or(StorageError::MissingBase { document_id: id })?;
    let base_seq = base
        .get("seq")
        .and_then(Cell::as_i64)
        .ok_or_else(|| StorageError::Shape("revision has no seq".to_string()))?;
    let version = base
        .get("version")
        .and_then(Cell::as_i64)
        .ok_or_else(|| StorageError::Shape("revision has no version".to_string()))?;
    let mut value = parse_json(
        base.get("content")
            .and_then(Cell::as_str)
            .ok_or_else(|| StorageError::Shape("revision has no content".to_string()))?,
    )?;
    let tail = executor.all(
        &format!(
            "SELECT seq, kind, version, content FROM {} WHERE document_id = ? AND seq > ? AND seq <= ?
             ORDER BY seq",
            schema.table("document_revisions")
        ),
        &[Cell::Integer(id), Cell::Integer(base_seq), Cell::Integer(upper)],
    )?;
    for revision in &tail {
        let kind = revision.get("kind").and_then(Cell::as_str).unwrap_or("");
        let revision_version = revision.get("version").and_then(Cell::as_i64).unwrap_or(-1);
        if kind != "delta" || revision_version != version {
            // 版本中间没有 base 兜着：说明这份库被写坏了，或者我们的写入漏了 base。
            return Err(StorageError::VersionCrossing { document_id: id });
        }
        let ops: Value = parse_json(
            revision
                .get("content")
                .and_then(Cell::as_str)
                .ok_or_else(|| StorageError::Shape("revision has no content".to_string()))?,
        )?;
        let wire = ops.as_array().ok_or_else(|| StorageError::DeltaDecode {
            document_id: id,
            message: "a delta revision is not an array of ops".to_string(),
        })?;
        let decoded = Decoder::new().decode(wire).map_err(|error| StorageError::DeltaDecode {
            document_id: id,
            message: error.to_string(),
        })?;
        let trusted = TrustedOps::from_decoded(decoded).map_err(|error| StorageError::DeltaDecode {
            document_id: id,
            message: error.to_string(),
        })?;
        value = apply_immutable_trusted(&value, &trusted).map_err(|error| StorageError::DeltaDecode {
            document_id: id,
            message: error.to_string(),
        })?;
    }
    Ok(Some(StoredDocument {
        record,
        version,
        value,
        deltas_since_base: tail.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_lifetime_is_half_open_and_current_ignores_created_at() {
        let live = json!({ "id": 1, "createdAt": 10 });
        assert!(alive_at(&live, DocumentPoint::Current));
        assert!(alive_at(&live, DocumentPoint::At(10)));
        assert!(!alive_at(&live, DocumentPoint::At(9)));

        let retired = json!({ "id": 1, "createdAt": 10, "retiredAt": 20 });
        assert!(!alive_at(&retired, DocumentPoint::Current));
        assert!(alive_at(&retired, DocumentPoint::At(10)));
        assert!(alive_at(&retired, DocumentPoint::At(19)));
        // 左闭右开：退役那个 seq 上已经不在世了。
        assert!(!alive_at(&retired, DocumentPoint::At(20)));
    }
}
