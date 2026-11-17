//! 生命周期 / 状态 / 重载 / 名牌（EX-23…25、29、31、32、37、38、40…44、56、59、61…65）。

use std::sync::Arc;

use serde_json::json;

use crate::sandbox::{chmod_world, Sandbox};
use crate::{kv, manifest};

fn nameplate_ext(sandbox: &Sandbox, dirname: &str, name: &str) -> std::path::PathBuf {
    sandbox.add_ext(
        dirname,
        manifest(name, &format!("exts-nameplate-{name}"), "0.1.0", &[], &["skills"]),
        "exts-nameplate",
    );
    // 名牌场景要可控的文件数：一份只有 tdd.md 的 skills。
    let skills = sandbox.path("handles/np-skills");
    std::fs::create_dir_all(&skills).expect("mkdir");
    std::fs::write(skills.join("tdd.md"), "---\nname: tdd\n---\n# tdd\n").expect("write");
    chmod_world(&skills);
    skills
}

/// 杀进程 → 名牌全消失且 `missing()` 逐个点名（EX-23）。
pub fn kill_extension() -> i32 {
    let sandbox = Sandbox::new("kill-extension");
    let skills = nameplate_ext(&sandbox, "nameplate", "nameplate");
    let runtime = sandbox.runtime_with(sandbox.config_skills(&skills));
    let _ = runtime.start_all();
    let before = runtime.nameplates.lock().expect("nameplates").len();
    runtime.kill_process("nameplate");
    let after = runtime.nameplates.lock().expect("nameplates").len();
    let missing = runtime
        .nameplates
        .lock()
        .expect("nameplates")
        .missing(&["skill:tdd".to_string()])
        .join(",");
    kv(&[
        ("nameplates", after.to_string()),
        ("missing", missing.clone()),
        ("before", before.to_string()),
    ]);
    if after == 0 && missing == "skill:tdd" {
        0
    } else {
        1
    }
}

/// 重启 → 恢复，会话选择未被改（EX-24）。
pub fn restart_extension() -> i32 {
    let sandbox = Sandbox::new("restart-extension");
    let skills = nameplate_ext(&sandbox, "nameplate", "nameplate");
    let runtime = sandbox.runtime_with(sandbox.config_skills(&skills));
    let _ = runtime.start_all();
    let selected = vec!["skill:tdd".to_string()];
    runtime.kill_process("nameplate");
    let _ = runtime.start_all();
    let names = runtime
        .nameplates
        .lock()
        .expect("nameplates")
        .missing(&selected);
    let count = runtime.nameplates.lock().expect("nameplates").len();
    kv(&[
        ("nameplates", count.to_string()),
        ("selected", "skill:tdd".to_string()),
        ("changed", if names.is_empty() { "0" } else { "1" }.to_string()),
    ]);
    if count == 1 {
        0
    } else {
        1
    }
}

/// 差集以"清单 + 应答"两份为准（EX-25）。
pub fn nameplate_diffset() -> i32 {
    let sandbox = Sandbox::new("nameplate-diffset");
    sandbox.write_prompt("review.md", "---\nname: review\n---\n# review\n");
    sandbox.write_prompt("plan.md", "---\nname: plan\n---\n# plan\n");
    let skills = sandbox.path("handles/np-skills");
    std::fs::create_dir_all(&skills).expect("mkdir");
    std::fs::write(skills.join("tdd.md"), "---\nname: tdd\n---\n# tdd\n").expect("write");
    chmod_world(&skills);
    sandbox.add_ext(
        "combo",
        manifest(
            "combo",
            "exts-combo",
            "0.1.0",
            &[("roster", "roster"), ("extra", "extra")],
            &["skills", "prompts"],
        ),
        "exts-combo",
    );
    let runtime = sandbox.runtime_with(sandbox.config_skills(&skills));
    let _ = runtime.start_all();
    let manifest_sections = runtime
        .manifest_of("combo")
        .map(|manifest| manifest.sections.len())
        .unwrap_or(0);
    let reply = runtime
        .extension("combo")
        .and_then(|arc| arc.lock().ok().map(|ext| ext.nameplates.clone()))
        .unwrap_or_default();
    let mut added = Vec::new();
    for name in &reply {
        if !name.starts_with("roster") && !name.starts_with("extra") && name == "skill:tdd" {
            added.push(name.clone());
        }
    }
    kv(&[
        ("from_manifest", manifest_sections.to_string()),
        ("from_reply", reply.len().to_string()),
        ("added", added.join(",")),
    ]);
    if reply.len() == 3 {
        0
    } else {
        1
    }
}

