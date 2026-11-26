//! 编译期只注入 target triple；身份的字节哈希在运行时算（见 src/version.rs）。
//!
//! #93：另外把每个插件的 `plugin.toml` 版本读出来，生成 `模块名 → version` 的静态表
//! （`$OUT_DIR/plugin_versions.rs`，由 `src/plugin_versions.rs` `include!` 进来）。
//! 不手写第二份：`/v1/capabilities` 报的版本与插件自己的清单同源，改清单才改得上。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    println!(
        "cargo:rustc-env=NOUS_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string())
    );
    emit_plugin_versions();
}

/// 扫描 `crates/*/*/plugin.toml`，生成 `pub const PLUGIN_VERSIONS: &[(&str, &str)]`。
///
/// `cargo:rerun-if-changed` 盯住**每一个** plugin.toml 和 `crates/` 下两层的目录本身
/// （新增 / 删除插件时目录 mtime 会变）：少了这一条，改了版本不重编 = 假绿。
fn emit_plugin_versions() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest_dir
        .ancestors()
        .find(|dir| dir.join("docs/plugins.md").is_file())
        .expect("找不到仓库根（docs/plugins.md 的上级）")
        .to_path_buf();
    let crates = root.join("crates");

    let mut entries: Vec<(String, String, String)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for vendor in read_dirs(&crates) {
        println!("cargo:rerun-if-changed={}", vendor.display());
        for module in read_dirs(&vendor) {
            let manifest = module.join("plugin.toml");
            if !manifest.is_file() {
                continue;
            }
            println!("cargo:rerun-if-changed={}", manifest.display());
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                println!(
                    "cargo:warning=读不出 {}，该模块版本回退发行版号",
                    manifest.display()
                );
                continue;
            };
            // 模块名取**清单里的全名**（`docs/plugins.md` 规则 12）：`Models` 的
            // 目录叫 `models`、包叫 `element-models`，三者的对应关系只有清单说得清。
            let name = read_field(&text, "name").unwrap_or_else(|| {
                module
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let vendor = read_field(&text, "vendor");
            match read_version(&text) {
                Some(version) => entries.push((name, version, vendor.unwrap_or_default())),
                None => {
                    // 回退不静默：清单存在但读不出合法 semver 是**仓的错**，不是环境问题。
                    // `cargo:warning` 只进构建日志，容易被当成噪声；所以同时把这份清单
                    // 记进生成表里的 `PLUGIN_VERSION_PROBLEMS`，让 `/v1/capabilities`
                    // 的 `versionSource: "release"` 之外还能被测试与人工查到具体是哪份。
                    println!(
                        "cargo:warning={} 没有合法的 version，该模块版本回退发行版号",
                        manifest.display()
                    );
                    problems.push(name);
                }
            }
        }
    }
    entries.sort();
    problems.sort();

    let mut out = String::from(
        "// 由 crates/Nous/Server/build.rs 生成 —— 别手改。\n\
         // 来源：crates/<vendor>/<module>/plugin.toml 的 version（#93）。\n\
         pub const PLUGIN_VERSIONS: &[(&str, &str)] = &[\n",
    );
    for (name, version, _) in &entries {
        writeln!(out, "    ({name:?}, {version:?}),").unwrap();
    }
    out.push_str("];\n\n// 出品方（规则 10）：模块名 → vendor。空串 = 清单没写 vendor。\n");
    out.push_str("pub const PLUGIN_VENDORS: &[(&str, &str)] = &[\n");
    for (name, _, vendor) in &entries {
        writeln!(out, "    ({name:?}, {vendor:?}),").unwrap();
    }
    out.push_str("];\n\n// 清单在、但 `version` 不是合法 `X.Y.Z` 的模块（它们回退发行版号）。\n");
    out.push_str("pub const PLUGIN_VERSION_PROBLEMS: &[&str] = &[\n");
    for name in &problems {
        writeln!(out, "    {name:?},").unwrap();
    }
    out.push_str("];\n");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out_dir.join("plugin_versions.rs"), out).expect("写 plugin_versions.rs");
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

/// 只认 `version = "X.Y.Z"` 这一行；注释、行尾空白都不影响。
fn read_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.split('#').next().unwrap_or("").trim();
        let value = line.strip_prefix("version")?.trim_start().strip_prefix('=')?;
        let value = value.trim();
        let value = value.strip_prefix('"')?.strip_suffix('"')?;
        let parts: Vec<&str> = value.split('.').collect();
        let ok = parts.len() == 3
            && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        ok.then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::read_version;

    #[test]
    fn reads_the_version_line_and_ignores_noise() {
        assert_eq!(
            read_version("name = \"ai\"\nversion = \"0.2.3\"  # 注释\n"),
            Some("0.2.3".into())
        );
        assert_eq!(read_version("version=\"1.2.3\"\n"), Some("1.2.3".into()));
    }

    #[test]
    fn rejects_non_semver_and_missing() {
        assert_eq!(read_version("version = \"0.2\"\n"), None);
        assert_eq!(read_version("version = \"0.2.x\"\n"), None);
        assert_eq!(read_version("name = \"ai\"\n"), None);
        // `version.workspace = true` 这类行不是插件版本，不许误读成 0.1.0
        assert_eq!(read_version("version.workspace = true\n"), None);
    }
}
