//! 数据监控室六屏复原：`/v1/stats/{overview,conversations,conversation/:id,cost,health}` 的判据。
//!
//! 每个测试自己用 rusqlite 往真库里插同形记录，再对端点输出与「手算期望」对账 ——
//! 不靠端点自证。库由 `common::Db::new` 建（走本库真迁移链，schema v2）。

mod common;

use common::Db;
use serde_json::{json, Value};

fn conn(db: &Db) -> rusqlite::Connection {
    rusqlite::Connection::open(&db.path).unwrap()
}

/// 插一条 entry（record 原样写入，库只管存）。
fn entry(db: &Db, id: i64, conversation: i64, record: Value) {
    let c = conn(db);
    c.execute(
        "INSERT INTO entries (id, conversation_id, head, commit_seq, record) VALUES (?1, ?2, NULL, ?3, ?4)",
        rusqlite::params![id, conversation, id, record.to_string()],
    )
    .unwrap();
}

/// 一条 user entry（喂 title / prompt）。
fn user_entry(db: &Db, id: i64, conversation: i64, text: &str) {
    entry(db, id, conversation, json!({
        "id": id, "conversationId": conversation, "kind": "pi.user",
        "model": [{ "role": "user", "timestamp": 1_700_000_000_000i64,
                    "content": [{ "type": "text", "text": text }] }],
    }));
}

/// 一条 assistant entry，带 usage 分项与可选 toolCall。
fn assistant_entry(
    db: &Db,
    id: i64,
    conversation: i64,
    at: i64,
    model_id: &str,
    usage: (i64, i64, i64, i64, i64), // input, output, cacheRead, cacheWrite, reasoning
    with_tool: bool,
    error: bool,
) {
    let (input, output, cache_read, cache_write, reasoning) = usage;
    let total = input + output + cache_read;
    let mut content = vec![json!({ "type": "text", "text": format!("answer #{id}") })];
    if with_tool {
        content.push(json!({
            "type": "toolCall", "id": format!("call-{id}"),
            "name": "read", "arguments": { "path": "/tmp/x" },
        }));
    }
    entry(db, id, conversation, json!({
        "id": id, "conversationId": conversation, "kind": "pi.assistant",
        "model": [{
            "role": "assistant", "provider": "p", "modelId": model_id,
            "timestamp": at, "stopReason": if error { "error" } else { "stop" },
            "content": content,
            "usage": {
                "input": input, "output": output, "cacheRead": cache_read,
                "cacheWrite": cache_write, "reasoning": reasoning,
                "totalTokens": total, "cost": { "total": 3 },
            },
        }],
    }));
}

fn conversation_row(db: &Db, id: i64, owner_conv: Option<i64>) {
    let c = conn(db);
    c.execute(
        "INSERT INTO conversations (id, owner_conversation_id, owner_task_id, record) VALUES (?1, ?2, NULL, ?3)",
        rusqlite::params![id, owner_conv, json!({ "id": id }).to_string()],
    )
    .unwrap();
}

fn task_row(db: &Db, id: i64, conversation: i64, status: &str, started: i64, ended: Option<i64>) {
    let record = json!({
        "id": id, "conversationId": conversation, "kind": "pi.agent", "status": status,
        "startedAt": started,
        "endedAt": ended,
        "state": { "outcome": { "status": "done" } },
    });
    let c = conn(db);
    c.execute(
        "INSERT INTO tasks (id, conversation_id, kind, status, abort_requested, background, record) VALUES (?1, ?2, '\"pi.agent\"', ?3, 0, 0, ?4)",
        rusqlite::params![id, conversation, status, record.to_string()],
    )
    .unwrap();
}

fn submission_row(db: &Db, id: i64, conversation: i64, status: &str) {
    let c = conn(db);
    c.execute(
        "INSERT INTO submissions (id, conversation_id, request_id, status, record) VALUES (?1, ?2, NULL, ?3, ?4)",
        rusqlite::params![id, conversation, status, json!({ "id": id }).to_string()],
    )
    .unwrap();
}

fn document_row(db: &Db, id: i64, conversation: i64, kind: &str) {
    let c = conn(db);
    c.execute(
        "INSERT INTO documents (id, kind, family, key_value, scope_kind, owner_id, created_at, retired_at, record) \
         VALUES (?1, ?2, 0, 'k', 'conversation', ?3, 1, NULL, ?4)",
        rusqlite::params![id, format!("\"{kind}\""), conversation, json!({ "id": id }).to_string()],
    )
    .unwrap();
    for (seq, kind) in [(1i64, "base"), (2, "delta")] {
        c.execute(
            "INSERT INTO document_revisions (document_id, seq, kind, version, content) VALUES (?1, ?2, ?3, 1, '{}')",
            rusqlite::params![id, seq, kind],
        )
        .unwrap();
    }
}

