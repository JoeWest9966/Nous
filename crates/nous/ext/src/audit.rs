//! 审计：四种事件（装 / 卸 / 禁用 / 熔断）各落一条 `pi.extension` 提交。
//!
//! 三条规矩（spec 8.5 / 7.3）：
//!
//! - **审计是提交**，不加端点；可见性靠 `entries` 面按 `kind` 过滤；
//! - **幂等**：同一份清单重放 10 次只多一条；
//! - `result` 与 `mutations` 在**同一次提交**里（EX-20），同一组 mutation 重放只落一次（EX-21）。
//!
//! 诊断**不进提交**（A6）：这里只写状态转换。

use std::collections::BTreeSet;
use std::path::Path;

use nous_durable::{DocumentContent, EntryQuery, ScanOrder, Storage, StorageWrite};
use serde_json::{json, Value};

use crate::error::{ExtError, ExtResult};

/// 审计事件。字符串就是判据里的 `kinds=`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEvent {
    Install,
    Uninstall,
    Disable,
    Breaker,
}

impl AuditEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditEvent::Install => "install",
            AuditEvent::Uninstall => "uninstall",
            AuditEvent::Disable => "disable",
            AuditEvent::Breaker => "breaker",
        }
    }
}

/// 产物来源：哈希只证明"内容没变"，**不证明"谁做的"**（spec 8.1）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provenance {
    pub hash: String,
    pub built_from: String,
    pub build_machine: String,
}

impl Provenance {
    pub fn to_json(&self) -> Value {
        json!({
            "hash": self.hash,
            "builtFrom": self.built_from,
            "buildMachine": self.build_machine,
        })
    }
}

/// 一次工具提交的回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolReceipt {
    pub entry_id: i64,
    pub commit_seq: i64,
    pub replayed: bool,
}

/// 审计库。**一个连接、单写者**。
pub struct Audit {
    storage: Storage,
    /// 审计 entry 住在哪个会话里（应用层配置；v1 固定用 1）。
    pub conversation: i64,
    /// 已提交的工具调用（重放只落一次，EX-21）。
    committed_calls: BTreeSet<u64>,
}

impl Audit {
    /// 打开（或新建）审计库。
    pub fn open(path: impl AsRef<Path>) -> ExtResult<Self> {
        let storage = Storage::open(path.as_ref())
            .map_err(|error| ExtError::Spawn(format!("open db: {error}")))?;
        let mut audit = Self {
            storage,
            conversation: 1,
            committed_calls: BTreeSet::new(),
        };
        audit.ensure_conversation()?;
        Ok(audit)
    }

    fn ensure_conversation(&mut self) -> ExtResult<()> {
        if self.storage.conversation(self.conversation).map_err(storage)? .is_some() {
            return Ok(());
        }
        let record = json!({ "id": self.conversation });
        self.storage
            .commit(&[vec![StorageWrite::Conversation(record)]], |_, _| {})
            .map_err(storage)?;
        Ok(())
    }

    /// 确保一个会话存在（判据的场景会话）。
    pub fn ensure_session(&mut self, id: i64) -> ExtResult<()> {
        if self.storage.conversation(id).map_err(storage)?.is_some() {
            return Ok(());
        }
        let record = json!({ "id": id });
        self.storage
            .commit(&[vec![StorageWrite::Conversation(record)]], |_, _| {})
            .map_err(storage)?;
        Ok(())
    }

    /// 库本身（场景要读 commit_seq 这类东西时）。
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// 一条 entry 的 `commit_seq`。
    pub fn commit_seq(&self, entry_id: i64) -> ExtResult<i64> {
        self.storage
            .entry(entry_id)
            .map_err(storage)?
            .map(|(_, seq)| seq)
            .ok_or_else(|| ExtError::Spawn(format!("entry {entry_id} vanished")))
    }

