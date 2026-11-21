//! 场景沙盒：真目录、真二进制、真库、真 uid。
//!
//! judge 的**期望值写死在附录 A**，这里只负责"执行场景 + 打印观测"（spec 读法第 7 条）：
//! 任何一行 `key=value` 都必须来自真实系统（文件 / 进程 / `/proc` / 库），不许在这里
//! `echo` 一个写死的期望值。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nous_ext::audit::Provenance;
use nous_ext::manifest::ArtifactMeta;
use nous_ext::{HandleValue, Isolation, Limits, Runtime, RuntimeConfig};
use nous_ext_sdk::manifest::Manifest;
use serde_json::{json, Value};

/// 一个场景的工作目录。
pub struct Sandbox {
    pub root: PathBuf,
    pub exts: PathBuf,
    pub artifacts: PathBuf,
    pub skills: PathBuf,
    pub prompts: PathBuf,
    pub probe: PathBuf,
    pub data: PathBuf,
    pub db: PathBuf,
    bin_dir: PathBuf,
}

impl Sandbox {
    /// 建一个沙盒。`skills` 用真实部署路径 `/opt/Nous-skills`（EX-26 的哨兵指着它）；
    /// 其余句柄住在沙盒里。
    pub fn new(case: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "nous-ext-probe-{case}-{}-{}",
            std::process::id(),
            nous_ext::runtime::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let exts = root.join("exts");
        let artifacts = root.join("artifacts");
        let prompts = root.join("handles/prompts");
        let probe = root.join("handles/probe");
        let data = root.join("data");
        for dir in [&exts, &artifacts, &prompts, &probe, &data] {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        // 句柄值住在沙盒里：每次跑都是干净的两份（判据之间不许互相污染）。
        // 真部署里 skills 指向 `/opt/Nous-skills`；EX-26 那一条单独指过去。
        let skills = root.join("handles/skills");
        std::fs::create_dir_all(&skills).expect("mkdir skills");
        std::fs::write(
            skills.join("SKILL.md"),
            "---\nname: tdd\ndescription: test-driven development\n---\n# tdd\n",
        )
        .expect("write SKILL.md");
        std::fs::write(
            skills.join("release.md"),
            "---\nname: release\ndescription: release checklist\n---\n# release\n",
        )
        .expect("write release.md");
        // 句柄值目录的权限：扩展以 60001 跑，必须读得到。
        chmod_world(&skills);
        let bin_dir = std::env::current_exe()
            .expect("current exe")
            .parent()
            .expect("parent")
            .to_path_buf();
        let mut sandbox = Self {
            root,
            exts,
            artifacts,
            skills,
            prompts,
            probe,
            data,
            db: PathBuf::new(),
            bin_dir,
        };
        sandbox.db = sandbox.root.join("nous.db");
        sandbox.chmod_all();
        sandbox
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn chmod_all(&mut self) {
        chmod_world(&self.root);
        self.chmod_public();
    }

    fn chmod_public(&self) {
        chmod_world(&self.exts);
        chmod_world(&self.artifacts);
        chmod_world(&self.root.join("handles"));
        let _ = std::fs::set_permissions(
            &self.root,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        );
    }

    /// 装一个样例扩展：写清单、把二进制复制进产物目录、写真实哈希的元数据。
    pub fn add_ext(&self, dirname: &str, manifest: Value, binary: &str) -> Manifest {
        let dir = self.exts.join(dirname);
        std::fs::create_dir_all(&dir).expect("mkdir ext");
        std::fs::write(
            dir.join("nous.json"),
            serde_json::to_string_pretty(&manifest).expect("manifest json"),
        )
        .expect("write manifest");
        let manifest_typed: Manifest =
            serde_json::from_value(manifest).expect("manifest shape");
        let source = self.bin_dir.join(binary);
        let target = self.artifacts.join(&manifest_typed.file);
        std::fs::copy(&source, &target).expect("copy artifact");
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
        let bytes = std::fs::read(&target).expect("read artifact");
        let meta = ArtifactMeta {
            hash: nous_ext::sha256::hex(&bytes),
            built_from: git_short(),
            build_machine: hostname(),
            protocol: 3,
            triple: "x86_64-unknown-linux-gnu".to_string(),
        };
        self.write_meta(&manifest_typed.file, &meta);
        chmod_world(&dir);
        chmod_dirs(&self.artifacts);
        manifest_typed
    }

    pub fn write_meta(&self, file: &str, meta: &ArtifactMeta) {
        let value = json!({
            "hash": meta.hash,
            "builtFrom": meta.built_from,
            "buildMachine": meta.build_machine,
            "protocol": meta.protocol,
            "triple": meta.triple,
        });
        std::fs::write(
            self.artifacts.join(format!("{file}.json")),
            serde_json::to_string_pretty(&value).expect("meta json"),
        )
        .expect("write meta");
    }

    /// 覆盖一个产物的元数据（不改二进制）——用于 triple / 来源场景。
    pub fn set_triple(&self, file: &str, triple: &str) {
        let path = self.artifacts.join(format!("{file}.json"));
        let mut value: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read meta")).expect("meta");
        value["triple"] = json!(triple);
        std::fs::write(&path, serde_json::to_string_pretty(&value).expect("meta")).expect("write meta");
    }

    /// 改一个字节（EX-48）。
    pub fn flip_byte(&self, file: &str) {
        let path = self.artifacts.join(file);
        let mut bytes = std::fs::read(&path).expect("read artifact");
        if let Some(byte) = bytes.last_mut() {
            *byte ^= 0xff;
        }
        std::fs::write(&path, &bytes).expect("write artifact");
    }

    /// 写 `probe` 句柄的内容（样例的场景开关）。
    pub fn set_mode(&self, mode: &str) {
        std::fs::write(self.probe.join("mode"), mode).expect("write mode");
        chmod_world(&self.probe);
    }

    pub fn write_prompt(&self, name: &str, text: &str) {
        std::fs::write(self.prompts.join(name), text).expect("write prompt");
        chmod_world(&self.prompts);
    }

    pub fn write_skill(&self, name: &str, text: &str) {
        std::fs::write(self.skills.join(name), text).expect("write skill");
        chmod_world(&self.skills);
    }

    pub fn handles(&self, missing_skills: bool) -> BTreeMap<String, HandleValue> {
        let mut handles = BTreeMap::new();
        if !missing_skills {
            handles.insert("skills".to_string(), HandleValue::dir(&self.skills));
        }
        handles.insert("prompts".to_string(), HandleValue::dir(&self.prompts));
        handles.insert("probe".to_string(), HandleValue::dir(&self.probe));
        handles
    }

    pub fn config(&self) -> RuntimeConfig {
        self.config_with(true, false)
    }

    pub fn config_with(&self, skills: bool, missing_skills: bool) -> RuntimeConfig {
        let mut config = RuntimeConfig::new(&self.exts, &self.artifacts);
        config.handles = self.handles(missing_skills);
        if !skills {
            config.handles.remove("skills");
        }
        config.data_dir = self.data.clone();
        config.db = Some(self.db.clone());
        config.limits = Limits {
            // 判据要跑得快：section 预算 / 握手都按 spec 默认，重载窗口小一点。
            ..Limits::default()
        };
        let euid = unsafe { libc::geteuid() };
        config.isolation = Isolation {
            enabled: euid == 0,
            close_fds: std::env::var("NOUS_NO_CLOSE_FDS").is_err(),
            ..Isolation::default()
        };
        config
    }

    pub fn runtime(&self) -> std::sync::Arc<Runtime> {
        self.runtime_with(self.config())
    }

    /// 把 `skills` 句柄指到别的目录（名牌场景要可控的文件数）。
    pub fn config_skills(&self, dir: &Path) -> RuntimeConfig {
        let mut config = self.config();
        config.handles.insert(
            "skills".to_string(),
            HandleValue::dir(dir),
        );
        config
    }

    pub fn runtime_with(&self, config: RuntimeConfig) -> std::sync::Arc<Runtime> {
        Runtime::new_with_push(config, None)
    }

    pub fn provenance(&self, file: &str) -> Option<Provenance> {
        let path = self.artifacts.join(format!("{file}.json"));
        let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        Some(Provenance {
            hash: value.get("hash")?.as_str()?.to_string(),
            built_from: value.get("builtFrom")?.as_str()?.to_string(),
            build_machine: value.get("buildMachine")?.as_str()?.to_string(),
        })
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if std::env::var("NOUS_PROBE_KEEP").is_err() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

/// 递归给 755/644。
pub fn chmod_world(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
            chmod_world(&entry.path());
        }
    } else {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
    }
}

/// 只改目录权限（755），不动文件的执行位：产物是从 `target/debug/<bin>` 复制来的，
/// 复制会保留 755；把它一起改成 644 会让 60001 连 exec 都做不了。
pub fn chmod_dirs(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
        chmod_dirs(&entry.path());
    }
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

/// 仓库根（`crates/app/nous-ext-probe` 往上三层）。
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn git_short() -> String {
    std::process::Command::new(git_program())
        .args(["-C", repo_root().to_str().unwrap_or("."), "rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "0000000".to_string())
}

/// 在仓库上跑一条命令，返回 stdout（EX-04 / EX-37 / EX-54 这类"扫仓库"的判据）。
pub fn shell(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .current_dir(repo_root())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .unwrap_or_default()
}

fn git_program() -> String { String::from("git") }
