//! 把 op 应用到值上。两个入口，对应上游两份实现：
//!
//! - [`apply_immutable_trusted`]：只接受**自己产生的** op（[`TrustedOps`]），不做完整形状
//!   校验就动手 —— 上游 `apply-immutable-trusted.ts` 的信任边界。
//! - [`TrustedOps::from_decoded`]：外部来源（wire / 磁盘 / 模型输出）进来的 op 必须经过
//!   这一道（逐条 [`super::op::validate_op`]）才能变成 `TrustedOps`。
//!
//! **类型层就是边界**：`apply_immutable_trusted` 的签名里只有 `&TrustedOps`，而 `TrustedOps`
//! 在 crate 外只能从 `from_decoded` 拿到。想把不可信 op 喂进可信路径，得先显式校验。
//!
//! ## 与上游的一处结构性差异（性能，不是语义）
//!
//! 上游用 `WeakSet` 记「本批已浅拷贝的容器」，做到 copy-on-write：只拷被触及的那条路径。
//! Rust 的 `Value` 没有结构共享，深拷贝一次是唯一的「浅拷贝」等价物，所以这里
//! **每批开始克隆一次 root**，之后所有 op 就地改这棵克隆树。语义相同：
//! 入参 `base` 与 op payload 都不会被改，返回的是独立的新值。真要 COW 得把节点换成
//! `Arc<Node>`，等有 profile 数据再说。

use serde_json::Value;

use super::op::{validate_index_in_range, validate_op};
use super::{truncate_utf16_units, Op, OpError, Path, Seg};

/// 本 crate 自己产生的 op 批次。
///
/// 构造它的两条路：crate 内部的 diff / tracker（`unchecked`），或外部 op 走
/// [`TrustedOps::from_decoded`] 逐条校验。**没有别的入口** —— 这就是信任边界。
#[derive(Debug, Clone, PartialEq)]
pub struct TrustedOps(Vec<Op>);

impl TrustedOps {
    /// crate 内部用：`diff_revisions` 与 tracker 产出的批次本来就是合法的。
    pub(crate) fn unchecked(ops: Vec<Op>) -> Self {
        TrustedOps(ops)
    }

    /// 外部 op 的唯一入口：逐条校验后才可信。
    pub fn from_decoded(ops: Vec<Op>) -> Result<Self, OpError> {
        for op in &ops {
            validate_op(op)?;
        }
        Ok(TrustedOps(ops))
    }

    pub fn as_ops(&self) -> &[Op] {
        &self.0
    }

