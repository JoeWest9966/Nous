//! 不可变修订的严格 JSON 校验。
//!
//! ## 上游做的一堆检查，这里大半不需要做
//!
//! `revision-validator.ts`（75 行）检查的是：容器必须是 plain object / null-prototype、
//! 数组必须稠密且只有可枚举的**数据**属性、不能有 symbol 属性、不能有 accessor、不能有环、
//! 数字必须有限。这些**全是 JS 对象模型的漏洞**：`undefined`、稀疏数组、原型链、`NaN`、
//! 环、getter —— 它们都能进到一个 `object` 里，而 `JSON.stringify` 会把它们悄悄抹平或改形。
//!
//! `serde_json::Value` 在类型上就只有 `Null / Bool / Number / String / Array / Object`，
//! 没有环（值树没有引用）、没有共享、没有 accessor、不可能稀疏。**所以「严格 JSON」在
//! 本库是类型保证，不是运行期检查。**
//!
//! 这里只保留一条真检查：**嵌套深度**。`serde_json` 的反序列化默认限 128 层，但值也可能
//! 由代码直接构造（比如工具输出拼接），递归遍历前要有个明确的上下界，否则栈会先爆。
//!
//! 上游的 WeakSet「已验证容器」缓存本库也没有对应物 —— 值树不共享，同一容器不会第二次出现。

use serde_json::Value;

use super::RevisionError;

/// 允许的嵌套深度。比 `serde_json` 的 128 层反序列化上限宽一截，给代码里手工构造的值留余量。
pub const MAX_REVISION_DEPTH: usize = 512;

/// 校验一个值能不能当不可变修订。
///
/// **返回的是 `()`，不是新值**：上游的 `validate<T>(value: T): T` 明确「校验与所有权转移
/// 是两件事」，Rust 里更不该让调用方误以为校验会净化数据。
pub fn validate_revision(value: &Value) -> Result<(), RevisionError> {
    // 迭代而不是递归：恶意深的输入不该让校验本身爆栈。
    let mut stack: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_REVISION_DEPTH {
            return Err(RevisionError::TooDeep { depth });
        }
        match node {
            Value::Array(items) => {
                for item in items {
                    stack.push((item, depth + 1));
                }
            }
            Value::Object(map) => {
                for (_, item) in map.iter() {
                    stack.push((item, depth + 1));
                }
            }
            // 叶子：serde_json 的 Number 已排除 NaN / Infinity。
            _ => {}
        }
    }
    Ok(())
}

/// 一个修订序列的校验器。
///
/// 上游按**对象身份**（`WeakSet`）记住已验证的容器，只对第一次见到的做严格校验。本库的值树
/// 不共享，没有「同一个容器再出现」这回事，所以这里只是把 API 形状留住 —— `nous-durable`
/// 读进来的每个修订调一次 `validate` 即可。
#[derive(Debug, Default)]
pub struct RevisionValidator;

impl RevisionValidator {
    pub fn new() -> Self {
        Self
    }

    pub fn validate(&self, value: &Value) -> Result<(), RevisionError> {
        validate_revision(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_json_values_are_accepted() {
        for value in [
            json!(null),
            json!(true),
            json!(0),
            json!(-1.5),
            json!("héllo😀"),
            json!([]),
            json!({}),
            json!({"a": [1, {"b": [null, false]}]}),
        ] {
            assert_eq!(RevisionValidator::new().validate(&value), Ok(()), "{value}");
        }
    }

    #[test]
    fn deep_nesting_is_rejected_before_it_blows_the_stack() {
        let mut value = json!(0);
        for _ in 0..(MAX_REVISION_DEPTH + 5) {
            value = Value::Array(vec![value]);
        }
        let error = validate_revision(&value).unwrap_err();
        assert!(matches!(error, RevisionError::TooDeep { .. }), "{error}");
    }

    #[test]
    fn a_revision_may_be_revalidated_without_changing_it() {
        // 上游的缓存是 WeakSet；本库无共享容器，重复校验就是重复遍历，语义不变。
        let value = json!({"a": {"b": 1}});
        let validator = RevisionValidator::new();
        assert_eq!(validator.validate(&value), Ok(()));
        assert_eq!(validator.validate(&value), Ok(()));
        assert_eq!(value, json!({"a": {"b": 1}}));
    }

    #[test]
    fn wide_but_shallow_values_are_fine() {
        // 一千个键的一层对象：深度 2，不该被深度上限误伤。
        let mut map = serde_json::Map::new();
        for index in 0..1_000 {
            map.insert(format!("k{index}"), json!(index));
        }
        assert_eq!(RevisionValidator::new().validate(&Value::Object(map)), Ok(()));
    }
}
