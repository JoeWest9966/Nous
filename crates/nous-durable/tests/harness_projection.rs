//! 投影的判据：`docs/harness.md` §2 那张表，一格一格钉住。
//!
//! 十条，按票面分四组：**5 条真数据驱动**（块与文本来自 `fixtures/agent.sqlite` 里真 assistant
//! 消息）+ **3 条构造**（两条来自用户点名的语义：应用时下标、参数增量只来自 `StreamingJsonParser`）
//! + **1 条 0 帧负向** + **1 条分派表**（不该产出消息帧的那几格）。
//!
//! fixture 不在时真数据那组整组跳过并打印原因，**不假绿**。

use std::path::{Path, PathBuf};

use nous_ai::utils::streaming_json::{JsonChange, PathSegment, StreamingJsonParser};
use nous_ai::{Api, AssistantMessage, AssistantMessageEvent};
use nous_chord::delta::apply::{apply_immutable_trusted, TrustedOps};
use nous_chord::delta::codec::Encoder;
use nous_chord::delta::draft::Draft;
use nous_chord::delta::{Op, Revision, Seg};
use nous_durable::harness::generation::live_record;
use nous_durable::harness::partial::PartialChanges;
use nous_durable::harness::project::message_changes;
use nous_durable::session::publish::{CommitChange, DocumentCommitChange};
use nous_durable::storage::import::ImportedStore;
use nous_durable::types::DocumentRecord;
use nous_chord::sqlite::SqliteValue as Cell;
use nous_wire::MessageChange;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// 真数据：fixture 里的真 assistant 消息
// ---------------------------------------------------------------------------

fn fixture_path() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agent.sqlite");
    path.exists().then_some(path)
}

/// fixture 里**每一条** assistant 消息（`entries.record.model[*]`）。上游就是这么存的：
/// 一条 assistant entry 的 `model` 是它经过的模型消息列表。
fn fixture_assistant_messages() -> Option<Vec<Value>> {
    let path = fixture_path()?;
    let store = ImportedStore::read(&path).expect("fixture 读得出来");
    let mut messages = Vec::new();
    for row in &store.entries {
        let Some(Cell::Text(text)) = row.get("record") else {
            continue;
        };
        let record: Value = serde_json::from_str(text).expect("fixture 的 record 不是合法 JSON");
        if record.get("kind").and_then(Value::as_str) != Some("pi.assistant") {
            continue;
        }
        let Some(model) = record.get("model").and_then(Value::as_array) else {
            continue;
        };
        messages.extend(model.iter().cloned());
    }
    Some(messages)
}