    pub fn into_ops(self) -> Vec<Op> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// 应用一批**可信** op，返回新值；`base` 与 op 里的值都不被修改。
///
/// 上游在类型上是 `applyImmutableTrusted<T>(target: T, ops: Op[]): T`，注释写明：
/// 这个函数不做完整 op 校验、不校验 payload 严格性，路径/类型错就直接抛。
pub fn apply_immutable_trusted(base: &Value, ops: &TrustedOps) -> Result<Value, OpError> {
    let mut root = base.clone();
    for op in ops.as_ops() {
        apply_one(&mut root, op)?;
    }
    Ok(root)
}

/// 就地应用一批 op（先由调用方保证 base 是独占的，比如刚从 `clone()` 拿到的）。
pub fn apply_in_place(root: &mut Value, ops: &[Op]) -> Result<(), OpError> {
    for op in ops {
        apply_one(root, op)?;
    }
    Ok(())
}

fn apply_one(root: &mut Value, op: &Op) -> Result<(), OpError> {
    match op {
        Op::Replace(value) => {
            // 上游注释：adopted, not copied —— 消费者拥有交给它的那一批。
            // Rust 里 clone 一次，代价清楚，也不会让调用方与返回值共享。
            *root = value.clone();
            Ok(())
        }
        Op::Splice(path, start, delete, items) => {
            let target = resolve_mut(root, path)?;
            let array = target
                .as_array_mut()
                .ok_or_else(|| OpError::UnresolvablePath(path.clone()))?;
            splice(array, *start, *delete, items);
            Ok(())
        }
        Op::Move(path, permutation) => {
            let target = resolve_mut(root, path)?;
            let array = target
                .as_array_mut()
                .ok_or_else(|| OpError::UnresolvablePath(path.clone()))?;
            if array.len() != permutation.len() {
                return Err(OpError::UnresolvablePath(path.clone()));
            }
            let previous = array.clone();
            for (index, source) in permutation.iter().enumerate() {
                array[index] = previous[*source].clone();
            }
            Ok(())
        }
        Op::Set(path, value) => {
            let (parent, key) = resolve_parent_mut(root, path)?;
            write(parent, key, value.clone())
        }
        Op::Remove(path) => {
            let (parent, key) = resolve_parent_mut(root, path)?;
            match parent {
                Value::Array(items) => {
                    let index = key.as_index().ok_or(OpError::UnsafePath(
                        "array deletions need a numeric index".into(),
                    ))? as usize;
                    if index >= items.len() {
                        return Err(OpError::UnresolvablePath(path.clone()));
                    }
                    items.remove(index);
                    Ok(())
                }
                Value::Object(map) => {
                    let key = key
                        .as_key()
                        .ok_or_else(|| OpError::UnresolvablePath(path.clone()))?;
                    map.remove(key);
                    Ok(())
                }
                _ => Err(OpError::UnresolvablePath(path.clone())),
            }
        }
        Op::Append(path, text) => {
            let (parent, key) = resolve_parent_mut(root, path)?;
            let current = read(parent, key).ok_or_else(|| OpError::UnresolvablePath(path.clone()))?;
            let current = current
                .as_str()
                .ok_or_else(|| OpError::UnresolvablePath(path.clone()))?;
            let next = format!("{current}{text}");
            write(parent, key, Value::String(next))
        }
        Op::Truncate(path, count) => {
            let (parent, key) = resolve_parent_mut(root, path)?;
            let current = read(parent, key).ok_or_else(|| OpError::UnresolvablePath(path.clone()))?;
            let current = current
                .as_str()
                .ok_or_else(|| OpError::UnresolvablePath(path.clone()))?;
            // count 是 **UTF-16 码元**，不是 chars、不是字节 —— 见 `docs/chord-delta-notes.md`。
            let next = truncate_utf16_units(current, *count).to_string();
            write(parent, key, Value::String(next))
        }
    }
}

/// `Array.prototype.splice` 的语义：越界会被夹到合法范围，负数从尾部数。
fn splice(array: &mut Vec<Value>, start: usize, delete: usize, items: &[Value]) {
    let length = array.len();
    let start = start.min(length);
    let delete = delete.min(length - start);
    array.splice(start..start + delete, items.iter().cloned());
}

fn write(parent: &mut Value, key: &Seg, value: Value) -> Result<(), OpError> {
    match parent {
        Value::Array(items) => {
            let index = key
                .as_index()
                .ok_or_else(|| OpError::UnsafePath("array keys must be indices".into()))?
                as usize;
            validate_index_in_range(items.len(), index as u64)?;
            if index == items.len() {
                items.push(value);
            } else {
                items[index] = value;
            }
            Ok(())
        }
        Value::Object(map) => {
            let key = key.as_key().ok_or_else(|| {
                OpError::UnsafePath("object keys must be strings".into())
            })?;
            map.insert(key.to_string(), value);
            Ok(())
        }
        _ => Err(OpError::UnresolvablePath(vec![key.clone()])),
    }
}

fn read<'a>(parent: &'a Value, key: &Seg) -> Option<&'a Value> {
    match (parent, key) {
        (Value::Array(items), Seg::Index(index)) => items.get(*index as usize),
        (Value::Object(map), Seg::Key(key)) => map.get(key),
        _ => None,
    }
}

/// 走路径拿到可写引用。
fn resolve_mut<'a>(root: &'a mut Value, path: &[Seg]) -> Result<&'a mut Value, OpError> {
    let mut node = root;
    for segment in path {
        node = match (node, segment) {
            (Value::Object(map), Seg::Key(key)) => map
                .get_mut(key)
                .ok_or_else(|| OpError::UnresolvablePath(path.to_vec()))?,
            (Value::Array(items), Seg::Index(index)) => items
                .get_mut(*index as usize)
                .ok_or_else(|| OpError::UnresolvablePath(path.to_vec()))?,
            _ => return Err(OpError::UnresolvablePath(path.to_vec())),
        };
    }
    Ok(node)
}

