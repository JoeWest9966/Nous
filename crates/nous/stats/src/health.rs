//! `GET /v1/stats/health` 的读口：任务 / 提交 / 文档 revision / WAL / 空闲页 / 维修工单。
//!
//! 六屏里的「维修」。回答「有没有卡住的活、有没有没人接的提交、能不能瘦」。
//! 全部现从 `tasks` / `submissions` / `documents` / `document_revisions` + `pragma` 读，
//! 不建表、不写库。`now_ms` 由端点注入（本层不直接看时钟，`lib.rs` 的规矩）。
//!
//! 维修工单只给**命令文本**，不执行：这页是只读的，VACUUM / checkpoint 得由人在停服后跑。

use std::collections::BTreeMap;
use std::path::Path;

use nous_chord::sqlite::SqliteExecutor;
use serde_json::{json, Value};

use crate::util;
use crate::Result;

/// 卡多久算「卡住」：10 分钟。比一次长的模型调用还久，比一顿饭短（面板同值）。
pub const STUCK_MS: i64 = 10 * 60 * 1000;
/// 任务 / 提交各读最近多少条（面板同值）。
const SCAN_LIMIT: i64 = 2000;

/// 任务状态展示顺序（在飞 → 收尾）。
const STATUS_ORDER: [&str; 6] = [
    "running", "completing", "waiting", "sleeping", "pending", "terminal",
];

/// 一条任务行（内部形状）。
struct Task {
    id: i64,
    conv: i64,
    kind: String,
    status: String,
    started_at: i64,
    ended_at: Option<i64>,
    outcome: Option<String>,
}

