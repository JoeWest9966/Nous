//! `triggers` 表的**完整定义**：私有字段、`Trigger::new` 唯一构造入口、封闭 spec 与五段 cron。
//!
//! 这里不认识 `nous-durable`（判据 1）：火起来之后的会话语义由调用方注入。

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Store;

/// 一条触发器的 slug（封闭集合，逐条 conformance）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerError {
    CodeShape,
    CodeTaken,
    CodeImmutable,
    CodeTombstoned,
    TemplateMissing,
    NoFiringSource,
    ExplicitPolicyRequired,
    UnknownSpecKey,
    SpecVersionMissing,
    OrchestrationNotATrigger,
    LedgerOnDefinition,
    SpecScheduleInvalid,
}

impl TriggerError {
    pub fn slug(&self) -> &'static str {
        match self {
            Self::CodeShape => "code-shape",
            Self::CodeTaken => "code-taken",
            Self::CodeImmutable => "code-immutable",
            Self::CodeTombstoned => "code-tombstoned",
            Self::TemplateMissing => "template-missing",
            Self::NoFiringSource => "no-firing-source",
            Self::ExplicitPolicyRequired => "explicit-policy-required",
            Self::UnknownSpecKey => "unknown-spec-key",
            Self::SpecVersionMissing => "spec-version-missing",
            Self::OrchestrationNotATrigger => "orchestration-not-a-trigger",
            Self::LedgerOnDefinition => "ledger-on-definition",
            Self::SpecScheduleInvalid => "spec-schedule-invalid",
        }
    }
}

impl fmt::Display for TriggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}
impl std::error::Error for TriggerError {}

/// `spec` 的封闭键集合。**唯一**允许 `deny_unknown_fields` 的地方：这是本地配置，
/// 不是协议；打错一个键名必须报错，不能静默变成「触发器不工作也不报错」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub spec_version: u32,
    pub schedule: Option<String>,
    pub on: Option<String>,
}

/// 点火许可里的一类人（Actor 的最小投影：durable 的三值语义不进本 crate）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FireActor {
    Human(String),
    Agent(String),
    System(String),
}

impl<'de> Deserialize<'de> for FireActor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let id = value.get("id").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        Ok(match value.get("kind").and_then(serde_json::Value::as_str) {
            Some("human") => Self::Human(id),
            Some("agent") => Self::Agent(id),
            _ => Self::System(id),
        })
    }
}

