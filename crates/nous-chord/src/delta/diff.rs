//! 从两个不可变修订算出一批紧凑 op（上游 `diff.ts`，525 行）。
//!
//! 两条纪律：
//!
//! - **精确但不规范。** 同一个结果可以由不同 op 组合表达；判据只有一条 ——
//!   `apply(base, diff(base, after)) == after`。测试不许比对 op 元组。
//!   （所以数组里那条 280 行的 identity→semantic→LCS 选锚点逻辑这里简化成
//!   「前后缀对齐 + 一段 splice」，重放结果相同，本地更少的代码。）
//! - **超限退回整根替换。** op 数超过 [`MAX_DELTA_OPERATIONS`]，或估算体积不比整根快照省，
//!   就直接发 `[r, after]`。
//!
//! `t` op 的计数是 **UTF-16 码元**（见 `docs/chord-delta-notes.md`）。

use serde_json::Value;

use super::apply::TrustedOps;
use super::is_reserved;
use super::{overlap, path, Op, Path, Seg};

/// 后缀扫描窗口（上游同名常量）。
pub const DEFAULT_OVERLAP_SCAN: usize = 65_536;
/// 一批里最多这么多 op；再多就整根替换。
pub const MAX_DELTA_OPERATIONS: usize = 4_096;
/// delta 估算体积超过这个数才和整根快照比大小（低于它一律用 delta）。
const SNAPSHOT_COMPARE_THRESHOLD: usize = 65_536;

/// 算出 `before → after` 的一批 op，以 [`TrustedOps`] 交出（它就是本模块自己产出的）。
pub fn diff_revisions(before: &Value, after: &Value) -> TrustedOps {
    let mut operations = Vec::new();
    diff_value(before, after, &mut Vec::new(), &mut operations);
    if operations.len() >= MAX_DELTA_OPERATIONS {
        return TrustedOps::unchecked(vec![Op::Replace(after.clone())]);
    }
    if operations.is_empty() || matches!(operations.first(), Some(Op::Replace(_))) {
        return TrustedOps::unchecked(operations);
    }
    let mut delta_cost = 2usize;
    for operation in &operations {
        delta_cost += operation_cost(operation) + 1;
    }
    if delta_cost < SNAPSHOT_COMPARE_THRESHOLD {
        return TrustedOps::unchecked(operations);
    }
    let snapshot_cost = json_cost(after) + 6;
    if delta_cost >= snapshot_cost {
        TrustedOps::unchecked(vec![Op::Replace(after.clone())])
    } else {
        TrustedOps::unchecked(operations)
    }
}

fn emit(operations: &mut Vec<Op>, operation: Op) {
    if operations.len() >= MAX_DELTA_OPERATIONS {
        // 满了就不再往里塞；`diff_revisions` 会看到 `len >= 上限` 并退回 `r`。
        return;
    }
    operations.push(operation);
}

fn emit_set(path: &Path, value: &Value, operations: &mut Vec<Op>) {
    if path.is_empty() {
        emit(operations, Op::Replace(value.clone()));
    } else {
        emit(operations, Op::Set(path.clone(), value.clone()));
    }
}

fn is_full(operations: &[Op]) -> bool {
    operations.len() >= MAX_DELTA_OPERATIONS
}

fn diff_value(before: &Value, after: &Value, path: &mut Path, operations: &mut Vec<Op>) {
    if before == after || is_full(operations) {
        return;
    }
    if let (Value::String(before), Value::String(after)) = (before, after) {
        if !path.is_empty() {
            emit_string(before, after, path, operations);
            return;
        }
    }
    match (before, after) {
        (Value::Array(before), Value::Array(after)) => diff_array(before, after, path, operations),
        (Value::Object(before), Value::Object(after)) => diff_object(before, after, path, operations),
        _ => emit_set(path, after, operations),
    }
}

