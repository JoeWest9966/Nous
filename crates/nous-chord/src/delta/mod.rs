//! chord delta 的核心类型：`Op` 词表、路径、重叠计算，以及 UTF-16 截断。
//!
//! 语义权威是 `docs/chord-delta-notes.md`（不变式清单）；实现按「语义责任」重新切文件，
//! 不照 `tracker.ts` 的 2,319 行切。上游位置：`packages/chord/src/delta/index.ts`。

pub mod apply;
pub mod codec;
pub mod diff;
pub mod draft;
pub mod op;
pub mod pending;
pub mod validator;

use serde_json::Value;

/// 路径段：对象键，或非负整数下标。
///
/// 上游是 `string | number`。Rust 里分成两个变体，因为 `Path` 要当 `HashMap` 的 key
/// （`pending` 的叶子索引），而 `format!` 拼字符串做 key 会撞（`["a.b"]` vs `["a","b"]`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Seg {
    Key(String),
    Index(u64),
}

impl Seg {
    pub fn key(key: impl Into<String>) -> Self {
        Seg::Key(key.into())
    }

    pub fn index(index: usize) -> Self {
        Seg::Index(index as u64)
    }

    pub fn as_key(&self) -> Option<&str> {
        match self {
            Seg::Key(key) => Some(key),
            Seg::Index(_) => None,
        }
    }

    pub fn as_index(&self) -> Option<u64> {
        match self {
            Seg::Index(index) => Some(*index),
            Seg::Key(_) => None,
        }
    }
}

/// 一条路径。空路径 = 根（只有 `p`/`m` 和 `r` 能碰根）。
pub type Path = Vec<Seg>;

/// 便捷构造：`path(&["a", "b"])`。
pub fn path(parts: &[&str]) -> Path {
    parts.iter().map(|part| Seg::key(*part)).collect()
}

/// 便捷构造：`path_with(&[Seg::key("items"), Seg::index(3)])`。
pub fn path_with(segments: &[Seg]) -> Path {
    segments.to_vec()
}

/// 七种 op。**`r` 是唯一能替换整根的 op**：`s`/`d`/`a`/`t` 的路径不能为空（由
/// [`op::validate_op`] 在边界处拒绝），`p`/`m` 可以指向根，因为被跟踪的值本身可能是个数组。
///
/// 「批次精确但不规范」：同一变化可以由不同的 op 组合表达（大改可以折成区域 splice、
/// 祖先 `s` 或 `r`）。**契约只在结果值上，不在 op 形状上** —— 一致性判据是
/// 「对同一 base 重放后 JSON 相等」，不是比对 op 元组。
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// `["r", value]`：整根替换，也是恢复点。
    Replace(Value),
    /// `["s", path, value]`：设值。
    Set(Path, Value),
    /// `["d", path]`：删除。
    Remove(Path),
    /// `["a", path, text]`：字符串追加。
    Append(Path, String),
    /// `["t", path, count]`：从头截掉 `count` 个 **UTF-16 码元**。
    Truncate(Path, usize),
    /// `["p", path, start, delete_count, items]`：数组 splice。
    Splice(Path, usize, usize, Vec<Value>),
    /// `["m", path, permutation]`：数组置换，`new[i] = old[permutation[i]]`。
    Move(Path, Vec<usize>),
}

impl Op {
    pub fn verb(&self) -> &'static str {
        match self {
            Op::Replace(_) => "r",
            Op::Set(..) => "s",
            Op::Remove(_) => "d",
            Op::Append(..) => "a",
            Op::Truncate(..) => "t",
            Op::Splice(..) => "p",
            Op::Move(..) => "m",
        }
    }

    /// 这条 op 的路径（`r` 没有）。
    pub fn path(&self) -> Option<&Path> {
        match self {
            Op::Replace(_) => None,
            Op::Set(path, _)
            | Op::Remove(path)
            | Op::Append(path, _)
            | Op::Truncate(path, _)
            | Op::Splice(path, ..)
            | Op::Move(path, _) => Some(path),
        }
    }

    /// `p`/`m` 可以指向根；其余不行。
    pub fn targets_root(&self) -> bool {
        matches!(self, Op::Replace(_))
    }
}

pub fn is_replace(op: &Op) -> bool {
    matches!(op, Op::Replace(_))
}

/// 批次以 `r` 开头 —— 这是**可判定的事实**，不是启发式：flush 保证 `r` 只在 index 0
/// 或不存在。读者据此把该批当恢复点。
pub fn is_base(ops: &[Op]) -> bool {
    matches!(ops.first(), Some(Op::Replace(_)))
}

