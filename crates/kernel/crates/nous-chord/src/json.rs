//! 严格 JSON：本库认为「什么是 JSON」的唯一说法。
//!
//! ## 上游是什么，这里剩什么
//!
//! `packages/chord/src/json.ts`（135 行）做两件事：**类型层**的 `JsonValue` /
//! `JsonRepresentation<T>`，和**运行期**的 `copyJson` / `isJsonValue`（拒 NaN、拒环、
//! 拒稀疏数组、拒 symbol / getter / 自定义原型）。
//!
//! 在 Rust 里，运行期那四项**由类型本身保证**，不需要每次递归检查：
//!
//! | 上游检查 | Rust 侧 |
//! |---|---|
//! | 环 | `serde_json::Value` 是树，`Rc`/`Rc` 环造不出来 |
//! | 稀疏数组 / 非索引键 | `Vec<Value>` 只有下标 |
//! | symbol / getter / 非普通原型 | 没有这套概念；`Value` 只有 map/seq |
//! | 非有限数（NaN / ±Inf） | `serde_json::Number` 表达不了，反序列化直接报错 |
//! | 非 JSON 标量（`undefined`、`bigint`、函数） | 类型系统里没有 |
//!
//! 剩下唯一能越界的只有**深度**：嵌套过深的 `Value` 会让递归遍历打穿栈。所以本文件只做
//! 深度校验，其余一律靠类型 —— 和 `delta::validator` 同一个取舍，理由也同一个：
//! **不要为了「照抄」而写一堆恒真的检查，那会让真正的检查埋在里面。**
//!
//! ## `JsonRepresentation<T>` 在 Rust 里
//!
//! TS 那个类型是「把应用类型递归清洗成严格 JSON」。Rust 的等价物是**结构体 + serde
//! derive**（`#[serde(rename_all = "camelCase")]`），未知负载就是 `serde_json::Value`。
//! 本库不提供「什么都收」的旁路类型（`serde_json::Value` 混进已有结构体就是那种旁路）。

use std::collections::BTreeMap;

use serde_json::Value;

/// 严格 JSON 值。TS 的 `JsonValue`。
pub type JsonValue = Value;

/// 允许的嵌套深度。比 `serde_json` 反序列化的默认 128 层宽松，比栈安全线紧。
pub const MAX_JSON_DEPTH: usize = 512;

/// 严格 JSON 校验失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonError {
    /// 嵌套超过 [`MAX_JSON_DEPTH`]。
    TooDeep { depth: usize },
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonError::TooDeep { depth } => {
                write!(f, "value nesting is too deep: {depth} > {MAX_JSON_DEPTH}")
            }
        }
    }
}

impl std::error::Error for JsonError {}

/// 复制一份归调用方所有的严格 JSON 树。TS 的 `copyJson`。
///
/// 上游还顺手裁掉 `undefined` 属性 / 重定义数据属性；Rust 里这两件事都不存在，
/// 所以这里只做深度校验再克隆。**深克隆语义保留**：改副本不影响原件。
pub fn copy_json(value: &Value) -> Result<JsonValue, JsonError> {
    let depth = depth_of(value);
    if depth > MAX_JSON_DEPTH {
        return Err(JsonError::TooDeep { depth });
    }
    Ok(value.clone())
}

/// 这个值是不是严格 JSON。TS 的 `isJsonValue`。
///
/// 在 Rust 里，能拿到 `&Value` 就已经是严格 JSON；唯一的失败面是深度。
pub fn is_json_value(value: &Value) -> bool {
    depth_of(value) <= MAX_JSON_DEPTH
}

/// 最大嵌套深度（叶子 = 1）。迭代实现，不用递归 —— 深值正是它要处理的输入。
pub fn depth_of(value: &Value) -> usize {
    let mut max = 0usize;
    // (节点, 当前深度)
    let mut stack: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((node, depth)) = stack.pop() {
        max = max.max(depth);
        match node {
            Value::Object(map) => stack.extend(map.values().map(|child| (child, depth + 1))),
            Value::Array(items) => stack.extend(items.iter().map(|child| (child, depth + 1))),
            _ => {}
        }
    }
    max
}

/// 把一个 map 的键排好序（本库写库前统一用它，键序不影响语义，只影响 diff 与日志）。
pub fn sorted_object(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> =
                map.iter().map(|(key, value)| (key, sorted_object(value))).collect();
            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted_object).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_fresh_tree_round_trips_copies_are_independent() {
        let original = json!({"a": [1, 2, {"b": null}], "c": "x"});
        let copy = copy_json(&original).unwrap();
        assert_eq!(copy, original);
        // 深克隆：改副本不影响原件。
        let mut copy = copy;
        copy["a"][0] = json!(99);
        assert_eq!(original["a"][0], json!(1));
    }

    #[test]
    fn depth_is_counted_iteratively_and_the_limit_is_enforced() {
        // 500 层数组：合法。
        let mut value = json!(0);
        for _ in 0..MAX_JSON_DEPTH - 1 {
            value = Value::Array(vec![value]);
        }
        assert_eq!(depth_of(&value), MAX_JSON_DEPTH);
        assert!(is_json_value(&value));
        assert!(copy_json(&value).is_ok());

        // 再深一层就超。
        let too_deep = Value::Array(vec![value]);
        assert_eq!(depth_of(&too_deep), MAX_JSON_DEPTH + 1);
        assert!(!is_json_value(&too_deep));
        assert_eq!(
            copy_json(&too_deep).unwrap_err(),
            JsonError::TooDeep {
                depth: MAX_JSON_DEPTH + 1
            }
        );
    }

    #[test]
    fn non_finite_numbers_are_rejected_by_the_type_not_by_a_check() {
        // `serde_json::Number` 表达不了 NaN / Infinity —— 上游那条 `Number.isFinite`
        // 检查在 Rust 里由反序列化完成，这里把它钉成判据。
        assert!(serde_json::from_str::<Value>("NaN").is_err());
        assert!(serde_json::from_str::<Value>("Infinity").is_err());
        assert!(serde_json::from_str::<Value>("1e9999").is_err());
    }

    #[test]
    fn sorting_is_stable_and_does_not_change_semantics() {
        let value = json!({"b": 1, "a": {"d": 2, "c": [{"f": 3, "e": 4}]}});
        let sorted = sorted_object(&value);
        assert_eq!(sorted, value);
        let text = serde_json::to_string(&sorted).unwrap();
        assert!(text.starts_with("{\"a\""), "键要排好：{text}");
    }

    #[test]
    fn scalar_and_empty_containers_are_depth_one() {
        assert_eq!(depth_of(&json!(null)), 1);
        assert_eq!(depth_of(&json!([])), 1);
        assert_eq!(depth_of(&json!({})), 1);
        assert_eq!(depth_of(&json!([[]])), 2);
    }
}