impl FireActor {
    pub fn matches(&self, other: &Self) -> bool {
        self == other
    }
    pub fn display(&self) -> String {
        match self {
            Self::Human(id) => format!("human:{id}"),
            Self::Agent(id) => format!("agent:{id}"),
            Self::System(id) => format!("system:{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateRef {
    ref_name: String,
}

impl<'de> Deserialize<'de> for TemplateRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        if value.get("kind").and_then(serde_json::Value::as_str) != Some("preset") {
            return Err(serde::de::Error::custom("template_ref.kind must be preset"));
        }
        let ref_name = value.get("ref").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        Ok(Self { ref_name })
    }
}

impl TemplateRef {
    pub fn ref_name(&self) -> &str {
        &self.ref_name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    New,
    Conversation(i64),
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        Ok(match value.get("kind").and_then(serde_json::Value::as_str) {
            Some("conversation") => Self::Conversation(value.get("id").and_then(serde_json::Value::as_i64).unwrap_or(0)),
            _ => Self::New,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    Serial,
    Queue,
}

/// `Trigger::new` 的唯一入参。
#[derive(Debug, Clone)]
pub struct NewTrigger<'a> {
    pub code: &'a str,
    pub name: &'a str,
    pub spec: &'a str,
    /// JSON 字面：`{"kind":"preset","ref":"..."}`。
    pub template_ref: &'a str,
    /// JSON 数组：`[{"kind":"cron","id":"scheduler"}]`。
    pub fires_allowed_by: &'a str,
    /// JSON：`{"kind":"new"}` 或 `{"kind":"conversation","id":42}`。
    pub target: &'a str,
    pub policy: &'a str,
}

/// 一行触发器。**字段私有，没有字面量构造**（负向哨兵：字段一 pub 就红）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    id: i64,
    code: String,
    name: String,
    spec: Spec,
    template_ref: TemplateRef,
    fires_allowed_by: Vec<FireActor>,
    target: Target,
    policy: Policy,
    enabled: bool,
    next_fire_at: Option<i64>,
    created_at: i64,
    deleted_at: Option<i64>,
}

impl Trigger {
    /// 唯一构造入口：校验九条准则 + spec 结构 + cron 五段 + `code` 占用。
    pub fn new(new: NewTrigger<'_>, now_ms: i64, store: Option<&Store>) -> std::result::Result<Self, TriggerError> {
        if !valid_code(new.code) {
            return Err(TriggerError::CodeShape);
        }
        let fires_allowed_by = parse_actors(new.fires_allowed_by)?;
        if fires_allowed_by.is_empty() {
            return Err(TriggerError::NoFiringSource);
        }
        if new.policy != "serial" && new.policy != "queue" {
            return Err(TriggerError::ExplicitPolicyRequired);
        }
        let template_ref = parse_template_ref(new.template_ref)?;
        let spec = parse_spec(new.spec)?;
        if spec.on.is_some() && spec.schedule.is_none() {
            return Err(TriggerError::NoFiringSource);
        }
        if let Some(cron) = spec.schedule.as_deref() {
            parse_five_field_cron(cron).map_err(|_| TriggerError::SpecScheduleInvalid)?;
        }
        let target = parse_target(new.target)?;
        let (id, _created_at) = match store {
            Some(store) => {
                let occupant = store.trigger(new.code).map_err(|_| TriggerError::CodeShape)?;
                match occupant {
                    Some(existing) if existing.deleted_at.is_some() => {
                        return Err(TriggerError::CodeTombstoned)
                    }
                    Some(_) => return Err(TriggerError::CodeTaken),
                    None => {
                        let serialized = serialize_definition(
                            &spec,
                            &template_ref,
                            &fires_allowed_by,
                            &target,
                            if new.policy == "serial" { Policy::Serial } else { Policy::Queue },
                        );
                        let created = store
                            .create_legacy_trigger(new.code, new.name, &serialized, now_ms)
                            .map_err(|_| TriggerError::CodeTaken)?;
                        (created.id, created.created_at)
                    }
                }
            }
            None => (0, now_ms),
        };
        Ok(Self {
            id,
            code: new.code.to_string(),
            name: new.name.to_string(),
            spec,
            template_ref,
            fires_allowed_by,
            target,
            policy: if new.policy == "serial" { Policy::Serial } else { Policy::Queue },
            enabled: true,
            next_fire_at: None,
            created_at: now_ms,
            deleted_at: None,
        })
    }

    pub fn id(&self) -> i64 { self.id }
    pub fn code(&self) -> &str { &self.code }
    pub fn name(&self) -> &str { &self.name }
    pub fn spec(&self) -> &Spec { &self.spec }
    pub fn template_ref(&self) -> &TemplateRef { &self.template_ref }
    pub fn fires_allowed_by(&self) -> &[FireActor] { &self.fires_allowed_by }
    pub fn target(&self) -> &Target { &self.target }
    pub fn policy(&self) -> Policy { self.policy }
    pub fn enabled(&self) -> bool { self.enabled }
    pub fn next_fire_at(&self) -> Option<i64> { self.next_fire_at }
    pub fn created_at(&self) -> i64 { self.created_at }
    pub fn deleted_at(&self) -> Option<i64> { self.deleted_at }

    /// 点火许可：`fired_by` 必须在名单里。
    pub fn permits(&self, fired_by: &FireActor) -> bool {
        self.fires_allowed_by.iter().any(|allowed| allowed.matches(fired_by))
    }

    /// 把完整定义写回 `triggers.spec`（建完即可改 state，定义自身不在库里另开列）。
    pub fn definition_json(&self) -> String {
        serialize_definition(&self.spec, &self.template_ref, &self.fires_allowed_by, &self.target, self.policy)
    }