/// 走到父节点，返回 (父, 最后一段)。
fn resolve_parent_mut<'a, 'b>(
    root: &'a mut Value,
    path: &'b Path,
) -> Result<(&'a mut Value, &'b Seg), OpError> {
    let (last, parent_path) = path
        .split_last()
        .ok_or_else(|| OpError::UnresolvablePath(path.clone()))?;
    let parent = resolve_mut(root, parent_path)?;
    if !matches!(parent, Value::Object(_) | Value::Array(_)) {
        return Err(OpError::UnresolvablePath(path.clone()));
    }
    Ok((parent, last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{path, path_with};
    use serde_json::json;

    fn trusted(ops: Vec<Op>) -> TrustedOps {
        TrustedOps::unchecked(ops)
    }

    #[test]
    fn set_delete_append_truncate_and_splice_on_one_document() {
        let base = json!({"a": {"b": "hello"}, "xs": [1, 2, 3], "keep": true});
        let ops = trusted(vec![
            Op::Set(path(&["a", "b"]), json!("bye")),
            Op::Append(path_with(&[Seg::key("a"), Seg::key("b")]), " now".into()),
            Op::Splice(path(&["xs"]), 1, 1, vec![json!(9)]),
            Op::Remove(path(&["keep"])),
        ]);
        let value = apply_immutable_trusted(&base, &ops).unwrap();
        assert_eq!(value, json!({"a": {"b": "bye now"}, "xs": [1, 9, 3]}));
        // base 没被改。
        assert_eq!(base, json!({"a": {"b": "hello"}, "xs": [1, 2, 3], "keep": true}));
    }

    #[test]
    fn truncate_counts_utf16_units_so_emoji_survive_a_roundtrip() {
        let base = json!({"text": "a😀b"});
        // "a😀b" = 1 + 2 + 1 = 4 个 UTF-16 码元。截 2 → 剩 "b"。
        let ops = trusted(vec![Op::Truncate(path(&["text"]), 2)]);
        assert_eq!(
            apply_immutable_trusted(&base, &ops).unwrap(),
            json!({"text": "b"})
        );
        // 截 1 个码元 = 只去掉 'a'，emoji 完整留着。
        let ops = trusted(vec![Op::Truncate(path(&["text"]), 1)]);
        assert_eq!(
            apply_immutable_trusted(&base, &ops).unwrap(),
            json!({"text": "😀b"})
        );
        // 截 3 个码元 = 'a' + emoji，剩 "b"。
        let ops = trusted(vec![Op::Truncate(path(&["text"]), 3)]);
        assert_eq!(
            apply_immutable_trusted(&base, &ops).unwrap(),
            json!({"text": "b"})
        );
        // 截到底。
        let ops = trusted(vec![Op::Truncate(path(&["text"]), 4)]);
        assert_eq!(
            apply_immutable_trusted(&base, &ops).unwrap(),
            json!({"text": ""})
        );
    }

    #[test]
    fn replace_is_the_only_op_that_touches_the_root() {
        let ops = trusted(vec![Op::Replace(json!([1, 2]))]);
        let value = apply_immutable_trusted(&json!({"gone": true}), &ops).unwrap();
        assert_eq!(value, json!([1, 2]));
    }

    #[test]
    fn move_permutes_by_snapshot_not_in_place() {
        let base = json!({"xs": ["a", "b", "c"]});
        let ops = trusted(vec![Op::Move(path(&["xs"]), vec![2, 0, 1])]);
        assert_eq!(
            apply_immutable_trusted(&base, &ops).unwrap(),
            json!({"xs": ["c", "a", "b"]})
        );
        // 长度不匹配是错误，不是静默半置换。
        let ops = trusted(vec![Op::Move(path(&["xs"]), vec![0, 1])]);
        assert!(apply_immutable_trusted(&base, &ops).is_err());
    }

    #[test]
    fn writes_past_the_end_are_rejected_and_exactly_at_the_end_appends() {
        let base = json!({"xs": [1]});
        let ok = trusted(vec![Op::Set(path_with(&[Seg::key("xs"), Seg::index(1)]), json!(2))]);
        assert_eq!(
            apply_immutable_trusted(&base, &ok).unwrap(),
            json!({"xs": [1, 2]})
        );
        let bad = trusted(vec![Op::Set(
            path_with(&[Seg::key("xs"), Seg::index(3)]),
            json!(4),
        )]);
        assert_eq!(
            apply_immutable_trusted(&base, &bad).unwrap_err(),
            OpError::IndexOutOfRange {
                index: 3,
                length: 1
            }
        );
    }

    // ─── 信任边界 ───────────────────────────────────────────────────────────

    #[test]
    fn external_ops_cannot_reach_the_trusted_path_without_validation() {
        // 不可信路径段：只有走 `from_decoded` 才会被拒。
        let unsafe_ops = vec![Op::Set(path(&["__proto__", "isAdmin"]), json!(true))];
        assert_eq!(
            TrustedOps::from_decoded(unsafe_ops.clone()).unwrap_err(),
            OpError::UnsafePath("__proto__".into())
        );
        // 而「可信」路径不校验形状：空路径的 `s` 直接被 apply 拒（UnresolvablePath），
        // 因为由 tracker 产生的批次里不可能有它。
        let root_set = trusted(vec![Op::Set(vec![], json!(1))]);
        let error = apply_immutable_trusted(&json!({}), &root_set).unwrap_err();
        assert_eq!(error, OpError::UnresolvablePath(vec![]));
    }

    #[test]
    fn trusted_ops_round_trip_through_validation() {
        let ops = vec![
            Op::Set(path(&["a"]), json!(1)),
            Op::Append(path(&["a"]), "!".into()),
        ];
        let trusted = TrustedOps::from_decoded(ops.clone()).unwrap();
        assert_eq!(trusted.as_ops(), &ops[..]);
        assert_eq!(trusted.len(), 2);
        assert!(!trusted.is_empty());
    }
}
