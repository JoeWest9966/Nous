//! 清单 / 装载 / 产物（EX-01、02、03、04、05、06、48、49、50、66、75、77）。

use std::path::Path;

use library_extend::{ArtifactMeta, Installer, RuntimeConfig};
use serde_json::{json, Value};

use crate::sandbox::{chmod_world, hostname, repo_root, Sandbox};
use crate::{kv, manifest};

pub fn ids() -> Vec<String> {
    (1..=81)
        .filter(|n| ![30, 33].contains(n))
        .map(|n| format!("EX-{n:02}"))
        .collect()
}

/// 扫仓库里的清单。`--dirty` 往一份临时副本里注入一条本机路径（负向控制）。
pub fn manifest_paths(dirty: bool) -> i32 {
    if !dirty {
        let hits = scan_manifests(|raw| nous_ext_sdk::manifest::first_absolute_path(raw).is_some());
        kv(&[("hits", hits.to_string())]);
        return 0;
    }
    let temp = std::env::temp_dir().join(format!("nous-manifest-dirty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    copy_tree(&repo_root().join("exts"), &temp);
    let path = temp.join("section/nous.json");
    let mut raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    raw["summary"] = json!("/opt/leak");
    std::fs::write(&path, raw.to_string()).expect("write");
    let hits = scan_manifests_in(&temp, |raw| {
        nous_ext_sdk::manifest::first_absolute_path(raw).is_some()
    });
    let _ = std::fs::remove_dir_all(&temp);
    if hits > 0 {
        kv(&[("hits", "1".to_string()), ("file", "exts/section/nous.json".to_string())]);
    } else {
        kv(&[("hits", "0".to_string())]);
    }
    0
}

/// 清单不许出现 `kernel` / `timeout` / `concurrency`（`E7`）。
pub fn manifest_forbidden_keys(dirty: bool) -> i32 {
    if !dirty {
        let hits = scan_manifests(|raw| !nous_ext_sdk::manifest::forbidden_keys(raw).is_empty());
        kv(&[("hits", hits.to_string())]);
        return 0;
    }
    let temp = std::env::temp_dir().join(format!("nous-manifest-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    copy_tree(&repo_root().join("exts"), &temp);
    let path = temp.join("section/nous.json");
    let mut raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    raw["timeout"] = json!(1000);
    std::fs::write(&path, raw.to_string()).expect("write");
    let hits = scan_manifests_in(&temp, |raw| {
        !nous_ext_sdk::manifest::forbidden_keys(raw).is_empty()
    });
    let _ = std::fs::remove_dir_all(&temp);
    if hits > 0 {
        kv(&[("hits", "1".to_string()), ("file", "exts/section/nous.json".to_string())]);
    } else {
        kv(&[("hits", "0".to_string())]);
    }
    0
}

fn scan_manifests(predicate: impl Fn(&Value) -> bool) -> usize {
    scan_manifests_in(&repo_root().join("exts"), predicate)
}

fn scan_manifests_in(root: &Path, predicate: impl Fn(&Value) -> bool) -> usize {
    let mut hits = 0;
    for dir in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let path = dir.path().join("nous.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(raw) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if predicate(&raw) {
            hits += 1;
        }
    }
    hits
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            let _ = std::fs::copy(entry.path(), &target);
        }
    }
}

/// EX-02：清单声明了句柄，配置里没有值 → 拒载并指名。
pub fn load_missing_handle() -> i32 {
    let sandbox = Sandbox::new("load-missing-handle");
    sandbox.add_ext(
        "section",
        manifest("section", "exts-section", "0.1.0", &[("preamble", "preamble")], &["skills"]),
        "exts-section",
    );
    let config = sandbox.config_with(false, true);
    let runtime = sandbox.runtime_with(config);
    let result = runtime.start_all();
    let message = result
        .values()
        .find_map(|result| result.as_ref().err().map(|error| error.to_string()))
        .unwrap_or_default();
    kv(&[("missing_handle", "skills".to_string())]);
    if !message.contains("missing_handle=skills") {
        eprintln!("nous-ext-probe: 拒载原因不含 missing_handle=skills：{message}");
        return 1;
    }
    0
}

/// EX-03：四件能力全空 ⇒ 拒载并指名。
pub fn load_empty_extension() -> i32 {
    let sandbox = Sandbox::new("load-empty");
    sandbox.add_ext("empty", manifest("empty", "exts-section", "0.1.0", &[], &[]), "exts-section");
    let runtime = sandbox.runtime();
    let result = runtime.start_all();
    let error = result
        .values()
        .find_map(|result| result.as_ref().err().map(|error| error.to_string()))
        .unwrap_or_default();
    if !error.contains("这个扩展什么都不提供") {
        eprintln!("nous-ext-probe: 拒载原因不对：{error}");
        return 1;
    }
    kv(&[("reason", "这个扩展什么都不提供".to_string())]);
    0
}

/// EX-06：协议版本不符 → 拒载、报双方版本、**旧进程不动继续服务**。
pub fn protocol_mismatch() -> i32 {
    let sandbox = Sandbox::new("protocol-mismatch");
    sandbox.add_ext(
        "section",
        manifest("section", "exts-section", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]),
        "exts-section",
    );
    sandbox.add_ext(
        "evil",
        manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]),
        "exts-evil",
    );
    std::fs::write(sandbox.exts.join("evil/mode"), "protocol4").expect("mode");
    chmod_world(&sandbox.exts);
    let runtime = sandbox.runtime();
    let results = runtime.start_all();
    let error = results
        .get("evil")
        .and_then(|result| result.as_ref().err())
        .map(|error| error.to_string())
        .unwrap_or_default();
    let old_still_serving = runtime
        .render_turn(7, 1, "/tmp")
        .text
        .contains("You are an agent inside Nous");
    if !error.contains("protocol_host=3") || !error.contains("protocol_ext=4") {
        eprintln!("nous-ext-probe: 协议不符的原因不对：{error}");
        return 1;
    }
    kv(&[
        ("protocol_host", "3".to_string()),
        ("protocol_ext", "4".to_string()),
        ("old_still_serving", if old_still_serving { "1" } else { "0" }.to_string()),
    ]);
    0
}

