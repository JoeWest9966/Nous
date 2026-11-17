//! 一次提交 → `message_update.changes[]`（8 类 `nous_wire::MessageChange`）。
//!
//! 上游 `pi-durable` **没有**这一步：它的 `message_update` 是 harness 直接读 provider 事件发的。
//! 本库把它做成「publication 的投影」，理由是 `docs/harness.md` §2 那张表 —— 客户端要的是
//! 「消息变了什么」，而库里已经有「这次提交改了哪些 op」这个事实，再让 #22 拿最终树反推一次
//! 增量，等于把同一件事算两遍，且两遍会有两个答案。
//!
//! ## 只有一格产出
//!
//! 唯一产出 `MessageChange` 的输入是：`Document::Change` + `record.kind == "pi.live"` +
//! op 触及 `generation.message`。其余七格一律 0 帧（`docs/harness.md` §2.1）——**不是**"没实现"，
//! 是"没有消息语义"。所以 #22 不能假设"每条 commit 都有一帧"。
//!
//! ## 两条铁律
//!
//! 1. **`Set` 永远不编码成 `*_delta`。** `delta` 的语义是「追加文本」；用 `set` 去表达「设值」，
//!    客户端会拼出坏 JSON 而且不报错 —— 谁都查不出来的那种降级。`Set` 一律落 `Block` /
//!    `Message`。
//! 2. **块的 `content_index` 是"应用该 op 时"的下标**，不是批前、也不是批后的。一批里同时有
//!    「splice 插入新块」和「往这个新块的 text 追加」时，两个 op 的下标含义相同（都是应用时），
//!    但**批后的最终树**里前面可能已被删过元素。投影按批后的树算下标，客户端就会拿到一个它
//!    还没创建的块号，然后**静默丢字**（不是崩，是少一段）。这条由
//!    `a_splice_and_a_growing_text_in_one_batch_keep_the_apply_time_index` 钉住。
//!
//! ## 保守的兜底
//!
//! 表达不了的形状（删块、置换、截断、整条消息范围内的任意 `Set`）一律发一条
//! `Message { message }`（取**批后**的完整消息）并**丢掉这一批剩下的 op**：客户端以 `Message`
//! 为准，而 `Message` 里已经是最终值，再追加后面那些增量反而会重复。这是 `docs/harness.md`
//! §2.2 里"整条替换：以 `message` 为准"那句的具体做法。

use nous_chord::delta::codec::Decoder;
use nous_chord::delta::{Op, Seg};
use nous_wire::MessageChange;
use serde_json::Value;

use super::generation::LIVE_KIND;
use super::HarnessError;
use crate::session::publish::{CommitChange, DocumentCommitChange};

/// 一条 commit 改动的投影结果。
///
/// 不产出消息帧的格子返回**空 vec**（不是 `Err`）：那是有意的语义，不是失败。只有 publication
/// 自身对不上（`ops` 不是数组、op 解不开）才 `Err` —— 那种错不该被当作"没有变化"悄悄咽掉。
pub fn message_changes(change: &CommitChange) -> Result<Vec<MessageChange>, HarnessError> {
    let CommitChange::Document(document) = change else {
        return Ok(Vec::new());
    };
    let DocumentCommitChange::Change {
        record,
        value,
        ops,
        ..
    } = document
    else {
        // `Retire` / `Copy` 是「没有值了」与「一条新 base」，都不回放 ops。
        return Ok(Vec::new());
    };
    if record.kind != LIVE_KIND {
        // `pi.transcript` / `pi.agent` / `pi.usage` / `pi.inbox`：没有消息语义。
        return Ok(Vec::new());
    }
    let wire = ops
        .as_array()
        .ok_or_else(|| HarnessError::Shape("publication ops is not an array".to_string()))?;
    // 每批一个**新的**解码器，和 `storage::documents::materialize` 一致：库里的 `ops` 是按批
    // 编码的（路径字典不跨批），所以投影这里也不许继承跨批状态。
    let decoded = Decoder::new()
        .decode(wire)
        .map_err(|error| HarnessError::Ops(error.to_string()))?;
    Ok(project(&decoded, value))
}

