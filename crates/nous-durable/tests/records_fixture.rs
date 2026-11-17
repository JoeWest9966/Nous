//! #13 的第一条判据：**每个类型都要能从真 fixture 里读出来**。
//!
//! 只测自造值一定漏字段 —— 自造值只能证明「我们写的能读回来」，证明不了「上游写的能读进来」。
//! 这里读的是 `fixtures/agent.sqlite`（1.18 MB，真引擎写的）：
//! 189 conversations / 209 entries / 128 tasks / 56 submissions / 961 documents。
//!
//! 两条断言，缺一不可：
//!
//! 1. **反序列化不报错** —— 形状对得上（少一个必填字段就红）。
//! 2. **序列化回去与源 JSON 逐字段相等** —— 名字对得上（多写或者写错一个键就红，丢字段也红）。
//!
//! fixture 不在时整组跳过并打印原因，**不假绿**。

use std::path::{Path, PathBuf};

use nous_durable::storage::import::ImportedStore;
use nous_durable::types::{
    ConversationRecord, DocumentFork, DocumentHistory, DocumentRecord, DocumentScope, EntryRecord,
    SubmissionFields, SubmissionRecord, SubmissionStatus, SubmissionType, TaskRecord,
};
use nous_chord::sqlite::{SqliteRow, SqliteValue as Cell};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

fn fixture_path() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agent.sqlite");
    path.exists().then_some(path)
}

fn record_text(row: &SqliteRow) -> String {
    match row.get("record") {
        Some(Cell::Text(text)) => text.clone(),
        other => panic!("record 列不是文本：{other:?}"),
    }
}

/// 把一行 `record` 解析成 `T`，再序列化回去，与源 JSON 逐字段比较。
fn round_trip<T: DeserializeOwned + Serialize>(row: &SqliteRow, table: &str) -> T {
    let text = record_text(row);
    let source: Value = serde_json::from_str(&text).expect("fixture 的 record 不是合法 JSON");
    let typed: T = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("{table} 的一条记录读不进类型：{error}\n{text}");
    });
    let written = serde_json::to_value(&typed).expect("记录总能序列化");
    assert_eq!(
        written, source,
        "{table} 往返后与源 JSON 不等（丢字段 / 多字段 / 名字错了）"
    );
    typed
}

#[test]
fn reads_every_conversation_record_from_the_fixture() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    assert_eq!(store.conversations.len(), 189);
    let mut with_parent = 0;
    for row in &store.conversations {
        let record: ConversationRecord = round_trip(row, "conversations");
        if let Some(parent) = record.parent {
            with_parent += 1;
            assert!(parent.conversation_id > 0 && parent.at > 0);
        }
        assert!(record.id > 0);
    }
    assert_eq!(with_parent, 8, "fixture 里 8 条会话有 parent");
}

#[test]
fn reads_every_entry_record_from_the_fixture() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    assert_eq!(store.entries.len(), 209);
    let mut kinds = std::collections::BTreeMap::new();
    for row in &store.entries {
        let record: EntryRecord = round_trip(row, "entries");
        *kinds.entry(record.kind.clone()).or_insert(0usize) += 1;
        assert!(record.conversation_id > 0);
    }
    // 四种 kind 都在真库里出现过，一个不少。
    assert_eq!(kinds.get("pi.assistant"), Some(&100));
    assert_eq!(kinds.get("pi.system"), Some(&17));
    assert_eq!(kinds.get("pi.tool-result"), Some(&36));
    assert_eq!(kinds.get("pi.user"), Some(&56));
}

#[test]
fn reads_every_task_record_from_the_fixture() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    assert_eq!(store.tasks.len(), 128);
    for row in &store.tasks {
        let record: TaskRecord = round_trip(row, "tasks");
        assert!(record.version >= 1);
        assert!(record.state.is_object(), "state 是不透明 JSON，但必须是个对象");
    }
}

#[test]
fn reads_every_submission_record_from_the_fixture() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    assert_eq!(store.submissions.len(), 56);
    let mut seen = std::collections::BTreeSet::new();
    for row in &store.submissions {
        let record: SubmissionRecord = round_trip(row, "submissions");
        seen.insert((record.fields.kind, record.fields.status));
    }
    // 真库里只出现这两种组合 —— 矩阵的其余格子由 records_domains.rs 用自造值覆盖。
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        vec![
            (SubmissionType::Input, SubmissionStatus::Done),
            (SubmissionType::Input, SubmissionStatus::Unanswered),
        ]
    );
}

