//! #54：entry / submission 里「谁写的」这个值类型。
//!
//! 形状固定为 `{"kind":"...","id":"..."}`：它是**内容**，不是身份判断（本票不做 `decide()`）。
//! 读旧记录时未知 / 缺 `kind` 一律降级成 `System`（fail-closed，绝不当 `Human`），
//! 没有 actor 字段时给 `system("unknown")` —— 那是「字段出现之前写的」，不是「没人写」。
//!
//! Actor 只有值语义；不读环境、不读线程局部，写路径上一路显式传参。

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt;

/// 一条记录的作者类别。**封闭三值**：未知值在反序列化时降级，不新增变体。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    Human,
    Agent,
    System,
}

/// 一条 entry / submission 是谁写的。`id` 是这一类里的标识（用户名 / agent 名 / 应用名）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Actor {
    pub kind: ActorKind,
    pub id: String,
}

impl Actor {
    pub fn system(id: impl Into<String>) -> Self {
        Self { kind: ActorKind::System, id: id.into() }
    }

    pub fn agent(id: impl Into<String>) -> Self {
        Self { kind: ActorKind::Agent, id: id.into() }
    }

    pub fn human(id: impl Into<String>) -> Self {
        Self { kind: ActorKind::Human, id: id.into() }
    }
}

/// fail-closed 的反序列化：未知 / 缺失 `kind` → `System`，只有恰好这三个字面量才认。
impl<'de> Deserialize<'de> for ActorKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let kind = Option::<String>::deserialize(deserializer)?;
        Ok(match kind.as_deref() {
            Some("human") => ActorKind::Human,
            Some("agent") => ActorKind::Agent,
            _ => ActorKind::System,
        })
    }
}

/// 手写反序列化：`kind` 缺席时不能报错（旧记录 / 脏数据都按 System 接住），
/// 但字段仍然是恰好两个 —— `id` 缺席才是形状错误。
impl<'de> Deserialize<'de> for Actor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ActorVisitor;

        impl<'de> Visitor<'de> for ActorVisitor {
            type Value = Actor;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an actor with an id and an optional kind")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut kind = None;
                let mut id = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "kind" => kind = Some(map.next_value::<ActorKind>()?),
                        "id" => id = Some(map.next_value::<String>()?),
                        _ => { let _: Value = map.next_value()?; }
                    }
                }
                Ok(Actor {
                    kind: kind.unwrap_or(ActorKind::System),
                    id: id.ok_or_else(|| de::Error::missing_field("id"))?,
                })
            }
        }

        deserializer.deserialize_map(ActorVisitor)
    }
}

/// 从一条记录里读 actor。没有字段、形状不对 → `system("unknown")`。
pub fn actor_of(record: &Value) -> Actor {
    record
        .get("actor")
        .and_then(|value| serde_json::from_value::<Actor>(value.clone()).ok())
        .unwrap_or_else(|| Actor::system("unknown"))
}