/// 块的类型（`text` / `thinking` / `toolCall`）。
fn block_type(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

// ---------------------------------------------------------------------------
// 一次提交：`pi.live` 文档改动
// ---------------------------------------------------------------------------

/// `pi.live` 的文档记录。从 `live_record` 来 —— 顺手证明它产出的形状真能读进
/// `DocumentRecord`（`createdAt` 由 storage 盖章，这里补上）。
fn live_document() -> DocumentRecord {
    let mut value = live_record(100, 2);
    value["createdAt"] = json!(1);
    serde_json::from_value(value).expect("live_record 的形状读得进 DocumentRecord")
}

/// 一条 `pi.live` 文档改动。`ops` 走编码器 —— 投影那边要用解码器读回来，往返本身就是判据的
/// 一部分（真实 publication 里的 `ops` 就是编码后的线上形状）。
fn live_change(value: Value, ops: &[Op]) -> CommitChange {
    CommitChange::Document(DocumentCommitChange::Change {
        record: live_document(),
        conversation_id: Some(2),
        version: 1,
        value,
        ops: Value::Array(Encoder::new().encode(ops)),
    })
}

fn project(value: Value, ops: &[Op]) -> Vec<MessageChange> {
    message_changes(&live_change(value, ops)).expect("投影不该失败")
}

/// `["generation", "message"]`。
fn message_path() -> Vec<Seg> {
    vec![Seg::key("generation"), Seg::key("message")]
}

/// `["generation", "message", "content", index]`。
fn content_path(index: usize) -> Vec<Seg> {
    let mut path = message_path();
    path.push(Seg::key("content"));
    path.push(Seg::index(index));
    path
}

/// 块里的某个字段：`content[index].field`。
fn field_path(index: usize, field: &str) -> Vec<Seg> {
    let mut path = content_path(index);
    path.push(Seg::key(field));
    path
}

/// 把真消息包成 `pi.live` 的值。
fn live_value(message: &Value) -> Value {
    json!({ "generation": { "attempt": 1, "message": message } })
}

fn empty_partial() -> std::sync::Arc<AssistantMessage> {
    std::sync::Arc::new(AssistantMessage::empty(Api::Faux, "faux", "faux", 0))
}

// ---------------------------------------------------------------------------
// 1–3：三种 start（真块）
// ---------------------------------------------------------------------------

/// 一条提交写下 `generation.message` 的每一块时，按**真块的类型**选 `*_start`。
///
/// 「新块」的判定是 `i == 应用该 op 时的 content 长度`：一条 `s` 落在 `["content", i]` 且
/// `i == 长度`，就是新块（`apply.rs` 的边界规则允许 `set(index == len)` 追加）。真实流里新块多
/// 半走 `splice`（那条落 `Block`，见第 8 条），但别的实现可以合法地发 `s`，所以这条得认。
///
/// `*_start` 带**全文**：跳过它等于把第一段吞掉。
#[test]
fn the_three_start_variants_come_from_the_real_blocks_of_fixture_messages() {
    let Some(messages) = fixture_assistant_messages() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let mut kinds = std::collections::BTreeSet::new();
    let mut messages_with_blocks = 0;
    for message in &messages {
        let content = message
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if content.is_empty() {
            continue;
        }
        messages_with_blocks += 1;
        // base 里消息是空的，这一批把每一块按顺序写进去 ⇒ 每一块都是「新块」。
        let base = json!({
            "generation": {
                "attempt": 1,
                "message": { "role": "assistant", "content": [] },
            }
        });
        let value = live_value(message);
        let ops: Vec<Op> = content
            .iter()
            .enumerate()
            .map(|(index, block)| Op::Set(content_path(index), block.clone()))
            .collect();

        let frames = project(value, &ops);
        assert_eq!(
            frames.len(),
            content.len(),
            "每一块都要有帧：一帧不能少（少了就是静默丢字）"
        );
        for (index, (frame, block)) in frames.iter().zip(content.iter()).enumerate() {
            let expected = match block_type(block) {
                "text" => MessageChange::TextStart {
                    content_index: index as u64,
                    block: block.clone(),
                },
                "thinking" => MessageChange::ThinkingStart {
                    content_index: index as u64,
                    block: block.clone(),
                },
                "toolCall" => MessageChange::ToolcallStart {
                    content_index: index as u64,
                    block: block.clone(),
                },
                other => panic!("fixture 里出现了没见过的块类型 {other:?}"),
            };
            assert_eq!(frame, &expected, "第 {index} 块的帧不对（base 是 {base}）");
            kinds.insert(block_type(block).to_string());
        }
    }
    assert!(
        messages_with_blocks > 50,
        "fixture 里 assistant 消息太少（{messages_with_blocks}），这条判据失去意义"
    );
    assert_eq!(
        kinds,
        ["text", "thinking", "toolCall"]
            .iter()
            .map(|kind| kind.to_string())
            .collect(),
        "三种块必须都真出现过，否则某一条 start 分支没被真数据走过"
    );
}

// ---------------------------------------------------------------------------
// 2：Block（真块，已存在的下标）
// ---------------------------------------------------------------------------

/// 块内部的某处被 `Set` ⇒ 发**整块** `Block`，块取批后的完整值。
///
/// 这是真实现里的形状：`assign_json` 遇到不可追加的字符串（provider 在块结束时把整段文本换掉）
/// 就退回整块 `set`，落在 `content[i].text` 上而不是 `content[i]` 上。客户端以 `Block` 为准，
/// 所以哪怕新文本比之前短也必须照发 —— **不许**拿它当 `text_delta` 的增量。
#[test]
fn an_existing_block_is_replaced_whole_when_one_of_its_fields_is_set() {
    let Some(messages) = fixture_assistant_messages() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let (index, block) = messages
        .iter()
        .filter_map(|message| {
            let content = message.get("content").and_then(Value::as_array)?;
            let index = content
                .iter()
                .position(|block| block_type(block) == "text")?;
            Some((index, content[index].clone()))
        })
        .next()
        .expect("fixture 里至少有一条带文本块的 assistant 消息");
    assert!(
        !block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty(),
        "真块得有文本"
    );

    // 真块 + 一个「变短了」的文本：替换方向也要能看出来。
    let mut replacement = block.clone();
    replacement["text"] = json!("换掉了");
    let value = json!({
        "generation": { "attempt": 1, "message": { "role": "assistant", "content": [replacement] } }
    });
    let ops = vec![Op::Set(field_path(index, "text"), json!("换掉了"))];

    assert_eq!(
        project(value, &ops),
        vec![MessageChange::Block {
            content_index: index as u64,
            block: replacement,
        }]
    );
}

// ---------------------------------------------------------------------------
// 3：Message（真消息，根替换）
// ---------------------------------------------------------------------------

/// 整根替换（`r`）：「以 `message` 为准」，一条帧，取**批后**的完整消息。
///
/// 同一批里 `r` 之后的 op 全部丢掉：`Message` 里已经是最终值，再追加那些增量会重复。
#[test]
fn a_root_replace_yields_one_message_frame_with_the_real_message() {
    let Some(messages) = fixture_assistant_messages() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let message = messages
        .iter()
        .find(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|content| !content.is_empty())
        })
        .expect("fixture 里至少有一条非空 assistant 消息");
    let value = live_value(message);

    let ops = vec![
        Op::Replace(value.clone()),
        // `r` 之后还有一个块级 op：它已经在 `value` 里了，绝不能再发一次。
        Op::Set(content_path(0), json!({"type": "text", "text": "duplicate"})),
    ];

    assert_eq!(
        project(value, &ops),
        vec![MessageChange::Message {
            message: message.clone()
        }]
    );
}

