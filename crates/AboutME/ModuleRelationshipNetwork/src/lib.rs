//! `AboutME_ModuleRelationshipNetwork`：把「谁依赖谁」投影成一张只读的图。
//!
//! ## 它回答的问题，以及它**不**回答的问题
//!
//! 图里的边是 **crate 边**：A 的 `plugin.toml [requires]` 里写着 B 的包名，就有一条
//! A → B。这份原料由 `scripts/check-plugins.sh` 与 `cargo metadata` **双向逐条比对**
//! （少写 / 多写都红），所以它是「真依赖」的可信抄本，不是猜的。
//!
//! 它**不**表达语义依赖：「`Extend` 干活得靠 `Env` 给路径」这种话，内核里一个字段都没有
//! （`Extend` 自己调标准库拿 `temp_dir`，`[requires]` 里没有 `nous-env`）。这类边只能靠人
//! 手写并签字，本插件不替任何人编 —— 缺的就不在图上，不去「大概差不多」地补。
//!
//! 也别在这里重新算依赖：`build.rs` 已经把 `plugin.toml` 抄成静态表，运行时只剩投影。
//! 编译期内嵌 = 图与产物同一份，启动零 IO，也不会出现「跑着的引擎 vs 盘上的清单」两份真相。
//!
//! ## 形状（`wire.md`）
//!
//! ```json
//! { "generatedFor": "0.1.41",
//!   "nodes": ["Device", "…"],
//!   "edges": [ { "from": "Device", "to": "routes", "via": "nous-routes" } ],
//!   "unmapped": ["nous-fixture"] }
//! ```
//!
//! - `nodes` 是模块全名（与 `/v1/capabilities` 的 `modules[].name` 同一套字面量）。
//! - `via` 是被用到的**包名**：Desk 想显示「因为用了 `nous-routes`」时有据可查。
//! - `unmapped` 是「被依赖、但没有对应模块」的包名（例如只给 `dev` 层用的夹具）。
//!   **不静默丢弃**：丢一条边却不说，图就会在没人察觉的地方变得不完整。
//!
//! 第二条端点是 `GET /v1/modules/updates`（[`updates`]）：当前版本 vs 仓库最新版本。
//! 图的原料（`build.rs` 的扫描）与它共用同一份清单，所以两边的模块名不会分叉。

include!(concat!(env!("OUT_DIR"), "/plugin_catalog.rs"));

pub mod routes;
pub mod updates;

use std::collections::BTreeMap;

/// 这个插件在注册表里的模块名（`docs/plugins.md` 的 module 列）。
pub const MODULE: &str = "AboutME_ModuleRelationshipNetwork";