/// 字符串：前缀相同发 `a`；有公共前后缀重叠发 `t` + `a`；否则 `s`。
fn emit_string(before: &str, after: &str, path: &mut Path, operations: &mut Vec<Op>) {
    if before == after {
        return;
    }
    if after.len() > before.len() && after.starts_with(before) {
        emit(
            operations,
            Op::Append(path.clone(), after[before.len()..].to_string()),
        );
        return;
    }
    let shared = overlap(before, after, DEFAULT_OVERLAP_SCAN, 64, 8);
    if shared == 0 {
        emit(
            operations,
            Op::Set(path.clone(), Value::String(after.to_string())),
        );
        return;
    }
    // 截掉的是 UTF-16 码元数。`t` 的 apply 也按这个单位 —— 两边必须一致，
    // 否则 emoji / 代理对会悄悄漂（`docs/chord-delta-notes.md` §1.4）。
    let before_units = super::utf16_len(before);
    let after_units = super::utf16_len(after);
    emit(operations, Op::Truncate(path.clone(), before_units - shared));
    if after_units > shared {
        emit(
            operations,
            Op::Append(
                path.clone(),
                super::suffix_utf16_units(after, shared).to_string(),
            ),
        );
    }
}

fn diff_object(
    before: &serde_json::Map<String, Value>,
    after: &serde_json::Map<String, Value>,
    path: &mut Path,
    operations: &mut Vec<Op>,
) {
    // 保留键的对象不能逐键 diff：发出的路径里会带危险段。整块折成祖先的 `s`。
    let has_reserved = before
        .keys()
        .chain(after.keys())
        .any(|key| is_reserved(key));
    if has_reserved {
        if before != after {
            emit_set(path, &Value::Object(after.clone()), operations);
        }
        return;
    }

    for (key, after_value) in after {
        if is_full(operations) {
            return;
        }
        path.push(Seg::Key(key.clone()));
        match before.get(key) {
            Some(before_value) => diff_value(before_value, after_value, path, operations),
            None => emit_set(path, after_value, operations),
        }
        path.pop();
    }
    for key in before.keys() {
        if is_full(operations) {
            return;
        }
        if !after.contains_key(key) {
            path.push(Seg::Key(key.clone()));
            emit(operations, Op::Remove(path.clone()));
            path.pop();
        }
    }
}

fn diff_array(before: &[Value], after: &[Value], path: &mut Path, operations: &mut Vec<Op>) {
    if before == after {
        return;
    }
    // 长度不变、首尾都变、且能构成双射时，发一条 `m`。
    if before.len() == after.len()
        && before.len() > 1
        && before.first() != after.first()
        && before.last() != after.last()
    {
        if let Some(order) = permutation(before, after) {
            emit(operations, Op::Move(path.clone(), order));
            return;
        }
    }
    // 公共前后缀对齐，中间一段换成一条 splice。
    let mut prefix = 0usize;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < before.len() - prefix
        && suffix < after.len() - prefix
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let before_end = before.len() - suffix;
    let after_end = after.len() - suffix;
    if prefix == before_end && prefix == after_end {
        return;
    }
    // 变动区域恰好 1 对 1：**递归进那个元素**，不发 `p`。
    //
    // 上游 `diff.ts:376-379` 同一分支（`beforeCount === 1 && afterCount === 1` →
    // `diffValue(before[beforeStart], after[afterStart], [...path, outputStart])`）。
    // 这条不是优化，是流式回答的命根子：`content` 里一个文本块在自己的 `text` 上变长，
    // 折成 `p` 就是把**整块**（整段已经流出来的文本）重新存一遍、发布一遍 —— 100 ms 一次
    // 节流下每次提交都带全文，正好是 `#14` 要消灭的那种 O(n²)。递归之后那条 `a` 才活得下来
    // （`harness/json.rs` 的 append 记录按路径覆盖 diff 的 op，路径对不上就白记）。
    if before_end - prefix == 1 && after_end - prefix == 1 {
        path.push(Seg::Index(prefix as u64));
        diff_value(&before[prefix], &after[prefix], path, operations);
        path.pop();
        return;
    }
    emit(
        operations,
        Op::Splice(
            path.clone(),
            prefix,
            before_end - prefix,
            after[prefix..after_end].to_vec(),
        ),
    );
}

