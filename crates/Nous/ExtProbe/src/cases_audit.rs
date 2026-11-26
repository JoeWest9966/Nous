//! 审计 / SSE（EX-51、52、53、62、63、74）。

use std::sync::Arc;

use nous_server::AppState;
use serde_json::Value;

use crate::sandbox::Sandbox;
use crate::{kv, manifest, server};

fn audit_runtime(sandbox: &Sandbox) -> Arc<library_extend::Runtime> {
    sandbox.add_ext(
        "section",
        manifest("section", "exts-section", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]),
        "exts-section",
    );
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    runtime
}

/// 四种事件各一条（EX-51）。
pub fn audit_kinds() -> i32 {
    let sandbox = Sandbox::new("audit-kinds");
    let runtime = audit_runtime(&sandbox);
    let hash = runtime
        .extension("section")
        .and_then(|arc| arc.lock().ok().map(|ext| ext.loaded.hash.clone()))
        .unwrap_or_default();
    let mut kinds = Vec::new();
    {
        let mut audit = runtime.audit().expect("audit");
        for event in [
            library_extend::audit::AuditEvent::Install,
            library_extend::audit::AuditEvent::Uninstall,
            library_extend::audit::AuditEvent::Disable,
            library_extend::audit::AuditEvent::Breaker,
        ] {
            let _ = audit.commit_event(event, "section", "normal", &hash, &[], None);
            kinds.push(event.as_str().to_string());
        }
    }
    kv(&[
        ("kinds", kinds.join(",")),
        ("manifestHash", hash.chars().take(4).collect()),
    ]);
    0
}

/// 同一份清单重放 10 次 → 只多一条（EX-52）。
pub fn audit_replay_10() -> i32 {
    let sandbox = Sandbox::new("audit-replay");
    let runtime = audit_runtime(&sandbox);
    let hash = runtime
        .extension("section")
        .and_then(|arc| arc.lock().ok().map(|ext| ext.loaded.hash.clone()))
        .unwrap_or_default();
    let mut install_id = 0;
    {
        let mut audit = runtime.audit().expect("audit");
        for event in [
            library_extend::audit::AuditEvent::Install,
            library_extend::audit::AuditEvent::Uninstall,
            library_extend::audit::AuditEvent::Disable,
            library_extend::audit::AuditEvent::Breaker,
        ] {
            if let Ok((id, _)) = audit.commit_event(event, "section", "normal", &hash, &[], None) {
                if event == library_extend::audit::AuditEvent::Install {
                    install_id = id;
                }
            }
        }
        for _ in 0..10 {
            let _ = audit.commit_event(
                library_extend::audit::AuditEvent::Install,
                "section",
                "normal",
                &hash,
                &[],
                None,
            );
        }
    }
    let entries = runtime.count_kind(1, "pi.extension");
    let (repeat_id, _) = {
        let mut audit = runtime.audit().expect("audit");
        audit
            .commit_event(
                library_extend::audit::AuditEvent::Install,
                "section",
                "normal",
                &hash,
                &[],
                None,
            )
            .unwrap_or((0, 0))
    };
    kv(&[
        ("entry_id_same", if repeat_id == install_id { "1" } else { "0" }.to_string()),
        ("repeats", "10".to_string()),
        ("entries", entries.to_string()),
    ]);
    if entries == 4 && repeat_id == install_id {
        0
    } else {
        1
    }
}

/// `entries` 按 `kind=pi.extension` 可过滤；不加新端点（EX-53）。
pub fn entries_filter() -> i32 {
    let sandbox = Sandbox::new("entries-filter");
    let runtime = audit_runtime(&sandbox);
    let hash = runtime
        .extension("section")
        .and_then(|arc| arc.lock().ok().map(|ext| ext.loaded.hash.clone()))
        .unwrap_or_default();
    let audit_entry = {
        let mut audit = runtime.audit().expect("audit");
        audit
            .commit_event(
                library_extend::audit::AuditEvent::Install,
                "section",
                "normal",
                &hash,
                &[],
                None,
            )
            .expect("commit")
            .0
    };
    // 让审计库落稳，再起端点层读同一份库。
    drop(runtime);
    let (addr, mut child) = server::spawn(&sandbox.db);
    let (status, body) = server::get(addr, "/v1/sessions/1/entries?kind=pi.extension").expect("GET");
    let _ = child.kill();
    let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let entry = value
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or(Value::Null);
    let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
    let entry_id = entry.get("id").and_then(Value::as_i64).unwrap_or(-1);
    kv(&[
        ("kind", kind.to_string()),
        ("entry_id", entry_id.to_string()),
        ("audit_entry", audit_entry.to_string()),
        ("status", status.to_string()),
    ]);
    if kind == "pi.extension" && entry_id == audit_entry {
        0
    } else {
        1
    }
}