/// 会踩到原型链的段名。op 是数据、可能回显模型输出，所以不可信。
///
/// Rust 的 `serde_json::Map` / `Vec` 没有原型链，**结构上天然免疫**；这里仍然显式拒绝，
/// 是为了让错误可见、且跨实现一致（别的语言写的那一端未必免疫）。
pub const RESERVED_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

pub fn is_reserved(segment: &str) -> bool {
    RESERVED_SEGMENTS.contains(&segment)
}

/// 一个 op 的形状/路径错误。上游是 `throw new TypeError(...)`；Rust 返回 `Err`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    /// 不是二元组（空元组 / 动词不认识）。
    UnknownVerb(String),
    /// 元数不对。
    Arity {
        verb: &'static str,
        expected: usize,
    },
    /// 类型不对（该是字符串/数组/整数的地方不是）。
    Shape {
        verb: &'static str,
        what: &'static str,
    },
    /// 路径段不安全（保留段名、负数、非整数）。
    UnsafePath(String),
    /// `s`/`d`/`a`/`t` 指向了根。
    EmptyPath,
    /// `m` 的置换不是双射。
    NotABijection,
    /// 路径解析不出来（键不存在、类型不是容器）。
    UnresolvablePath(Path),
    /// 数组下标超过 `length`（`arr[7] = x` 在长度 3 的数组上会造出稀疏数组，
    /// 序列化成 `null` 后副本永久分叉；同时它让单个 op 分配 40 亿元素成为 DoS）。
    IndexOutOfRange { index: u64, length: usize },
    /// wire 层用了一个没定义过的 id。
    UnknownPathId(u64),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::UnknownVerb(verb) => write!(f, "unknown op verb: {verb}"),
            OpError::Arity { verb, expected } => {
                write!(f, "{verb} arity: expected {expected} elements")
            }
            OpError::Shape { verb, what } => write!(f, "{verb} shape: bad {what}"),
            OpError::UnsafePath(segment) => write!(f, "unsafe path segment: {segment}"),
            OpError::EmptyPath => f.write_str("path is empty"),
            OpError::NotABijection => f.write_str("m permutation is not a bijection"),
            OpError::UnresolvablePath(path) => write!(f, "unresolvable path: {path:?}"),
            OpError::IndexOutOfRange { index, length } => {
                write!(f, "index {index} out of range for array of {length}")
            }
            OpError::UnknownPathId(id) => write!(f, "unknown path id: {id}"),
        }
    }
}

impl std::error::Error for OpError {}

/// 严格 JSON 校验失败。上游抛 `TypeError`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionError {
    /// 超过允许的嵌套深度。`serde_json` 的反序列化默认限 128 层，这里再兜一道，
    /// 免得递归遍历把栈打穿。
    TooDeep { depth: usize },
}

impl std::fmt::Display for RevisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RevisionError::TooDeep { depth } => {
                write!(f, "replicated state nesting is too deep: {depth}")
            }
        }
    }
}

impl std::error::Error for RevisionError {}

// ─── 字符串：UTF-16 码元 ─────────────────────────────────────────────────────

/// 一个字符串有多少 **UTF-16 码元**（= JS 的 `string.length`）。
///
/// `t` op 的 `count` 就是这个单位。用 `chars().count()`（Unicode 标量）或字节数都会
/// 在 emoji / 代理对上悄悄漂 —— 详见 `docs/chord-delta-notes.md` §1.4「`t` 在 Rust 里
/// 要显式决定单元」。
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// 从头截掉 `units` 个 UTF-16 码元。
///
/// 落在代理对中间时按码元的真实边界截断：切完的字符串仍然是合法 UTF-8
/// （JS 允许产生孤立代理项，Rust 的 `&str` 不允许；这里取「丢弃整个字符」的一侧，
/// 因为我们不接受孤立代理项进入值域）。
pub fn truncate_utf16_units(text: &str, units: usize) -> &str {
    if units == 0 {
        return text;
    }
    let mut seen = 0usize;
    for (byte_index, ch) in text.char_indices() {
        if seen >= units {
            return &text[byte_index..];
        }
        seen += ch.len_utf16();
    }
    ""
}

/// 从第 `units` 个 UTF-16 码元起的后缀（`truncate` 的对称操作，`a` op 用）。
pub fn suffix_utf16_units(text: &str, units: usize) -> &str {
    truncate_utf16_units(text, units)
}