    /// 从库里的 JSON 原文还原一份（读端点用；不改 id）。
    pub fn from_row(row: &crate::triggers::Trigger, json: &str) -> std::result::Result<Self, TriggerError> {
        let definition: Definition = serde_json::from_str(json).map_err(|_| TriggerError::UnknownSpecKey)?;
        Ok(Self {
            id: row.id,
            code: row.code.clone(),
            name: row.name.clone(),
            spec: Spec { spec_version: definition.spec.spec_version, schedule: definition.spec.schedule, on: definition.spec.on },
            template_ref: definition.template_ref,
            fires_allowed_by: definition.fires_allowed_by,
            target: definition.target,
            policy: definition.policy,
            enabled: row.enabled,
            next_fire_at: row.next_fire_at,
            created_at: row.created_at,
            deleted_at: row.deleted_at,
        })
    }
}

#[derive(Deserialize)]
struct SpecInner {
    spec_version: u32,
    schedule: Option<String>,
    on: Option<String>,
}

#[derive(Deserialize)]
struct Definition {
    spec: SpecInner,
    template_ref: TemplateRef,
    fires_allowed_by: Vec<FireActor>,
    target: Target,
    policy: Policy,
}

fn serialize_definition(
    spec: &Spec,
    template_ref: &TemplateRef,
    fires_allowed_by: &[FireActor],
    target: &Target,
    policy: Policy,
) -> String {
    // 手写 JSON：Actor / Target 的 wire 形状是 `{"kind":...,"id":...}`，不用 serde tag。
    let actors: Vec<serde_json::Value> = fires_allowed_by
        .iter()
        .map(|actor| {
            let (kind, id) = match actor {
                FireActor::Human(id) => ("human", id),
                FireActor::Agent(id) => ("agent", id),
                FireActor::System(id) => ("system", id),
            };
            serde_json::json!({ "kind": kind, "id": id })
        })
        .collect();
    let target_json = match target {
        Target::New => serde_json::json!({ "kind": "new" }),
        Target::Conversation(id) => serde_json::json!({ "kind": "conversation", "id": id }),
    };
    serde_json::json!({
        "spec": spec,
        "template_ref": { "kind": "preset", "ref": template_ref.ref_name },
        "fires_allowed_by": actors,
        "target": target_json,
        "policy": match policy { Policy::Serial => "serial", Policy::Queue => "queue" },
    })
    .to_string()
}

/// `code` 一旦定型就不许改（库里的 SQL 触发器是物理保证，这个函数是定义层的显式哨兵）。
pub fn ensure_code_immutable(existing: &str, requested: &str) -> std::result::Result<(), TriggerError> {
    if existing != requested { Err(TriggerError::CodeImmutable) } else { Ok(()) }
}

fn valid_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    (3..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes[1..]
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

fn parse_spec(raw: &str) -> std::result::Result<Spec, TriggerError> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| TriggerError::UnknownSpecKey)?;
    if let Some(map) = value.as_object() {
        for key in map.keys() {
            match key.as_str() {
                "spec_version" | "schedule" | "on" => {}
                "orchestration" | "trigger" | "triggers" | "chain" | "next" => {
                    return Err(TriggerError::OrchestrationNotATrigger)
                }
                "fires" | "fire_at" | "ledger" | "state" => {
                    return Err(TriggerError::LedgerOnDefinition)
                }
                _ => return Err(TriggerError::UnknownSpecKey),
            }
        }
    }
    if value.get("spec_version").is_none() {
        return Err(TriggerError::SpecVersionMissing);
    }
    serde_json::from_value(value).map_err(|error| {
        if error.to_string().contains("unknown field") {
            TriggerError::UnknownSpecKey
        } else {
            TriggerError::SpecVersionMissing
        }
    })
}

fn parse_actors(raw: &str) -> std::result::Result<Vec<FireActor>, TriggerError> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| TriggerError::NoFiringSource)?;
    let array = value.as_array().ok_or(TriggerError::NoFiringSource)?;
    array
        .iter()
        .map(|item| {
            // 票面示例里允许 `"cron"` / `"human"` 这样的短写法；设计文档里是 #54 的
            // `{kind,id}`。两种都认，但落库后统一成 #54 的形状。
            if let Some(id) = item.as_str() {
                return Ok(match id {
                    "human" => FireActor::Human("human".to_string()),
                    id if id.starts_with("ext:") || id.starts_with("agent:") => {
                        FireActor::Agent(id.to_string())
                    }
                    other => FireActor::System(other.to_string()),
                });
            }
            let kind = item.get("kind").and_then(serde_json::Value::as_str).ok_or(TriggerError::NoFiringSource)?;
            let id = item.get("id").and_then(serde_json::Value::as_str).ok_or(TriggerError::NoFiringSource)?.to_string();
            Ok(match kind {
                "human" => FireActor::Human(id),
                "agent" => FireActor::Agent(id),
                _ => FireActor::System(id),
            })
        })
        .collect()
}