/// 解码后的 op 批次 + 批后的完整值 → 帧。分开暴露是因为判据要直接喂 op 批次。
pub fn project(ops: &[Op], value: &Value) -> Vec<MessageChange> {
    let mut frames = Vec::new();
    // 应用该 op 时的 `content` 长度。**不能**用批后的长度：那正是铁律 2 的反面。
    let mut length = base_content_length(value, ops);
    for op in ops {
        let Some(path) = op.path() else {
            // `Replace`：整根换了。整条消息以最终值为准。
            if push_whole(&mut frames, value) {
                return frames;
            }
            return frames;
        };
        if message_path().starts_with(path) {
            // 路径是 `generation.message` 的**前缀**（`[]` / `["generation"]` / 消息本身）：
            // 整条消息被重建或删掉。删掉时不发帧（`wire.md` 没有"消息消失"帧，硬发一条空
            // `message` 会让气泡凭空清空），剩下的 op 也不用看了。
            match op {
                Op::Remove(_) => return frames,
                Op::Set(_, replacement) if replacement.is_null() => return frames,
                _ => {
                    push_whole(&mut frames, value);
                    return frames;
                }
            }
        }
        let Some(rest) = path.strip_prefix(message_path().as_slice()) else {
            // 不触及 `generation.message`（`run` / `tools` / `compactions` / `attempt`…）。
            continue;
        };
        match rest {
            // 整条消息本身。
            [] => match op {
                Op::Remove(_) => return frames,
                Op::Set(_, replacement) if replacement.is_null() => return frames,
                Op::Set(_, replacement) => {
                    frames.push(MessageChange::Message {
                        message: replacement.clone(),
                    });
                    return frames;
                }
                _ => {
                    push_whole(&mut frames, value);
                    return frames;
                }
            },
            [Seg::Key(key), tail @ ..] if key == "content" => {
                match content_step(&mut frames, op, tail, value, &mut length) {
                    Step::Continue => {}
                    Step::Stop => return frames,
                }
            }
            // `role` / `usage` / 别的消息级字段：表达不了，整条替换。
            _ => {
                push_whole(&mut frames, value);
                return frames;
            }
        }
    }
    frames
}

/// 一个 op 处理完之后：接着看，还是停。
enum Step {
    Continue,
    Stop,
}

/// `generation.message.content` 之下的一步。
fn content_step(
    frames: &mut Vec<MessageChange>,
    op: &Op,
    tail: &[Seg],
    value: &Value,
    length: &mut usize,
) -> Step {
    let Some((head, rest)) = tail.split_first() else {
        // `content` 这个数组本身。
        return match op {
            Op::Splice(_, start, delete, items) => {
                if *delete > 0 {
                    // 删块表达不出来（`MessageChange` 只有"替换/追加"，没有"删掉第 i 个"），
                    // 整条替换最省事也最不会错。
                    return whole(frames, value);
                }
                for (offset, item) in items.iter().enumerate() {
                    frames.push(MessageChange::Block {
                        content_index: (*start + offset) as u64,
                        block: item.clone(),
                    });
                }
                *length += items.len();
                Step::Continue
            }
            Op::Set(_, replacement) => match replacement.as_array() {
                Some(blocks) => {
                    for (index, block) in blocks.iter().enumerate() {
                        frames.push(MessageChange::Block {
                            content_index: index as u64,
                            block: block.clone(),
                        });
                    }
                    *length = blocks.len();
                    Step::Continue
                }
                None => whole(frames, value),
            },
            _ => whole(frames, value),
        };
    };
    let Seg::Index(index) = head else {
        // `content` 之下不是下标：形状不对，整条替换。
        return whole(frames, value);
    };
    let index = *index as usize;
    match op {
        Op::Set(_, block) if rest.is_empty() => {
            if index < *length {
                frames.push(MessageChange::Block {
                    content_index: index as u64,
                    block: block.clone(),
                });
                Step::Continue
            } else if index == *length {
                // 新块。`*_start` 带全文，跳过它等于把第一段吞掉。
                frames.push(start_of(index, block));
                *length += 1;
                Step::Continue
            } else {
                // 中间缺了块：客户端没有 `length..index` 这些块，光给它这一个会静默丢字。
                whole(frames, value)
            }
        }
        // 块内的某处被设值（`arguments.path`、`text` 整段替换……）：整块替换，**并停下**。
        //
        // 停下的理由：`Block` 带的是**批后**的块，同一批里后面的追加已经算在它里面了；再发那些
        // 增量，客户端会把同一段文本追加两次。单发一条整块就已经是最终状态。
        Op::Set(_, _) => block_stop(frames, value, index),
        Op::Append(_, text) => {
            if index >= *length {
                // 客户端还没这个块：追加落在空气上。整块发过去（并停下，理由同上）。
                return block_stop(frames, value, index);
            }
            match rest {
                [Seg::Key(field)] if field == "text" => {
                    frames.push(MessageChange::TextDelta {
                        content_index: index as u64,
                        delta: text.clone(),
                    });
                }
                [Seg::Key(field)] if field == "thinking" => {
                    frames.push(MessageChange::ThinkingDelta {
                        content_index: index as u64,
                        delta: text.clone(),
                    });
                }
                [Seg::Key(field), arguments @ ..] if field == "arguments" => {
                    // 增量**只**来自 `ToolCallArgumentsStream` / `StreamingJsonParser` 落到
                    // `arguments` 上的 `a`（`harness/partial.rs` 的 `apply_change`）。这里
                    // 不再切一次字符串：切出来的片段拼不回合法 JSON，而每一帧都是合法的 ——
                    // 于是不报错，只是永远拼不对。
                    frames.push(MessageChange::ToolcallDelta {
                        content_index: index as u64,
                        path: arguments_path(arguments),
                        delta: text.clone(),
                    });
                }
                _ => return block_stop(frames, value, index),
            }
            Step::Continue
        }
        // 块内部的 splice（例如参数里的数组被整体换掉）：整块替换。
        Op::Splice(..) => block_stop(frames, value, index),
        // 删块 / 置换 / 截断：`MessageChange` 里没有对应形状。
        Op::Remove(_) | Op::Move(..) | Op::Truncate(..) | Op::Replace(_) => whole(frames, value),
    }
}