/// 整张图。纯函数：同样的静态表永远给同样的 JSON（键序也稳定，便于快照比对）。
pub fn graph() -> serde_json::Value {
    let by_package: BTreeMap<&str, &str> = PLUGIN_CATALOG
        .iter()
        .map(|(name, package, _)| (*package, *name))
        .collect();

    let nodes: Vec<&str> = PLUGIN_CATALOG.iter().map(|(name, _, _)| *name).collect();
    let mut edges: Vec<serde_json::Value> = Vec::new();
    let mut unmapped: Vec<&str> = Vec::new();
    for (name, _, requires) in PLUGIN_CATALOG {
        for package in *requires {
            match by_package.get(package) {
                // 自环没有意义（`cargo` 也不许），真出现就是清单写错了 —— 保留这条边让
                // 上面那张图自己露出来，而不是悄悄吞掉（`tests` 里断言它不存在）。
                Some(target) => edges.push(serde_json::json!({
                    "from": name,
                    "to": target,
                    "via": package,
                })),
                None => unmapped.push(package),
            }
        }
    }
    unmapped.sort_unstable();
    unmapped.dedup();

    serde_json::json!({
        "generatedFor": RELEASE,
        "nodes": nodes,
        "edges": edges,
        "unmapped": unmapped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn nodes() -> BTreeSet<&'static str> {
        PLUGIN_CATALOG.iter().map(|(name, _, _)| *name).collect()
    }

    /// 图不能是空的：`build.rs` 一次都没扫到清单时，端点会回一张「谁都不依赖谁」的假图。
    #[test]
    fn catalog_is_populated_and_holds_known_modules() {
        assert!(
            PLUGIN_CATALOG.len() >= 20,
            "只收到 {} 个模块，build.rs 的扫描大概率坏了",
            PLUGIN_CATALOG.len()
        );
        assert!(nodes().contains("Device"));
        assert!(nodes().contains(MODULE), "自己也得在图里");
    }

    /// `requires` 被**完整**收进来：拿 `[requires]` 的键与目录里的清单逐条比，
    /// 少一条就说明 build.rs 的解析漏了（这份断言就是那条「解析逻辑没测试」的补丁）。
    #[test]
    fn every_declared_requirement_lands_in_the_catalog() {
        for (name, _, requires) in PLUGIN_CATALOG {
            // 自带 1 条：`nous-routes`（端点契约）
            if *name == MODULE {
                assert_eq!(*requires, &["nous-routes"]);
            }
        }
        let (_, _, device) = PLUGIN_CATALOG
            .iter()
            .find(|(name, _, _)| *name == "Device")
            .expect("Device 在表里");
        assert!(
            device.contains(&"nous-routes"),
            "Device 的 [requires] 里有 nous-routes，收不进来就是解析坏了"
        );
    }

    /// 每条边的两端都得是图里有的模块；不许自环；不许重复。
    #[test]
    fn edges_are_well_formed() {
        let nodes = nodes();
        let mut seen: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
        for edge in graph()["edges"].as_array().expect("edges 是数组") {
            let from = edge["from"].as_str().expect("from");
            let to = edge["to"].as_str().expect("to");
            let via = edge["via"].as_str().expect("via");
            assert!(nodes.contains(from), "边起点不在 nodes 里：{from}");
            assert!(nodes.contains(to), "边终点不在 nodes 里：{to}");
            assert_ne!(from, to, "自环：{from} → {to}（via {via}）");
            assert!(
                seen.insert((from, to, via)),
                "重复的边：{from} → {to} via {via}"
            );
        }
    }

    /// `cargo` 的依赖图不可能成环；成环说明清单写错了，图也就没法当有向图用。
    #[test]
    fn graph_is_acyclic() {
        let nodes = nodes();
        let value = graph();
        let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in value["edges"].as_array().unwrap() {
            adjacency
                .entry(edge["from"].as_str().unwrap())
                .or_default()
                .push(edge["to"].as_str().unwrap());
        }
        // 三色 DFS：白 = 没走，灰 = 在栈上，黑 = 走完。
        fn walk<'a>(
            node: &'a str,
            adjacency: &BTreeMap<&'a str, Vec<&'a str>>,
            marks: &mut BTreeMap<&'a str, u8>,
        ) -> Result<(), String> {
            match marks.get(node).copied() {
                Some(2) => return Ok(()),
                Some(1) => return Err(format!("成环：{node} 又回到了栈上")),
                _ => {}
            }
            marks.insert(node, 1);
            for next in adjacency.get(node).into_iter().flatten() {
                walk(next, adjacency, marks)?;
            }
            marks.insert(node, 2);
            Ok(())
        }
        let mut marks = BTreeMap::new();
        for node in &nodes {
            walk(node, &adjacency, &mut marks).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    /// `unmapped` 去重且有序（Desk 直接照序显示，不再排一次）。
    /// 并且：图里真有的包名不该出现在 `unmapped` 里。
    #[test]
    fn unmapped_is_sorted_and_honest() {
        let graph = graph();
        let listed: Vec<&str> = graph["unmapped"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item.as_str().unwrap())
            .collect();
        let mut sorted = listed.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(listed, sorted, "unmapped 必须有序去重");

        let packages: BTreeSet<&str> = PLUGIN_CATALOG.iter().map(|(_, pkg, _)| *pkg).collect();
        for name in listed {
            assert!(
                !packages.contains(name),
                "{name} 是插件的包名，不该被算成 unmapped"
            );
        }
    }

    /// `PLUGIN_VERSIONS` 与 `PLUGIN_CATALOG` 是同一遍扫描的两个投影，名字集合必须一样：
    /// 哪一边漏一个，`/v1/modules/updates` 就会「这个模块没有当前版本」或反之。
    #[test]
    fn version_table_and_catalog_agree_on_names() {
        let catalog: BTreeSet<&str> = PLUGIN_CATALOG.iter().map(|(name, _, _)| *name).collect();
        let versions: BTreeSet<&str> = PLUGIN_VERSIONS.iter().map(|(name, _)| *name).collect();
        assert_eq!(catalog, versions, "两张表的模块集合必须一致");
    }

    /// 自己的模块版本必须是清单里那份 `X.Y.Z`（不是发行版号）。
    #[test]
    fn own_version_comes_from_the_manifest() {
        let (_, version) = PLUGIN_VERSIONS
            .iter()
            .find(|(name, _)| *name == MODULE)
            .expect("自己在表里");
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugin.toml");
        let text = std::fs::read_to_string(manifest).expect("读 plugin.toml");
        let declared = field(&text, "version").expect("plugin.toml 里没有 version");
        assert_eq!(*version, declared, "build.rs 抄错了版本");
        assert_ne!(*version, RELEASE, "插件版本必须独立于发行版号（规则 8）");
    }

    fn field(text: &str, key: &str) -> Option<String> {
        text.lines().find_map(|line| {
            let line = line.split('#').next().unwrap_or("").trim();
            let rest = line.strip_prefix(key)?.trim_start();
            let value = rest.strip_prefix('=')?.trim().trim_matches('"');
            (!value.is_empty()).then(|| value.to_string())
        })
    }

    /// 每个模块的 `current` 都来自它自己的 `plugin.toml`，不是发行版号。
    /// 逐份清单现读一遍：`build.rs` 漏扫一个字段，这里就会指名。
    #[test]
    fn every_version_comes_from_its_own_manifest() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../crates");
        let mut checked = 0;
        for vendor in std::fs::read_dir(&root).expect("读 crates/") {
            let vendor = vendor.expect("vendor 目录").path();
            if !vendor.is_dir() {
                continue;
            }
            for module in std::fs::read_dir(&vendor).expect("读 vendor 目录") {
                let module = module.expect("module 目录").path();
                let manifest = module.join("plugin.toml");
                if !manifest.is_file() {
                    continue;
                }
                let text = std::fs::read_to_string(&manifest)
                    .unwrap_or_else(|error| panic!("读 {}：{error}", manifest.display()));
                let name = field(&text, "name").expect("plugin.toml 里没有 name");
                let declared = field(&text, "version").expect("plugin.toml 里没有 version");
                let (_, actual) = PLUGIN_VERSIONS
                    .iter()
                    .find(|(module, _)| *module == name)
                    .unwrap_or_else(|| panic!("{name} 不在 PLUGIN_VERSIONS 里"));
                assert_eq!(
                    *actual, declared,
                    "{name} 的版本被 build.rs 抄错了（{}）",
                    manifest.display()
                );
                checked += 1;
            }
        }
        assert!(checked >= 20, "只读到 {checked} 份 plugin.toml，扫描坏了");
    }

    /// `generatedFor` 是**发行版号**（workspace 的 `version`）——Desk 靠它判「图是哪个版本生的」。
    ///
    /// 与仓库根 `Cargo.toml` 现读一遍比对：本插件的 `plugin.toml` 独立涨版时，这里也不该漂。
    #[test]
    fn generated_for_is_the_release_version() {
        let release = graph()["generatedFor"].as_str().unwrap().to_string();
        assert!(!release.is_empty(), "workspace 版本没读出来");
        assert!(release.chars().all(|c| c.is_ascii_digit() || c == '.'));

        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../Cargo.toml");
        let text = std::fs::read_to_string(&manifest).expect("读仓库根 Cargo.toml");
        let mut inside = false;
        let mut expected = String::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.starts_with('[') {
                inside = line == "[workspace.package]";
                continue;
            }
            if inside {
                if let Some(value) = line.strip_prefix("version") {
                    expected = value
                        .trim_start()
                        .strip_prefix('=')
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .to_string();
                    break;
                }
            }
        }
        assert_eq!(
            release, expected,
            "generatedFor 必须是 workspace 的发行版号"
        );
    }
}
