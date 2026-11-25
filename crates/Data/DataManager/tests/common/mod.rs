//! 判据辅助：造一份最小 durable 库（真 sqlite，schema 与本库一致），再插入真 entry/task。
//! 测试从 entry JSON 自己手算期望值，不靠 stats 端点自证。

use std::path::PathBuf;

use nous_durable::{Storage, StorageWrite};
use serde_json::{json, Value};

pub struct Db {
    pub path: PathBuf,
}

impl Db {
    /// 新空库：`Storage::open` 跑本库迁移（durable_schema v2）。
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "data-data-manager62-{tag}-{}-{:?}.sqlite",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
        let mut storage = Storage::open(&path).unwrap();
        storage.close().unwrap();
        Self { path }
    }

    /// 插一条 assistant entry：跟真实 entry JSON 同形（kind/model[]/usage/cost）。
    pub fn assistant(
        &self,
        id: i64,
        conversation: i64,
        at_ms: i64,
        provider: &str,
        model_id: &str,
        tokens: i64,
        cost: i64,
        error: bool,
    ) {
        let conn = rusqlite::Connection::open(&self.path).unwrap();
        let record = json!({
            "id": id,
            "conversationId": conversation,
            "kind": "pi.assistant",
            "model": [{
                "role": "assistant",
                "provider": provider,
                "modelId": model_id,
                "timestamp": at_ms,
                "stopReason": if error { "error" } else { "stop" },
                "usage": {
                    "totalTokens": tokens,
                    "cost": { "total": cost },
                },
            }],
        });
        let next_seq: i64 = conn.query_row(
            "SELECT next_seq FROM durable_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        conn.execute(
            "INSERT INTO record_ids (id, record_type) VALUES (?1, 'entry')",
            [id],
        ).unwrap();
        conn.execute(
            "INSERT INTO entries (id, conversation_id, head, commit_seq, record) VALUES (?1, ?2, NULL, ?3, ?4)",
            rusqlite::params![id, conversation, next_seq, record.to_string()],
        ).unwrap();
        conn.execute(
            "UPDATE durable_metadata SET next_seq = ?1, next_id = ?2 WHERE singleton = 1",
            rusqlite::params![next_seq + 1, (id + 1).to_string()],
        ).unwrap();
        drop(conn);
    }

    /// 插一条 task，status 是六态之一。
    pub fn task(&self, id: i64, conversation: i64, status: &str) {
        let conn = rusqlite::Connection::open(&self.path).unwrap();
        let record: Value = json!({
            "id": id, "conversationId": conversation, "kind": "pi.agent", "status": status,
        });
        conn.execute(
            "INSERT INTO tasks (id, conversation_id, kind, status, abort_requested, background, record) VALUES (?1, ?2, '\"pi.agent\"', ?3, 0, 0, ?4)",
            rusqlite::params![id, conversation, status, record.to_string()],
        ).unwrap();
        drop(conn);
    }

    /// `/v1/sessions/:id/entries` 同形投影：客户端从它逐条手算 totals。
    pub fn entries_projection(&self) -> Vec<Value> {
        let conn = rusqlite::Connection::open_with_flags(
            &self.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ).unwrap();
        let mut stmt = conn.prepare("SELECT id, record FROM entries ORDER BY id").unwrap();
        let rows = stmt.query_map([], |row| {
            let text: String = row.get(1)?;
            Ok(serde_json::from_str::<Value>(&text).unwrap())
        }).unwrap();
        rows.map(Result::unwrap).collect()
    }
}

/// 测试自己从 entries 手算：只认有 timestamp 的 pi.assistant（与 wire.md 同规矩）。
pub fn hand_totals(entries: &[Value]) -> (i64, i64, i64, i64) {
    let mut tokens = 0;
    let mut cost = 0;
    let mut requests = 0;
    let mut errors = 0;
    for entry in entries {
        if entry["kind"] != json!("pi.assistant") { continue; }
        let Some(usage) = entry["model"][0].get("usage") else { continue };
        if entry["model"][0].get("timestamp").and_then(Value::as_i64).unwrap_or(0) == 0 { continue; }
        tokens += usage["totalTokens"].as_i64().unwrap_or(0);
        cost += usage["cost"]["total"].as_i64().unwrap_or(0);
        requests += 1;
        if entry["model"][0]["stopReason"] == json!("error") { errors += 1; }
    }
    (tokens, cost, requests, errors)
}

/// 保留 StorageWrite 的导入，方便将来扩展 fixture。
#[allow(dead_code)]
fn _assert_storage_write_type(_: Option<StorageWrite>) {}
