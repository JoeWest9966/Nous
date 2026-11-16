//! 慢消费者的挂起队列：还没取走的 op，等的时候顺手折叠。
//!
//! 四个状态（`empty / batch / folding / replace`）、折叠规则、`10_000` 溢出都与上游
//! `pending.ts` 一一对应 —— 它本身就是纯算法，没什么可改形状的。
//!
//! 两条容易写错的：
//!
//! - **叶子索引的 key 必须单射**：`["a.b"]` 与 `["a","b"]` 是不同的路径，用字符串拼 key
//!   会把两条写入合成一条，静默丢更新。这里用 `Path = Vec<Seg>` 直接当 `HashMap` key。
//! - **入队批次永不被修改**：批次以 `Arc<Vec<Op>>` 入队，合并只造新 op，`take()` 交出去的
//!   是 `Arc` —— 同一批喂给多个消费者是安全的。

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::{Op, Path};

/// 超过这个数，挂起状态只剩「用当前值替换」。
pub const MAX_PENDING_OPS: usize = 10_000;

/// `take()` 交出去的东西：折叠后的 op，或溢出后的 `Replace`（由队列的主人把它变成一次
/// 整根替换）。
#[derive(Debug, Clone, PartialEq)]
pub enum PendingTake {
    Ops(Arc<Vec<Op>>),
    Replace,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Empty,
    /// 只积了一批：按引用留着，跟得上的消费者从不付折叠成本。
    Batch(Arc<Vec<Op>>),
    Folding {
        ops: Vec<Op>,
        /// 路径 → 该路径上最后一条叶子 op 的位置。
        leaves: HashMap<Path, usize>,
    },
    /// 溢出了：`take()` 之前、或以 `r` 开头的批次到来之前，什么都不再积。
    Replace,
}

/// 一个状态对一个消费者的挂起队列。
#[derive(Debug, Default)]
pub struct PendingOps {
    state: State,
}

impl PendingOps {
    pub fn new() -> Self {
        Self::default()
    }

    /// 入队一次提交的 op。
    pub fn push(&mut self, ops: Arc<Vec<Op>>) {
        if ops.is_empty() {
            return;
        }
        // 以 `r` 开头的批次（tracker 与 diff 唯一会放 `r` 的地方）丢弃全部积压（含溢出态），
        // 并原样保留该批次。
        if super::is_base(ops.as_slice()) {
            self.state = State::Batch(ops);
            return;
        }
        match std::mem::replace(&mut self.state, State::Empty) {
            State::Empty => self.state = State::Batch(ops),
            State::Batch(previous) => {
                self.state = State::Folding {
                    ops: Vec::new(),
                    leaves: HashMap::new(),
                };
                let folded_previous = self.fold(previous.as_slice());
                if folded_previous {
                    self.fold(ops.as_slice());
                }
            }
            State::Folding { ops: pending, leaves } => {
                self.state = State::Folding {
                    ops: pending,
                    leaves,
                };
                self.fold(ops.as_slice());
            }
            State::Replace => self.state = State::Replace,
        }
    }

    /// 折叠后的挂起条数；溢出后是 1（那是队列主人要发的一次替换）。
    ///
    /// 单批按原样计一次 —— 所以一次大于 `MAX_PENDING_OPS` 的提交会完整算作挂起，
    /// 直到第二批到达才溢出。
    pub fn size(&self) -> usize {
        match &self.state {
            State::Empty => 0,
            State::Replace => 1,
            State::Batch(ops) => ops.len(),
            State::Folding { ops, .. } => ops.len(),
        }
    }

    /// 交出全部挂起并回到空。
    pub fn take(&mut self) -> PendingTake {
        match std::mem::replace(&mut self.state, State::Empty) {
            State::Empty => PendingTake::Ops(Arc::new(Vec::new())),
            State::Batch(ops) => PendingTake::Ops(ops),
            State::Folding { ops, .. } => PendingTake::Ops(Arc::new(ops)),
            State::Replace => PendingTake::Replace,
        }
    }

    /// 把 `ops` 折进当前 folding 状态；返回 false 表示已经溢出。
    fn fold(&mut self, ops: &[Op]) -> bool {
        let State::Folding {
            ops: pending,
            leaves,
        } = &mut self.state
        else {
            return true;
        };
        for op in ops {
            match op {
                Op::Replace(_) => {
                    pending.clear();
                    leaves.clear();
                }
                Op::Append(..) => {
                    if merge_leaf(op, pending, leaves) {
                        continue;
                    }
                }
                Op::Set(_, value) => {
                    if is_primitive(value) {
                        if merge_leaf(op, pending, leaves) {
                            continue;
                        }
                    } else if !leaves.is_empty() {
                        leaves.clear();
                    }
                }
                _ => {
                    if !leaves.is_empty() {
                        leaves.clear();
                    }
                }
            }
            pending.push(op.clone());
            if pending.len() > MAX_PENDING_OPS {
                self.state = State::Replace;
                return false;
            }
        }
        true
    }
}

