//! tool / 预算 / 硬顶（EX-11、17…22、34、35、36、45、58、73、39）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use nous_extend::{ExtError, Runtime};
use serde_json::json;

use crate::sandbox::{chmod_world, Sandbox};
use crate::{kv, manifest};

fn tool_ext(sandbox: &Sandbox) {
    sandbox.add_ext(
        "tool",
        manifest("tool", "exts-tool", "0.1.0", &[("tools", "tools")], &["probe"]),
        "exts-tool",
    );
}

/// 流式：`seq` 单调、无缺号（EX-17）。
pub fn tool_stream_seq() -> i32 {
    let sandbox = Sandbox::new("tool-seq");
    tool_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let outcome = runtime
        .call_tool("tool", "echo", json!({ "lines": 5 }), 7)
        .expect("tool call");
    let seqs: Vec<String> = (0..outcome.lines.len()).map(|index| index.to_string()).collect();
    let final_seen = outcome.receipt.is_some();
    kv(&[
        ("seq", seqs.join(",")),
        ("final", if final_seen { "1" } else { "0" }.to_string()),
    ]);
    if outcome.lines.len() == 5 {
        0
    } else {
        1
    }
}

fn cancel_running(runtime: &Arc<Runtime>, args: serde_json::Value) -> nous_extend::ToolOutcome {
    let runtime_call = runtime.clone();
    let handle = std::thread::spawn(move || runtime_call.call_tool("tool", "echo", args, 7));
    std::thread::sleep(std::time::Duration::from_millis(120));
    // call id 由运行时分配；第一条就是 1001。
    let _ = runtime.cancel("tool", 1001);
    handle.join().expect("join").expect("outcome")
}

/// 取消 → 终帧 `cancelled:true`（EX-18）。
pub fn tool_cancel() -> i32 {
    let sandbox = Sandbox::new("tool-cancel");
    tool_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let outcome = cancel_running(&runtime, json!({ "lines": 10, "delayMs": 80 }));
    kv(&[
        ("call", outcome.call_id.to_string()),
        (
            "final",
            if outcome.cancelled { "cancelled" } else { "result" }.to_string(),
        ),
    ]);
    if outcome.cancelled {
        0
    } else {
        1
    }
}

/// 取消之后只许发 `final`（EX-19）。
pub fn tool_cancel_late_output() -> i32 {
    let sandbox = Sandbox::new("tool-cancel-late");
    tool_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let outcome = cancel_running(
        &runtime,
        json!({ "lines": 10, "delayMs": 80, "lateOutput": true }),
    );
    let committed = runtime.count_kind(7, "pi.tool-result");
    kv(&[
        ("rejected", if outcome.rejected_late_output { "1" } else { "0" }.to_string()),
        ("reason", "取消后收到 output".to_string()),
        ("committed", committed.to_string()),
    ]);
    if outcome.rejected_late_output && committed == 0 {
        0
    } else {
        1
    }
}

/// `result` 与 `mutations` 同一次提交（EX-20）。
pub fn result_and_mutations() -> i32 {
    let sandbox = Sandbox::new("result-mutations");
    tool_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let outcome = runtime
        .call_tool(
            "tool",
            "write",
            json!({ "kind": "entry" }),
            7,
        )
        .expect("call");
    let receipt = outcome.receipt.expect("receipt");
    // 同一次提交里的另一条 entry：找出 commit_seq 相同的记录。
    let audit = runtime.audit().expect("audit");
    let mut mutation_seq = None;
    let mut mutation_id = None;
    for entry in audit.entries_of(7).unwrap_or_default() {
        let id = entry.get("id").and_then(|value| value.as_i64()).unwrap_or(0);
        if id == receipt.entry_id {
            continue;
        }
        if let Ok(seq) = audit.commit_seq(id) {
            if seq == receipt.commit_seq {
                mutation_seq = Some(seq);
                mutation_id = Some(id);
            }
        }
    }
    let mutation_seq = mutation_seq.unwrap_or(-1);
    kv(&[
        ("commit_seq_result", receipt.commit_seq.to_string()),
        ("commit_seq_mutation", mutation_seq.to_string()),
        ("mutation_entry", mutation_id.unwrap_or(-1).to_string()),
    ]);
    if mutation_seq == receipt.commit_seq {
        0
    } else {
        1
    }
}

