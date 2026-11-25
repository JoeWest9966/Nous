//! `data-manager` 应用层模块（#62）：只读观测聚合。
//!
//! 三条端点从两张现成的库里现算，不建表、不写库、不 spawn 进程、不直接看时钟。
//! 公共接口是几个纯读函数；**端点也归本插件**（[`routes`]，Magento 2 那种模块自带
//! controller），宿主只注入两个库路径（[`routes::Host`]）。
//!
//! ## 判据的接缝
//!
//! - 测试自己从 `/v1/sessions/:id/entries` 同形 JSON 手算 totals，再与 [`usage`] 对账；
//! - 测试自己开同一个 sqlite 读 `tasks.status`，与 [`tasks`] 对账；
//! - `db` 与 `nous-app --check` 共用 [`nous_durable::ops::check`] 的读口。

use std::collections::BTreeMap;
use std::path::Path;

use nous_chord::sqlite::{LocalDatabase, SqliteExecutor, SqliteValue};
use serde_json::{json, Value};

pub mod db;
pub mod routes;
pub mod util;
pub mod overview;
pub mod conversations;
pub mod cost;
pub mod health;

/// 聚合错误：读口失败一律带路径与原因，端点层映射成 `{error}`。
#[derive(Debug)]
pub enum StatsError {
    Storage(String),
}

impl std::fmt::Display for StatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "stats storage: {message}"),
        }
    }
}

impl std::error::Error for StatsError {}

impl From<nous_durable::StorageError> for StatsError {
    fn from(error: nous_durable::StorageError) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<nous_store::StoreError> for StatsError {
    fn from(error: nous_store::StoreError) -> Self {
        Self::Storage(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StatsError>;

/// `group_by` 的三种桶。非法值由端点层拒绝（400），不在这里猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Day,
    Model,
    Conversation,
}

impl GroupBy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "day" => Some(Self::Day),
            "model" => Some(Self::Model),
            "conversation" => Some(Self::Conversation),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Model => "model",
            Self::Conversation => "conversation",
        }
    }
}

/// `from`/`to` 闭开区间：`[from, to)`。缺省是全量。
#[derive(Debug, Clone, Copy, Default)]
pub struct UsageWindow {
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// 四个数一组，totals 与每行同形。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Totals {
    pub total_tokens: i64,
    pub cost_total: f64,
    pub requests: i64,
    pub errors: i64,
}

impl Totals {
    fn add(&mut self, other: Totals) {
        self.total_tokens += other.total_tokens;
        self.cost_total += other.cost_total;
        self.requests += other.requests;
        self.errors += other.errors;
    }

    fn as_json(self) -> Value {
        json!({
            "totalTokens": self.total_tokens,
            "costTotal": cost_json(self.cost_total),
            "requests": self.requests,
            "errors": self.errors,
        })
    }
}

/// 金额按原值累加（真库里是数字；避免微单位编码引入解析歧义）。
fn cost_json(total: f64) -> Value {
    json!(total)
}

fn number(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| v.as_i64())
        .or_else(|| value.and_then(Value::as_u64).and_then(|n| i64::try_from(n).ok()))
        .or_else(|| value.and_then(Value::as_f64).map(|n| n as i64))
        .unwrap_or(0)
}

/// 一条 assistant entry 的 `model[]` 里的那一条消息。只认有时间戳的消息；
/// 没有时间戳就无法归桶（wire.md 钉死：不进任何桶，也不进 totals）。
fn entry_totals(record: &Value) -> Option<(i64, String, Totals)> {
    if record.get("kind").and_then(Value::as_str) != Some("pi.assistant") {
        return None;
    }
    let message = record.get("model")?.as_array()?.first()?;
    let at = number(message.get("timestamp"));
    if at == 0 {
        return None;
    }
    let usage = message.get("usage");
    let tokens = number(usage.and_then(|u| u.get("totalTokens")));
    let cost = number(usage.and_then(|u| u.get("cost")).and_then(|c| c.get("total")));    let errors = i64::from(
        message.get("stopReason").and_then(Value::as_str) == Some("error"),
    );
    let model = message
        .get("provider")
        .and_then(Value::as_str)
        .zip(message.get("modelId").and_then(Value::as_str))
        .map(|(provider, model_id)| format!("{provider}/{model_id}"))
        .unwrap_or_else(|| "unknown".to_string());
    Some((at, model, Totals {
        total_tokens: tokens,
        cost_total: cost as f64,
        requests: 1,
        errors,
    }))
}

