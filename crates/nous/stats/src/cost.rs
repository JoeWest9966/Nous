//! `GET /v1/stats/cost` 的读口：tokens / 钱，按天 / 小时 / 模型 / 会话四组桶。
//!
//! 六屏里的「账单」。和 `/v1/stats/usage` 的区别：usage 是**一个** `group_by`、四元组；
//! cost 是**四组一起**、每组带 `input/output/cacheRead`，外加 `byHour` 与那句 `note`。
//! 两者都从 `entries.record` 现算，不建表、不写库。
//!
//! 面板原话必须带上（**cost 全 0 不代表免费**）：provider 不回 price 时 pi-durable 记 0，
//! `tokens` 才是真数。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::util;
use crate::Result;

/// 扫多少条 entry（面板同值）。
const SCAN_LIMIT: i64 = 20_000;

const NOTE: &str =
    "cost 为 0 不代表免费：provider 没回 price 的时候 pi-durable 记 0，tokens 才是真数。";

/// 一个桶的累加器。
#[derive(Default)]
struct Bucket {
    tokens: i64,
    cost: f64,
    calls: i64,
    input: i64,
    output: i64,
    cache_read: i64,
}

impl Bucket {
    fn add(&mut self, input: i64, output: i64, cache_read: i64, tokens: i64, cost: f64) {
        self.tokens += tokens;
        self.cost += cost;
        self.calls += 1;
        self.input += input;
        self.output += output;
        self.cache_read += cache_read;
    }

    fn to_json(&self, label: &str) -> Value {
        json!({
            "label": label,
            "tokens": self.tokens,
            "cost": self.cost,
            "calls": self.calls,
            "input": self.input,
            "output": self.output,
            "cacheRead": self.cache_read,
        })
    }
}

/// 一条 assistant entry 的用量分项（对齐面板 `entry.usage`）。
struct EntryUsage {
    at: i64,
    model: String,
    conv: String,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    tokens: i64,
    cost: f64,
}

fn entry_usage(record: &Value) -> Option<EntryUsage> {
    if record.get("kind").and_then(Value::as_str) != Some("pi.assistant") {
        return None;
    }
    let message = record.get("model")?.as_array()?.first()?;
    let usage = message.get("usage")?;
    let num = |value: Option<&Value>| {
        value
            .and_then(Value::as_i64)
            .or_else(|| value.and_then(Value::as_f64).map(|n| n as i64))
            .unwrap_or(0)
    };
    let at = num(message.get("timestamp"));
    let model = message
        .get("model")
        .or_else(|| message.get("modelId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .map(|name| {
            message
                .get("provider")
                .and_then(Value::as_str)
                .map(|provider| format!("{provider}/{name}"))
                .unwrap_or(name)
        })
        .unwrap_or_else(|| "?".to_string());
    let conv = record
        .get("conversationId")
        .map(|value| {
            value
                .as_i64()
                .map(|id| format!("#{id}"))
                .unwrap_or_else(|| value.to_string().trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "?".to_string());
    Some(EntryUsage {
        at,
        model,
        conv,
        input: num(usage.get("input")),
        output: num(usage.get("output")),
        cache_read: num(usage.get("cacheRead")),
        cache_write: num(usage.get("cacheWrite")),
        reasoning: num(usage.get("reasoning")),
        tokens: num(usage.get("totalTokens")),
        cost: usage
            .get("cost")
            .and_then(|cost| cost.get("total"))
            .and_then(Value::as_f64)
            .or_else(|| {
                usage
                    .get("cost")
                    .and_then(|cost| cost.get("total"))
                    .and_then(Value::as_i64)
                    .map(|n| n as f64)
            })
            .unwrap_or(0.),
    })
}

/// 面板的 `day` / `hour` 标签：`MM-DD` 与 `MM-DD HHh`（UTC 日期 + 本地小时；面板同算法）。
fn day_label(epoch_ms: i64) -> String {
    let (_, month, day) = civil_from_days(epoch_ms.div_euclid(86_400_000));
    format!("{month:02}-{day:02}")
}

fn hour_label(epoch_ms: i64) -> String {
    let hour = epoch_ms.rem_euclid(86_400_000) / 3_600_000;
    format!("{} {:02}h", day_label(epoch_ms), hour)
}

/// Howard Hinnant 的 days_from_civil 逆运算（UTC，不引 chrono）。
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

/// `GET /v1/stats/cost`：四组桶 + totals + note。
pub fn cost(durable_db: &Path) -> Result<Value> {
    let db = util::open_read_only(durable_db)?;
    let rows = util::all(
        &db,
        &format!("SELECT record FROM entries ORDER BY id DESC LIMIT {SCAN_LIMIT}"),
    )?;

    let mut totals = Bucket::default();
    let mut total_cache_write = 0i64;
    let mut total_reasoning = 0i64;
    let mut by_day: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_hour: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_model: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_conv: BTreeMap<String, Bucket> = BTreeMap::new();

    for row in &rows {
        let Some(text) = row.get("record").and_then(nous_chord::sqlite::SqliteValue::as_str) else {
            continue;
        };
        let Some(record) = util::parse_record(text) else { continue };
        let Some(usage) = entry_usage(&record) else { continue };

        totals.add(usage.input, usage.output, usage.cache_read, usage.tokens, usage.cost);
        total_cache_write += usage.cache_write;
        total_reasoning += usage.reasoning;

        if usage.at > 0 {
            let day = day_label(usage.at);
            let hour = hour_label(usage.at);
            by_day.entry(day).or_default().add(
                usage.input,
                usage.output,
                usage.cache_read,
                usage.tokens,
                usage.cost,
            );
            by_hour.entry(hour).or_default().add(
                usage.input,
                usage.output,
                usage.cache_read,
                usage.tokens,
                usage.cost,
            );
        }
        by_model.entry(usage.model).or_default().add(
            usage.input,
            usage.output,
            usage.cache_read,
            usage.tokens,
            usage.cost,
        );
        by_conv.entry(usage.conv).or_default().add(
            usage.input,
            usage.output,
            usage.cache_read,
            usage.tokens,
            usage.cost,
        );
    }

    let collect = |map: &BTreeMap<String, Bucket>| -> Vec<Value> {
        map.iter().map(|(label, bucket)| bucket.to_json(label)).collect()
    };
    let mut models = collect(&by_model);
    models.sort_by_key(|item| std::cmp::Reverse(item["tokens"].as_i64().unwrap_or(0)));
    let mut convs = collect(&by_conv);
    convs.sort_by_key(|item| std::cmp::Reverse(item["tokens"].as_i64().unwrap_or(0)));

    Ok(json!({
        "totals": {
            "entries": totals.calls,
            "tokens": totals.tokens,
            "input": totals.input,
            "output": totals.output,
            "cacheRead": totals.cache_read,
            "cacheWrite": total_cache_write,
            "reasoning": total_reasoning,
            "cost": totals.cost,
        },
        "byDay": collect(&by_day),
        "byHour": collect(&by_hour),
        "byModel": models,
        "byConv": convs,
        "note": NOTE,
    }))
}