    /// 这个 kind 的 entry 数量。
    pub fn count_kind(&self, conversation: i64, kind: &str) -> ExtResult<usize> {
        Ok(self
            .entries_of(conversation)?
            .into_iter()
            .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some(kind))
            .count())
    }

    /// 列出某会话的全部 entry（场景用；小库）。
    pub fn entries_of(&self, conversation: i64) -> ExtResult<Vec<Value>> {
        let query = EntryQuery {
            conversation_id: conversation,
            min_entry_id: None,
            max_entry_id: None,
            order: Some(ScanOrder::Ascending),
        };
        let page = self
            .storage
            .scan_entries(&query, 100_000, None)
            .map_err(storage)?;
        match page {
            Ok(page) => Ok(page.items),
            Err(refusal) => Err(ExtError::Spawn(format!("scan refused: {refusal:?}"))),
        }
    }

    /// 最近一次状态转换（用于重启后恢复：计数归零，状态仍在，EX-61）。
    pub fn latest_states(&self) -> ExtResult<Vec<(String, String)>> {
        let mut latest: std::collections::BTreeMap<String, String> = Default::default();
        for entry in self.entries_of(self.conversation)? {
            if entry.get("kind").and_then(Value::as_str) != Some("pi.extension") {
                continue;
            }
            let Some(data) = entry.get("data") else { continue };
            let (Some(name), Some(state)) = (
                data.get("name").and_then(Value::as_str),
                data.get("state").and_then(Value::as_str),
            ) else {
                continue;
            };
            latest.insert(name.to_string(), state.to_string());
        }
        Ok(latest.into_iter().collect())
    }

    /// 落一条 `pi.extension` 提交。幂等键 = `(event, name, manifestHash)`。
    ///
    /// 返回 `(entry_id, commit_seq)`。重复事件返回既有那一条的 id/seq（`replayed=true`）。
    pub fn commit_event(
        &mut self,
        event: AuditEvent,
        name: &str,
        state: &str,
        manifest_hash: &str,
        handles: &[String],
        provenance: Option<&Provenance>,
    ) -> ExtResult<(i64, i64)> {
        // 幂等：同 (event, name, hash) 已在库里 → 不写。
        for entry in self.entries_of(self.conversation)? {
            if entry.get("kind").and_then(Value::as_str) != Some("pi.extension") {
                continue;
            }
            let Some(data) = entry.get("data") else { continue };
            let matches = data.get("event").and_then(Value::as_str) == Some(event.as_str())
                && data.get("name").and_then(Value::as_str) == Some(name)
                && data.get("manifestHash").and_then(Value::as_str) == Some(manifest_hash);
            if matches {
                let id = entry.get("id").and_then(Value::as_i64).unwrap_or(0);
                return Ok((id, self.commit_seq(id)?));
            }
        }
        let mut data = json!({
            "event": event.as_str(),
            "name": name,
            "state": state,
            "manifestHash": manifest_hash,
            "handles": handles,
        });
        if let Some(provenance) = provenance {
            data["provenance"] = provenance.to_json();
        }
        let id = self.storage.mint_id().map_err(storage)?;
        let record = json!({
            "id": id,
            "conversationId": self.conversation,
            "kind": "pi.extension",
            "data": data,
        });
        self.storage
            .commit(&[vec![StorageWrite::Entry(record)]], |_, _| {})
            .map_err(storage)?;
        let seq = self.commit_seq(id)?;
        Ok((id, seq))
    }

    /// 写一段系统提示词，**只在内容变了的时候**才提交（同轮两次 = 零增量，EX-14）。
    ///
    /// 返回 `(entry_id, committed)`。
    pub fn ensure_system(&mut self, conversation: i64, generation: i64, text: &str) -> ExtResult<(i64, bool)> {
        self.ensure_session(conversation)?;
        let entries = self.entries_of(conversation)?;
        if let Some(last) = entries
            .iter()
            .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("pi.system"))
            .next_back()
        {
            let same = last
                .get("data")
                .and_then(|data| data.get("text"))
                .and_then(Value::as_str)
                == Some(text);
            if same {
                return Ok((last.get("id").and_then(Value::as_i64).unwrap_or(0), false));
            }
        }
        let id = self.storage.mint_id().map_err(storage)?;
        let record = json!({
            "id": id,
            "conversationId": conversation,
            "kind": "pi.system",
            "data": { "text": text, "generation": generation },
        });
        self.storage
            .commit(&[vec![StorageWrite::Entry(record)]], |_, _| {})
            .map_err(storage)?;
        Ok((id, true))
    }

    /// 段熔断：写**一次**删除增量（仅该段），幂等。
    ///
    /// 返回是否真的写了（`delta=1` 只在第一次）。
    pub fn commit_section_removal(&mut self, extension: &str, segment: &str) -> ExtResult<bool> {
        let kind = "pi.extension.sections";
        let exists = self.entries_of(self.conversation)?.iter().any(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some(kind)
                && entry
                    .get("data")
                    .and_then(|data| data.get("extension"))
                    .and_then(Value::as_str)
                    == Some(extension)
                && entry
                    .get("data")
                    .and_then(|data| data.get("segment"))
                    .and_then(Value::as_str)
                    == Some(segment)
        });
        if exists {
            return Ok(false);
        }
        let id = self.storage.mint_id().map_err(storage)?;
        let record = json!({
            "id": id,
            "conversationId": self.conversation,
            "kind": kind,
            "data": { "extension": extension, "segment": segment, "removed": true },
        });
        self.storage
            .commit(&[vec![StorageWrite::Entry(record)]], |_, _| {})
            .map_err(storage)?;
        Ok(true)
    }

    /// 一条普通 entry（用户 / 助手 / 工具结果），给场景铺底。
    pub fn commit_entry(&mut self, conversation: i64, kind: &str, data: Value) -> ExtResult<i64> {
        self.ensure_session(conversation)?;
        let id = self.storage.mint_id().map_err(storage)?;
        let record = json!({
            "id": id,
            "conversationId": conversation,
            "kind": kind,
            "data": data,
        });
        self.storage
            .commit(&[vec![StorageWrite::Entry(record)]], |_, _| {})
            .map_err(storage)?;
        Ok(id)
    }

    /// `result` 与 `mutations` 的**同一次提交**（EX-20）。
    ///
    /// `mutations` 只认两种原语：`entry`（追加 entry）/ `document`（写文档）。别的形状是
    /// **明确错误**，调用方要把它变成一条 error result 回给模型（EX-22），不许静默丢弃。
    pub fn commit_tool(
        &mut self,
        conversation: i64,
        call_id: u64,
        text: &str,
        mutations: &[Value],
    ) -> ExtResult<ToolReceipt> {
        if !self.committed_calls.insert(call_id) {
            // 同一组重放：不写第二次（EX-21）。回执读既有那条 result。
            let existing = self
                .entries_of(conversation)?
                .into_iter()
                .filter(|entry| {
                    entry.get("kind").and_then(Value::as_str) == Some("pi.tool-result")
                        && entry
                            .get("data")
                            .and_then(|data| data.get("call"))
                            .and_then(Value::as_u64)
                            == Some(call_id)
                })
                .next_back();
            if let Some(entry) = existing {
                let id = entry.get("id").and_then(Value::as_i64).unwrap_or(0);
                return Ok(ToolReceipt {
                    entry_id: id,
                    commit_seq: self.commit_seq(id)?,
                    replayed: true,
                });
            }
        }

        self.ensure_session(conversation)?;
        let mut writes = Vec::new();
        let result_id = self.storage.mint_id().map_err(storage)?;
        writes.push(StorageWrite::Entry(json!({
            "id": result_id,
            "conversationId": conversation,
            "kind": "pi.tool-result",
            "data": { "text": text, "call": call_id, "mutations": mutations },
        })));

        for mutation in mutations {
            match mutation.get("kind").and_then(Value::as_str) {
                Some("entry") => {
                    let template = mutation.get("entry").cloned().unwrap_or_else(|| json!({}));
        let id = self.storage.mint_id().map_err(storage)?;
                    let mut record = template;
                    record["id"] = json!(id);
                    record["conversationId"] = json!(conversation);
                    writes.push(StorageWrite::Entry(record));
                }
                Some("document") => {
                    let template = mutation.get("document").cloned().unwrap_or_else(|| json!({}));
        let id = self.storage.mint_id().map_err(storage)?;
                    let key = template
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or("document")
                        .to_string();
                    let value = template.get("value").cloned().unwrap_or_else(|| json!({}));
                    let record = json!({
                        "id": id,
                        "kind": format!("pi.extension.{key}"),
                        "scope": { "kind": "conversation", "conversationId": conversation },
                        "history": "latest",
                        "fork": "current",
                    });
                    writes.push(StorageWrite::DocumentCreate {
                        id,
                        record,
                        content: DocumentContent::Base { version: 1, value },
                    });
                }
                other => {
                    return Err(ExtError::HardLimit {
                        class: crate::error::FailureClass::Error,
                        detail: format!(
                            "reason=mutations 必须有 kind（收到 {:?}，只认 entry / document）",
                            other.unwrap_or("<missing>")
                        ),
                    });
                }
            }
        }

        self.storage.commit(&[writes], |_, _| {}).map_err(storage)?;
        Ok(ToolReceipt {
            entry_id: result_id,
            commit_seq: self.commit_seq(result_id)?,
            replayed: false,
        })
    }
}

fn storage(error: nous_durable::StorageError) -> ExtError {
    ExtError::Spawn(format!("storage: {error}"))
}