/// `a` 的最长后缀，是 `b` 的前缀 —— 单位是 UTF-16 码元（与 JS 的实现逐字节等价）。
///
/// `scan` 限制在后缀窗口内找（上游 `DEFAULT_OVERLAP_SCAN = 65_536`）；`probe` 先用长头
/// 找（候选少，能抓住滚动窗口产生的大重叠），再退回单字符。候选数超过
/// `max_candidates` 就放弃并返回 0 —— 返回 0 只会让调用方发一个更大的 `s`，**永远不会错**。
pub fn overlap(a: &str, b: &str, scan: usize, probe: usize, max_candidates: usize) -> usize {
    if a.is_empty() || b.is_empty() || scan == 0 {
        return 0;
    }
    let a: Vec<u16> = a.encode_utf16().collect();
    let b: Vec<u16> = b.encode_utf16().collect();
    let tail: &[u16] = if a.len() > scan {
        &a[a.len() - scan..]
    } else {
        &a[..]
    };

    let mut heads = vec![probe.min(b.len()), 1];
    heads.dedup();
    for head in heads {
        let head = &b[..head];
        let mut tried = 0usize;
        let mut at = 0usize;
        while at + head.len() <= tail.len() {
            let found = tail[at..]
                .windows(head.len())
                .position(|window| window == head)
                .map(|offset| at + offset);
            match found {
                Some(position) => {
                    tried += 1;
                    if tried > max_candidates {
                        break;
                    }
                    let n = tail.len() - position;
                    if n <= b.len() && tail[position..] == b[..n] {
                        return n;
                    }
                    at = position + 1;
                }
                None => break,
            }
        }
        if head.len() == 1 {
            break;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_length_differs_from_chars_and_bytes() {
        // U+1F600 = 一个 char、两个 UTF-16 码元、四个 UTF-8 字节。
        assert_eq!("😀".chars().count(), 1);
        assert_eq!("😀".len(), 4);
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(utf16_len("a😀é"), 1 + 2 + 1);
    }

    #[test]
    fn truncate_counts_utf16_units_not_chars() {
        // 截 2 个 UTF-16 码元 = 恰好切掉 emoji。
        assert_eq!(truncate_utf16_units("😀abc", 2), "abc");
        // 截 1 个码元落在代理对中间：整个字符丢弃（不产生孤立代理项）。
        assert_eq!(truncate_utf16_units("😀abc", 1), "abc");
        assert_eq!(truncate_utf16_units("abc", 1), "bc");
        assert_eq!(truncate_utf16_units("abc", 0), "abc");
        assert_eq!(truncate_utf16_units("abc", 99), "");
        // 组合字符是按码元的，不按字素簇。
        assert_eq!(truncate_utf16_units("é", 1), "");
    }

    #[test]
    fn overlap_is_measured_in_utf16_units() {
        assert_eq!(overlap("hello world", "world peace", 65_536, 64, 8), 5);
        assert_eq!(overlap("abc", "xyz", 65_536, 64, 8), 0);
        assert_eq!(overlap("", "abc", 65_536, 64, 8), 0);
        // scan = 0 直接放弃。
        assert_eq!(overlap("abc", "abc", 0, 64, 8), 0);
        // emoji 参与时返回的是码元数，不是字节数。
        assert_eq!(overlap("x😀", "😀y", 65_536, 64, 8), 2);
        // 退化输入（单字符重复）不能挂：候选超限就返回 0。
        assert_eq!(overlap("aaaa", "aaa", 65_536, 64, 8), 3);
    }

    #[test]
    fn is_base_only_looks_at_the_first_op() {
        assert!(!is_base(&[]));
        assert!(is_base(&[Op::Replace(Value::Null)]));
        assert!(!is_base(&[Op::Set(path(&["a"]), Value::Null)]));
        // 中途出现的 r 不算 base 批次（flush 不会产生，但判据只看首元素）。
        assert!(!is_base(&[
            Op::Set(path(&["a"]), Value::Null),
            Op::Replace(Value::Null)
        ]));
    }

    #[test]
    fn reserved_segments_are_rejected_by_name() {
        for name in RESERVED_SEGMENTS {
            assert!(is_reserved(name));
        }
        assert!(!is_reserved("prototype_chain"));
        assert!(!is_reserved("constructor_"));
    }

    #[test]
    fn seg_keeps_keys_and_indices_distinct() {
        // ["a.b"] 与 ["a","b"] 必须是不同路径 —— 拼字符串做 key 会合并它们。
        let flat = path(&["a.b"]);
        let nested = path(&["a", "b"]);
        assert_ne!(flat, nested);
        assert_eq!(Seg::index(1), Seg::Index(1));
        assert_ne!(Seg::index(1), Seg::key("1"));
        assert_eq!(Seg::index(7).as_index(), Some(7));
        assert_eq!(Seg::key("k").as_key(), Some("k"));
    }
}