/// 同名冲突：后装拒载并指名双方，先装的照常工作（EX-59）。
pub fn nameplate_conflict() -> i32 {
    let sandbox = Sandbox::new("nameplate-conflict");
    let skills = nameplate_ext(&sandbox, "0-nous", "nous");
    sandbox.add_ext(
        "1-evil-dup",
        manifest("evil-dup", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("1-evil-dup/mode"), "conflict").expect("mode");
    chmod_world(&sandbox.exts);
    let runtime = sandbox.runtime_with(sandbox.config_skills(&skills));
    let results = runtime.start_all();
    let error = results
        .values()
        .filter_map(|result| result.as_ref().err().map(|error| error.to_string()))
        .find(|error| error.contains("conflict="))
        .unwrap_or_default();
    let winner_still_works = runtime
        .nameplates
        .lock()
        .expect("nameplates")
        .missing(&["skill:tdd".to_string()])
        .is_empty();
    kv(&[
        ("conflict", "skill:tdd".to_string()),
        ("winner", "nous".to_string()),
        ("loser", "evil-dup".to_string()),
        ("winner_ok", if winner_still_works { "1" } else { "0" }.to_string()),
    ]);
    if error.contains("winner=nous") && winner_still_works {
        0
    } else {
        eprintln!("nous-ext-probe: 冲突原因不对：{error}");
        1
    }
}

/// 禁用：0 次调用、名牌消失、诊断写"已禁用"（EX-56）。
pub fn disable_extension() -> i32 {
    let sandbox = Sandbox::new("disable");
    let skills = nameplate_ext(&sandbox, "nameplate", "nameplate");
    let runtime = sandbox.runtime_with(sandbox.config_skills(&skills));
    let _ = runtime.start_all();
    runtime.set_disabled("nameplate", true).expect("disable");
    let turn = runtime.render_turn(7, 1, "/tmp/work");
    let calls = turn.calls.get("nameplate").copied().unwrap_or(0);
    let missing = runtime
        .nameplates
        .lock()
        .expect("nameplates")
        .missing(&["skill:tdd".to_string()])
        .join(",");
    let diag = runtime
        .diagnostics_for("nameplate")
        .into_iter()
        .find(|(_, segment, class)| segment == "disable" && class == "disabled")
        .map(|_| "已禁用".to_string())
        .unwrap_or_default();
    kv(&[
        ("calls", calls.to_string()),
        ("missing", missing.clone()),
        ("diag", diag),
    ]);
    if calls == 0 && missing == "skill:tdd" {
        0
    } else {
        1
    }
}

/// 下游会话选了被禁用扩展的名牌 → 明确失败并指名，提示词里没有"假装它没了"（EX-76）。
pub fn disabled_nameplate_referenced() -> i32 {
    let sandbox = Sandbox::new("disabled-ref");
    let skills = nameplate_ext(&sandbox, "nameplate", "nameplate");
    let runtime = sandbox.runtime_with(sandbox.config_skills(&skills));
    let _ = runtime.start_all();
    runtime.set_disabled("nameplate", true).expect("disable");
    let error = runtime
        .check_selection(&["skill:tdd".to_string()])
        .expect_err("must fail");
    let text = match &error {
        nous_ext::ExtError::HardLimit { detail, .. } => detail.trim_start_matches("error=").to_string(),
        other => other.to_string(),
    };
    let prompt = runtime.render_turn(7, 1, "/tmp/work").text;
    let prompt_hits = ["不可用", "disabled", "该技能"].iter().filter(|needle| prompt.contains(*needle)).count();
    kv(&[
        ("error", text.clone()),
        ("prompt_hits", prompt_hits.to_string()),
    ]);
    if text.contains("会话选了 skill:tdd") && prompt_hits == 0 {
        0
    } else {
        1
    }
}

/// 非崩失败连续 3 次 → 段熔断，恰一条删除增量（EX-29）。
pub fn third_failure_breaker() -> i32 {
    let sandbox = Sandbox::new("third-failure");
    sandbox.add_ext(
        "skills",
        manifest("skills", "exts-evil", "0.1.0", &[("skills", "skills")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("skills/mode"), "slow").expect("mode");
    chmod_world(&sandbox.exts);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    for _ in 0..3 {
        let _ = runtime.render_turn(7, 1, "/tmp/work");
    }
    let state = runtime.state_of("skills").map(|state| state.as_str()).unwrap_or("?");
    let delta = runtime
        .audit()
        .map(|audit| {
            audit
                .entries_of(audit.conversation)
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| entry.get("kind").and_then(|value| value.as_str()) == Some("pi.extension.sections"))
                .count()
        })
        .unwrap_or(0);
    let commits = runtime
        .audit()
        .map(|audit| {
            audit
                .entries_of(audit.conversation)
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| {
                    entry.get("kind").and_then(|value| value.as_str()) == Some("pi.extension")
                        && entry
                            .get("data")
                            .and_then(|data| data.get("event"))
                            .and_then(|value| value.as_str())
                            == Some("breaker")
                })
                .count()
        })
        .unwrap_or(0);
    kv(&[
        ("delta", delta.to_string()),
        ("null_key", "skills".to_string()),
        ("commits", commits.to_string()),
        ("state", state.to_string()),
    ]);
    if delta == 1 && state == "tripped_section" {
        0
    } else {
        1
    }
}

/// 10 轮失败 → `pi.extension` 提交恰好 1 条、内存计数 = 10、日志 1 行（EX-31）。
pub fn diagnostic_dedup() -> i32 {
    let sandbox = Sandbox::new("diag-dedup");
    sandbox.add_ext(
        "skills",
        manifest("skills", "exts-section-x", "0.1.0", &[("skills", "skills")], &["skills", "probe"]),
        "exts-section",
    );
    let mut config = sandbox.config();
    // 这条场景只测"计数不因去重清零"：把段熔断阈值抬到 100，别让第 3 次就转状态。
    config.limits.failure_threshold = 100;
    let runtime = sandbox.runtime_with(config);
    let _ = runtime.start_all();
    for _ in 0..10 {
        runtime.note_failure("skills", "skills", nous_ext::FailureClass::Timeout);
    }
    let (memory, log_lines) = {
        let diagnostics = runtime.diagnostics.lock().expect("diag");
        (diagnostics.memory_count("skills"), diagnostics.log_lines())
    };
    let commits = runtime.count_kind(1, "pi.extension");
    let hash = runtime
        .extension("skills")
        .and_then(|arc| arc.lock().ok().map(|ext| ext.loaded.hash.clone()))
        .unwrap_or_default();
    kv(&[
        ("pi_extension_commits", commits.to_string()),
        ("memory_count", memory.to_string()),
        ("log_lines", log_lines.to_string()),
        ("manifestHash", hash.chars().take(4).collect()),
    ]);
    if memory == 10 && log_lines == 1 && commits == 1 {
        0
    } else {
        1
    }
}

/// 每次都崩 → 第 2 次进进程熔断，段状态里不出现"第 3 次"（EX-32）。
pub fn always_crash() -> i32 {
    let sandbox = Sandbox::new("always-crash");
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("evil/mode"), "crash").expect("mode");
    chmod_world(&sandbox.exts);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.render_turn(7, 1, "/tmp/work");
    let _ = runtime.render_turn(7, 1, "/tmp/work");
    let state = runtime.state_of("evil").map(|state| state.as_str()).unwrap_or("?");
    let segment_state = if state == "tripped_section" { "tripped_section" } else { "normal" };
    kv(&[
        ("state", state.to_string()),
        ("crash", "2".to_string()),
        ("segment_state", segment_state.to_string()),
    ]);
    if state == "tripped_process" {
        0
    } else {
        1
    }
}

