//! op 的词汇校验 —— 对应上游 `assertValidOp` / `assertSafePath` / `assertPermutation`。
//!
//! 上游用 `asserts op is Op` 的类型断言 + `throw`；Rust 里类型已经把大部分形状钉住了，
//! 所以这里只剩三件真事：**路径非空**、**路径段安全**、**置换是双射**。边界处（`decode`）
//! 还要多做一条：`p`/`m` 之外的动词不许指向根。
//!
//! 为什么要显式拒绝保留段名：Rust 的 `serde_json` 没有原型链，天然免疫 `__proto__` 污染；
//! 但 op 是**跨实现的数据**，别的语言写的那一端未必免疫，错误要在这里就可见。

use super::{is_reserved, Op, OpError, Seg};

/// 路径段只许是字符串键或非负整数下标，且拒绝保留段名。
pub fn validate_path(path: &[Seg]) -> Result<(), OpError> {
    for segment in path {
        match segment {
            Seg::Key(key) => {
                if is_reserved(key) {
                    return Err(OpError::UnsafePath(key.clone()));
                }
            }
            Seg::Index(_) => {}
        }
    }
    Ok(())
}

/// 非空路径的版本（`s`/`d`/`a`/`t` 用）。
pub fn validate_non_empty_path(path: &[Seg]) -> Result<(), OpError> {
    if path.is_empty() {
        return Err(OpError::EmptyPath);
    }
    validate_path(path)
}

/// `m` 的置换必须是双射：长度一致、下标在界内、每个下标恰好一次。
///
/// 非双射会让元素重复或丢失，副本与发布值不同 —— 而且是静默的。
pub fn validate_permutation(permutation: &[usize]) -> Result<(), OpError> {
    let length = permutation.len();
    let mut seen = vec![false; length];
    for &index in permutation {
        if index >= length || seen[index] {
            return Err(OpError::NotABijection);
        }
        seen[index] = true;
    }
    Ok(())
}

/// 校验一条**已解码**的 op。`apply` 在动手前调它。
pub fn validate_op(op: &Op) -> Result<(), OpError> {
    match op {
        Op::Replace(_) => Ok(()),
        Op::Set(path, _) | Op::Remove(path) | Op::Append(path, _) | Op::Truncate(path, _) => {
            validate_non_empty_path(path)
        }
        Op::Splice(path, _, _, _) => validate_path(path),
        Op::Move(path, permutation) => {
            validate_path(path)?;
            validate_permutation(permutation)
        }
    }
}

/// 写进数组的下标只能命中现有元素，或恰好 `length`（追加一个）。
///
/// 这不是随便定的上限：稀疏数组过不了 JSON 往返（洞序列化成 `null`，回来是真实属性），
/// 于是 `arr[7] = x`（长度 3）已经产生副本无法匹配的状态；它也顺手挡掉
/// `["s", ["xs", 4294967290], 1]` 这种一条 op 分配 40 亿元素的 DoS。
pub fn validate_index_in_range(parent_len: usize, index: u64) -> Result<(), OpError> {
    if index > parent_len as u64 {
        return Err(OpError::IndexOutOfRange {
            index,
            length: parent_len,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{path, path_with};
    use serde_json::json;

    #[test]
    fn set_delete_append_truncate_reject_the_root() {
        for op in [
            Op::Set(vec![], json!(1)),
            Op::Remove(vec![]),
            Op::Append(vec![], "x".into()),
            Op::Truncate(vec![], 1),
        ] {
            assert_eq!(validate_op(&op), Err(OpError::EmptyPath), "{op:?}");
        }
    }

    #[test]
    fn splice_and_move_may_target_the_root() {
        assert_eq!(
            validate_op(&Op::Splice(vec![], 0, 0, vec![json!(1)])),
            Ok(())
        );
        assert_eq!(validate_op(&Op::Move(vec![], vec![0])), Ok(()));
    }

    #[test]
    fn reserved_segments_are_rejected_wherever_they_appear() {
        let op = Op::Set(path(&["a", "__proto__", "isAdmin"]), json!(true));
        assert_eq!(
            validate_op(&op),
            Err(OpError::UnsafePath("__proto__".into()))
        );
        assert!(validate_path(&path(&["constructor"])).is_err());
        assert!(validate_path(&path_with(&[Seg::index(0), Seg::key("prototype")])).is_err());
        assert!(validate_path(&path(&["ok", "fine"])).is_ok());
    }

    #[test]
    fn permutation_must_be_a_bijection() {
        assert_eq!(validate_permutation(&[0, 1, 2]), Ok(()));
        assert_eq!(validate_permutation(&[2, 0, 1]), Ok(()));
        assert_eq!(validate_permutation(&[]), Ok(()));
        // 越界
        assert_eq!(validate_permutation(&[0, 3]), Err(OpError::NotABijection));
        // 重复（长度对，但不双射）
        assert_eq!(validate_permutation(&[0, 0]), Err(OpError::NotABijection));
        // 少了
        assert_eq!(validate_permutation(&[1]), Err(OpError::NotABijection));
    }

    #[test]
    fn array_writes_stop_at_length_but_allow_append() {
        assert_eq!(validate_index_in_range(3, 3), Ok(()));
        assert_eq!(validate_index_in_range(3, 2), Ok(()));
        assert_eq!(
            validate_index_in_range(3, 4),
            Err(OpError::IndexOutOfRange {
                index: 4,
                length: 3
            })
        );
        // 不会给一条 op 分配四十亿元素的余地。
        assert!(validate_index_in_range(3, 4_294_967_290).is_err());
    }
}