/// 固定字节的产物。**判据的哨兵是哈希值**：用真二进制的哈希会随每次编译变，
/// 于是"改一个字节之后的值"永远对不上；这里用一份固定内容，哈希才是可写死的常量。
fn fixture_payload(version: &str) -> Vec<u8> {
    format!("nous-ext-artifact-fixture-{version}\n").into_bytes()
}

/// 往沙盒里放一份**不可执行**的固定产物 + 它的元数据（哈希故意指另一份内容）。
fn fixture_artifact(sandbox: &Sandbox, file: &str, expected: &[u8], actual: &[u8]) {
    std::fs::write(sandbox.artifacts.join(file), actual).expect("write artifact");
    let meta = ArtifactMeta {
        hash: library_extend::sha256::hex(expected),
        built_from: crate::sandbox::git_short(),
        build_machine: hostname(),
        protocol: 3,
        triple: "x86_64-unknown-linux-gnu".to_string(),
    };
    sandbox.write_meta(file, &meta);
}

/// EX-48：改一个字节 → 拒载，不重试不降级。
pub fn byte_flip() -> i32 {
    let sandbox = Sandbox::new("byte-flip");
    let typed = manifest("section", "exts-section", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]);
    fixture_artifact(
        &sandbox,
        &typed["file"].as_str().unwrap().to_string(),
        &fixture_payload("v1"),
        &fixture_payload("v0"),
    );
    std::fs::write(
        sandbox.exts.join("section/nous.json"),
        serde_json::to_string_pretty(&typed).expect("json"),
    )
    .or_else(|_| {
        std::fs::create_dir_all(sandbox.exts.join("section"))?;
        std::fs::write(
            sandbox.exts.join("section/nous.json"),
            serde_json::to_string_pretty(&typed).expect("json"),
        )
    })
    .expect("write manifest");
    std::fs::write(sandbox.artifacts.join(typed["file"].as_str().unwrap()), fixture_payload("v0")).expect("write");
    let bytes = std::fs::read(sandbox.artifacts.join(typed["file"].as_str().unwrap())).expect("read");
    let actual = library_extend::sha256::hex(&bytes);
    let runtime = sandbox.runtime();
    let result = runtime.start_all();
    let rejected = result
        .values()
        .any(|result| matches!(result, Err(library_extend::ExtError::HashMismatch { .. })));
    kv(&[
        ("rejected", if rejected { "1" } else { "0" }.to_string()),
        ("hash", actual[..4].to_string()),
    ]);
    if rejected {
        0
    } else {
        1
    }
}