// ---------------------------------------------------------------------------
// 4：TextDelta（真文本）
// ---------------------------------------------------------------------------

/// 真文本追加：`a` 落在 `content[i].text` 上 ⇒ 一条 `TextDelta`，`delta` 就是那条 op 的文本。
///
/// 这是节流提交的主力（`PartialChanges::take` 的 `AssignPlan::Append`）。
#[test]
fn a_real_text_append_becomes_one_text_delta() {
    let Some(messages) = fixture_assistant_messages() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let (index, block) = messages
        .iter()
        .filter_map(|message| {
            let content = message.get("content").and_then(Value::as_array)?;
            let index = content
                .iter()
                .position(|block| {
                    block_type(block) == "text"
                        && block
                            .get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| !text.is_empty())
                })?;
            Some((index, content[index].clone()))
        })
        .next()
        .expect("fixture 里至少有一条带非空文本块的 assistant 消息");

    let text = block
        .get("text")
        .and_then(Value::as_str)
        .expect("文本块有 text")
        .to_string();
    // 按字符边界切：真文本里有中文，按字节切会 panic（也说明这条判据用的是真数据）。
    let (prefix, tail) = text.split_at(text.char_indices().nth(text.chars().count() / 2).map_or(0, |(byte, _)| byte));
    assert!(!prefix.is_empty() && !tail.is_empty());

    let mut committed = block.clone();
    committed["text"] = json!(prefix);
    let mut grown = block.clone();
    grown["text"] = json!(text);

    let value = json!({
        "generation": { "attempt": 1, "message": { "role": "assistant", "content": [grown] } }
    });
    let ops = vec![Op::Append(field_path(index, "text"), tail.to_string())];

    assert_eq!(
        project(value, &ops),
        vec![MessageChange::TextDelta {
            content_index: index as u64,
            delta: tail.to_string(),
        }]
    );
}

// ---------------------------------------------------------------------------
// 5：终态提交不发消息帧（真终态形状）
// ---------------------------------------------------------------------------

/// 终态那次提交把 `generation` 整个删掉（`generation_change(None)` ⇒ `d` 在
/// `["generation"]`）。**不发帧**：`wire.md` 没有「消息消失」帧，硬发一条空 `message` 会让气泡
/// 凭空清空；客户端靠 `message_end`（entry 变化，另一类帧）收场。
#[test]
fn a_terminal_commit_that_removes_the_generation_produces_no_frames() {
    let Some(messages) = fixture_assistant_messages() else {
        println!("跳过：fixtures/agent.sqlite 不在");
        return;
    };
    let message = messages
        .iter()
        .find(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|content| !content.is_empty())
        })
        .expect("fixture 里至少有一条非空 assistant 消息");

    // 批前是这一轮的 partial，批后 `pi.live` 只剩 `{}`（`run` / `tools` 也一起删掉了）。
    let before = live_value(message);
    let draft = Draft::new(&before, Revision::new(0));
    let after = json!({});
    assert_eq!(
        draft.value(),
        &before,
        "Draft 不改 base（这里只是确认这条判据用的批前值是真的 partial）"
    );

    let ops = vec![Op::Remove(vec![Seg::key("generation")])];
    assert!(project(after, &ops).is_empty(), "终态删键不许发帧");
}

