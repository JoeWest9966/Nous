//! `harness`：流式回答怎么落进文档（`#14`）。形状与投影规则见 `docs/harness.md`。
//!
//! 上游对应 `packages/durable/src/harness/`，但**只借语义和坑，不借结构**（`AGENTS.md` §一）：
//! `watch` 那套事件流不要（事件走 `nous-wire`），`Session` 那层的回调也不进来。

pub mod json;

use nous_ai::utils::streaming_json::PathSegment;
use nous_chord::delta::{Path, Seg};

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
    use nous_ai::utils::streaming_json::{index, key};

    #[test]
    fn the_path_conversion_maps_both_segment_kinds() {
        assert_eq!(seg(&[]), Path::new());
        assert_eq!(
            seg(&[key("content"), index(3), key("text")]),
            vec![Seg::key("content"), Seg::index(3), Seg::key("text")]
        );
    }
}
