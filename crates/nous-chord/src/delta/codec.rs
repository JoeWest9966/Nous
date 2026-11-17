//! wire 压缩：路径字典 + 短写。只活在 `encode` 与 `decode` 之间 —— `Op` 本身不知道
//! 字典的存在（上游 `index.ts` 的 `WireOp` 那一段）。
//!
//! 三种压缩，仅此三种：
//!
//! ```text
//! ["#", id, path]   某条路径**第二次**使用时定义 id
//! 数字 ref           引用之前定义过的 id
//! 短写              省略路径，复用本批上一条 op 的路径（靠元数区分）
//! ```
//!
//! 两条硬规矩：
//!
//! - **短写作用域是本批**，不跨批。跨批会让一批的首条 op 依赖上一批的末条 —— 读者跳批
//!   或重排就直接解到错路径上去。只有 id 是跨批状态，而字典把它显式化了。
//! - **`r` 是恢复点**：编码器遇 `r` 清空两侧字典、解码器遇 `r` 清空 id 表。留着 id 会让
//!   恢复后的引用指向读者从没见过的定义，恢复直接失败（`docs/chord-delta-notes.md` §4）。
//!
//! 一个 encoder/decoder 对只服务**一条有序状态流**（同一连接上两个独立水合的会话是两条流）。

use std::collections::HashMap;

use serde_json::{json, Value};

use super::op::validate_path;
use super::{Op, OpError, Path, Seg};

/// 编码一条路径成 JSON 数组（`["a", 3]`）。
pub fn path_to_json(path: &[Seg]) -> Value {
    Value::Array(
        path.iter()
            .map(|segment| match segment {
                Seg::Key(key) => Value::String(key.clone()),
                Seg::Index(index) => json!(index),
            })
            .collect(),
    )
}

/// 从 JSON 数组读一条路径。
pub fn path_from_json(value: &Value) -> Result<Path, OpError> {
    let items = value
        .as_array()
        .ok_or(OpError::Shape {
            verb: "path",
            what: "path is not an array",
        })?;
    let mut path = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::String(key) => path.push(Seg::Key(key.clone())),
            Value::Number(number) => {
                let index = number.as_u64().ok_or(OpError::UnsafePath(item.to_string()))?;
                path.push(Seg::Index(index));
            }
            other => return Err(OpError::UnsafePath(other.to_string())),
        }
    }
    validate_path(&path)?;
    Ok(path)
}

/// 路径的字典键。单射是语义要求（`["a.b"]` 与 `["a","b"]` 不同）；编码只是实现细节。
fn path_key(path: &[Seg]) -> String {
    path_to_json(path).to_string()
}

/// 编码器。**第二次**使用某条路径时才定义 id：定义本身比路径贵，首次就 interna
/// 在「只写一次」的路径上是亏的。
#[derive(Debug, Default)]
pub struct Encoder {
    seen: Vec<String>,
    ids: HashMap<String, usize>,
    next_id: usize,
}

impl Encoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(&mut self, ops: &[Op]) -> Vec<Value> {
        // 短写作用域 = 本批。
        let mut previous: Option<String> = None;
        let mut out = Vec::with_capacity(ops.len());
        for op in ops {
            if let Op::Replace(value) = op {
                out.push(json!(["r", value]));
                // 恢复点：之后的东西必须自给自足。
                self.seen.clear();
                self.ids.clear();
                self.next_id = 0;
                previous = None;
                continue;
            }
            let path = op.path().expect("non-replace ops carry a path");
            let key = path_key(path);
            let short = previous.as_deref() == Some(key.as_str());

            if short {
                match op {
                    Op::Set(_, value) => out.push(json!(["s", value])),
                    Op::Remove(_) => out.push(json!(["d"])),
                    Op::Append(_, text) => out.push(json!(["a", text])),
                    Op::Truncate(_, count) => out.push(json!(["t", count])),
                    Op::Splice(_, start, delete, items) => {
                        out.push(json!(["p", start, delete, items]))
                    }
                    Op::Move(_, permutation) => out.push(json!(["m", permutation])),
                    Op::Replace(_) => unreachable!(),
                }
                continue;
            }

            let reference = if let Some(id) = self.ids.get(&key) {
                Value::from(*id)
            } else if self.seen.iter().any(|seen| seen == &key) {
                let id = self.next_id;
                self.next_id += 1;
                self.ids.insert(key.clone(), id);
                // 第二次使用：先定义，再引用。
                out.push(json!(["#", id, path_to_json(path)]));
                Value::from(id)
            } else {
                // 第一次使用：内联。
                self.seen.push(key.clone());
                path_to_json(path)
            };

            match op {
                Op::Set(_, value) => out.push(json!(["s", reference, value])),
                Op::Remove(_) => out.push(json!(["d", reference])),
                Op::Append(_, text) => out.push(json!(["a", reference, text])),
                Op::Truncate(_, count) => out.push(json!(["t", reference, count])),
                Op::Splice(_, start, delete, items) => {
                    out.push(json!(["p", reference, start, delete, items]))
                }
                Op::Move(_, permutation) => out.push(json!(["m", reference, permutation])),
                Op::Replace(_) => unreachable!(),
            }
            previous = Some(key);
        }
        out
    }
}

