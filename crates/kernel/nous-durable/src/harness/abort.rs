//! 中断落在库里的那一步（票 `#14`，`docs/harness.md` §1 ④）。
//!
//! 上游 `generation.ts:668-674` 的 `convertPartial`：把 `pi.live.generation.message` 复制成一条
//! `stopReason: "aborted"` 的 assistant entry，然后由调用方替换或删掉 `generation`。本库**自写**，
//! 因为上游没有独立模块，这一步藏在 `Tx` 回调里（`AGENTS.md` §一：只借语义，不借结构）。
//!
//! ## 两处与上游不同
//!
//! 1. **不在一个事务里**。上游 `convertPartial(tx, live, conversationId)` 拿事务句柄，读 `live` 与
//!    写 entry 是原子的。本库 `Session::commit` 收一批写，所以这里先读快照再提交 —— 中间有一个
//!    窗口。今天观察不到（`pi.live` 只有 harness 一个写者），但这是本库已知的一处弱化，
//!    要补就得给 `Session` 加「带读的提交」，那是改 `#13` 的公开面。
//! 2. **entry 的 id 由调用方给**。上游 `tx.appendEntry` 自己 mint；本库
//!    `StorageWrite::Entry` 要求记录里带 `id`。
//!
//! ## 与 `classify` 的竞争（`docs/harness.md` §1 ③ 附 (3) 的反面）
//!
//! abort 到达时若有一次提交在飞，`Session` 的存储锁会让它先跑完，`convert_partial` 再读到**它之后**
//! 的 `generation`。若那时的 `generation` 已被终态提交清掉，这里返回 `Ok(None)` ——
//! 不写第二条 entry。判据 `an_abort_after_the_terminal_commit_writes_nothing` 钉的就是这条
//! 「committed state stays consistent」。

use serde_json::json;

use crate::session::{CommitOptions, Session};
use crate::{DocumentContent, Id, StorageWrite};

use super::HarnessError;

/// 把流式留下的 partial 落成一条 `stopReason: "aborted"` 的 assistant entry，并清掉 `generation`。
///
/// 没有 partial 时什么都不写（`Ok(None)`），返回写进去的 entry id。
pub async fn convert_partial(
    session: &Session,
    live: Id,
    conversation: Id,
    entry_id: Id,
) -> Result<Option<Id>, HarnessError> {
    let snapshot = session
        .snapshot(live)
        .await?
        .ok_or(HarnessError::MissingDocument(live))?;
    let state: super::live::LiveState = serde_json::from_value(snapshot.value.clone())
        .map_err(|error| HarnessError::Shape(error.to_string()))?;
    let Some(message) = state
        .generation
        .as_ref()
        .and_then(|generation| generation.message.as_ref())
    else {
        return Ok(None);
    };
    let mut message = message.clone();
    if let Some(object) = message.as_object_mut() {
        object.insert("stopReason".to_string(), json!("aborted"));
    }
    let record = json!({
        "id": entry_id,
        "conversationId": conversation,
        "kind": "pi.assistant",
        "model": [message],
    });
    let current = nous_chord::delta::Revision::new(snapshot.version as u64);
    let mut draft = nous_chord::delta::draft::Draft::new(&snapshot.value, current);
    draft
        .remove(&[nous_chord::delta::Seg::key("generation")])
        .map_err(|error| HarnessError::Ops(error.to_string()))?;
    let trusted = draft
        .prepare(current)
        .map_err(|error| HarnessError::Ops(error.to_string()))?;
    let writes = vec![
        StorageWrite::Entry(record),
        StorageWrite::DocumentChange {
            id: live,
            content: DocumentContent::Delta {
                version: snapshot.version,
                ops: serde_json::Value::Array(
                    nous_chord::delta::codec::Encoder::new().encode(&trusted.into_ops()),
                ),
            },
        },
    ];
    session.commit(writes, CommitOptions::default()).await?;
    Ok(Some(entry_id))
}