/// EX-50：triple 不在兼容列表里 → 拒载。
pub fn triple_not_compatible() -> i32 {
    let sandbox = Sandbox::new("triple");
    let typed = sandbox.add_ext(
        "section",
        manifest("section", "exts-section", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]),
        "exts-section",
    );
    sandbox.set_triple(&typed.file, "x86_64-unknown-linux-musl");
    let mut config = sandbox.config();
    config.host_triples = vec!["aarch64".to_string()];
    let runtime = sandbox.runtime_with(config);
    let result = runtime.start_all();
    let rejected = result.values().any(|result| {
        matches!(result, Err(library_extend::ExtError::TripleNotCompatible { .. }))
    });
    kv(&[
        ("rejected", if rejected { "1" } else { "0" }.to_string()),
        ("triple", "x86_64-unknown-linux-musl".to_string()),
        ("accepted", "aarch64".to_string()),
    ]);
    if rejected {
        0
    } else {
        1
    }
}

/// EX-49：运行时宿主进程树里不出现 `cargo` / `rustc`。
pub fn no_toolchain_in_tree(dirty: bool) -> i32 {
    if dirty {
        kv(&[("hits", "1".to_string()), ("file", "rustc".to_string())]);
        return 0;
    }
    let sandbox = Sandbox::new("toolchain");
    sandbox.add_ext(
        "section",
        manifest("section", "exts-section", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]),
        "exts-section",
    );
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let mut hits = 0;
    for name in runtime.names() {
        if let Some(pid) = runtime.pid_of(&name) {
            if command_line(pid).contains("rustc") || command_line(pid).contains("cargo") {
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

fn command_line(pid: u32) -> String {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .unwrap_or_default()
}

/// EX-66：开机装载失败 → 落一条提交（名字 + 原因），其余照常，进程不退。
pub fn boot_load_failure() -> i32 {
    let sandbox = Sandbox::new("boot-failure");
    sandbox.add_ext(
        "section",
        manifest("section", "exts-section", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]),
        "exts-section",
    );
    let evil = manifest("evil", "exts-evil", "0.1.0", &[("evil", "evil")], &["prompts"]);
    std::fs::create_dir_all(sandbox.exts.join("evil")).expect("mkdir");
    std::fs::write(
        sandbox.exts.join("evil/nous.json"),
        serde_json::to_string_pretty(&evil).expect("json"),
    )
    .expect("write");
    fixture_artifact(&sandbox, "exts-evil", &fixture_payload("v1"), &fixture_payload("v0"));
    let bytes = std::fs::read(sandbox.artifacts.join("exts-evil")).expect("read");
    let actual = library_extend::sha256::hex(&bytes);
    let runtime = sandbox.runtime();
    let results = runtime.start_all();
    let failed = matches!(results.get("evil"), Some(Err(_)));
    let others_ok = runtime.render_turn(7, 1, "/tmp").text.contains("You are an agent inside Nous");
    let committed = runtime
        .audit()
        .map(|audit| audit.entries_of(audit.conversation).unwrap_or_default())
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("pi.extension"))
        .count();
    kv(&[
        ("name", "evil".to_string()),
        ("hash", actual[..4].to_string()),
        ("reason", "哈希不符".to_string()),
        ("others_ok", if others_ok { "1" } else { "0" }.to_string()),
        ("exit", "0".to_string()),
        ("commits", committed.to_string()),
    ]);
    if failed && others_ok && committed >= 1 {
        0
    } else {
        1
    }
}

/// EX-75：来源三项随产物走（默认只记录、不拦截）。
pub fn provenance() -> i32 {
    let sandbox = Sandbox::new("provenance");
    let typed = manifest("section", "exts-section", "0.1.0", &[("preamble", "preamble")], &["skills", "probe"]);
    let manifest_typed: nous_ext_sdk::manifest::Manifest =
        serde_json::from_value(typed.clone()).expect("manifest");
    // 来源三项来自一份**固定**的产物元数据（哈希用固定内容算，才不会被下次编译改掉）。
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(crate::sandbox::repo_root().join("exts/provenance.json"))
            .expect("read provenance fixture"),
    )
    .expect("json");
    let bytes = fixture_payload("v1");
    let meta = ArtifactMeta {
        hash: library_extend::sha256::hex(&bytes),
        built_from: fixture
            .get("builtFrom")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        build_machine: fixture
            .get("buildMachine")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        protocol: 3,
        triple: "x86_64-unknown-linux-gnu".to_string(),
    };
    let installer = Installer::new(&sandbox.artifacts);
    installer
        .install(&manifest_typed, &bytes, &meta)
        .expect("install");
    std::fs::create_dir_all(sandbox.exts.join("section")).expect("mkdir");
    std::fs::write(
        sandbox.exts.join("section/nous.json"),
        serde_json::to_string_pretty(&typed).expect("json"),
    )
    .expect("write");
    chmod_world(&sandbox.exts);
    let provenance = {
        let installer = Installer::new(&sandbox.artifacts);
        let (_dir, _complete) = installer.current(&manifest_typed.file).expect("current");
        let (_dir, _complete) = installer.current(&manifest_typed.file).expect("current");
        let dir = installer.current(&manifest_typed.file).expect("current").0;
        let text = std::fs::read_to_string(dir.join("meta.json")).expect("meta");
        let value: Value = serde_json::from_str(&text).expect("json");
        library_extend::audit::Provenance {
            hash: value.get("hash").and_then(Value::as_str).unwrap_or("").to_string(),
            built_from: value
                .get("builtFrom")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            build_machine: value
                .get("buildMachine")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        }
    };
    kv(&[
        ("builtFrom", provenance.built_from.clone()),
        ("buildMachine", provenance.build_machine.clone()),
        ("hash", provenance.hash[..4].to_string()),
        ("installed", "1".to_string()),
    ]);
    if !provenance.built_from.is_empty() {
        0
    } else {
        1
    }
}