#[test]
fn overview_reports_real_dbstat_bytes_and_pragmas() {
    let db = Db::new("overview");
    user_entry(&db, 1, 1, "第一条会话的开场白");
    assistant_entry(&db, 2, 1, 1_700_000_000_000, "m", (10, 20, 5, 1, 0), false, false);
    assistant_entry(&db, 3, 1, 1_700_000_001_000, "m", (7, 3, 0, 0, 0), false, false);

    let out = nous_stats::overview(&db.path).unwrap();
    // pragma 是真数。
    assert!(out["pragmas"]["pageSize"].as_i64().unwrap() > 0, "{out}");
    assert!(out["pragmas"]["pageCount"].as_i64().unwrap() > 0);
    assert_eq!(out["pragmas"]["schemaVersion"], json!(2));
    // 表行数。
    assert_eq!(out["counts"]["entries"], json!(3));
    assert_eq!(out["counts"]["conversations"], json!(0));
    // dbstat 真页数：entries 表必须有字节与非零页。
    assert_eq!(out["dbstatOk"], json!(true), "dbstat 应该可用：{out}");
    let tables = out["tables"].as_array().unwrap();
    let entries_table = tables.iter().find(|t| t["name"] == json!("entries")).unwrap();
    assert!(entries_table["bytes"].as_i64().unwrap() > 0, "entries 表字节应 > 0");
    assert!(entries_table["pages"].as_i64().unwrap() > 0);
    // entriesByKind 分布之和 == counts.entries。
    let kinds = out["entriesByKind"].as_array().unwrap();
    let kind_total: i64 = kinds.iter().map(|k| k["count"].as_i64().unwrap()).sum();
    assert_eq!(kind_total, 3);
    assert!(kinds.iter().any(|k| k["kind"] == json!("pi.assistant")));
    // integrity 真跑过且 ok。
    assert_eq!(out["integrity"]["checked"], json!(true));
    assert_eq!(out["integrity"]["ok"], json!(true));
    // bytes 自洽（free <= db）。
    assert!(out["bytes"]["free"].as_i64().unwrap() <= out["bytes"]["db"].as_i64().unwrap());
    std::fs::remove_file(&db.path).ok();
}

#[test]
fn conversations_aggregate_per_conversation_and_totals() {
    let db = Db::new("convs");
    conversation_row(&db, 1, None);
    conversation_row(&db, 2, Some(1));
    user_entry(&db, 1, 1, "会话一的第一句");
    assistant_entry(&db, 2, 1, 1_700_000_000_000, "m1", (10, 20, 5, 0, 0), true, false); // total 35
    assistant_entry(&db, 3, 1, 1_700_000_010_000, "m2", (5, 5, 0, 0, 0), false, true); // total 10
    assistant_entry(&db, 4, 2, 1_700_000_020_000, "m1", (1, 1, 0, 0, 0), false, false); // total 2

    let out = nous_stats::conversations(&db.path).unwrap();
    assert_eq!(out["count"], json!(2));
    let list = out["list"].as_array().unwrap();

    let conv1 = list.iter().find(|r| r["id"] == json!(1)).unwrap();
    assert_eq!(conv1["entries"], json!(3), "user + 2 assistant");
    assert_eq!(conv1["tokens"], json!(45), "35 + 10");
    assert_eq!(conv1["toolCalls"], json!(1));
    assert_eq!(conv1["firstAt"], json!(1_700_000_000_000i64));
    assert_eq!(conv1["lastAt"], json!(1_700_000_010_000i64));
    assert_eq!(conv1["title"], json!("会话一的第一句"));
    let models = conv1["models"].as_array().unwrap();
    assert!(models.contains(&json!("m1")) && models.contains(&json!("m2")));

    let conv2 = list.iter().find(|r| r["id"] == json!(2)).unwrap();
    assert_eq!(conv2["ownerConv"], json!(1));
    assert_eq!(conv2["entries"], json!(1));
    assert_eq!(conv2["tokens"], json!(2));

    assert_eq!(out["totals"]["entries"], json!(4));
    assert_eq!(out["totals"]["tokens"], json!(47));
    std::fs::remove_file(&db.path).ok();
}

