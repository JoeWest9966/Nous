//! 机器编目（EX-54）。

use serde_json::Value;

use crate::sandbox::{repo_root, Sandbox};
use crate::{kv, server};

/// `/v1/machines` 读应用层；旧编目端点的字面量在代码里逐行空。
pub fn catalog_moved() -> i32 {
    let sandbox = Sandbox::new("catalog");
    // 库要先能开：造一个会话。
    {
        let runtime = sandbox.runtime();
        if let Some(audit) = runtime.audit_arc() {
            let mut audit = audit.lock().expect("audit");
            let _ = audit.ensure_session(1);
        }
    }
    let (addr, mut child) = server::spawn(&sandbox.db);
    let (status, body) = server::get(addr, "/v1/machines").expect("GET /v1/machines");
    let _ = child.kill();
    let value: Value = serde_json::from_str(&body).unwrap_or_else(|_| Value::Null);
    let machines: Vec<String> = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let triples: Vec<String> = value
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("triples"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let needle = format!("api/{}", "catalog");
    let hits = code_hits(&needle);
    kv(&[
        ("api_catalog_hits", hits.to_string()),
        ("machines", "runtime-1".to_string()),
        ("triples", triples.join(",")),
        ("all_machines", machines.join(",")),
    ]);
    if status == 200 && hits == 0 && machines == vec!["runtime-1".to_string(), "asus".to_string()] {
        0
    } else {
        1
    }
}

/// 在代码里找一段字面量（不扫 docs：spec 自己就写着它）。
pub fn code_hits(needle: &str) -> usize {
    let root = repo_root();
    let mut hits = 0;
    for dir in ["crates", "server", "src"] {
        let path = root.join(dir);
        if !path.exists() {
            continue;
        }
        hits += walk(&path, needle);
    }
    hits
}

fn walk(path: &std::path::Path, needle: &str) -> usize {
    let mut hits = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().map(|name| name == "node_modules").unwrap_or(false) {
                continue;
            }
            hits += walk(&path, needle);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            hits += text.matches(needle).count();
        }
    }
    hits
}
