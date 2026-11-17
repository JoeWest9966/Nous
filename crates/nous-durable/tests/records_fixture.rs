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
// #23 把「解析不了」那个坑填了：下面两条测试换成正面判据（能解析 + collapse 顺序对）。

/// #23 判据 1：`pi.system` 的 message 能**逐字段**读进 `nous_ai::Message`，写回去与源 JSON 相等。
///
/// 这条测试的前身断言的是反面（「解析不了 / 会丢键」）。`nous-ai` 补上 `Message::System` 之后，
/// 那个世界不存在了 —— 按我们自己的规矩，删断言、删那一节 `docs/pitfalls-from-pi.md`，
/// **不是改断言让它继续绿**。
#[test]
fn system_message_round_trips_every_field_from_the_fixture() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");

    let mut system_count = 0;
    let mut with_tools = 0;
    let mut with_sections = 0;
    let mut removed_seen = 0;
    let mut section_shapes: Vec<Vec<String>> = Vec::new();

    for row in &store.entries {
        let record: EntryRecord = round_trip(row, "entries");
        if record.kind != "pi.system" {
            continue;
        }
        system_count += 1;
        let model = record.model.clone().expect("pi.system 带 model");
        assert_eq!(model.len(), 1, "每条 pi.system 的 model 只有一条 message");
        let source = model[0].clone();

        let message: nous_ai::Message = serde_json::from_value(source.clone())
            .unwrap_or_else(|error| panic!("pi.system 读不进 Message::System：{error}\n{source}"));
        let nous_ai::Message::System(system) = message else {
            panic!("pi.system 读成了别的变体：{source}");
        };

        // 逐字段：序列化回去与源 JSON 完全相等（名字、值、该缺席的键都算）。
        let written = serde_json::to_value(&*system).unwrap();
        assert_eq!(written, source, "pi.system 往返后与源 JSON 不等");

        let mut keys: Vec<&str> = source.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut written_keys: Vec<&str> = written
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        written_keys.sort_unstable();
        assert_eq!(written_keys, keys, "键集合必须一字不差");

        if let Some(tools) = &system.tools_added {
            with_tools += 1;
            let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
            assert_eq!(names, vec!["read", "write", "edit", "bash"], "工具名照真数据");
            assert!(tools[0].parameters.as_value().is_object(), "工具声明不是空壳");
        }
        if let Some(sections) = &system.sections {
            with_sections += 1;
            let mut names: Vec<String> = sections.iter().map(|(name, _)| name.to_string()).collect();
            names.sort();
            section_shapes.push(names);
        }
        removed_seen += system.tools_removed.iter().flatten().count();
        assert!(system.timestamp > 0, "真数据的 timestamp 是毫秒");
    }

    assert_eq!(system_count, 17, "fixture 里 pi.system 的条数");
    assert_eq!(with_tools, 15, "15 条带 toolsAdded（各 4 个工具）");
    assert_eq!(with_sections, 14, "14 条带 sections");
    assert_eq!(removed_seen, 0, "fixture 里从未出现 toolsRemoved");

    let mut expected: Vec<Vec<String>> = Vec::new();
    for _ in 0..8 {
        expected.push(vec!["cwd".into(), "preamble".into()]);
    }
    for _ in 0..4 {
        expected.push(vec!["cwd".into(), "preamble".into(), "prompts".into()]);
    }
    for _ in 0..2 {
        expected.push(vec!["preamble".into(), "prompts".into()]);
    }
    expected.sort();
    section_shapes.sort();
    assert_eq!(section_shapes, expected, "sections 的键组合照真数据");
}