fn utc_day(epoch_ms: i64) -> String {
    // 自己算 UTC 日期，不引入 chrono，也不读本地时区（wire.md: day 是 UTC）。
    let days = epoch_ms.div_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Howard Hinnant 的 days_from_civil 逆运算（proleptic Gregorian，UTC）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

/// `GET /v1/stats/usage`：全量扫 `entries.record` 一次，Rust 侧解析 JSON 聚合。
/// 不建索引、不建物化表、不改 durable、不另开写通道（票面实现约束 1/2/3）。
pub fn usage(
    durable_db: &Path,
    window: UsageWindow,
    group_by: GroupBy,
) -> Result<Value> {
    let db = LocalDatabase::open_read_only(durable_db).map_err(|error| {
        StatsError::Storage(format!("open {}: {error}", durable_db.display()))
    })?;
    let rows = db
        .all("SELECT record FROM entries", &[])
        .map_err(|error| StatsError::Storage(error.to_string()))?;
    let mut totals = Totals::default();
    let mut buckets: BTreeMap<String, Totals> = BTreeMap::new();
    for row in rows {
        let text = match row.get("record").and_then(SqliteValue::as_str) {
            Some(text) => text,
            None => continue,
        };
        let record: Value = match serde_json::from_str(text) {
            Ok(record) => record,
            Err(_) => continue,
        };
        let Some((at, model, request)) = entry_totals(&record) else { continue };
        // 闭开区间 [from, to)：from 命中、to 不命中（wire.md 钉死）。
        if window.from.is_some_and(|from| at < from) || window.to.is_some_and(|to| at >= to) {
            continue;
        }
        let key = match group_by {
            GroupBy::Day => utc_day(at),
            GroupBy::Model => model,
            GroupBy::Conversation => record
                .get("conversationId")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
        };
        totals.add(request);
        buckets.entry(key).or_default().add(request);
    }
    let rows = buckets
        .into_iter()
        .map(|(key, value)| {
            let mut item = json!({ "key": key, "label": key });
            if let Some(object) = item.as_object_mut() {
                for (name, field) in [
                    ("totalTokens", json!(value.total_tokens)),
                    ("costTotal", cost_json(value.cost_total)),
                    ("requests", json!(value.requests)),
                    ("errors", json!(value.errors)),
                ] {
                    object.insert(name.to_string(), field);
                }
            }
            item
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "groupBy": group_by.as_str(),
        "from": window.from,
        "to": window.to,
        "totals": totals.as_json(),
        "rows": rows,
    }))
}

/// `GET /v1/stats/tasks`：`tasks` 表 `GROUP BY status` 现算（不从内存计数器读）。
pub fn tasks(durable_db: &Path) -> Result<Value> {
    let db = LocalDatabase::open_read_only(durable_db).map_err(|error| {
        StatsError::Storage(format!("open {}: {error}", durable_db.display()))
    })?;
    let rows = db
        .all("SELECT status, count(*) AS n FROM tasks GROUP BY status", &[])
        .map_err(|error| StatsError::Storage(error.to_string()))?;
    let mut by_state = BTreeMap::new();
    for name in [
        "pending", "running", "waiting", "sleeping", "completing", "terminal",
    ] {
        by_state.insert(name.to_string(), 0_i64);
    }
    for row in rows {
        let Some(status) = row.get("status").and_then(SqliteValue::as_str) else { continue };
        let count = row.get("n").and_then(SqliteValue::as_i64).unwrap_or(0);
        by_state.insert(status.to_string(), count);
    }
    let active = by_state
        .iter()
        .filter(|(name, _)| name.as_str() != "terminal")
        .map(|(_, count)| *count)
        .sum::<i64>();
    Ok(json!({ "byState": by_state, "active": active }))
}

/// `GET /v1/stats/db`：两库读数（见 [`db`] 模块）。
pub fn db(durable_db: &Path, plugins_db: &Path) -> Result<Value> {
    db::report(durable_db, plugins_db)
}

/// `GET /v1/stats/overview`：durable 库的 X 光片（见 [`overview`] 模块）。
pub fn overview(durable_db: &Path) -> Result<Value> {
    overview::overview(durable_db)
}

/// `GET /v1/stats/conversations`：会话树（见 [`conversations`] 模块）。
pub fn conversations(durable_db: &Path) -> Result<Value> {
    conversations::conversations(durable_db)
}

/// `GET /v1/stats/conversation/:id`：一条会话的明细（含 prompt 重建）。
pub fn conversation(durable_db: &Path, id: i64) -> Result<Option<Value>> {
    conversations::conversation(durable_db, id)
}

/// `GET /v1/stats/cost`：tokens / 钱，按天 / 小时 / 模型 / 会话四组桶。
pub fn cost(durable_db: &Path) -> Result<Value> {
    cost::cost(durable_db)
}

/// `GET /v1/stats/health`：任务 / 提交 / 文档 / WAL / 空闲页 / 维修工单。
pub fn health(durable_db: &Path, now_ms: i64) -> Result<Value> {
    health::health(durable_db, now_ms)
}