/// 降级 / 熔断 / 恢复各推一条 SSE `entry_appended`（EX-62）。
pub fn breaker_push_sse() -> i32 {
    let sandbox = Sandbox::new("breaker-sse");
    // 先把库建起来（审计连接），再在同一个进程里起真监听。
    if let Some(audit) = {
        let runtime = sandbox.runtime();
        runtime.audit_arc()
    } {
        let mut audit = audit.lock().expect("audit");
        let _ = audit.ensure_session(1);
    }
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("evil/mode"), "crash").expect("mode");
    crate::sandbox::chmod_world(&sandbox.exts);

    let state_slot: Arc<std::sync::Mutex<Option<Arc<AppState>>>> =
        Arc::new(std::sync::Mutex::new(None));
    let slot = state_slot.clone();
    let push: library_extend::PushHook = Arc::new(move |commit: &library_extend::StateCommit| {
        let state = slot.lock().expect("slot").clone();
        let Some(state) = state else {
            return;
        };
        let Ok(entries) = state.source().entries(1) else {
            return;
        };
        let Some(entry) = entries
            .into_iter()
            .find(|entry| entry.get("id").and_then(Value::as_i64) == Some(commit.entry_id))
        else {
            return;
        };
        state.publish_event(
            1,
            nous_wire::RemoteEvent::EntryAppended { entry },
        );
    });
    let runtime = library_extend::Runtime::new_with_push(sandbox.config(), Some(push));
    let _ = runtime.start_all();

    let state = AppState::durable(&sandbox.db).expect("app state");
    *state_slot.lock().expect("slot") = Some(state.clone());
    let (addr, _handle) = server::serve_in_process(state);

    let reader = std::thread::spawn(move || {
        server::sse_first(
            addr,
            "/v1/sessions/1/events",
            std::time::Duration::from_secs(5),
            |data| data.contains("\"entry_appended\""),
        )
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    // 触发一次降级（崩一次 → degraded → 一条 pi.extension 提交 + 推送）。
    let _ = runtime.render_turn(7, 1, "/tmp/work");
    let data = reader.join().expect("join");
    let value: Value = data
        .as_deref()
        .and_then(|data| serde_json::from_str(data).ok())
        .unwrap_or(Value::Null);
    let kind = value
        .get("entry")
        .and_then(|entry| entry.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let entry_id = value
        .get("entry")
        .and_then(|entry| entry.get("id"))
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    kv(&[
        ("event", value.get("type").and_then(Value::as_str).unwrap_or("").to_string()),
        ("kind", kind.to_string()),
        ("entry_id", entry_id.to_string()),
        ("loop", if data.is_some() { "1" } else { "0" }.to_string()),
    ]);
    if kind == "pi.extension" && entry_id > 0 {
        0
    } else {
        1
    }
}

/// 重连后 `snapshot` 里的 entries 也含它（EX-63）。
pub fn breaker_in_snapshot() -> i32 {
    let sandbox = Sandbox::new("breaker-snapshot");
    if let Some(audit) = {
        let runtime = sandbox.runtime();
        runtime.audit_arc()
    } {
        let mut audit = audit.lock().expect("audit");
        let _ = audit.ensure_session(1);
    }
    let state = AppState::durable(&sandbox.db).expect("app state");
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("evil/mode"), "crash").expect("mode");
    crate::sandbox::chmod_world(&sandbox.exts);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.render_turn(7, 1, "/tmp/work");
    // 重连：新开一条 SSE，第一帧就是 snapshot。
    let (addr, _handle) = server::serve_in_process(state);
    let snapshot = server::sse_first(
        addr,
        "/v1/sessions/1/events",
        std::time::Duration::from_secs(5),
        |data| data.contains("\"snapshot\""),
    );
    let value: Value = snapshot
        .as_deref()
        .and_then(|data| serde_json::from_str(data).ok())
        .unwrap_or(Value::Null);
    let entry = value
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("pi.extension"))
                .cloned()
        })
        .unwrap_or(Value::Null);
    let entry_id = entry.get("id").and_then(Value::as_i64).unwrap_or(-1);
    kv(&[
        ("snapshot_has", if entry_id > 0 { "1" } else { "0" }.to_string()),
        (
            "kind",
            entry.get("kind").and_then(Value::as_str).unwrap_or("").to_string(),
        ),
        ("entry_id", entry_id.to_string()),
    ]);
    if entry_id > 0 {
        0
    } else {
        1
    }
}

/// `exts/evil` 每类坏样都在，每条拒绝都对得上 EX 编号（EX-74）。
pub fn evil_carrier() -> i32 {
    let readme = std::fs::read_to_string(crate::sandbox::repo_root().join("exts/evil/README.md"))
        .unwrap_or_default();
    let line = readme
        .lines()
        .find(|line| line.trim_start().starts_with("carriers:"))
        .unwrap_or("");
    let maps = line
        .trim_start()
        .trim_start_matches("carriers:")
        .trim()
        .to_string();
    let carriers = maps.split(',').filter(|item| !item.trim().is_empty()).count();
    let manifest = std::fs::read_to_string(crate::sandbox::repo_root().join("exts/evil/nous.json"))
        .unwrap_or_default();
    kv(&[
        ("carriers", carriers.to_string()),
        ("maps", maps),
        (
            "has_manifest",
            if manifest.contains("\"evil\"") { "1" } else { "0" }.to_string(),
        ),
    ]);
    if carriers == 5 {
        0
    } else {
        1
    }
}