/// 双射置换：`after[i] = before[permutation[i]]`。重复值按出现次序对齐。
fn permutation(before: &[Value], after: &[Value]) -> Option<Vec<usize>> {
    if before.len() != after.len() {
        return None;
    }
    let mut positions: Vec<(&Value, Vec<usize>, usize)> = Vec::new();
    for (index, value) in before.iter().enumerate() {
        match positions.iter_mut().find(|(seen, _, _)| seen == &value) {
            Some((_, indices, _)) => indices.push(index),
            None => positions.push((value, vec![index], 0)),
        }
    }
    let mut result = Vec::with_capacity(after.len());
    for value in after {
        let entry = positions
            .iter_mut()
            .find(|(seen, _, _)| seen == &value)
            .filter(|(_, indices, used)| *used < indices.len())?;
        result.push(entry.1[entry.2]);
        entry.2 += 1;
    }
    Some(result)
}

// ─── 体积估算（用来决定 delta 还是整根快照）────────────────────────────────

fn json_cost(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => text.chars().count() + 2,
        Value::Array(items) => {
            let mut cost = 2;
            for (index, item) in items.iter().enumerate() {
                cost += json_cost(item) + usize::from(index > 0);
            }
            cost
        }
        Value::Object(map) => {
            let mut cost = 2;
            for (index, (key, item)) in map.iter().enumerate() {
                cost += key.chars().count() + 3 + json_cost(item) + usize::from(index > 0);
            }
            cost
        }
    }
}

fn path_cost(path: &Path) -> usize {
    let mut cost = 2;
    for (index, segment) in path.iter().enumerate() {
        cost += match segment {
            Seg::Key(key) => key.chars().count() + 2,
            Seg::Index(raw) => raw.to_string().len(),
        } + usize::from(index > 0);
    }
    cost
}

fn operation_cost(operation: &Op) -> usize {
    match operation {
        Op::Replace(value) => 6 + json_cost(value),
        Op::Set(path, value) => 7 + path_cost(path) + json_cost(value),
        Op::Remove(path) => 6 + path_cost(path),
        Op::Append(path, text) => 7 + path_cost(path) + text.chars().count() + 2,
        Op::Truncate(path, count) => 7 + path_cost(path) + count.to_string().len(),
        Op::Splice(path, start, delete, items) => {
            10 + path_cost(path)
                + start.to_string().len()
                + delete.to_string().len()
                + json_cost(&Value::Array(items.clone()))
        }
        Op::Move(path, permutation) => {
            7 + path_cost(path) + json_cost(&Value::Array(
                permutation
                    .iter()
                    .map(|index| Value::from(*index))
                    .collect(),
            ))
        }
    }
}

/// 便捷：`diff_revisions` 之后直接应用，验证「重放得到 after」。
pub fn diff_and_apply(before: &Value, after: &Value) -> Value {
    let ops = diff_revisions(before, after);
    super::apply::apply_immutable_trusted(before, &ops).expect("self-produced ops always apply")
}

/// 便捷：把 `after` 拆成一串只有根替换的批次（测试恢复点用）。
pub fn replacements(after: &Value) -> TrustedOps {
    TrustedOps::unchecked(vec![Op::Replace(after.clone())])
}

