//! 插件自己的版本（#93）：`模块名 → version`，编译期从各 `plugin.toml` 读出来。
//!
//! 规则 8 把 `plugin.toml` 的 `version` 与发行版号解绑之后，`/v1/capabilities` 再报
//! `env!("CARGO_PKG_VERSION")` 就等于把 14 个插件的经历抹成同一个数 —— 桌面端
//! 「关于 → 插件」列表上看不出谁动过谁没动过。
//!
//! 表是**生成物**（`build.rs` 写 `$OUT_DIR/plugin_versions.rs`，这里 `include!` 进来），
//! 不手写第二份：改 `plugin.toml` → 重编 → capabilities 跟着变。
//! 取不到（模块没有清单 / 清单没写合法 semver）才回退发行版号，且**如实说明**回退 ——
//! 静默回退等于又骗一次。

include!(concat!(env!("OUT_DIR"), "/plugin_versions.rs"));

/// 发行版号：取不到插件版本时的回退值，也是 `server.version` 的来源。
pub const RELEASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 这个模块的版本 + 它是不是回退来的。
///
/// `fallback` 为 `true` 时 `version == RELEASE_VERSION`，调用方必须把这件事说出去
/// （capabilities 里加 `versionSource`），不许当插件版本静默报。
pub fn version_of(module: &str) -> (String, bool) {
    match PLUGIN_VERSIONS.iter().find(|(name, _)| *name == module) {
        Some((_, version)) => ((*version).to_string(), false),
        None => (RELEASE_VERSION.to_string(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::{version_of, PLUGIN_VERSIONS, RELEASE_VERSION};

    #[test]
    fn table_is_not_empty_and_names_are_unique() {
        assert!(!PLUGIN_VERSIONS.is_empty(), "生成表是空的：build.rs 没读到任何 plugin.toml");
        let mut names: Vec<&str> = PLUGIN_VERSIONS.iter().map(|(name, _)| *name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "模块名在生成表里重复");
    }

    #[test]
    fn every_value_is_semver() {
        for (name, version) in PLUGIN_VERSIONS {
            let parts: Vec<&str> = version.split('.').collect();
            assert_eq!(parts.len(), 3, "{name} 的 version={version} 不是 X.Y.Z");
            for part in parts {
                assert!(
                    part.parse::<u64>().is_ok(),
                    "{name} 的 version={version} 含非数字段"
                );
            }
        }
    }

    #[test]
    fn unknown_module_falls_back_to_release_version_and_says_so() {
        let (version, fallback) = version_of("绝不存在这个模块");
        assert_eq!(version, RELEASE_VERSION);
        assert!(fallback, "回退必须如实标出来");
    }
}
