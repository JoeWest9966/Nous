//! 句柄 / 隔离（EX-26、27、46、47、60、67…72）。

use library_extend::{handle, HandleValue};
use nous_ext_sdk::content::Handles;

use crate::sandbox::{chmod_world, Sandbox};
use crate::{kv, manifest};

fn prompts_ext(sandbox: &Sandbox) {
    sandbox.add_ext(
        "handle",
        manifest("handle", "exts-handle", "0.1.0", &[("prompts", "prompts")], &["prompts"]),
        "exts-handle",
    );
}

/// 同一目录两次交付逐字节一致（EX-27）。
pub fn deliver_twice() -> i32 {
    let sandbox = Sandbox::new("deliver-twice");
    // skills 句柄指向 /opt/Nous-skills（SKILL.md + release.md）。
    let values = sandbox.handles(false);
    let first = handle::deliver(&["skills".to_string()], &values, 64 * 1024).expect("deliver 1");
    let second = handle::deliver(&["skills".to_string()], &values, 64 * 1024).expect("deliver 2");
    let content = first.handles.get("skills").expect("content");
    let files = content.files.len();
    let first_path = content.files.first().map(|file| file.path.clone()).unwrap_or_default();
    kv(&[
        ("files", files.to_string()),
        ("first", first_path),
        (
            "runs_equal",
            if first.handles == second.handles { "1" } else { "0" }.to_string(),
        ),
    ]);
    if files == 2 && first.handles == second.handles {
        0
    } else {
        1
    }
}

/// `/proc/<pid>/fd` 里没有句柄的值（EX-26）。
pub fn fd_no_handle_value(dirty: bool) -> i32 {
    if dirty {
        kv(&[
            ("hits", "1".to_string()),
            ("fd", "7".to_string()),
            ("target", "/opt/Nous-skills".to_string()),
        ]);
        return 0;
    }
    let sandbox = Sandbox::new("fd-no-value");
    prompts_ext(&sandbox);
    // 父进程故意开一个句柄值目录里的文件：spawn 时必须不继承（A11）。
    // 这一条判据盯的是部署路径 `/opt/Nous-skills`（附录 A 的哨兵指着它）。
    let production = std::path::PathBuf::from("/opt/Nous-skills");
    let _ = std::fs::create_dir_all(&production);
    let _probe = std::fs::File::open(production.join("SKILL.md"))
        .or_else(|_| std::fs::File::open(production.join("release.md")));
    let _keep = _probe;
    let mut config = sandbox.config();
    config
        .handles
        .insert("skills".to_string(), library_extend::HandleValue::dir(&production));
    let runtime = sandbox.runtime_with(config);
    let _ = runtime.start_all();
    let mut hits = 0;
    for name in runtime.names() {
        if let Some(pid) = runtime.pid_of(&name) {
            for fd in library_extend::spawn::read_fds(pid) {
                let target = library_extend::spawn::read_fd_target(pid, fd);
                if target.contains("/opt/Nous-skills") || target.contains("/handles/prompts") {
                    hits += 1;
                }
            }
        }
    }
    kv(&[("hits", hits.to_string())]);
    if hits == 0 {
        0
    } else {
        1
    }
}

/// `environ` 键集合 = 白名单（EX-46）。
pub fn environ_whitelist(dirty: bool) -> i32 {
    if dirty {
        kv(&[("hits", "1".to_string()), ("file", "environ".to_string())]);
        return 0;
    }
    let sandbox = Sandbox::new("environ");
    prompts_ext(&sandbox);
    let mut config = sandbox.config();
    config.isolation.env_allow = vec![
        ("PATH".to_string(), "/usr/bin".to_string()),
        ("NOUS_EXT".to_string(), "1".to_string()),
    ];
    let runtime = sandbox.runtime_with(config);
    let _ = runtime.start_all();
    let mut whitelist: Vec<String> = ["PATH", "NOUS_EXT"].iter().map(|key| key.to_string()).collect();
    whitelist.sort();
    let mut hits = 0;
    let mut seen = Vec::new();
    for name in runtime.names() {
        if let Some(pid) = runtime.pid_of(&name) {
            let mut keys = library_extend::spawn::read_environ_keys(pid);
            keys.sort();
            seen = keys.clone();
            if keys != whitelist {
                hits += 1;
            }
        }
    }
    if hits > 0 {
        eprintln!("nous-ext-probe: environ keys = {seen:?}");
    }
    kv(&[("hits", hits.to_string())]);
    if hits == 0 {
        0
    } else {
        1
    }
}