// ---------------------------------------------------------------------------
// 6：ThinkingDelta（构造）
// ---------------------------------------------------------------------------

/// `a` 落在 `content[i].thinking` 上 ⇒ `ThinkingDelta`。思考不是正文，客户端别往气泡里塞。
#[test]
fn _constructed_a_thinking_append_becomes_one_thinking_delta() {
    let value = json!({
        "generation": {
            "attempt": 1,
            "message": {
                "role": "assistant",
                "content": [{"type": "thinking", "thinking": "先算一下", "thinkingSignature": "r"}],
            },
        }
    });
    let ops = vec![Op::Append(field_path(0, "thinking"), "再算一下".to_string())];

    assert_eq!(
        project(value, &ops),
        vec![MessageChange::ThinkingDelta {
            content_index: 0,
            delta: "再算一下".to_string(),
        }]
    );
}

// ---------------------------------------------------------------------------
// 7：ToolcallDelta 的增量只来自 StreamingJsonParser（构造，用户点名）
// ---------------------------------------------------------------------------

/// 一条带工具调用的 `pi.live` 值：`content[1]` 是参数还在流的调用。
fn toolcall_value(arguments: Value) -> Value {
    live_value(&json!({
        "role": "assistant",
        "content": [
            {"type": "text", "text": "查一下"},
            {"type": "toolCall", "id": "call_1", "name": "bash", "arguments": arguments},
        ],
    }))
}

/// 把一条 `toolcall_delta` 的 `path` + `delta` 应用到参数上（客户端那份实现的最小版）。
fn apply_delta(arguments: &mut Value, path: &str, delta: &str) {
    let mut segments: Vec<&str> = path.split('.').collect();
    assert_eq!(segments.first(), Some(&"arguments"), "path 以 arguments 开头");
    segments.remove(0);
    let Some((last, parents)) = segments.split_last() else {
        // 直接追加到参数根（退化形状：参数还是个字符串）。
        panic!("path 至少要给出一层：{path}");
    };
    let mut cursor = arguments;
    for segment in parents {
        if cursor.get(*segment).is_none() {
            cursor[*segment] = json!({});
        }
        cursor = cursor.get_mut(*segment).expect("路径逐层存在");
    }
    match cursor.get_mut(*last) {
        Some(Value::String(existing)) => existing.push_str(delta),
        Some(slot) => *slot = json!(delta),
        None => cursor[*last] = json!(delta),
    }
}

