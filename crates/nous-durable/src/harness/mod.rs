//! `harness`：流式回答怎么落进文档（`#14`）。形状与投影规则见 `docs/harness.md`。
//!
//! 上游对应 `packages/durable/src/harness/`，但**只借语义和坑，不借结构**（`AGENTS.md` §一）：
//! `watch` 那套事件流不要（事件走 `nous-wire`），`Session` 那层的回调也不进来。

pub mod abort;
pub mod clock;
pub mod generation;
pub mod json;
pub mod live;
pub mod partial;
pub mod project;

use nous_ai_types::utils::streaming_json::PathSegment;
use nous_chord::delta::{Path, Seg};

use crate::session::SessionError;
use crate::Id;

/// `harness` 层的失败。**不是**拒绝：拒绝是 `CommitOutcome::refused`。
#[derive(Debug)]
pub enum HarnessError {
    /// 会话层（已关、存储失败、形状不符）。
    Session(SessionError),
    /// 记录形状对不上：读回来的文档不是 `pi.live` 的样子，或者流没给终态消息。
    Shape(String),
    /// 要改的文档不在。上游这一步隐式创建；本库要求调用方先把文档建出来（`DocumentCreate`）。
    MissingDocument(Id),
    /// chord 的 op 校验失败（路径越界、修订号变了）。
    Ops(String),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarnessError::Session(error) => write!(f, "{error}"),
            HarnessError::Shape(message) => f.write_str(message),
            HarnessError::MissingDocument(id) => write!(f, "document {id} is not in the store"),
            HarnessError::Ops(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for HarnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HarnessError::Session(error) => Some(error),
            _ => None,
        }
    }
}

/// 把 `prefix` 和 `tail` 接成一条绝对路径。
pub fn under(prefix: &[Seg], tail: &[Seg]) -> Path {
    let mut path = Vec::with_capacity(prefix.len() + tail.len());
    path.extend_from_slice(prefix);
    path.extend_from_slice(tail);
    path
}

/// 把 `streaming_json` 的路径转成 chord 的路径。
///
/// **全库只此一处做这个转换**。散在 `assign_json` / `apply_change` / 投影里各自写一遍，
/// 以后要改路径表示（比如给 `Seg` 加变体）就得满树找，而漏掉的那一处不会编译失败 ——
/// 它会在运行时把路径写错，写到一个谁也不看的位置。
pub fn seg(path: &[PathSegment]) -> Path {
    path.iter()
        .map(|segment| match segment {
            PathSegment::Key(name) => Seg::Key(name.clone()),
            PathSegment::Index(index) => Seg::Index(*index as u64),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_ai_types::utils::streaming_json::{index, key};

    #[test]
    fn the_path_conversion_maps_both_segment_kinds() {
        assert_eq!(seg(&[]), Path::new());
        assert_eq!(
            seg(&[key("content"), index(3), key("text")]),
            vec![Seg::key("content"), Seg::index(3), Seg::key("text")]
        );
    }
}