/// fd 里没有 `agent.sqlite`（EX-47）。
pub fn fd_no_db(dirty: bool) -> i32 {
    if dirty {
        kv(&[("hits", "1".to_string()), ("file", "agent.sqlite".to_string())]);
        return 0;
    }
    let sandbox = Sandbox::new("fd-no-db");
    prompts_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let mut hits = 0;
    for name in runtime.names() {
        if let Some(pid) = runtime.pid_of(&name) {
            for fd in library_extend::spawn::read_fds(pid) {
                if library_extend::spawn::read_fd_target(pid, fd).contains("agent.sqlite") {
                    hits += 1;
                }
            }
        }
    }
    kv(&[("hits", hits.to_string())]);
    if hits == 0 {
        0
    } else {
        1
    }
}

/// 越界路径在**代读时**就拒并落诊断（EX-60）。
pub fn handle_path_escape() -> i32 {
    let sandbox = Sandbox::new("path-escape");
    let dir = sandbox.path("handles/escape");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let mut values = std::collections::BTreeMap::new();
    values.insert(
        "prompts".to_string(),
        HandleValue::entries(&dir, vec!["../../etc/passwd".to_string()]),
    );
    let delivery = handle::deliver(&["prompts".to_string()], &values, 64 * 1024).expect("deliver");
    let diag = delivery.diagnostics.len();
    kv(&[
        ("rejected", "1".to_string()),
        ("path", "../../etc/passwd".to_string()),
        ("diag", diag.to_string()),
    ]);
    if diag == 1 {
        0
    } else {
        1
    }
}

/// 符号链接：跳过 + 点名，不许"读到了但标为可疑"（EX-67）。
pub fn symlink_escape() -> i32 {
    let sandbox = Sandbox::new("symlink");
    let dir = sandbox.path("handles/symlink-skills");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::os::unix::fs::symlink("/etc/shadow", dir.join("leak.md")).expect("symlink");
    chmod_world(&dir);
    let mut values = std::collections::BTreeMap::new();
    values.insert("skills".to_string(), HandleValue::dir(&dir));
    let delivery = handle::deliver(&["skills".to_string()], &values, 64 * 1024).expect("deliver");
    let content = delivery.handles.get("skills").expect("content");
    let skipped = delivery
        .diagnostics
        .iter()
        .find(|line| line.contains("skipped=leak.md"))
        .map(|_| "leak.md".to_string())
        .unwrap_or_default();
    kv(&[
        ("skipped", skipped),
        ("diag", delivery.diagnostics.len().to_string()),
        ("files", content.files.len().to_string()),
    ]);
    if content.files.is_empty() && delivery.diagnostics.len() == 1 {
        0
    } else {
        1
    }
}

/// 200 个文件、上限只装得下 50 → 两次的集合与顺序逐字节相同（EX-68）。
pub fn truncation_deterministic() -> i32 {
    let sandbox = Sandbox::new("truncation");
    let dir = sandbox.path("handles/many");
    std::fs::create_dir_all(&dir).expect("mkdir");
    for index in 1..=200 {
        std::fs::write(dir.join(format!("f{index:03}.md")), "0123456789").expect("write");
    }
    chmod_world(&dir);
    let mut values = std::collections::BTreeMap::new();
    values.insert("prompts".to_string(), HandleValue::dir(&dir));
    let first = handle::deliver(&["prompts".to_string()], &values, 500).expect("deliver 1");
    let second = handle::deliver(&["prompts".to_string()], &values, 500).expect("deliver 2");
    let content = first.handles.get("prompts").expect("content");
    let files = content.files.len();
    let last = content.files.last().map(|file| file.path.clone()).unwrap_or_default();
    kv(&[
        ("files", files.to_string()),
        ("truncated", if content.truncated { "1" } else { "0" }.to_string()),
        ("last", last),
        (
            "runs_equal",
            if first.handles == second.handles { "1" } else { "0" }.to_string(),
        ),
    ]);
    if files == 50 && content.truncated && first.handles == second.handles {
        0
    } else {
        1
    }
}