#[test]
fn conversation_detail_has_tasks_documents_submissions_and_prompt() {
    let db = Db::new("detail");
    conversation_row(&db, 1, None);
    user_entry(&db, 1, 1, "你好");
    assistant_entry(&db, 2, 1, 1_700_000_000_000, "m", (10, 20, 5, 0, 0), false, false);
    task_row(&db, 10, 1, "terminal", 1_700_000_000_000, Some(1_700_000_002_000));
    submission_row(&db, 20, 1, "unanswered");
    document_row(&db, 30, 1, "pi.agent");

    let out = nous_stats::conversation(&db.path, 1).unwrap().expect("会话 1 存在");
    // entries 按时间正序，seq 是真 commit_seq。
    let entries = out["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["kind"], json!("pi.user"));
    assert_eq!(entries[1]["seq"], json!(2));
    // tasks 带起止与时长。
    let tasks = out["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["kind"], json!("pi.agent"), "kind 去引号");
    assert_eq!(tasks[0]["ms"], json!(2000));
    assert_eq!(tasks[0]["outcome"], json!("done"));
    // documents 带 revision 计数。
    let docs = out["documents"].as_array().unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["kind"], json!("pi.agent"));
    assert_eq!(docs[0]["revisions"], json!(2));
    // submissions。
    assert_eq!(out["submissions"][0]["status"], json!("unanswered"));
    // prompt 重建：消息流 + realInputTokens。
    let prompt = &out["prompt"];
    assert_eq!(prompt["messages"].as_array().unwrap().len(), 2);
    assert_eq!(prompt["totals"]["realInputTokens"], json!(10), "最后一条 assistant 的 input");
    assert_eq!(prompt["totals"]["messages"], json!(2));

    // 不存在的会话 → None（端点层 404）。
    assert!(nous_stats::conversation(&db.path, 999).unwrap().is_none());
    std::fs::remove_file(&db.path).ok();
}

#[test]
fn cost_buckets_carry_usage_breakdown_and_hour_bucket() {
    let db = Db::new("cost");
    // 两天各一条，验证 byDay 两个桶、byHour 也在。
    assistant_entry(&db, 1, 1, 1_700_000_000_000, "m1", (10, 20, 5, 2, 1), false, false);
    assistant_entry(&db, 2, 1, 1_708_640_000_000, "m2", (7, 3, 1, 4, 2), false, false);

    let out = nous_stats::cost(&db.path).unwrap();
    let totals = &out["totals"];
    assert_eq!(totals["entries"], json!(2));
    assert_eq!(totals["input"], json!(17));
    assert_eq!(totals["output"], json!(23));
    assert_eq!(totals["cacheRead"], json!(6));
    assert_eq!(totals["cacheWrite"], json!(6));
    assert_eq!(totals["reasoning"], json!(3));
    assert_eq!(totals["cost"], json!(6.0));
    // tokens 只算 totalTokens（= input + output + cacheRead）。
    assert_eq!(totals["tokens"], json!(46));

    assert_eq!(out["byDay"].as_array().unwrap().len(), 2, "{out}");
    assert!(!out["byHour"].as_array().unwrap().is_empty(), "byHour 不应为空");
    let models = out["byModel"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    // 每个桶都带 input/output/cacheRead。
    for bucket in models {
        assert!(bucket["input"].as_i64().unwrap() >= 0);
        assert!(bucket["output"].as_i64().is_some());
        assert!(bucket["cacheRead"].as_i64().is_some());
    }
    assert!(out["note"].as_str().unwrap().contains("不代表免费"));
    std::fs::remove_file(&db.path).ok();
}

#[test]
fn health_reports_stuck_slowest_submissions_and_revisions() {
    let db = Db::new("health");
    let now = 1_700_100_000_000i64;
    // 一个卡住的任务（running 且起于 20 分钟前）。
    task_row(&db, 1, 1, "running", now - 20 * 60 * 1000, None);
    // 两个已结束的任务（用于 avg / p95 / slowest）。
    task_row(&db, 2, 1, "terminal", now - 100_000, Some(now - 90_000)); // 10s
    task_row(&db, 3, 1, "terminal", now - 100_000, Some(now - 94_000)); // 6s
    submission_row(&db, 10, 1, "unanswered");
    submission_row(&db, 11, 1, "done");
    document_row(&db, 20, 1, "pi.agent");

    let out = nous_stats::health(&db.path, now).unwrap();
    // 卡住任务。
    let stuck = out["tasks"]["stuck"].as_array().unwrap();
    assert_eq!(stuck.len(), 1);
    assert_eq!(stuck[0]["id"], json!(1));
    assert!(stuck[0]["ageMs"].as_i64().unwrap() >= 20 * 60 * 1000);
    // 状态分布。
    let by_status = out["tasks"]["byStatus"].as_array().unwrap();
    assert!(by_status.iter().any(|b| b["status"] == json!("running") && b["count"] == json!(1)));
    // 时长。
    assert_eq!(out["tasks"]["avgMs"], json!(8000), "(10000 + 6000) / 2");
    assert_eq!(out["tasks"]["p95"], json!(10000));
    assert_eq!(out["tasks"]["slowest"].as_array().unwrap().len(), 2);
    // 提交。
    assert_eq!(out["submissions"]["unanswered"].as_array().unwrap().len(), 1);
    assert_eq!(out["submissions"]["total"], json!(2));
    // 文档 revision：1 个文档 × (1 base + 1 delta)。
    assert_eq!(out["documents"]["base"], json!(1));
    assert_eq!(out["documents"]["delta"], json!(1));
    assert_eq!(out["documents"]["revisions"], json!(2));
    assert_eq!(out["documents"]["documents"], json!(1));
    // 维修工单至少 4 条，且只给命令不执行。
    let maint = out["maintenance"].as_array().unwrap();
    assert!(maint.len() >= 4);
    assert!(maint.iter().any(|m| m["cmd"].as_str().unwrap().contains("wal_checkpoint")));
    std::fs::remove_file(&db.path).ok();
}
