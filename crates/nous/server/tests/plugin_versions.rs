//! #93：`/v1/capabilities` 报的 `modules[].version` 必须是**插件自己的**版本。
//!
//! 判据从仓库真相出发，不点名模块：遍历 `docs/plugins.md` 主表的行 → 找到它的目录 →
//! 读那份 `plugin.toml` → 与 HTTP 回包逐条比。新增一个插件不用改这份测试。
//!
//! 还有一条防「假绿」：`build.rs` 必须对 `plugin.toml` 挂了 `cargo:rerun-if-changed`，
//! 否则改了版本不重编，上面那条比对就是在比两份旧值。

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use nous_server::{router, AppState};
use serde_json::Value;
use tower::ServiceExt;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// `docs/plugins.md` 主表里「有 crate 目录」的行：`module -> crates/<vendor>/<module>`。
///
/// 读法与 `scripts/check-plugins.sh` 一致：只认反引号包住、名字是 `[a-z][a-z0-9-]*` 的第一格；
/// `(待建)` / `—` 的行跳过（它们没有 `plugin.toml`）。
fn plugin_dirs() -> BTreeMap<String, PathBuf> {
    let doc = std::fs::read_to_string(repo_root().join("docs/plugins.md")).unwrap();
    let mut rows = BTreeMap::new();
    for line in doc.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = trimmed.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 4 || !cells[0].starts_with('`') {
            continue;
        }
        let module = cells[0].trim_matches('`').trim();
        if module.is_empty() || !module.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            continue;
        }
        let crate_cell = cells[2];
        if crate_cell.starts_with("(待建)") || crate_cell.trim_matches('`').trim() == "—" {
            continue;
        }
        let dir = repo_root().join("crates").join(cells[1]).join(module);
        if dir.join("plugin.toml").is_file() {
            rows.insert(module.to_string(), dir);
        }
    }
    rows
}

fn manifest_version(dir: &Path) -> String {
    let text = std::fs::read_to_string(dir.join("plugin.toml")).unwrap();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(rest) = line.strip_prefix("version") {
            if let Some(value) = rest.trim_start().strip_prefix('=') {
                return value.trim().trim_matches('"').to_string();
            }
        }
    }
    panic!("{} 没有 version 行", dir.display());
}

async fn capabilities(app: &Router) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/capabilities")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn capabilities_reports_each_plugins_own_version() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let body = capabilities(&app).await;
    let modules = body["modules"].as_array().unwrap();

    let dirs = plugin_dirs();
    assert!(
        dirs.len() >= 14,
        "docs/plugins.md 里只认出 {} 个插件目录 —— 判据的遍历面被吃掉了",
        dirs.len()
    );

    let mut checked = 0;
    for module in modules {
        let name = module["name"].as_str().unwrap();
        // 清单里的模块不一定都是 crate（triggers / combos / archive …），那些没有 plugin.toml；
        // 有 plugin.toml 的必须与自己的清单版本逐个相等。
        let Some(dir) = dirs.get(name) else {
            continue;
        };
        let want = manifest_version(dir);
        assert_eq!(
            module["version"].as_str().unwrap(),
            want,
            "{name} 的 capabilities.version 与它自己的 plugin.toml 不等"
        );
        assert!(
            module.get("versionSource").is_none(),
            "{name} 有 plugin.toml 却报了回退版本（versionSource=release）"
        );
        checked += 1;
    }
    assert!(checked >= 10, "只比对了 {checked} 个模块 —— 判据在空转");
}

/// 反面哨兵：版本不能全是同一个数。规则 8 解绑之后，14 个插件各走各的版本线；
/// 全等说明又退回「一个仓一个号」了（这条不点名具体值，只数不同值的个数）。
#[tokio::test]
async fn capabilities_versions_are_not_all_the_same_number() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let body = capabilities(&app).await;
    let versions: std::collections::BTreeSet<&str> = body["modules"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["version"].as_str())
        .collect();
    assert!(
        versions.len() >= 5,
        "capabilities 里只有 {} 个不同的模块版本（{versions:?}）—— 又变成一个号了",
        versions.len()
    );
}

/// `build.rs` 没挂 `rerun-if-changed` 时，改了 `plugin.toml` 不重编，上面两条就在比旧值。
/// 这条钉住「改了清单会触发重编」的那行代码。
#[test]
fn build_script_watches_every_plugin_manifest() {
    let build = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs")).unwrap();
    assert!(
        build.contains("cargo:rerun-if-changed"),
        "build.rs 没有 cargo:rerun-if-changed —— 改 plugin.toml 不会重编"
    );
    assert!(
        build.contains("plugin.toml"),
        "build.rs 没有盯 plugin.toml"
    );
    // 生成物必须真的来自 OUT_DIR，不是源码里手写的第二份表。
    let lib = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/plugin_versions.rs"),
    )
    .unwrap();
    assert!(
        lib.contains("include!(concat!(env!(\"OUT_DIR\")"),
        "src/plugin_versions.rs 不是 include! 生成物 —— 那是手写的第二份事实源"
    );
}