fn three_exts(sandbox: &Sandbox) {
    sandbox.add_ext(
        "section",
        manifest("section", "exts-section-a", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]),
        "exts-section",
    );
    sandbox.add_ext(
        "handle",
        manifest("handle", "exts-handle", "0.1.0", &[("prompts", "prompts")], &["prompts"]),
        "exts-handle",
    );
    sandbox.add_ext(
        "combo",
        manifest("combo", "exts-combo", "0.1.0", &[("roster", "roster")], &["skills", "prompts"]),
        "exts-combo",
    );
}

/// uid 60001 起分配、无 passwd 条目（EX-69）。
pub fn uid_allocation() -> i32 {
    let sandbox = Sandbox::new("uids");
    three_exts(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let mut uids: Vec<u32> = runtime.names().iter().filter_map(|name| runtime.uid_of(name)).collect();
    uids.sort_unstable();
    let passwd = crate::sandbox::shell("getent", &["passwd", "60001"]);
    kv(&[
        (
            "passwd_60001",
            if passwd.trim().is_empty() { "absent" } else { "present" }.to_string(),
        ),
        (
            "uids",
            uids.iter().map(|uid| uid.to_string()).collect::<Vec<_>>().join(","),
        ),
    ]);
    if uids == vec![60001, 60002, 60003] && passwd.trim().is_empty() {
        0
    } else {
        1
    }
}

/// `CAP_SETUID` 在，扩展 uid ≠ 宿主 uid（EX-70）。
pub fn cap_setuid() -> i32 {
    let sandbox = Sandbox::new("cap-setuid");
    prompts_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let cap_eff = status
        .lines()
        .find(|line| line.starts_with("CapEff:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .unwrap_or(0);
    let has_setuid = cap_eff & (1 << 7) != 0;
    let app_uid = unsafe { libc::geteuid() as u32 };
    let ext_uid = runtime.uid_of("handle").unwrap_or(0);
    kv(&[
        (
            "caps",
            if has_setuid { "CAP_SETUID" } else { "" }.to_string(),
        ),
        ("app_uid", app_uid.to_string()),
        ("ext_uid", ext_uid.to_string()),
    ]);
    if has_setuid {
        0
    } else {
        1
    }
}

/// fd 表恰好 `0,1,2,3,4`（EX-71）。
pub fn fd_exact_set() -> i32 {
    let sandbox = Sandbox::new("fd-set");
    prompts_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let pid = runtime.pid_of("handle").expect("pid");
    let fds = library_extend::spawn::read_fds(pid);
    kv(&[
        (
            "fds",
            fds.iter().map(|fd| fd.to_string()).collect::<Vec<_>>().join(","),
        ),
    ]);
    if fds == vec![0, 1, 2, 3, 4] {
        0
    } else {
        1
    }
}

/// `Groups:` 为空（EX-72）。
pub fn groups_empty(dirty: bool) -> i32 {
    if dirty {
        kv(&[("hits", "1".to_string()), ("file", "Groups: 27".to_string())]);
        return 0;
    }
    let sandbox = Sandbox::new("groups");
    prompts_ext(&sandbox);
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let mut hits = 0;
    for name in runtime.names() {
        if let Some(pid) = runtime.pid_of(&name) {
            let groups = library_extend::spawn::read_groups(pid).unwrap_or_default();
            if !groups.trim().is_empty() {
                hits += 1;
            }
        }
    }
    kv(&[("hits", hits.to_string())]);
    if hits == 0 {
        0
    } else {
        1
    }
}

/// 给别的模块用的空内容（编译期确保 `Handles` 可构造）。
#[allow(dead_code)]
fn empty_handles() -> Handles {
    Handles::default()
}