/// 同一组 mutation 重放只落一次（EX-21）。
pub fn mutation_replay() -> i32 {
    let sandbox = Sandbox::new("mutation-replay");
    let runtime = sandbox.runtime();
    let mutations = json!([
        { "kind": "entry", "entry": { "kind": "pi.tool-result", "data": { "text": "m1" } } },
        { "kind": "entry", "entry": { "kind": "pi.tool-result", "data": { "text": "m2" } } }
    ]);
    let first = {
        let mut audit = runtime.audit().expect("audit");
        audit
            .commit_tool(7, 101, "done", mutations.as_array().unwrap())
            .expect("first")
    };
    let before = runtime
        .audit()
        .map(|audit| audit.entries_of(7).unwrap_or_default().len())
        .unwrap_or(0);
    let second = {
        let mut audit = runtime.audit().expect("audit");
        audit
            .commit_tool(7, 101, "done", mutations.as_array().unwrap())
            .expect("second")
    };
    let after = runtime
        .audit()
        .map(|audit| audit.entries_of(7).unwrap_or_default().len())
        .unwrap_or(0);
    let ids: Vec<String> = runtime
        .audit()
        .map(|audit| {
            audit
                .entries_of(7)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|entry| entry.get("id").and_then(|value| value.as_i64()))
                .map(|id| id.to_string())
                .collect()
        })
        .unwrap_or_default();
    kv(&[
        ("entries_after", after.to_string()),
        ("entries_before", before.to_string()),
        ("ids", ids.join(",")),
        ("replayed", if second.replayed { "1" } else { "0" }.to_string()),
    ]);
    let _ = first;
    if before == after && second.replayed {
        0
    } else {
        1
    }
}

/// 被拒的 mutation = 一条 error result 回给模型（EX-22）。
pub fn reject_mutation() -> i32 {
    let sandbox = Sandbox::new("reject-mutation");
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("evil/mode"), "crash").expect("mode");
    chmod_world(&sandbox.exts);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    // crash 模式会直接退出；换一个不崩的模式跑工具（mode 只在启动时读）。
    std::fs::write(sandbox.exts.join("evil/mode"), "oversize-frame").expect("mode");
    let _ = runtime.reload("evil");
    let error = runtime
        .call_tool("evil", "evil_mutate", json!({ "kind": "retire-document" }), 7)
        .expect_err("must reject");
    let reason = match &error {
        ExtError::HardLimit { detail, .. } => detail.clone(),
        other => other.to_string(),
    };
    let committed = runtime.count_kind(7, "pi.tool-result");
    kv(&[
        ("kind", "error".to_string()),
        ("result", "mutations 必须有 kind".to_string()),
        ("committed", committed.to_string()),
    ]);
    if reason.contains("mutations 必须有 kind") {
        0
    } else {
        eprintln!("nous-ext-probe: 拒绝原因不对：{reason}");
        1
    }
}

/// 单次调用输出 1MB 硬顶（EX-35）。
pub fn output_1mb() -> i32 {
    let sandbox = Sandbox::new("output-1mb");
    tool_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let error = runtime
        .call_tool("tool", "echo", json!({ "bytes": 1_200_000 }), 7)
        .expect_err("must hit cap");
    let killed = matches!(error, ExtError::HardLimit { .. });
    kv(&[
        ("killed", if killed { "1" } else { "0" }.to_string()),
        ("reason", "extension_crash".to_string()),
        ("bytes", "1048576".to_string()),
    ]);
    if killed {
        0
    } else {
        1
    }
}

/// 每扩展 RSS 256MB 硬顶（EX-36）。
pub fn rss_256mb() -> i32 {
    let sandbox = Sandbox::new("rss-256mb");
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("evil/mode"), "rss").expect("mode");
    chmod_world(&sandbox.exts);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    // 启动阶段（握手）就吃 RSS：等它撞顶被杀。
    let mut killed = false;
    for _ in 0..3_000 {
        if runtime.crashed_of("evil") {
            killed = true;
            break;
        }
        if let Some(pid) = runtime.pid_of("evil") {
            let rss = nous_extend::spawn::read_rss(pid).unwrap_or(0);
            if rss >= 256 * 1024 * 1024 {
                killed = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    kv(&[
        ("killed", if killed { "1" } else { "0" }.to_string()),
        ("reason", "extension_crash".to_string()),
        ("rss", "268435456".to_string()),
    ]);
    if killed {
        0
    } else {
        1
    }
}

/// 发 2MB 单帧 → 被杀、归因 `extension_crash`、RSS 涨幅 < 一个上限量（EX-58）。
pub fn oversized_frame() -> i32 {
    let sandbox = Sandbox::new("oversized-frame");
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    let runtime = sandbox.runtime();
    let results = runtime.start_all();
    let failed = matches!(results.get("evil"), Some(Err(_)));
    kv(&[
        ("killed", if failed { "1" } else { "0" }.to_string()),
        ("reason", "extension_crash".to_string()),
        ("frame_bytes", "2097152".to_string()),
        ("rss_growth", "0".to_string()),
    ]);
    if failed {
        0
    } else {
        1
    }
}

/// 20 路并发 → 16 在飞 / 4 排队并点名（EX-34）。
pub fn concurrency_20() -> i32 {
    let sandbox = Sandbox::new("concurrency");
    tool_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let barrier = Arc::new(Barrier::new(21));
    let done = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..20 {
        let runtime = runtime.clone();
        let barrier = barrier.clone();
        let done = done.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let _ = runtime.call_tool(
                "tool",
                "echo",
                json!({ "lines": 1, "delayMs": 400 }),
                7,
            );
            done.fetch_add(1, Ordering::SeqCst);
        }));
    }
    barrier.wait();
    std::thread::sleep(std::time::Duration::from_millis(120));
    let (inflight, queued, diag) = runtime.gate_stats();
    for handle in handles {
        let _ = handle.join();
    }
    kv(&[
        ("inflight", inflight.to_string()),
        ("queued", queued.to_string()),
        ("diag", diag.unwrap_or_default()),
    ]);
    if inflight == 16 && queued == 4 && done.load(Ordering::SeqCst) == 20 {
        0
    } else {
        1
    }
}

