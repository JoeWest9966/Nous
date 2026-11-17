//! `nous-ext-probe`：判据的执行器。
//!
//! 一个 case = 一个场景 = 一组真操作（起进程、发帧、读 `/proc`、落提交），stdout 只放
//! `key=value` 观测。**期望值写死在附录 A 里，这里不打印期望值**（spec 读法第 7 条）。
//!
//! SKIP 不是这个程序的职责：`judge.sh` 负责"载体不存在 → SKIP"。

mod cases_audit;
mod cases_catalog;
mod cases_handle;
mod cases_life;
mod cases_manifest;
mod cases_section;
mod cases_tool;
mod sandbox;
mod server;

use serde_json::{json, Value};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "--install-worker" {
        // 原子切换的受害者：被杀在任意时点都不许留下半装（EX-77）。
        let root = args.get(2).cloned().unwrap_or_default();
        let version = args.get(3).cloned().unwrap_or_else(|| "0.1.0".to_string());
        cases_manifest::install_worker(&root, &version);
        return;
    }
    if args.len() >= 2 && args[1] == "--ids" {
        for id in cases_manifest::ids() {
            println!("{id}");
        }
        return;
    }
    let Some(case) = args.get(1) else {
        eprintln!("用法：nous-ext-probe <case> [--dirty]");
        std::process::exit(2);
    };
    let dirty = args.iter().any(|arg| arg == "--dirty");
    let code = match case.as_str() {
        // 清单 / 装载 / 产物（EX-01…06、48、49、50、66、75、77）
        "manifest-paths" => cases_manifest::manifest_paths(dirty),
        "manifest-forbidden-keys" => cases_manifest::manifest_forbidden_keys(dirty),
        "load-missing-handle" => cases_manifest::load_missing_handle(),
        "load-empty-extension" => cases_manifest::load_empty_extension(),
        "protocol-mismatch" => cases_manifest::protocol_mismatch(),
        "byte-flip" => cases_manifest::byte_flip(),
        "triple-not-compatible" => cases_manifest::triple_not_compatible(),
        "no-toolchain-in-tree" => cases_manifest::no_toolchain_in_tree(dirty),
        "boot-load-failure" => cases_manifest::boot_load_failure(),
        "provenance" => cases_manifest::provenance(),
        "atomic-swap-20x" => cases_manifest::atomic_swap_20x(),
        "kernel-untouched-v2" => cases_manifest::kernel_untouched_v2(),
        // section / 帧（EX-07…16、55、78、79、80、28）
        "one-process-per-extension" => cases_section::one_process_per_extension(),
        "frame-keys" => cases_section::frame_keys(),
        "frame-forbidden-keys" => cases_section::frame_forbidden_keys(dirty),
        "calls-per-turn" => cases_section::calls_per_turn(),
        "truncated-frame" => cases_section::truncated_frame(),
        "section-twice" => cases_section::section_twice(),
        "pi-system-count" => cases_section::pi_system_count(),
        "shuffle-order" => cases_section::shuffle_order(),
        "wrong-tag" => cases_section::wrong_tag(),
        "crash-once" => cases_section::crash_once(),
        "canary-cross-session" => cases_section::canary_cross_session(),
        "canary-cross-extension" => cases_section::canary_cross_extension(),
        "canary-cross-request" => cases_section::canary_cross_request(),
        "clock-invisible" => cases_section::clock_invisible(),
        // tool / 预算 / 硬顶（EX-17…22、34、35、36、58、45、73、11、39）
        "tool-stream-seq" => cases_tool::tool_stream_seq(),
        "tool-cancel" => cases_tool::tool_cancel(),
        "tool-cancel-late-output" => cases_tool::tool_cancel_late_output(),
        "result-and-mutations" => cases_tool::result_and_mutations(),
        "mutation-replay" => cases_tool::mutation_replay(),
        "reject-mutation" => cases_tool::reject_mutation(),
        "output-1mb" => cases_tool::output_1mb(),
        "rss-256mb" => cases_tool::rss_256mb(),
        "oversized-frame" => cases_tool::oversized_frame(),
        "concurrency-20" => cases_tool::concurrency_20(),
        "out-of-cwd-write" => cases_tool::out_of_cwd_write(),
        "eacces" => cases_tool::eacces(),
        "inflight-across-reload" => cases_tool::inflight_across_reload(),
        "callid-across-reload" => cases_tool::callid_across_reload(),
        // 生命周期 / 状态 / 重载 / 名牌（EX-23…25、29、31、32、37、38、40…44、56、59、61…65）
        "kill-extension" => cases_life::kill_extension(),
        "restart-extension" => cases_life::restart_extension(),
        "nameplate-diffset" => cases_life::nameplate_diffset(),
        "nameplate-conflict" => cases_life::nameplate_conflict(),
        "disable-extension" => cases_life::disable_extension(),
        "disabled-nameplate-referenced" => cases_life::disabled_nameplate_referenced(),
        "third-failure-breaker" => cases_life::third_failure_breaker(),
        "diagnostic-dedup" => cases_life::diagnostic_dedup(),
        "always-crash" => cases_life::always_crash(),
        "count-after-restart" => cases_life::count_after_restart(),
        "idle-cpu" => cases_life::idle_cpu(),
        "reload-fails-keeps-old" => cases_life::keeps_old_on_failure(),
        "partial-write" => cases_life::partial_write(),
        "write-complete" => cases_life::write_complete(),
        "parse-fails" => cases_life::parse_fails(),
        "reload-merge-20" => cases_life::merge_20_files(),
        "reload-two-exts" => cases_life::two_exts_changed(),
        "single-reload-entry" => cases_life::single_reload_entry(),
        "reload-triggers" => cases_life::reload_triggers(),
        // 句柄 / 隔离（EX-26、27、46、47、60、67…72）
        "deliver-twice" => cases_handle::deliver_twice(),
        "fd-no-handle-value" => cases_handle::fd_no_handle_value(dirty),
        "environ-whitelist" => cases_handle::environ_whitelist(dirty),
        "fd-no-db" => cases_handle::fd_no_db(dirty),
        "handle-path-escape" => cases_handle::handle_path_escape(),
        "symlink-escape" => cases_handle::symlink_escape(),
        "truncation-deterministic" => cases_handle::truncation_deterministic(),
        "uid-allocation" => cases_handle::uid_allocation(),
        "cap-setuid" => cases_handle::cap_setuid(),
        "fd-exact-set" => cases_handle::fd_exact_set(),
        "groups-empty" => cases_handle::groups_empty(dirty),
        // 审计 / SSE / 编目（EX-51…54、62、63、76）
        "audit-kinds" => cases_audit::audit_kinds(),
        "audit-replay-10" => cases_audit::audit_replay_10(),
        "entries-filter" => cases_audit::entries_filter(),
        "breaker-push-sse" => cases_audit::breaker_push_sse(),
        "breaker-in-snapshot" => cases_audit::breaker_in_snapshot(),
        "evil-carrier" => cases_audit::evil_carrier(),
        "catalog-moved" => cases_catalog::catalog_moved(),
        other => {
            eprintln!("nous-ext-probe: 没有这个 case：{other}");
            std::process::exit(2);
        }
    };
    std::process::exit(code);
}

/// 一个干净的最小清单。
pub fn manifest(
    name: &str,
    file: &str,
    version: &str,
    sections: &[(&str, &str)],
    handles: &[&str],
) -> Value {
    json!({
        "name": name,
        "version": version,
        "protocol": 3,
        "file": file,
        "entry": file,
        "sections": sections
            .iter()
            .map(|(key, tag)| json!({ "key": key, "tag": tag }))
            .collect::<Vec<Value>>(),
        "handles": handles,
        "summary": format!("{name} sample"),
        "category": "sample",
    })
}

/// `key=value` 一行。
pub fn kv(pairs: &[(&str, String)]) {
    let line: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    println!("{}", line.join(" "));
}