/// 解码器。每条 op 在使用前都会校验（等价于上游的 `assertValidWireOp`）。
#[derive(Debug, Default)]
pub struct Decoder {
    paths: HashMap<usize, Path>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn decode(&mut self, wire: &[Value]) -> Result<Vec<Op>, OpError> {
        let mut previous: Option<Path> = None;
        let mut out = Vec::with_capacity(wire.len());
        for op in wire {
            let tuple = op.as_array().ok_or(OpError::Shape {
                verb: "wire",
                what: "op is not a tuple",
            })?;
            let verb = tuple
                .first()
                .and_then(Value::as_str)
                .ok_or(OpError::Shape {
                    verb: "wire",
                    what: "op verb is not a string",
                })?;

            if verb == "#" {
                if tuple.len() != 3 {
                    return Err(OpError::Arity {
                        verb: "#",
                        expected: 3,
                    });
                }
                let id = tuple[1].as_u64().ok_or(OpError::Shape {
                    verb: "#",
                    what: "id",
                })? as usize;
                let path = path_from_json(&tuple[2])?;
                self.paths.insert(id, path);
                continue;
            }

            if verb == "r" {
                if tuple.len() != 2 {
                    return Err(OpError::Arity {
                        verb: "r",
                        expected: 2,
                    });
                }
                out.push(Op::Replace(tuple[1].clone()));
                // 恢复点：字典在这之后必须重新攒。
                self.paths.clear();
                previous = None;
                continue;
            }

            // 元数告诉我们有没有 ref：短写省掉了它。
            let short = match verb {
                "d" => tuple.len() == 1,
                "p" => tuple.len() == 4,
                "s" | "a" | "t" | "m" => tuple.len() == 2,
                other => return Err(OpError::UnknownVerb(other.to_string())),
            };

            let path: Path = if short {
                previous.clone().ok_or(OpError::EmptyPath)?
            } else {
                let reference = &tuple[1];
                let resolved = if let Some(id) = reference.as_u64() {
                    self.paths
                        .get(&(id as usize))
                        .cloned()
                        .ok_or(OpError::UnknownPathId(id))?
                } else {
                    path_from_json(reference)?
                };
                previous = Some(resolved.clone());
                resolved
            };

            let path_is_empty = path.is_empty();
            let op = match verb {
                "s" => {
                    let value = if short { &tuple[1] } else { tuple.get(2).ok_or(
                        OpError::Arity {
                            verb: "s",
                            expected: 3,
                        },
                    )? };
                    Op::Set(path, value.clone())
                }
                "d" => Op::Remove(path),
                "a" => {
                    let text = if short { &tuple[1] } else { tuple.get(2).ok_or(
                        OpError::Arity {
                            verb: "a",
                            expected: 3,
                        },
                    )? };
                    Op::Append(
                        path,
                        text.as_str()
                            .ok_or(OpError::Shape {
                                verb: "a",
                                what: "value",
                            })?
                            .to_string(),
                    )
                }
                "t" => {
                    let count = if short { &tuple[1] } else { tuple.get(2).ok_or(
                        OpError::Arity {
                            verb: "t",
                            expected: 3,
                        },
                    )? };
                    Op::Truncate(
                        path,
                        count.as_u64().ok_or(OpError::Shape {
                            verb: "t",
                            what: "count",
                        })? as usize,
                    )
                }
                "p" => {
                    let (start, delete, items) = if short {
                        (&tuple[1], &tuple[2], &tuple[3])
                    } else {
                        (
                            tuple.get(2).ok_or(OpError::Arity {
                                verb: "p",
                                expected: 5,
                            })?,
                            tuple.get(3).ok_or(OpError::Arity {
                                verb: "p",
                                expected: 5,
                            })?,
                            tuple.get(4).ok_or(OpError::Arity {
                                verb: "p",
                                expected: 5,
                            })?,
                        )
                    };
                    let start = start
                        .as_u64()
                        .ok_or(OpError::Shape {
                            verb: "p",
                            what: "index",
                        })? as usize;
                    let delete = delete
                        .as_u64()
                        .ok_or(OpError::Shape {
                            verb: "p",
                            what: "remove",
                        })? as usize;
                    let items = items
                        .as_array()
                        .ok_or(OpError::Shape {
                            verb: "p",
                            what: "items",
                        })?
                        .clone();
                    Op::Splice(path, start, delete, items)
                }
                "m" => {
                    let permutation = if short { &tuple[1] } else { tuple.get(2).ok_or(
                        OpError::Arity {
                            verb: "m",
                            expected: 3,
                        },
                    )? };
                    let permutation = permutation
                        .as_array()
                        .ok_or(OpError::Shape {
                            verb: "m",
                            what: "permutation",
                        })?
                        .iter()
                        .map(|item| {
                            item.as_u64().map(|n| n as usize).ok_or(OpError::Shape {
                                verb: "m",
                                what: "permutation index",
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    super::op::validate_permutation(&permutation)?;
                    Op::Move(path, permutation)
                }
                other => return Err(OpError::UnknownVerb(other.to_string())),
            };

            // `s`/`d`/`a`/`t` 不许指向根；`p`/`m` 可以。
            if !matches!(op, Op::Splice(..) | Op::Move(..)) && path_is_empty {
                return Err(OpError::EmptyPath);
            }
            out.push(op);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{path, path_with};

    fn roundtrip(ops: Vec<Op>) -> Vec<Op> {
        let wire = Encoder::new().encode(&ops);
        Decoder::new().decode(&wire).expect("decode")
    }

    #[test]
    fn first_use_inlines_and_second_use_defines_an_id() {
        let ops = vec![
            Op::Set(path(&["a", "b"]), json!(1)),
            Op::Set(path(&["c"]), json!(3)),
            Op::Set(path(&["a", "b"]), json!(2)),
            Op::Set(path(&["a", "b"]), json!(4)),
        ];
        let wire = Encoder::new().encode(&ops);
        // 第 1 条内联；第 2 条内联；第 3 条是第二次使用 → 定义 id 0 并引用；
        // 第 4 条与上一条同路径 → 短写。
        assert_eq!(wire[0], json!(["s", ["a", "b"], 1]));
        assert_eq!(wire[1], json!(["s", ["c"], 3]));
        assert_eq!(wire[2], json!(["#", 0, ["a", "b"]]));
        assert_eq!(wire[3], json!(["s", 0, 2]));
        assert_eq!(wire[4], json!(["s", 4]), "同路径连续出现时走短写");
        assert_eq!(roundtrip(ops.clone()), ops);
    }

    #[test]
    fn short_form_reuses_the_previous_path_within_the_batch_only() {
        let ops = vec![
            Op::Set(path(&["x"]), json!("v")),
            Op::Append(path(&["x"]), "y".into()),
            Op::Truncate(path(&["x"]), 1),
            Op::Remove(path(&["x"])),
        ];
        let wire = Encoder::new().encode(&ops);
        assert_eq!(wire[0], json!(["s", ["x"], "v"]));
        assert_eq!(wire[1], json!(["a", "y"]));
        assert_eq!(wire[2], json!(["t", 1]));
        assert_eq!(wire[3], json!(["d"]));
        assert_eq!(roundtrip(ops.clone()), ops);

        // 短写不跨批：下一批第一条必须自带路径引用（内联路径或 id），不能是短写形式。
        // id 是**唯一**的跨批状态，所以这里走「第二次使用 → 定义 id」：`#` + 引用。
        let mut encoder = Encoder::new();
        let _ = encoder.encode(&[Op::Set(path(&["x"]), json!("v"))]);
        let second = encoder.encode(&[Op::Append(path(&["x"]), "y".into())]);
        assert_eq!(second[0], json!(["#", 0, ["x"]]), "跨批保留 id，显式定义");
        assert_eq!(second[1], json!(["a", 0, "y"]), "不是短写 `[a, y]`");
        // 一条全新的 encoder 里，路径是「第一次使用」→ 内联。
        let mut fresh = Encoder::new();
        assert_eq!(
            fresh.encode(&[Op::Append(path(&["x"]), "y".into())]),
            vec![json!(["a", ["x"], "y"])]
        );
    }

    #[test]
    fn a_replace_resets_both_side_dictionaries() {
        let ops = vec![
            Op::Set(path(&["a"]), json!(1)),
            Op::Set(path(&["b"]), json!(9)),
            Op::Set(path(&["a"]), json!(2)),
            Op::Replace(json!({"a": 2, "b": 9})),
            // 恢复点之后必须自给自足：这里又是「第一次使用」→ 内联。
            Op::Set(path(&["a"]), json!(3)),
            Op::Set(path(&["b"]), json!(9)),
            Op::Set(path(&["a"]), json!(4)),
        ];
        let wire = Encoder::new().encode(&ops);
        assert_eq!(wire[0], json!(["s", ["a"], 1]));
        assert_eq!(wire[1], json!(["s", ["b"], 9]));
        assert_eq!(wire[2], json!(["#", 0, ["a"]]));
        assert_eq!(wire[3], json!(["s", 0, 2]));
        assert_eq!(wire[4], json!(["r", {"a": 2, "b": 9}]));
        assert_eq!(wire[5], json!(["s", ["a"], 3]));
        assert_eq!(wire[6], json!(["s", ["b"], 9]));
        assert_eq!(
            wire[7],
            json!(["#", 0, ["a"]]),
            "恢复后 id 从 0 重新数"
        );
        assert_eq!(wire[8], json!(["s", 0, 4]));
        assert_eq!(roundtrip(ops.clone()), ops);
    }

    #[test]
    fn decoder_rejects_an_id_it_never_saw() {
        // 恢复点是 r：解码器在 r 处清空 id 表，之后引用旧 id 必须失败。
        let wire = vec![
            json!(["s", ["a"], 1]),
            json!(["#", 0, ["a"]]),
            json!(["s", 0, 2]),
            json!(["r", {}]),
            json!(["s", 0, 3]),
        ];
        let error = Decoder::new().decode(&wire).unwrap_err();
        assert_eq!(error, OpError::UnknownPathId(0));
    }

    #[test]
    fn decoder_rejects_unknown_verbs_instead_of_skipping_them() {
        let error = Decoder::new()
            .decode(&[json!(["z", ["a"], 1])])
            .unwrap_err();
        assert_eq!(error, OpError::UnknownVerb("z".into()));
    }

    #[test]
    fn decoder_rejects_short_form_without_a_previous_path() {
        let error = Decoder::new().decode(&[json!(["a", "x"])]).unwrap_err();
        assert_eq!(error, OpError::EmptyPath);
    }

    #[test]
    fn decoder_rejects_root_targets_for_path_ops_but_allows_them_for_splices() {
        assert_eq!(
            Decoder::new().decode(&[json!(["s", [], 1])]).unwrap_err(),
            OpError::EmptyPath
        );
        assert_eq!(
            Decoder::new()
                .decode(&[json!(["p", [], 0, 0, [1]])])
                .unwrap(),
            vec![Op::Splice(vec![], 0, 0, vec![json!(1)])]
        );
    }

    #[test]
    fn decoder_rejects_a_non_bijective_permutation() {
        assert_eq!(
            Decoder::new().decode(&[json!(["m", ["a"], [0, 0]])]).unwrap_err(),
            OpError::NotABijection
        );
    }

    #[test]
    fn decoder_rejects_reserved_segments() {
        assert_eq!(
            Decoder::new()
                .decode(&[json!(["s", ["__proto__", "isAdmin"], true])])
                .unwrap_err(),
            OpError::UnsafePath("__proto__".into())
        );
    }

    #[test]
    fn arrays_of_mixed_segments_survive_a_roundtrip() {
        let ops = vec![
            Op::Set(path_with(&[Seg::key("items"), Seg::index(2)]), json!("x")),
            Op::Splice(path(&["items"]), 1, 2, vec![json!(1), json!({"k": [true, null]})]),
            Op::Move(path(&["items"]), vec![2, 0, 1]),
        ];
        assert_eq!(roundtrip(ops.clone()), ops);
    }
}