/// 参数增量**必须**是 `StreamingJsonParser` 的 append 增量，不是「参数文本的任意前缀」。
///
/// 两条断言：
/// 1. 投影出来的 `toolcall_delta` 序列与该解析器在同一批 chunk 上吐出的 `Append` 序列**逐个相等**
///    （路径与文本都相等）—— 自己按长度切字符串过不了这一条；
/// 2. 把这些增量按客户端规则应用到参数上，得到的就是最终的参数对象 —— 「拼得回去」。
///
/// 为什么第 1 条能红：按长度切分永远拼不回合法 JSON（每一帧都合法，合起来不是），所以那条路
/// 只会在客户端静默拼坏，不会在服务端报错。
#[test]
fn _constructed_toolcall_deltas_are_the_parser_increments_not_sliced_text() {
    // 第一段故意把字符串留在**没闭合**的地方：闭合的值是一次 `Set`（带全文），只有还在长的
    // 字符串才是 `Append` —— 而 `toolcall_delta` 只表达后者。
    let pieces = ["{\"path\":\"/tmp", "/a\",\"depth\":", "3}"];
    let mut base = toolcall_value(json!({}));
    let index = 1;

    // 期望值：同一个解析器、同一批 chunk 上的 `Append`。
    let mut parser = StreamingJsonParser::new();
    let mut expected: Vec<(String, String)> = Vec::new();
    for piece in pieces {
        for change in parser.append(piece) {
            if let JsonChange::Append { path, text } = change {
                expected.push((arguments_path(&path), text));
            }
        }
    }
    assert!(!expected.is_empty(), "这批 chunk 至少要产生一次追加");

    // 真实现：`PartialChanges` + `Draft`，逐 chunk 提交（与 `generation::flush` 同一条路）。
    let mut changes = PartialChanges::new();
    changes.observe(&AssistantMessageEvent::ToolCallStart {
        content_index: index,
        partial: empty_partial(),
    });
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut arguments = json!({});
    for piece in pieces {
        changes.observe(&AssistantMessageEvent::ToolCallDelta {
            content_index: index,
            delta: piece.to_string(),
            partial: empty_partial(),
        });
        let mut message = base["generation"]["message"].clone();
        let taken = changes.take(&mut message);
        let mut draft = Draft::new(&base, Revision::new(1));
        changes
            .commit(&mut draft, &[Seg::key("generation")], &mut message, taken)
            .expect("提交能写");
        let next = draft.value().clone();
        let ops = draft.prepare(Revision::new(1)).expect("prepare").into_ops();
        for frame in project(next.clone(), &ops) {
            match frame {
                MessageChange::ToolcallDelta {
                    content_index,
                    path,
                    delta,
                } => {
                    assert_eq!(content_index, index as u64, "下标必须是流里的那个块");
                    apply_delta(&mut arguments, &path, &delta);
                    seen.push((path, delta));
                }
                // 整块替换：客户端以它为准（参数整份换掉）。投影在它之后就停，所以不会重复。
                MessageChange::Block { block, .. } => {
                    arguments = block
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                }
                other => panic!("参数流只该产出 toolcall_delta 或 block，拿到 {other:?}"),
            }
        }
        base = next;
    }

    assert_eq!(
        seen, expected,
        "投影的增量与解析器的增量必须逐条相等（顺序也要一样）"
    );
    assert_eq!(
        arguments,
        json!({"path": "/tmp/a", "depth": 3}),
        "把帧按客户端规则应用一遍，参数必须正好是最终值"
    );
}

/// `JsonPath` → `toolcall_delta.path`（`arguments` 开头，逐段 `.`）。
fn arguments_path(path: &[PathSegment]) -> String {
    let mut joined = String::from("arguments");
    for segment in path {
        joined.push('.');
        match segment {
            PathSegment::Key(key) => joined.push_str(key),
            PathSegment::Index(index) => joined.push_str(&index.to_string()),
        }
    }
    joined
}

// ---------------------------------------------------------------------------
// 8：一批里先 splice 新块、再往它的 text 追加（构造，用户点名）
// ---------------------------------------------------------------------------

/// 块的 `content_index` 是**应用该 op 时**的下标。
///
/// 一批里「splice 插入新块」+「往这个块的 text 追加」时，两个 op 的下标含义相同；如果投影按
/// **批后的最终树**去算、或者漏掉 splice 那一帧，客户端就拿到一个它还没创建的块号 —— 然后静默
/// 丢字（不是崩，是少一段）。
///
/// 帧必须是 `[Block(新块, i), TextDelta(i)]`，顺序不许反、下标必须相等。
///
/// 这批 op 是**手写的**（`Draft` 造不出这种批次：`prepare()` 永远先重跑 `diff_revisions`，账本
/// 只覆写 diff 的 op、不产生 op —— `docs/decisions.md` 那条），但它是**合法的线上批次**，所以
/// 先用重放证明它正好把 `base` 变成 `value`，再喂给投影。
#[test]
fn _constructed_a_splice_and_a_growing_text_in_one_batch_keep_the_apply_time_index() {
    let index = 2; // 新块接在两块之后
    let partial_block = json!({"type": "text", "text": "前半"});
    let full_block = json!({"type": "text", "text": "前半后半"});

    let base = toolcall_value(json!({}));
    let mut message = base["generation"]["message"].clone();
    message["content"]
        .as_array_mut()
        .expect("content 是数组")
        .push(full_block);
    let value = live_value(&message);

    let mut content = message_path();
    content.push(Seg::key("content"));
    let ops = vec![
        // 生产里新块是 `assign_json` 的 `splice` 落下来的。
        Op::Splice(content, index, 0, vec![partial_block.clone()]),
        // 同一批里往这个新块的 text 追加（生产里是 `AssignPlan::Append` 的 `a`）。
        Op::Append(field_path(index, "text"), "后半".to_string()),
    ];
    let trusted = TrustedOps::from_decoded(ops.clone()).expect("op 形状合法");
    assert_eq!(
        apply_immutable_trusted(&base, &trusted).expect("能重放"),
        value,
        "这批 op 必须正好把 base 变成 value，否则下面断言的是个非法批次"
    );

    assert_eq!(
        project(value, &ops),
        vec![
            MessageChange::Block {
                content_index: index as u64,
                block: partial_block,
            },
            MessageChange::TextDelta {
                content_index: index as u64,
                delta: "后半".to_string(),
            },
        ],
        "先 Block 后 TextDelta，下标都是应用时的 {index}"
    );
}