fn parse_template_ref(raw: &str) -> std::result::Result<TemplateRef, TriggerError> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| TriggerError::TemplateMissing)?;
    let name = value
        .get("ref")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty() && value.get("kind").and_then(serde_json::Value::as_str) == Some("preset"))
        .ok_or(TriggerError::TemplateMissing)?
        .to_string();
    Ok(TemplateRef { ref_name: name })
}

fn parse_target(raw: &str) -> std::result::Result<Target, TriggerError> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| TriggerError::CodeShape)?;
    match value.get("kind").and_then(serde_json::Value::as_str) {
        Some("new") => Ok(Target::New),
        Some("conversation") => value
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .map(Target::Conversation)
            .ok_or(TriggerError::CodeShape),
        _ => Err(TriggerError::CodeShape),
    }
}

/// 五段 cron：分 时 日 月 周。数字或 `*`、`*/n`；不引 cron crate。
pub fn parse_five_field_cron(cron: &str) -> std::result::Result<Cron, TriggerError> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(TriggerError::SpecScheduleInvalid);
    }
    let mut parsed = Vec::with_capacity(5);
    for (index, field) in fields.iter().enumerate() {
        let (base, step) = match field.split_once('/') {
            Some((base, step)) => {
                let step = step.parse::<u32>().map_err(|_| TriggerError::SpecScheduleInvalid)?;
                if step == 0 {
                    return Err(TriggerError::SpecScheduleInvalid);
                }
                (base, step)
            }
            None => (*field, 1),
        };
        let range = CRON_RANGES[index];
        if base == "*" {
            parsed.push(CronField::Star { step, min: range.0 });
            continue;
        }
        let value = base.parse::<u32>().map_err(|_| TriggerError::SpecScheduleInvalid)?;
        if value < range.0 || value > range.1 {
            return Err(TriggerError::SpecScheduleInvalid);
        }
        parsed.push(CronField::Value(value));
    }
    Ok(Cron {
        minute: parsed[0].clone(),
        hour: parsed[1].clone(),
        day: parsed[2].clone(),
        month: parsed[3].clone(),
        weekday: parsed[4].clone(),
    })
}

const CRON_RANGES: [(u32, u32); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum CronField {
    Star { step: u32, min: u32 },
    Value(u32),
}

impl CronField {
    fn matches(&self, value: u32) -> bool {
        match self {
            Self::Star { step, min } => (value - *min) % *step == 0,
            Self::Value(expected) => value == *expected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cron {
    minute: CronField,
    hour: CronField,
    day: CronField,
    month: CronField,
    weekday: CronField,
}

impl Cron {
    pub fn matches_ms(&self, now_ms: i64) -> bool {
        let (minute, hour, day, month, weekday) = parts(now_ms);
        self.minute.matches(minute)
            && self.hour.matches(hour)
            && self.month.matches(month)
            && (self.day.matches(day) || self.weekday.matches(weekday))
    }
}

/// epoch 毫秒 → (分, 时, 日, 月, 周日 0=周日)。手写 civil-from-days，不引 chrono。
fn parts(now_ms: i64) -> (u32, u32, u32, u32, u32) {
    let days = now_ms.div_euclid(86_400_000);
    let millis = now_ms.rem_euclid(86_400_000);
    let hour = (millis / 3_600_000) as u32;
    let minute = ((millis % 3_600_000) / 60_000) as u32;
    let (_, month, day) = civil_from_days(days);
    let weekday = (days + 4).rem_euclid(7) as u32;
    (minute, hour, day, month, weekday)
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

/// `fire_due` 的一条结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueFire {
    pub code: String,
    pub fire_at: i64,
}

/// 纯函数：算出到点的 `(code, fire_at)`。**内部零时钟**，也不碰库。
///
/// `enabled` 与墓碑是库里的列，由调用方先过滤；这里只看定义里的 schedule。
pub fn fire_due(triggers: &[Trigger], now_ms: i64) -> Vec<DueFire> {
    let mut due = Vec::new();
    for trigger in triggers {
        if !trigger.enabled || trigger.deleted_at.is_some() {
            continue;
        }
        let Some(schedule) = trigger.spec.schedule.as_deref() else { continue };
        let Ok(cron) = parse_five_field_cron(schedule) else { continue };
        let aligned = now_ms.div_euclid(60_000) * 60_000;
        if cron.matches_ms(aligned) {
            due.push(DueFire { code: trigger.code.clone(), fire_at: aligned });
        }
    }
    due
}