/// 便捷：从对象键构造一条路径。
pub fn key_path(key: &str) -> Path {
    path(&[key])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diff_ops(before: &Value, after: &Value) -> Vec<Op> {
        diff_revisions(before, after).into_ops()
    }

    /// 单元素变动必须递归，不许折成 `p`：否则流式文本的每次提交都带整块全文。
    #[test]
    fn a_single_changed_array_element_is_diffed_in_place_not_spliced() {
        let before = json!({"content": [{"type": "text", "text": "Hel"}]});
        let after = json!({"content": [{"type": "text", "text": "Hello"}]});
        assert_eq!(
            diff_ops(&before, &after),
            vec![Op::Append(
                vec![Seg::key("content"), Seg::index(0), Seg::key("text")],
                "lo".to_string()
            )]
        );

        // 两端对齐后中间只剩一个元素：同样递归。
        let before = json!({"content": ["head", {"text": "a"}, "tail"]});
        let after = json!({"content": ["head", {"text": "ab"}, "tail"]});
        assert_eq!(
            diff_ops(&before, &after),
            vec![Op::Append(
                vec![Seg::key("content"), Seg::index(1), Seg::key("text")],
                "b".to_string()
            )]
        );

        // 元素里的字符串整个换掉（只共享一个后缀）也还是递归：发 `t` + `a`，不是 `p`。
        let before = json!({"content": [{"type": "text"}]});
        let after = json!({"content": [{"type": "thinking"}]});
        let ops = diff_ops(&before, &after);
        assert!(
            !ops.iter().any(|op| matches!(op, Op::Splice(..))),
            "不该折成 `p`，实际是 {ops:?}"
        );
        assert_eq!(diff_and_apply(&before, &after), after);

        // 区域不是 1 对 1（长出来/删掉了）仍旧发一条 `p`。
        let before = json!({"content": [{"text": "a"}]});
        let after = json!({"content": [{"text": "a"}, {"text": "b"}]});
        assert!(matches!(
            diff_ops(&before, &after).as_slice(),
            [Op::Splice(..)]
        ));
    }

    #[test]
    fn identical_revisions_produce_no_ops() {
        assert!(diff_ops(&json!({"a": 1}), &json!({"a": 1})).is_empty());
        assert!(diff_ops(&json!("x"), &json!("x")).is_empty());
        assert!(diff_ops(&json!([1, 2]), &json!([1, 2])).is_empty());
    }

    #[test]
    fn a_pure_append_emits_one_append_op() {
        let ops = diff_ops(&json!({"t": "héllo"}), &json!({"t": "héllo world"}));
        assert_eq!(ops, vec![Op::Append(key_path("t"), " world".into())]);
    }

    #[test]
    fn a_rolling_window_emits_a_truncate_then_an_append() {
        // "hello world" → "world peace"：`world` 是前者的后缀、后者的前缀。
        let ops = diff_ops(&json!({"t": "hello world"}), &json!({"t": "world peace"}));
        assert_eq!(
            ops,
            vec![
                Op::Truncate(key_path("t"), 6),
                Op::Append(key_path("t"), " peace".into()),
            ]
        );
        assert_eq!(
            diff_and_apply(&json!({"t": "hello world"}), &json!({"t": "world peace"})),
            json!({"t": "world peace"})
        );
    }

    #[test]
    fn a_middle_change_without_a_shared_edge_falls_back_to_a_set() {
        // 没有前后缀重叠 → 一个 `s`，不是一个错误的 `t`。
        let ops = diff_ops(&json!({"t": "abcXYZ"}), &json!({"t": "abc123XYZ"}));
        assert_eq!(
            ops,
            vec![Op::Set(key_path("t"), json!("abc123XYZ"))]
        );
    }

    #[test]
    fn truncate_counts_utf16_units_in_the_diff_too() {
        // before = "😀x" (3 units), after = "x" → 公共后缀 "x" → 截掉 2 个码元（emoji），
        // 不是 1（chars）也不是 4（字节）。
        let ops = diff_ops(&json!({"t": "😀x"}), &json!({"t": "x"}));
        assert_eq!(ops, vec![Op::Truncate(key_path("t"), 2)]);
        // 截到空字符串走 `s`（后缀重叠需要有非空的 after），这是上游同一条规则。
        assert_eq!(
            diff_ops(&json!({"t": "😀"}), &json!({"t": ""})),
            vec![Op::Set(key_path("t"), json!(""))]
        );

        // 重叠跨 emoji：before "😀😀y"（5 码元）, after "😀y!" —— 重叠是 "😀y"（3 码元），
        // 所以截掉 5-3 = 2 个码元 = 一整个 emoji。按 chars 计会得到 1（错的）。
        let ops = diff_ops(&json!({"t": "😀😀y"}), &json!({"t": "😀y!"}));
        assert_eq!(
            ops,
            vec![
                Op::Truncate(key_path("t"), 2),
                Op::Append(key_path("t"), "!".into()),
            ]
        );
        assert_eq!(
            diff_and_apply(&json!({"t": "😀😀y"}), &json!({"t": "😀y!"})),
            json!({"t": "😀y!"})
        );
        // 没有重叠时老老实实发 `s`（不发一个错的 `t`）。
        let ops = diff_ops(&json!({"t": "a😀b"}), &json!({"t": "a😀Xb"}));
        assert_eq!(ops, vec![Op::Set(key_path("t"), json!("a😀Xb"))]);
        assert_eq!(
            diff_and_apply(&json!({"t": "a😀b"}), &json!({"t": "a😀Xb"})),
            json!({"t": "a😀Xb"})
        );
    }

    #[test]
    fn object_changes_emit_sets_and_deletes() {
        let ops = diff_ops(
            &json!({"keep": 1, "gone": 2, "change": 3}),
            &json!({"keep": 1, "change": 4, "added": 5}),
        );
        assert!(ops.contains(&Op::Remove(key_path("gone"))));
        assert!(ops.contains(&Op::Set(key_path("change"), json!(4))));
        assert!(ops.contains(&Op::Set(key_path("added"), json!(5))));
        // 没变的键不发 op。
        assert!(!ops.iter().any(|op| op.path() == Some(&key_path("keep"))));
    }

    #[test]
    fn arrays_diff_as_one_splice_over_the_aligned_middle() {
        let before = json!({"xs": [1, 2, 3, 4]});
        let after = json!({"xs": [1, 9, 9, 4]});
        let ops = diff_ops(&before, &after);
        assert_eq!(
            ops,
            vec![Op::Splice(path(&["xs"]), 1, 2, vec![json!(9), json!(9)])]
        );
        assert_eq!(diff_and_apply(&before, &after), after);
    }

    #[test]
    fn a_full_reorder_emits_a_permutation() {
        let before = json!({"xs": [1, 2, 3]});
        let after = json!({"xs": [3, 1, 2]});
        assert_eq!(
            diff_ops(&before, &after),
            vec![Op::Move(path(&["xs"]), vec![2, 0, 1])]
        );
    }

    #[test]
    fn reserved_key_objects_collapse_to_a_whole_set() {
        let before = json!({"a": {"__proto__": {"isAdmin": false}}});
        let after = json!({"a": {"__proto__": {"isAdmin": true}}});
        let ops = diff_ops(&before, &after);
        // 不许发出带保留段的路径。
        assert!(ops.iter().all(|op| op.path().map_or(true, |path| !path
            .iter()
            .any(|seg| seg.as_key() == Some("__proto__")))));
        assert_eq!(diff_and_apply(&before, &after), after);
    }

    #[test]
    fn huge_diffs_fall_back_to_a_root_replacement() {
        // 每个键都变，op 数超过上限 → 整根替换。
        let mut before = serde_json::Map::new();
        let mut after = serde_json::Map::new();
        for index in 0..(MAX_DELTA_OPERATIONS + 10) {
            before.insert(format!("k{index}"), json!(0));
            after.insert(format!("k{index}"), json!(1));
        }
        let before = Value::Object(before);
        let after = Value::Object(after);
        let ops = diff_ops(&before, &after);
        assert_eq!(ops, vec![Op::Replace(after.clone())]);
    }

    #[test]
    fn the_type_change_from_string_to_number_is_a_set() {
        let before = json!({"a": "1"});
        let after = json!({"a": 1});
        assert_eq!(
            diff_ops(&before, &after),
            vec![Op::Set(key_path("a"), json!(1))]
        );
    }
}