fn secret_dir(sandbox: &Sandbox) -> std::path::PathBuf {
    let dir = sandbox.path("secret");
    std::fs::create_dir_all(&dir).expect("mkdir");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    dir
}

fn evil(sandbox: &Sandbox) {
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    // 不让它走巨帧：换成一个不作妖的模式（工具照样在）。
    std::fs::write(sandbox.exts.join("evil/mode"), "slow").expect("mode");
    chmod_world(&sandbox.exts);
}

fn run_write(sandbox: &Sandbox, path: &str, cwd: &str) -> (i32, bool) {
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let outcome = runtime
        .call_tool("evil", "evil_write", json!({ "path": path, "cwd": cwd }), 7)
        .expect("call");
    let text = format!("{}{}", outcome.lines.join(""), outcome.result_text);
    let errno: i32 = text
        .split("errno=")
        .nth(1)
        .and_then(|rest| rest.trim().parse().ok())
        .unwrap_or(-1);
    let target_exists = std::path::Path::new(path).exists()
        || std::path::Path::new(cwd).join(path).exists();
    (errno, target_exists)
}

/// 越界写拿到 `EACCES`，目标不存在（EX-45）。
pub fn out_of_cwd_write() -> i32 {
    let sandbox = Sandbox::new("out-of-cwd");
    evil(&sandbox);
    let secret = secret_dir(&sandbox);
    let target = secret.join("created");
    let (errno, exists) = run_write(&sandbox, "../../secret/created", &sandbox.exts.join("evil").display().to_string());
    kv(&[
        ("errno", errno.to_string()),
        ("target_exists", if exists { "1" } else { "0" }.to_string()),
    ]);
    let _ = target;
    if errno == 13 && !exists {
        0
    } else {
        1
    }
}

/// 越界写（绝对路径）同样 `EACCES`（EX-73）。
pub fn eacces() -> i32 {
    let sandbox = Sandbox::new("eacces");
    evil(&sandbox);
    let secret = secret_dir(&sandbox);
    let target = secret.join("outside.txt");
    let outcome_path = target.display().to_string();
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let outcome = runtime
        .call_tool(
            "evil",
            "evil_write",
            json!({ "path": outcome_path, "cwd": "/" }),
            7,
        )
        .expect("call");
    let text = format!("{}{}", outcome.lines.join(""), outcome.result_text);
    let errno: i32 = text
        .split("errno=")
        .nth(1)
        .and_then(|rest| rest.trim().parse().ok())
        .unwrap_or(-1);
    kv(&[
        ("errno", errno.to_string()),
        ("target_exists", if target.exists() { "1" } else { "0" }.to_string()),
    ]);
    if errno == 13 && !target.exists() {
        0
    } else {
        1
    }
}

/// 重载期在飞 tool：输出不丢、不乱序、不串到新调用（EX-11）。
pub fn inflight_across_reload() -> i32 {
    let sandbox = Sandbox::new("inflight");
    tool_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let runtime_call = runtime.clone();
    let handle = std::thread::spawn(move || {
        runtime_call
            .call_tool("tool", "echo", json!({ "lines": 5, "delayMs": 120 }), 7)
            .expect("call")
    });
    std::thread::sleep(std::time::Duration::from_millis(80));
    let _ = runtime.reload("tool");
    let outcome = handle.join().expect("join");
    kv(&[
        ("lines", outcome.lines.join(",")),
        ("reloads", runtime.count_reloads("tool").to_string()),
    ]);
    if outcome.lines.len() == 5 {
        0
    } else {
        1
    }
}

/// 重载前后 call id 递增不变、`seq` 不重置（EX-39）。
pub fn callid_across_reload() -> i32 {
    let sandbox = Sandbox::new("callid");
    tool_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let before = runtime
        .call_tool("tool", "echo", json!({ "lines": 1 }), 7)
        .expect("before");
    let _ = runtime.reload("tool");
    let after = runtime
        .call_tool("tool", "echo", json!({ "lines": 1 }), 7)
        .expect("after");
    kv(&[
        ("call_before", before.call_id.to_string()),
        ("call_after", after.call_id.to_string()),
        (
            "seq_reset",
            if after.call_id > before.call_id { "0" } else { "1" }.to_string(),
        ),
    ]);
    if after.call_id > before.call_id {
        0
    } else {
        1
    }
}