// ---------------------------------------------------------------------------
// 9：0 帧负向（用户点名）
// ---------------------------------------------------------------------------

/// `pi.live` 变了、但**没触及** `generation.message` ⇒ 0 帧。
///
/// 工具卡片、重试提示、压缩状态走各自的 SSE 事件，不是 `message_update.changes[]`。#22 因此
/// 不能假设「每条 commit 都有消息帧」，也不能"反正有 changes 就发一帧空的"。
#[test]
fn a_live_change_that_misses_the_message_produces_no_frames() {
    let value = json!({
        "generation": { "attempt": 1, "message": { "role": "assistant", "content": [
            {"type": "text", "text": "全文"}
        ]}},
        "tools": [{"callId": "call_1", "name": "bash", "status": "running", "output": "跑着\n"}],
        "run": {"taskId": 7, "inputs": [11], "turns": 1},
    });
    let ops = vec![
        Op::Set(vec![Seg::key("run")], json!({"taskId": 7, "inputs": [11], "turns": 2})),
        Op::Append(
            vec![Seg::key("tools"), Seg::index(0), Seg::key("output")],
            "还在跑\n".to_string(),
        ),
        Op::Set(
            vec![Seg::key("generation"), Seg::key("attempt")],
            json!(2),
        ),
    ];

    assert!(
        project(value, &ops).is_empty(),
        "没触及 generation.message 的提交一帧都不许发"
    );
}

// ---------------------------------------------------------------------------
// 10：分派表（不产出消息帧的那几格）
// ---------------------------------------------------------------------------

/// 分派表：`Retire` / `Copy` / 非 `pi.live` 的文档 / 非文档改动，一律 0 帧。
///
/// 这几格写错的表现是「客户端收到一条它不该收到的消息帧」—— 例如 `Retire` 发 `Message` 会让
/// 气泡凭空清空，`Copy` 回放 ops 会让客户端从半个状态开始拼。
#[test]
fn the_dispatch_table_yields_no_frames_outside_a_live_document_change() {
    let record = live_document();

    // 退役：没有值了。
    assert!(message_changes(&CommitChange::Document(DocumentCommitChange::Retire {
        record: record.clone(),
        conversation_id: Some(2),
    }))
    .expect("不报错")
    .is_empty());

    // 复制：新 base，不回放 ops。
    assert!(message_changes(&CommitChange::Document(DocumentCommitChange::Copy {
        record: record.clone(),
        conversation_id: 2,
        source: nous_durable::CopySource {
            id: 100,
            at: nous_durable::DocumentPoint::Current,
        },
    }))
    .expect("不报错")
    .is_empty());

    // 别的 kind：同样一条「像消息」的 op，也不发。
    let mut other = live_document();
    other.kind = "pi.transcript".to_string();
    let value = live_value(&json!({"role": "assistant", "content": [{"type": "text", "text": "x"}]}));
    let ops = Value::Array(Encoder::new().encode(&[Op::Set(content_path(0), json!({"type":"text","text":"x"}))]));
    assert!(
        message_changes(&CommitChange::Document(DocumentCommitChange::Change {
            record: other,
            conversation_id: Some(2),
            version: 1,
            value: value.clone(),
            ops: ops.clone(),
        }))
        .expect("不报错")
        .is_empty(),
        "非 pi.live 的文档不发消息帧"
    );

    // 反过来：同一份 ops 在 pi.live 上就会发 —— 证明上面那条空不是"ops 本来就解不出帧"。
    assert_eq!(
        project(value, &[Op::Set(content_path(0), json!({"type":"text","text":"x"}))]).len(),
        1
    );
}