fn is_primitive(value: &Value) -> bool {
    !value.is_object() && !value.is_array()
}

/// 把叶子 op 并进同一路径上的最后一条叶子 op；并进去返回 true，否则把它记成「即将 push
/// 到的那个位置」。
fn merge_leaf(op: &Op, pending: &mut [Op], leaves: &mut HashMap<Path, usize>) -> bool {
    let path = op.path().expect("leaf ops carry a path").clone();
    if let Some(&at) = leaves.get(&path) {
        // 索引只指向叶子 op。
        if let Some(merged) = merge(&pending[at], op) {
            pending[at] = merged;
            return true;
        }
    }
    leaves.insert(path, pending.len());
    false
}

/// `next` 折进 `previous`（同路径），折不动就是 `None`。
fn merge(previous: &Op, next: &Op) -> Option<Op> {
    // 设值覆盖设值或追加。
    if let Op::Set(path, value) = next {
        return Some(Op::Set(path.clone(), value.clone()));
    }
    let Op::Append(next_path, next_text) = next else {
        return None;
    };
    match previous {
        Op::Append(previous_path, previous_text) => Some(Op::Append(
            previous_path.clone(),
            format!("{previous_text}{next_text}"),
        )),
        // 只有字符串设值能吃下追加；tracker 不会给别的原始值追加。
        Op::Set(path, Value::String(text)) if path == next_path => Some(Op::Set(
            path.clone(),
            Value::String(format!("{text}{next_text}")),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{path, path_with, Seg};
    use serde_json::json;

    fn batch(ops: Vec<Op>) -> Arc<Vec<Op>> {
        Arc::new(ops)
    }

    /// 每条 op 落在不同路径上 —— 折不动，才能把队列真堆到上限。
    fn distinct_sets(count: usize) -> Vec<Op> {
        (0..count)
            .map(|index| Op::Set(vec![Seg::Key(format!("k{index}"))], json!(index)))
            .collect()
    }

    fn taken(pending: &mut PendingOps) -> Vec<Op> {
        match pending.take() {
            PendingTake::Ops(ops) => ops.as_ref().clone(),
            PendingTake::Replace => panic!("expected ops, got replace"),
        }
    }

    #[test]
    fn a_single_batch_is_kept_by_reference() {
        let ops = vec![Op::Set(path(&["a"]), json!(1))];
        let shared = batch(ops.clone());
        let mut pending = PendingOps::new();
        pending.push(shared.clone());
        assert_eq!(pending.size(), 1);
        match pending.take() {
            PendingTake::Ops(taken) => {
                // 同一个 Arc：跟得上的消费者零折叠成本。
                assert!(Arc::ptr_eq(&taken, &shared));
            }
            PendingTake::Replace => panic!("expected ops"),
        }
    }

    #[test]
    fn appends_on_the_same_path_join_into_one_leaf() {
        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Append(path(&["t"]), "a".into())]));
        pending.push(batch(vec![
            Op::Append(path(&["t"]), "b".into()),
            Op::Append(path(&["t"]), "c".into()),
        ]));
        assert_eq!(pending.size(), 1);
        assert_eq!(taken(&mut pending), vec![Op::Append(path(&["t"]), "abc".into())]);
    }

    #[test]
    fn a_set_replaces_a_pending_set_and_a_string_set_absorbs_an_append() {
        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Set(path(&["a"]), json!("x"))]));
        pending.push(batch(vec![Op::Append(path(&["a"]), "y".into())]));
        // a 跟在字符串 s 后接进它。
        assert_eq!(taken(&mut pending), vec![Op::Set(path(&["a"]), json!("xy"))]);

        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Set(path(&["a"]), json!(1))]));
        pending.push(batch(vec![Op::Set(path(&["a"]), json!(2))]));
        assert_eq!(taken(&mut pending), vec![Op::Set(path(&["a"]), json!(2))]);
    }

    #[test]
    fn structural_ops_clear_the_leaf_index() {
        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Set(path(&["a"]), json!("ab"))]));
        pending.push(batch(vec![Op::Remove(path(&["a"]))]));
        pending.push(batch(vec![Op::Append(path(&["a"]), "c".into())]));
        // 结构 op 之后，append 不再折进那条旧的 `s`。
        assert_eq!(
            taken(&mut pending),
            vec![
                Op::Set(path(&["a"]), json!("ab")),
                Op::Remove(path(&["a"])),
                Op::Append(path(&["a"]), "c".into()),
            ]
        );
    }

    #[test]
    fn distinct_paths_never_merge_even_when_a_string_key_would_collide() {
        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Append(path(&["a.b"]), "x".into())]));
        pending.push(batch(vec![Op::Append(path(&["a", "b"]), "y".into())]));
        // 字符串拼 key 会把这两条合成 "xy"，静默丢一条写入。
        assert_eq!(pending.size(), 2);
        // 数字 1 与字符串 "1" 在同一索引生命周期里可以共键（结构 op 会清索引），
        // 但这里是不同长度、不同层级，必须分开。
        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Set(
            path_with(&[Seg::key("xs"), Seg::index(1)]),
            json!("i"),
        )]));
        pending.push(batch(vec![Op::Set(
            path_with(&[Seg::key("xs"), Seg::key("1")]),
            json!("k"),
        )]));
        assert_eq!(pending.size(), 2);
    }

    #[test]
    fn a_batch_starting_with_a_replace_drops_everything_pending() {
        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Append(path(&["t"]), "a".into())]));
        pending.push(batch(vec![Op::Append(path(&["t"]), "b".into())]));
        pending.push(batch(vec![Op::Replace(json!({"fresh": true}))]));
        assert_eq!(
            taken(&mut pending),
            vec![Op::Replace(json!({"fresh": true}))]
        );
        // 溢出态也一样被 r 打断。
        let mut pending = PendingOps::new();
        pending.push(batch(distinct_sets(MAX_PENDING_OPS + 1)));
        pending.push(batch(vec![Op::Set(path(&["b"]), json!(2))]));
        assert_eq!(pending.take(), PendingTake::Replace);
        pending.push(batch(vec![Op::Replace(json!(null))]));
        assert_eq!(taken(&mut pending), vec![Op::Replace(json!(null))]);
    }

    #[test]
    fn one_oversized_batch_counts_in_full_until_a_second_one_arrives() {
        let mut pending = PendingOps::new();
        pending.push(batch(distinct_sets(MAX_PENDING_OPS + 5)));
        // 单批按原样计一次，不折叠、不溢出。
        assert_eq!(pending.size(), MAX_PENDING_OPS + 5);
        assert!(matches!(pending.take(), PendingTake::Ops(_)));

        // 第二批一到就溢出成 replace。
        let mut pending = PendingOps::new();
        pending.push(batch(distinct_sets(MAX_PENDING_OPS + 5)));
        pending.push(batch(vec![Op::Set(path(&["b"]), json!(1))]));
        assert_eq!(pending.size(), 1);
        assert_eq!(pending.take(), PendingTake::Replace);
    }

    #[test]
    fn folding_cannot_grow_past_the_cap_one_op_at_a_time() {
        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Set(path(&["a"]), json!(0))]));
        // 每条都在不同路径，折不动 → 线性堆积，第 MAX+1 条时溢出。
        pending.push(batch(
            (0..=MAX_PENDING_OPS)
                .map(|index| Op::Set(vec![Seg::Key(format!("k{index}"))], json!(index)))
                .collect(),
        ));
        assert_eq!(pending.take(), PendingTake::Replace);
    }

    #[test]
    fn take_is_atomic_and_leaves_the_queue_empty() {
        let mut pending = PendingOps::new();
        pending.push(batch(vec![Op::Set(path(&["a"]), json!(1))]));
        assert_eq!(pending.size(), 1);
        let _ = pending.take();
        assert_eq!(pending.size(), 0);
        assert_eq!(taken(&mut pending), Vec::<Op>::new());
        // 空批入队是 no-op。
        pending.push(batch(vec![]));
        assert_eq!(pending.size(), 0);
    }

    #[test]
    fn a_received_batch_is_never_mutated() {
        let first = batch(vec![Op::Append(path(&["t"]), "a".into())]);
        let snapshot = first.as_ref().clone();
        let mut pending = PendingOps::new();
        pending.push(first.clone());
        pending.push(batch(vec![Op::Append(path(&["t"]), "b".into())]));
        let _ = pending.take();
        // 第一批原样还在（别的消费者可能还拿着它）。
        assert_eq!(first.as_ref(), &snapshot);
    }
}