fn read_tasks(db: &dyn SqliteExecutor) -> Vec<Task> {
    util::all(
        db,
        &format!(
            "SELECT id, conversation_id, kind, status, record FROM tasks ORDER BY id DESC LIMIT {SCAN_LIMIT}"
        ),
    )
    .unwrap_or_default()
    .iter()
    .map(|row| {
        let record = row
            .get("record")
            .and_then(nous_chord::sqlite::SqliteValue::as_str)
            .and_then(util::parse_record)
            .unwrap_or(Value::Null);
        let started = record.get("startedAt").and_then(Value::as_i64).unwrap_or(0);
        let ended = record.get("endedAt").and_then(Value::as_i64);
        let outcome = record
            .get("state")
            .and_then(|state| state.get("outcome"))
            .and_then(|outcome| outcome.get("status"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        Task {
            id: util::col_i64(row, "id"),
            conv: util::col_i64(row, "conversation_id"),
            kind: util::unquote(&util::col_str(row, "kind")),
            status: util::col_str(row, "status"),
            started_at: started,
            ended_at: ended,
            outcome,
        }
    })
    .collect()
}

fn task_json(task: &Task) -> Value {
    json!({
        "id": task.id,
        "kind": task.kind,
        "status": task.status,
        "startedAt": task.started_at,
        "endedAt": task.ended_at,
        "ms": task.ended_at.unwrap_or(0) - task.started_at,
        "conv": task.conv,
        "outcome": task.outcome,
    })
}

/// `GET /v1/stats/health`：一整份体检报告。
pub fn health(durable_db: &Path, now_ms: i64) -> Result<Value> {
    if !durable_db.exists() {
        return Ok(empty_report(durable_db));
    }
    let db = util::open_read_only(durable_db)?;
    let tasks = read_tasks(&db);

    // 按状态分组（含 sample，对齐面板）。
    let mut by_status_map: BTreeMap<String, (i64, Vec<Value>)> = BTreeMap::new();
    for task in &tasks {
        let entry = by_status_map.entry(task.status.clone()).or_default();
        entry.0 += 1;
        if entry.1.len() < 4 {
            entry.1.push(json!({ "id": task.id, "kind": task.kind }));
        }
    }
    let mut by_status: Vec<Value> = by_status_map
        .into_iter()
        .map(|(status, (count, sample))| {
            json!({ "status": status, "count": count, "sample": sample })
        })
        .collect();
    by_status.sort_by_key(|item| {
        let status = item["status"].as_str().unwrap_or("");
        STATUS_ORDER
            .iter()
            .position(|known| *known == status)
            .unwrap_or(STATUS_ORDER.len())
    });

    let stuck: Vec<Value> = tasks
        .iter()
        .filter(|task| {
            matches!(task.status.as_str(), "running" | "completing" | "waiting")
                && task.started_at > 0
                && now_ms - task.started_at > STUCK_MS
        })
        .map(|task| {
            let mut value = task_json(task);
            value["ageMs"] = json!(now_ms - task.started_at);
            value
        })
        .collect();
    let sleeping = tasks.iter().filter(|task| task.status == "sleeping").count() as i64;

    // 时长统计（有起止且 ended >= started）。
    let durations: Vec<i64> = tasks
        .iter()
        .filter_map(|task| {
            task.ended_at
                .filter(|ended| *ended >= task.started_at && task.started_at > 0)
                .map(|ended| ended - task.started_at)
        })
        .collect();
    let avg = if durations.is_empty() {
        0
    } else {
        durations.iter().sum::<i64>() / durations.len() as i64
    };
    let mut sorted = durations.clone();
    sorted.sort_unstable();
    let p95 = if sorted.is_empty() {
        0
    } else {
        sorted[((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1)]
    };

    // 最慢 6 个（有起止的）。
    let mut slowest: Vec<&Task> = tasks
        .iter()
        .filter(|task| task.started_at > 0 && task.ended_at.is_some_and(|ended| ended >= task.started_at))
        .collect();
    slowest.sort_by_key(|task| std::cmp::Reverse(task.ended_at.unwrap_or(0) - task.started_at));
    let slowest: Vec<Value> = slowest.iter().take(6).map(|task| task_json(task)).collect();

    // 提交：状态分布 + 未答。
    let submissions: Vec<(i64, i64, String)> = util::all(
        &db,
        &format!(
            "SELECT id, conversation_id, status FROM submissions ORDER BY id DESC LIMIT {SCAN_LIMIT}"
        ),
    )
    .unwrap_or_default()
    .iter()
    .map(|row| {
        (
            util::col_i64(row, "id"),
            util::col_i64(row, "conversation_id"),
            util::col_str(row, "status"),
        )
    })
    .collect();
    let mut sub_status: BTreeMap<String, i64> = BTreeMap::new();
    for (_, _, status) in &submissions {
        *sub_status.entry(status.clone()).or_default() += 1;
    }
    let unanswered: Vec<Value> = submissions
        .iter()
        .filter(|(_, _, status)| status == "unanswered")
        .map(|(id, conv, status)| json!({ "id": id, "conv": conv, "status": status }))
        .collect();

    // 文档 revision：base / delta。
    let revisions = util::all(
        &db,
        "SELECT kind, count(*) AS n FROM document_revisions GROUP BY kind",
    )
    .unwrap_or_default();
    let base = revisions
        .iter()
        .find(|row| util::col_str(row, "kind") == "base")
        .map(|row| util::col_i64(row, "n"))
        .unwrap_or(0);
    let delta = revisions
        .iter()
        .find(|row| util::col_str(row, "kind") == "delta")
        .map(|row| util::col_i64(row, "n"))
        .unwrap_or(0);
    let docs = util::all(
        &db,
        "SELECT kind, count(*) AS n FROM documents GROUP BY kind ORDER BY n DESC",
    )
    .unwrap_or_default();
    let doc_total: i64 = docs.iter().map(|row| util::col_i64(row, "n")).sum();
    let by_kind: Vec<Value> = docs
        .iter()
        .map(|row| json!({ "kind": util::unquote(&util::col_str(row, "kind")), "n": util::col_i64(row, "n") }))
        .collect();
    let revision_total = base + delta;
    let delta_ratio = if revision_total > 0 {
        delta as f64 / revision_total as f64
    } else {
        0.
    };

    let wal_bytes = util::sidecar_bytes(durable_db, "-wal");
    let page_size = util::pragma_i64(&db, nous_chord::sqlite::inspect::Pragma::PageSize);
    let freelist = util::pragma_i64(&db, nous_chord::sqlite::inspect::Pragma::FreelistCount);
    let freelist_bytes = freelist * page_size;

    Ok(json!({
        "tasks": {
            "total": tasks.len() as i64,
            "byStatus": by_status,
            "stuck": stuck,
            "sleeping": sleeping,
            "avgMs": avg,
            "p95": p95,
            "slowest": slowest,
        },
        "submissions": {
            "total": submissions.len() as i64,
            "byStatus": sub_status
                .iter()
                .map(|(status, n)| json!({ "status": status, "n": n }))
                .collect::<Vec<_>>(),
            "unanswered": unanswered,
        },
        "documents": {
            "documents": doc_total,
            "byKind": by_kind,
            "revisions": revision_total,
            "base": base,
            "delta": delta,
            "deltaRatio": delta_ratio,
        },
        "walBytes": wal_bytes,
        "freelist": { "free": freelist, "bytes": freelist_bytes },
        "maintenance": maintenance(durable_db, wal_bytes, freelist, freelist_bytes),
    }))
}

/// 维修工单：只给命令文本，不执行（这页只读）。
fn maintenance(durable_db: &Path, wal_bytes: i64, freelist: i64, freelist_bytes: i64) -> Vec<Value> {
    let path = durable_db.display().to_string();
    vec![
        json!({
            "label": "先做一次备份",
            "why": "改动任何东西之前，先把库整份复制走 —— VACUUM 也是写操作。",
            "cmd": format!("cp \"{path}\" \"{path}.$(date +%Y%m%d-%H%M).bak\""),
            "danger": false,
        }),
        json!({
            "label": "把 WAL 收回去",
            "why": format!("WAL 现在 {wal_bytes} 字节。checkpoint 只读的页面做不了，得由写者或 sqlite3 来。"),
            "cmd": nous_chord::sqlite::inspect::advice_wal_checkpoint(durable_db),
            "danger": false,
        }),
        json!({
            "label": "VACUUM 要回空闲页",
            "why": format!("空闲页 {freelist} 个（{freelist_bytes} 字节）。这块停止服务后再跑 —— 它要独占写者。"),
            "cmd": format!("sqlite3 \"{path}\" \"VACUUM;\""),
            "danger": false,
        }),
        json!({
            "label": "看它到底是不是好的",
            "why": "integrity_check 会把整库走一遍；库大了要几分钟。",
            "cmd": nous_chord::sqlite::inspect::advice_integrity_check(durable_db),
            "danger": false,
        }),
    ]
}

/// 库还不存在时的空报告（端点照样 200，不把「没建」伪装成「坏了」）。
fn empty_report(durable_db: &Path) -> Value {
    json!({
        "tasks": { "total": 0, "byStatus": [], "stuck": [], "sleeping": 0, "avgMs": 0, "p95": 0, "slowest": [] },
        "submissions": { "total": 0, "byStatus": [], "unanswered": [] },
        "documents": { "documents": 0, "byKind": [], "revisions": 0, "base": 0, "delta": 0, "deltaRatio": 0. },
        "walBytes": 0,
        "freelist": { "free": 0, "bytes": 0 },
        "maintenance": maintenance(durable_db, 0, 0, 0),
    })
}
