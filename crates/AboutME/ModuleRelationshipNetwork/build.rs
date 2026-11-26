//! 构建期把「每个插件 `plugin.toml` 的 `[requires]`」抄成一张静态表
//! （`$OUT_DIR/plugin_catalog.rs`，由 `src/lib.rs` `include!` 进来）。
//!
//! 为什么不运行时读盘：端点要给出的是**这个二进制装着什么**，不是「跑起来的机器上此刻有什么」。
//! 编译期生成 = 图与产物同一份，`j1900` 启动时零 IO；也不像 `build.rs` 里跑 `cargo metadata`
//! 那样递归调 cargo（增量构建会锁、会慢）。
//!
//! 为什么不自己算依赖：`[requires]` 由 `scripts/check-plugins.sh` 与 `cargo metadata`
//! **双向逐条比对**（少写 / 多写都红），所以它就是「插件真依赖了谁」的可信抄本 ——
//! 本文件只做机械转录，不做判读。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    for path in [
        "crates/AboutME/ModuleRelationshipNetwork/build.rs",
        "crates/AboutME/ModuleRelationshipNetwork/Cargo.toml",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    emit_plugin_catalog();
}

/// 扫描 `crates/*/*/plugin.toml`，生成：
///
/// - `pub const RELEASE: &str` —— 发行版号（workspace 的 `version`），图的「生成为哪一版」。
/// - `pub const PLUGIN_CATALOG: &[(&str, &str, &[&str])] = &[(模块全名, 包名, &[requires 的包名])]`。
///
/// `cargo:rerun-if-changed` 盯住**每一份** plugin.toml 与两层目录（新增 / 删除插件时目录
/// mtime 会变）：少了这一条，改了 `[requires]` 不重编 = 图上还是旧的边，那是假绿。
fn emit_plugin_catalog() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .ancestors()
        .find(|dir| dir.join("docs/plugins.md").is_file())
        .expect("找不到仓库根（docs/plugins.md 的上级）")
        .to_path_buf();
    println!("cargo:rerun-if-changed={}", root.join("Cargo.toml").display());
    let crates = root.join("crates");

    let mut entries: Vec<(String, String, Vec<String>)> = Vec::new();
    for vendor in read_dirs(&crates) {
        println!("cargo:rerun-if-changed={}", vendor.display());
        for module in read_dirs(&vendor) {
            let manifest = module.join("plugin.toml");
            if !manifest.is_file() {
                continue;
            }
            println!("cargo:rerun-if-changed={}", manifest.display());
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                // 读不出来就不进表：`check-plugins.sh` 会指名这份清单，别在构建日志里演安静。
                println!(
                    "cargo:warning=读不出 {}，该模块不进关系网",
                    manifest.display()
                );
                continue;
            };
            let name = read_field(&text, "name").unwrap_or_else(|| {
                module
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let package = read_field(&text, "crate").unwrap_or_else(|| name.clone());
            entries.push((name, package, read_requires(&text)));
        }
    }
    entries.sort();

    let mut out = String::from(
        "// 由 crates/AboutME/ModuleRelationshipNetwork/build.rs 生成 —— 别手改。\n\
         // 来源：crates/<vendor>/<module>/plugin.toml 的 name / crate / [requires]。\n",
    );
    let release = read_workspace_version(&root.join("Cargo.toml")).unwrap_or_default();
    writeln!(out, "pub const RELEASE: &str = {release:?};\n").unwrap();
    out.push_str("pub const PLUGIN_CATALOG: &[(&str, &str, &[&str])] = &[\n");
    for (name, package, requires) in &entries {
        let list: Vec<String> = requires.iter().map(|dep| format!("{dep:?}")).collect();
        writeln!(
            out,
            "    ({name:?}, {package:?}, &[{}]),",
            list.join(", ")
        )
        .unwrap();
    }
    out.push_str("];\n");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out_dir.join("plugin_catalog.rs"), out).expect("写 plugin_catalog.rs");
}

fn read_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// 读一个字符串字段（`vendor = "data"`）。注释、行尾空白都不影响。
fn read_field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.split('#').next().unwrap_or("").trim();
        let value = line.strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
        let value = value.strip_prefix('"')?.strip_suffix('"')?;
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// 读 `[requires]` 段的**键**（包名），忽略值（版本约束不在图里表达）。
///
/// 段一直读到下一个 `[` 开头的行；空段（`[requires]` 紧跟别的段）返回空表。
fn read_requires(text: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            inside = line.starts_with("[requires]");
            continue;
        }
        if !inside {
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        if !key.is_empty() {
            deps.push(key.to_string());
        }
    }
    deps.sort();
    deps
}

/// workspace 的 `[workspace.package] version`（发行版号）。
fn read_workspace_version(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut inside = false;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            inside = line == "[workspace.package]";
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(value) = line.strip_prefix("version") {
            let value = value.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

// 解析逻辑不在 `#[cfg(test)]` 里做单元测试：build script 不是 `cargo test` 的测试目标，
// 写了也永远不会跑（那会是假绿）。真值校验放在 `src/lib.rs` 的 `tests`：直接对
// `PLUGIN_CATALOG` 这份**产物**断言（模块数、已知边、requires 全被收进来）。