/// 整块替换（取**批后**的块）并停下：表达不了"块内某个字段被设值"，但整块永远是对的，而停下
/// 保证后面的增量不会在已经算进这块的内容上再加一遍。
fn block_stop(frames: &mut Vec<MessageChange>, value: &Value, index: usize) -> Step {
    match block_at(value, index) {
        Some(block) => {
            frames.push(MessageChange::Block {
                content_index: index as u64,
                block,
            });
            Step::Stop
        }
        None => whole(frames, value),
    }
}

/// 整条替换，并告诉调用方"停下"。
fn whole(frames: &mut Vec<MessageChange>, value: &Value) -> Step {
    push_whole(frames, value);
    Step::Stop
}

/// 把批后的完整消息推成一帧 `Message`；消息不在就不推。返回是否推了。
fn push_whole(frames: &mut Vec<MessageChange>, value: &Value) -> bool {
    let Some(message) = message_of(value) else {
        return false;
    };
    frames.push(MessageChange::Message {
        message: message.clone(),
    });
    true
}

/// 按块的 `type` 选 `*_start`；认不出的类型退回 `Block`（整块替换永远是对的）。
fn start_of(index: usize, block: &Value) -> MessageChange {
    let content_index = index as u64;
    match block.get("type").and_then(Value::as_str) {
        Some("text") => MessageChange::TextStart {
            content_index,
            block: block.clone(),
        },
        Some("thinking") => MessageChange::ThinkingStart {
            content_index,
            block: block.clone(),
        },
        Some("toolCall") => MessageChange::ToolcallStart {
            content_index,
            block: block.clone(),
        },
        _ => MessageChange::Block {
            content_index,
            block: block.clone(),
        },
    }
}

/// `["generation", "message"]`。
fn message_path() -> [Seg; 2] {
    [Seg::key("generation"), Seg::key("message")]
}

/// 批后的 `generation.message`。
fn message_of(value: &Value) -> Option<&Value> {
    value.get("generation").and_then(|generation| generation.get("message"))
}

/// 批后 `content[index]`。
fn block_at(value: &Value, index: usize) -> Option<Value> {
    message_of(value)?.get("content")?.as_array()?.get(index).cloned()
}

/// `["content", i]` 之后的参数子路径，写成 `arguments` + 逐段 `.x`（`i` 用数字）。
///
/// `wire.md` 只说它是"JSON 指针式子路径"，`toolcall_delta` 的消费方按 `.` 拆。空子路径时就是
/// `arguments`（整个参数被追加，只会出现在参数还是字符串的退化形状里）。
fn arguments_path(rest: &[Seg]) -> String {
    let mut path = String::from("arguments");
    for segment in rest {
        path.push('.');
        match segment {
            Seg::Key(key) => path.push_str(key),
            Seg::Index(index) => path.push_str(&index.to_string()),
        }
    }
    path
}

/// 批**前**的 `content` 长度：这一批里**第一个可能创建块**的下标。
///
/// 只有 `Set(["content", i], block)` 的"新块还是旧块"需要它。三条事实：
///
/// - `Splice` 落在 `["content"]` 上、`start == 长度` ⇒ 追加，创建新块；
/// - `Set` 落在 `["content", i]` 上 ⇒ **可能**创建（`apply.rs` 的边界规则允许 `set(index == len)`
///   追加，别的实现也会这么发）；
/// - 落在块**内部**（`content[i].text`、`content[i].arguments.x`）⇒ 块一定已经存在。
///
/// 所以取「批后长度」与「所有创建候选下标」里的最小值。这条读法对真实形状（新块走 `p`、
/// 整块重写走块内部的 `s`）是精确的；唯一的含糊是"某个实现用顶层 `s` 重写一个**已存在**的块"——
/// 那会被读成新块，发一条 `*_start`。两种帧都带全文，客户端重置累加器后接着收增量仍然正确，
/// 所以这里不做更复杂的推断（做也推断不出来：op 里没有 base 长度）。
fn base_content_length(value: &Value, ops: &[Op]) -> usize {
    let final_length = message_of(value)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let content = [
        Seg::key("generation"),
        Seg::key("message"),
        Seg::key("content"),
    ];
    let mut length = final_length;
    for op in ops {
        let Some(path) = op.path() else { continue };
        if path.as_slice() == content.as_slice() {
            match op {
                Op::Splice(_, start, _, _) => length = length.min(*start),
                Op::Set(_, replacement) => {
                    if let Some(blocks) = replacement.as_array() {
                        // 整个数组被设：长度由它自己决定，不能更短。
                        length = length.min(final_length.min(blocks.len()));
                    }
                }
                _ => {}
            }
            continue;
        }
        let Some(rest) = path.strip_prefix(content.as_slice()) else {
            continue;
        };
        // `["content", i]` 本身（顶层 `s`/`d`）是创建候选；更深说明块已存在。
        if let [Seg::Index(index)] = rest {
            length = length.min(*index as usize);
        }
    }
    length
}