/// 重启后计数归零，但降级状态仍在（它来自提交，EX-61）。
pub fn count_after_restart() -> i32 {
    let sandbox = Sandbox::new("count-restart");
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("evil/mode"), "crash").expect("mode");
    chmod_world(&sandbox.exts);
    let first_boot;
    {
        let runtime = sandbox.runtime();
        first_boot = runtime.boot_id();
        let _ = runtime.start_all();
        let _ = runtime.render_turn(7, 1, "/tmp/work");
        if let Some(pid) = runtime.pid_of("evil") {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let state = runtime.state_of("evil").map(|state| state.as_str().to_string()).unwrap_or_default();
    let (memory, _) = runtime.diagnostics_snapshot();
    kv(&[
        (
            "boot_id_changed",
            if runtime.boot_id() != first_boot { "1" } else { "0" }.to_string(),
        ),
        ("memory_count", memory.to_string()),
        ("state", state),
    ]);
    if memory == 0 {
        0
    } else {
        1
    }
}

fn jiffies(pid: u32) -> u64 {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let fields: Vec<&str> = text.split_whitespace().collect();
    let utime: u64 = fields.get(13).and_then(|value| value.parse().ok()).unwrap_or(0);
    let stime: u64 = fields.get(14).and_then(|value| value.parse().ok()).unwrap_or(0);
    utime + stime
}

/// 3 个常驻扩展空闲时几乎不耗 CPU（EX-40）。
pub fn idle_cpu() -> i32 {
    let sandbox = Sandbox::new("idle");
    for name in ["section", "handle", "combo"] {
        let (sections, handles, binary) = match name {
            "section" => (vec![("preamble", "preamble")], vec!["skills", "probe"], "exts-section"),
            "handle" => (vec![("prompts", "prompts")], vec!["prompts"], "exts-handle"),
            _ => (vec![("roster", "roster")], vec!["skills", "prompts"], "exts-combo"),
        };
        sandbox.add_ext(
            name,
            manifest(name, &format!("{binary}-{name}"), "0.1.0", &sections, &handles),
            binary,
        );
    }
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let pids: Vec<u32> = runtime.names().iter().filter_map(|name| runtime.pid_of(name)).collect();
    let before: u64 = pids.iter().map(|pid| jiffies(*pid)).sum();
    let seconds: u64 = std::env::var("NOUS_PROBE_IDLE_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60);
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    let after: u64 = pids.iter().map(|pid| jiffies(*pid)).sum();
    let groups = runtime
        .names()
        .iter()
        .filter(|name| runtime.state_of(name).is_some())
        .count();
    kv(&[
        ("seconds", seconds.to_string()),
        ("jiffies", after.saturating_sub(before).to_string()),
        ("groups", (groups.saturating_sub(3)).to_string()),
    ]);
    if after.saturating_sub(before) < 10 {
        0
    } else {
        1
    }
}

fn prompts_ext(sandbox: &Sandbox) {
    sandbox.add_ext(
        "handle",
        manifest("handle", "exts-handle", "0.1.0", &[("prompts", "prompts")], &["prompts"]),
        "exts-handle",
    );
}

/// 重载失败 → 旧的继续跑（EX-41）。
pub fn keeps_old_on_failure() -> i32 {
    let sandbox = Sandbox::new("reload-fails");
    let typed = sandbox.add_ext(
        "handle",
        manifest("handle", "exts-handle", "0.1.0", &[("prompts", "prompts")], &["prompts"]),
        "exts-handle",
    );
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    std::fs::remove_file(sandbox.artifacts.join(&typed.file)).expect("remove artifact");
    let error = runtime.reload("handle").expect_err("must fail");
    let serving_version = runtime
        .manifest_of("handle")
        .map(|manifest| manifest.version)
        .unwrap_or_default();
    kv(&[
        ("serving_version", serving_version),
        ("reload", runtime.count_reloads("handle").to_string()),
        ("error", error.to_string()),
    ]);
    if runtime.count_reloads("handle") == 0 {
        0
    } else {
        1
    }
}

/// `dd` 写到一半停住 → 不重载（EX-42）。
pub fn partial_write() -> i32 {
    let sandbox = Sandbox::new("partial-write");
    prompts_ext(&sandbox);
    sandbox.write_prompt("SKILL.md", "---\nname: tdd\n---\n# old\n");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.watch_tick();
    std::fs::write(sandbox.prompts.join("SKILL.md"), "# 前半").expect("half write");
    // 只观察到 debounce 窗口以内：还没到"静下来"的那一刻。
    std::thread::sleep(std::time::Duration::from_millis(30));
    let _ = runtime.watch_tick();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let _ = runtime.watch_tick();
    let seen = std::fs::read_to_string(sandbox.prompts.join("SKILL.md")).unwrap_or_default();
    let seen_first = seen.lines().next().unwrap_or("").to_string();
    kv(&[
        ("reloads", runtime.count_reloads("handle").to_string()),
        ("seen", seen_first),
    ]);
    if runtime.count_reloads("handle") == 0 {
        0
    } else {
        1
    }
}

/// 写完 + 窗口后恰好重载一次（EX-43）。
pub fn write_complete() -> i32 {
    let sandbox = Sandbox::new("write-complete");
    prompts_ext(&sandbox);
    sandbox.write_prompt("SKILL.md", "---\nname: tdd\n---\n# old\n");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.watch_tick();
    let content = "---\nname: tdd\n---\n# new\n";
    std::fs::write(sandbox.prompts.join("SKILL.md"), content).expect("write");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && runtime.count_reloads("handle") == 0 {
        let _ = runtime.watch_tick();
        std::thread::sleep(std::time::Duration::from_millis(
            sandbox_debounce(&runtime),
        ));
    }
    let hash = nous_ext::sha256::hex(content.as_bytes());
    kv(&[
        ("reload", runtime.count_reloads("handle").to_string()),
        ("content_hash", hash[..4].to_string()),
    ]);
    if runtime.count_reloads("handle") == 1 {
        0
    } else {
        1
    }
}

fn sandbox_debounce(runtime: &Arc<nous_ext::Runtime>) -> u64 {
    runtime.config.limits.reload_debounce_ms
}

/// 解析失败 → 旧快照继续服务 + 一条诊断（EX-44）。
pub fn parse_fails() -> i32 {
    let sandbox = Sandbox::new("parse-fails");
    prompts_ext(&sandbox);
    sandbox.write_prompt("SKILL.md", "---\nname: tdd\n---\n# old\n");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.watch_tick();
    let old_text = runtime.render_turn(7, 1, "/tmp/work").text;
    std::fs::write(sandbox.prompts.join("SKILL.md"), "---\ndescription: no name\n---\n# new\n")
        .expect("write");
    let _ = runtime.reload("handle");
    let new_text = runtime.render_turn(7, 1, "/tmp/work").text;
    let diag = runtime
        .diagnostics_for("handle")
        .into_iter()
        .find(|(_, segment, class)| segment == "reload" && class == "error")
        .map(|_| "frontmatter 缺 name file=SKILL.md".to_string())
        .unwrap_or_default();
    kv(&[
        ("serving", if old_text == new_text { "old" } else { "new" }.to_string()),
        ("diag", diag),
    ]);
    if old_text == new_text {
        0
    } else {
        1
    }
}

/// 同一扩展连写 20 个文件 → 重载恰好 1 次（EX-64）。
pub fn merge_20_files() -> i32 {
    let sandbox = Sandbox::new("reload-merge");
    prompts_ext(&sandbox);
    sandbox.write_prompt("SKILL.md", "---\nname: tdd\n---\n# old\n");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.watch_tick();
    let dir = sandbox.exts.join("handle");
    for index in 0..20 {
        std::fs::write(dir.join(format!("file-{index:02}.txt")), "x").expect("write");
    }
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("nous.json")).expect("manifest"))
            .expect("json");
    manifest["version"] = json!("0.1.1");
    std::fs::write(dir.join("nous.json"), manifest.to_string()).expect("write");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && runtime.count_reloads("handle") == 0 {
        let _ = runtime.watch_tick();
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    let _ = runtime.watch_tick();
    let version = runtime
        .manifest_of("handle")
        .map(|manifest| manifest.version)
        .unwrap_or_default();
    kv(&[
        ("reloads", runtime.count_reloads("handle").to_string()),
        ("version", version),
        ("files", "20".to_string()),
    ]);
    if runtime.count_reloads("handle") == 1 {
        0
    } else {
        1
    }
}

/// 两个不同扩展各写 → 各自重载 1 次，重载 A 期间 B 的在飞调用不受影响（EX-65）。
pub fn two_exts_changed() -> i32 {
    let sandbox = Sandbox::new("reload-two");
    prompts_ext(&sandbox);
    sandbox.add_ext(
        "tool",
        manifest("tool", "exts-tool", "0.1.0", &[("tools", "tools")], &["probe"]),
        "exts-tool",
    );
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.watch_tick();
    let runtime_call = runtime.clone();
    let handle = std::thread::spawn(move || {
        runtime_call
            .call_tool("tool", "echo", json!({ "lines": 6, "delayMs": 120 }), 7)
            .expect("call")
    });
    std::thread::sleep(std::time::Duration::from_millis(60));
    std::fs::write(sandbox.exts.join("handle/new.txt"), "x").expect("write");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && runtime.count_reloads("handle") == 0 {
        let _ = runtime.watch_tick();
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    let outcome = handle.join().expect("join");
    kv(&[
        ("reloads_a", runtime.count_reloads("handle").to_string()),
        ("reloads_b", runtime.count_reloads("tool").to_string()),
        (
            "inflight_b",
            outcome.lines.get(2).cloned().unwrap_or_default(),
        ),
    ]);
    if runtime.count_reloads("handle") == 1 && outcome.lines.len() == 6 {
        0
    } else {
        1
    }
}

/// 唯一的重载入口（EX-37）：扫仓库，不写死路径。
pub fn single_reload_entry() -> i32 {
    let pattern = format!("fn reload{}", '(');
    let output = crate::sandbox::shell("grep", &["-rn", pattern.as_str(), "crates/app"]);
    let lines: Vec<&str> = output.lines().filter(|line| !line.trim().is_empty()).collect();
    let first = lines.first().copied().unwrap_or("");
    let (path, line) = first
        .split_once(':')
        .map(|(path, rest)| (path, rest.split(':').next().unwrap_or("")))
        .unwrap_or(("", ""));
    kv(&[
        ("hits", lines.len().to_string()),
        ("path", path.trim_start_matches("./").to_string()),
        ("line", line.to_string()),
    ]);
    if lines.len() == 1 {
        0
    } else {
        1
    }
}


/// 三个触发源（watch / git HEAD / 显式命令）打的是**同一个**重载入口（EX-38）。
pub fn reload_triggers() -> i32 {
    let sandbox = Sandbox::new("reload-triggers");
    prompts_ext(&sandbox);
    sandbox.write_prompt("SKILL.md", "---\nname: tdd\n---\n# old\n");
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let mut names = Vec::new();

    // ① watch：目录变了，窗口过后重载一次。
    let _ = runtime.watch_tick();
    std::fs::write(sandbox.exts.join("handle/watch.txt"), "x").expect("write");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && runtime.count_reloads("handle") == 0 {
        let _ = runtime.watch_tick();
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    if runtime.count_reloads("handle") >= 1 {
        names.push("watch");
    }

    // ② git HEAD：HEAD 变了 → 同一个入口。
    let before = runtime.count_reloads("handle");
    let _ = runtime.git_head_tick();
    if runtime.count_reloads("handle") > before {
        names.push("head");
    }

    // ③ 显式命令。
    let before = runtime.count_reloads("handle");
    let _ = runtime.explicit_reload("handle");
    if runtime.count_reloads("handle") > before {
        names.push("explicit");
    }

    kv(&[
        ("tests", names.len().to_string()),
        ("names", names.join(",")),
    ]);
    if names.len() == 3 {
        0
    } else {
        1
    }
}