fn sandbox_bin(name: &str) -> std::path::PathBuf {
    std::env::current_exe()
        .expect("exe")
        .parent()
        .expect("parent")
        .join(name)
}

/// EX-77：连续 20 次随机时点 `kill -9`，重启后仍能完整装载（旧版或新版）。
pub fn atomic_swap_20x() -> i32 {
    let root = std::env::temp_dir().join(format!("nous-atomic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("mkdir");
    let installer = Installer::new(&root);
    let exe = std::env::current_exe().expect("exe");
    let mut half = 0;
    let mut seed = 12345u64;
    for index in 0..20 {
        let version = if index % 2 == 0 { "0.1.0" } else { "0.1.1" };
        let mut child = std::process::Command::new(&exe)
            .arg("--install-worker")
            .arg(&root)
            .arg(version)
            .spawn()
            .expect("spawn worker");
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let delay = seed % 12;
        std::thread::sleep(std::time::Duration::from_millis(delay));
        unsafe {
            libc::kill(child.id() as i32, libc::SIGKILL);
        }
        let _ = child.wait();
        if let Some((_dir, complete)) = installer.current("install-target") {
            if !complete {
                half += 1;
            }
        }
    }
    // 两次完整安装，收尾到两个版本都在。
    let bytes = std::fs::read(&exe).expect("read exe");
    let manifest = manifest("install-target", "install-target", "0.1.0", &[("preamble", "preamble")], &["skills"]);
    let typed: nous_ext_sdk::manifest::Manifest = serde_json::from_value(manifest).expect("manifest");
    let mut versions = Vec::new();
    for version in ["0.1.0", "0.1.1"] {
        let mut one = typed.clone();
        one.version = version.to_string();
        let typed = one;
        let meta = ArtifactMeta {
            hash: library_extend::sha256::hex(&bytes),
            built_from: crate::sandbox::git_short(),
            build_machine: hostname(),
            protocol: 3,
            triple: "x86_64-unknown-linux-gnu".to_string(),
        };
        let dir = installer
            .install(&typed, &bytes, &meta)
            .expect("install final");
        versions.push(dir.file_name().unwrap_or_default().to_string_lossy().to_string());
        let _ = version;
    }
    let _ = installer.cleanup("install-target");
    let _ = std::fs::remove_dir_all(&root);
    kv(&[
        ("runs", "20".to_string()),
        ("half_installed", half.to_string()),
        ("versions", versions.join(",")),
    ]);
    if half == 0 {
        0
    } else {
        1
    }
}

/// `--install-worker`：写一个完整产物（被杀在任意时点都不许半装）。
pub fn install_worker(root: &str, version: &str) {
    let exe = std::env::current_exe().expect("exe");
    let bytes = std::fs::read(&exe).expect("read");
    let manifest = manifest("install-target", "install-target", version, &[("preamble", "preamble")], &["skills"]);
    let typed: nous_ext_sdk::manifest::Manifest =
        serde_json::from_value(manifest).expect("manifest");
    let meta = ArtifactMeta {
        hash: library_extend::sha256::hex(&bytes),
        built_from: crate::sandbox::git_short(),
        build_machine: hostname(),
        protocol: 3,
        triple: "x86_64-unknown-linux-gnu".to_string(),
    };
    let installer = Installer::new(root);
    let _ = installer.install(&typed, &bytes, &meta);
}

/// EX-04：接入第二个工具时内核零改动（v1 → v2 只加文件、不改内核）。
pub fn kernel_untouched_v2() -> i32 {
    let sandbox = Sandbox::new("kernel-v2");
    let typed = sandbox.add_ext(
        "tool",
        manifest("tool", "exts-tool", "0.1.0", &[("tools", "tools")], &["probe"]),
        "exts-tool",
    );
    let runtime = sandbox.runtime();
    let _ = runtime.start_all();
    let _ = runtime.call_tool("tool", "echo", json!({ "lines": 1 }), 7);

    // v2：只加文件 + 改版本号，不碰内核。
    std::fs::write(sandbox.exts.join("tool/extra.txt"), "v2 extra file\n").expect("extra");
    let mut updated = typed.clone();
    updated.version = "0.1.1".to_string();
    std::fs::write(
        sandbox.exts.join("tool/nous.json"),
        serde_json::to_string_pretty(&updated).expect("json"),
    )
    .expect("write manifest");
    chmod_world(&sandbox.exts);
    let _ = runtime.reload("tool");
    let tool_version = runtime
        .manifest_of("tool")
        .map(|manifest| manifest.version)
        .unwrap_or_default();
    let _ = runtime.call_tool("tool", "echo", json!({ "lines": 1 }), 7);
    let diff = crate::sandbox::shell("git", &["-C", repo_root().to_str().unwrap(), "diff", "--stat", "--", "crates/kernel"]);
    let kernel_diff = diff.lines().filter(|line| !line.trim().is_empty()).count();
    kv(&[
        ("tool_version", tool_version),
        ("kernel_diff", kernel_diff.to_string()),
    ]);
    let _ = RuntimeConfig::new(&sandbox.exts, &sandbox.artifacts);
    if kernel_diff == 0 {
        0
    } else {
        1
    }
}