/// #23 判据 2：**真数据**上的 collapse 顺序与上游一致。
///
/// 期望值由测试自己从原始 JSON 算（独立于库代码），不调用库里任何函数 —— 否则就是
/// 「用实现验证实现」。fixture 里没有 `pi.compaction`（0 条），所以「content 按序追加」
/// 由 `crates/nous-ai/tests/system_messages.rs` 的构造数据钉；这里只验真数据有的东西。
#[test]
fn collapse_system_messages_matches_upstream_order_on_the_fixture() {
    let Some(path) = fixture_path() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let store = ImportedStore::read(&path).expect("fixture 读得出来");

    // 按会话分组，**按 entry id 序**（transcript 顺序），只留 system 消息。
    let mut by_conversation: std::collections::BTreeMap<i64, Vec<Value>> =
        std::collections::BTreeMap::new();
    for row in &store.entries {
        let record: EntryRecord = round_trip(row, "entries");
        if record.kind != "pi.system" {
            continue;
        }
        let model = record.model.clone().expect("带 model");
        by_conversation
            .entry(record.conversation_id)
            .or_default()
            .push(model[0].clone());
    }
    assert_eq!(
        by_conversation.len(),
        15,
        "17 条 system 里有两对是同一会话的「第二次」"
    );
    let doubles: Vec<i64> = by_conversation
        .iter()
        .filter(|(_, messages)| messages.len() == 2)
        .map(|(conversation, _)| *conversation)
        .collect();
    assert_eq!(doubles, vec![2, 14], "真数据里只有两个会话有中途 system");

    for (conversation, raws) in &by_conversation {
        let messages: Vec<nous_ai::Message> = raws
            .iter()
            .map(|raw| serde_json::from_value(raw.clone()).expect("读得进 Message::System"))
            .collect();

        // 期望值：独立从原始 JSON 算。
        let mut content: Vec<String> = Vec::new();
        let mut sections: Vec<(String, String)> = Vec::new();
        let mut tools: Vec<String> = Vec::new();
        for raw in raws {
            let text = raw["content"].as_str().unwrap_or_default();
            if !text.is_empty() {
                content.push(text.to_string());
            }
            if let Some(patch) = raw.get("sections").and_then(Value::as_object) {
                for (name, value) in patch {
                    match value {
                        Value::Null => sections.retain(|(key, _)| key != name),
                        Value::String(text) => match sections.iter_mut().find(|(key, _)| key == name)
                        {
                            Some(slot) => slot.1 = text.clone(),
                            None => sections.push((name.clone(), text.clone())),
                        },
                        other => panic!("section 值不是字符串或 null：{other}"),
                    }
                }
            }
            if let Some(added) = raw.get("toolsAdded").and_then(Value::as_array) {
                for tool in added {
                    let name = tool["name"].as_str().expect("工具名").to_string();
                    if !tools.contains(&name) {
                        tools.push(name);
                    }
                }
            }
            if let Some(removed) = raw.get("toolsRemoved").and_then(Value::as_array) {
                for tool in removed {
                    let name = tool["name"].as_str().expect("工具名");
                    tools.retain(|existing| existing != name);
                }
            }
        }
        let mut expected_parts = content.clone();
        expected_parts.extend(sections.iter().map(|(_, value)| value.clone()));
        let expected = expected_parts.join("\n\n");

        let collapsed = nous_ai::transform::system::collapse_system_messages(&messages);
        let leading = match collapsed.first() {
            Some(nous_ai::Message::System(system)) => system.as_ref(),
            other => panic!("会话 {conversation} 的 collapse 没有首条 system：{other:?}"),
        };
        assert_eq!(
            nous_ai::transform::system::get_system_message_text(leading),
            expected,
            "会话 {conversation} 的 collapse 文本与上游同序不符"
        );
        let got_sections: Vec<String> = leading
            .sections
            .as_ref()
            .map(|sections| sections.iter().map(|(name, _)| name.to_string()).collect())
            .unwrap_or_default();
        let want_sections: Vec<String> = sections.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(got_sections, want_sections, "会话 {conversation} 的 sections 序");
        let got_tools: Vec<String> = leading
            .tools_added
            .as_ref()
            .map(|tools| tools.iter().map(|tool| tool.name.clone()).collect())
            .unwrap_or_default();
        assert_eq!(got_tools, tools, "会话 {conversation} 的工具集与首次声明序");
        assert!(
            collapsed[1..]
                .iter()
                .all(|message| !matches!(message, nous_ai::Message::System(_))),
            "collapse 之后 body 里不该还有 system"
        );
    }
}