#[test]
fn reads_every_document_record_from_the_fixture() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    assert_eq!(store.documents.len(), 961);
    let mut seen = std::collections::BTreeMap::new();
    for row in &store.documents {
        let record: DocumentRecord = round_trip(row, "documents");
        let key = match (&record.scope, record.history, record.fork) {
            (DocumentScope::Conversation { .. }, Some(DocumentHistory::Latest), Some(DocumentFork::Initial)) => {
                "conversation/latest/initial"
            }
            (DocumentScope::Conversation { .. }, Some(DocumentHistory::Rewindable), Some(DocumentFork::AsOf)) => {
                "conversation/rewindable/asOf"
            }
            (DocumentScope::Session, None, None) => "session/-/-",
            other => panic!("fixture 里出现了没预料到的 scope/history/fork 组合：{other:?}"),
        };
        *seen.entry(key).or_insert(0usize) += 1;
    }
    assert_eq!(seen.get("conversation/latest/initial"), Some(&768));
    assert_eq!(seen.get("conversation/rewindable/asOf"), Some(&192));
    assert_eq!(seen.get("session/-/-"), Some(&1));
}

/// `SubmissionFields` 单独用（`SubmissionCreate` 复用它）时，也要能读 fixture 的真记录。
#[test]
fn reads_submission_fields_without_the_id() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    for row in &store.submissions {
        let source: Value = serde_json::from_str(&record_text(row)).unwrap();
        let mut without_id = source.clone();
        without_id.as_object_mut().unwrap().remove("id").expect("每条都有 id");
        let fields: SubmissionFields = serde_json::from_value(without_id.clone())
            .expect("去掉 id 之后仍然是合法的 SubmissionFields");
        assert_eq!(serde_json::to_value(&fields).unwrap(), without_id);
    }
}

/// 陷阱守卫（`docs/pitfalls-from-pi.md`「`nous_ai::Message` 比引擎存的数据窄」）。
///
/// 这两条断言**故意断言「解析不了 / 会丢键」**：`nous-ai` 是 `rpi-ai@v0.24.2` 的冻结拷贝，
/// 它的 `Message` 只有 `user` / `assistant` / `toolResult`，而 `pi` 引擎往同一张表里还写
/// `pi.system`；`pi` 的 assistant / tool-result 还带 `durationMs`。所以**任何**「把库里存的
/// `model` 解析成 `nous_ai::Message`」的地方都会失败或静默丢字段。
///
/// 哪天 `nous-ai` 补上了 system 变体和未知键兜底，这两条会红 —— 那时是**好事**：说明坑填了，
/// 该删的是这两条断言和那条 pitfall，而不是改断言让它继续绿。
#[test]
fn nous_ai_message_is_narrower_than_what_the_engine_stored() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");

    let mut system_error = None;
    let mut dropped_duration = Vec::new();
    for row in &store.entries {
        let record: EntryRecord = round_trip(row, "entries");
        let Some(model) = record.model else { continue };
        let Some(first) = model.first() else { continue };
        match serde_json::from_value::<nous_ai::Message>(first.clone()) {
            Err(error) => {
                if record.kind == "pi.system" {
                    system_error.get_or_insert(error.to_string());
                }
            }
            Ok(message) => {
                // 解析成功的那几类：`durationMs` 在 `nous_ai` 的消息类型里不存在，往返即丢。
                let back = serde_json::to_value(message).unwrap();
                if first.get("durationMs").is_some() && back.get("durationMs").is_none() {
                    dropped_duration.push(record.kind.clone());
                }
            }
        }
    }
    let system_error = system_error.expect("fixture 里 pi.system 的 message 带 role:\"system\"");
    assert!(
        system_error.contains("system"),
        "该是「认不出 system」这个错，实际：{system_error}"
    );
    dropped_duration.sort();
    dropped_duration.dedup();
    assert_eq!(
        dropped_duration,
        vec!["pi.assistant".to_string(), "pi.tool-result".to_string()],
        "这几类 message 的 durationMs 会被 nous_ai::Message 吞掉"
    );
}
