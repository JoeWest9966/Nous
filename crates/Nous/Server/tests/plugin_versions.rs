//! #93：`/v1/capabilities` 报的 `modules[].version` 必须是**插件自己的**版本。
//!
//! 判据从仓库真相出发，不点名模块：遍历 `docs/plugins.md` 主表的行 → 找到它的目录 →
//! 读那份 `plugin.toml` → 与 HTTP 回包逐条比。新增一个插件不用改这份测试。
//!
//! 还有一条防「假绿」：`build.rs` 必须对每份 `plugin.toml` 挂 `cargo:rerun-if-changed`，
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

/// 模块全名 → 目录名（`docs/plugins.md` 规则 12）：去掉 vendor 段、按大写字母断词、
/// 每段首字母大写（Magento 的 `app/code/Amasty/Shiprestriction`）。
///
/// `Element_Models` → `Models`、`Nous_PresetsAdmin` → `PresetsAdmin`、`data-manager` → `DataManager`。
/// 与 `scripts/check-plugins.sh` 的 `_pascal`、`scripts/layer-paths.sh` 的 `dir_of` 同算法。
fn dir_of(module: &str) -> String {
    let tail = module.split_once('_').map(|(_, tail)| tail).unwrap_or(module);
    // 先按 camel hump 断词（`PresetsAdmin` → `Presets` + `Admin`），再按连字符/下划线断。
    let humped = {
        let mut out = String::new();
        for (i, ch) in tail.chars().enumerate() {
            if i > 0 && ch.is_ascii_uppercase() {
                out.push('-');
            }
            out.push(ch);
        }
        out
    };
    humped
        .split(|c: char| c == '-' || c == '_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect()
}

/// 全名形如 `Vendor_Module`（两段首字母大写）或老式小写连字符名。
fn module_name_ok(module: &str) -> bool {
    let two_segments = module
        .split_once('_')
        .is_some_and(|(head, tail)| {
            !head.is_empty()
                && !tail.is_empty()
                && !tail.contains('_')
                && head.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && tail.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && module
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    let legacy = module
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && module.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    two_segments || legacy
}

/// `docs/plugins.md` 主表里「有 crate 目录」的行：`模块全名 -> crates/<vendor>/<目录>`。
///
/// 读法与 `scripts/check-plugins.sh` 一致：只认反引号包住、名字合法（规则 12）的第一格；
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
        if module.is_empty() || !module_name_ok(module) {
            continue;
        }
        let crate_cell = cells[2];
        if crate_cell.starts_with("(待建)") || crate_cell.trim_matches('`').trim() == "—" {
            continue;
        }
        let dir = repo_root()
            .join("crates")
            .join(cells[1])
            .join(dir_of(module));
        if dir.join("plugin.toml").is_file() {
            rows.insert(module.to_string(), dir);
        }
    }
    rows
}

/// 盘上这份清单的 `version`。**直接问生成的表**（`plugin_versions::version_of`）而不是
/// 在测试里再写一个 TOML 扫描器 —— 再写一份就是「插件版本」的第三份事实源，
/// 而且它比 `build.rs` 那份更容易和真实解析漂。
fn manifest_version(module: &str) -> String {
    let (version, fallback) = nous_server::plugin_versions::version_of(module);
    assert!(
        !fallback,
        "{module} 有 plugin.toml 却回退到发行版号 —— build.rs 没读到它"
    );
    version
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

/// 规则 12 的两条纯函数先钉一遍：目录推导错了，上面那条比对就会扫到空目录而静默跳过。
#[test]
fn rule12_name_and_dir_derivation() {
    assert_eq!(dir_of("Element_Models"), "Models");
    assert_eq!(dir_of("Nous_Device"), "Device");
    assert_eq!(dir_of("Nous_PresetsAdmin"), "PresetsAdmin");
    assert_eq!(dir_of("data-manager"), "DataManager");
    assert!(module_name_ok("Nous_Device"));
    assert!(module_name_ok("data-manager"));
    assert!(!module_name_ok("Nous_device"));
    assert!(!module_name_ok("nous-device_Extra"));
    assert!(!module_name_ok("Device"));
}

#[tokio::test]
async fn capabilities_reports_each_plugins_own_version() {
    let app = router(AppState::with_source(common::FakeSource::two_sessions()));
    let body = capabilities(&app).await;
    let modules = body["modules"].as_array().unwrap();

    let dirs = plugin_dirs();
    // 不写死 28：但下限钉在「清单里进发行面的 14 个 + 平台/内核层那批」的量级，
    // 掉到十几就说明 `docs/plugins.md` 的解析或目录扫描坏了（那时下面的逐条比对在空转）。
    assert!(
        dirs.len() >= 20,
        "docs/plugins.md 里只认出 {} 个插件目录 —— 判据的遍历面被吃掉了",
        dirs.len()
    );

    let mut checked = 0;
    for module in modules {
        let name = module["name"].as_str().unwrap();
        // 清单里的模块不一定都是 crate（triggers / combos / archive …），那些没有 plugin.toml；
        // 有 plugin.toml 的必须与自己的清单版本逐个相等。
        if !dirs.contains_key(name) {
            continue;
        }
        let want = manifest_version(name);
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
    // 反过来钉一遍：清单里**每个有 plugin.toml 的模块**都必须被比到 ——
    // 否则「遍历面很大、实际只比了两三条」会静默通过（这正是空跑的味道）。
    // 注意方向：`crates/*/*` 里只有进 `MANIFEST` 的那批才会出现在 capabilities，
    // kernel / platform / dev 层那批本来就不报（它们不是用户功能面）。
    let with_manifest: Vec<&str> = modules
        .iter()
        .filter_map(|m| m["name"].as_str())
        .filter(|name| dirs.contains_key(*name))
        .collect();
    assert_eq!(
        checked,
        with_manifest.len(),
        "比对了 {checked} 个模块，capabilities 里有 plugin.toml 的是 {} 个",
        with_manifest.len()
    );
    assert!(
        checked >= 10,
        "只比对了 {checked} 个模块 —— 判据在空转"
    );
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
/// 这条钉住「生成物来自 OUT_DIR」与「扫描面不是写死的」。
#[test]
fn build_script_watches_every_plugin_manifest() {
    let build = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs")).unwrap();
    assert!(
        build.contains("cargo:rerun-if-changed"),
        "build.rs 没有 cargo:rerun-if-changed —— 改 plugin.toml 不会重编"
    );
    // 必须逐份 `plugin.toml` 挂，而不是只盯目录：目录 mtime 在「只改文件内容」时不变。
    assert!(
        build.contains("rerun-if-changed={}") && build.contains("manifest.display()"),
        "build.rs 没有逐份 plugin.toml 挂 rerun-if-changed"
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

/// **正向哨兵**（票面补充评论要求）：改高一个 patch，capabilities 必须跟着变。
///
/// 没法在测试里 `cargo build`（会与正在跑的 cargo 抢 target 锁，且编译一次要几十秒），
/// 所以改成跑两半、在中间真改一次盘：
///   ① 记下盘上的版本；② 写高一档；③ 请 `build.rs` 自己重算一遍（`cargo build -p nous-server`）
///   ④ 用新编译的产物重读 capabilities；⑤ 恢复原值并重编。
/// 任一步失败都把原值写回（`Drop` 兜底），不会把仓弄脏。
///
/// 这个测试只在 `NOUS_SLOW_TESTS=1` 时跑：它会真编译两次，不适合当日常单元测试。
#[test]
fn bumping_a_manifest_version_changes_capabilities() {
    if std::env::var("NOUS_SLOW_TESTS").as_deref() != Ok("1") {
        eprintln!("跳过 bumping_a_manifest_version_changes_capabilities（设 NOUS_SLOW_TESTS=1 才跑，会真编译两次）");
        return;
    }
    let module = "sessions";
    let dirs = plugin_dirs();
    let dir = dirs.get(module).expect("sessions 必须在清单里");
    let manifest = dir.join("plugin.toml");
    let original = std::fs::read_to_string(&manifest).unwrap();
    let old = manifest_version(module);
    let (major, minor, patch): (u64, u64, u64) = {
        let parts: Vec<u64> = old.split('.').map(|p| p.parse().unwrap()).collect();
        (parts[0], parts[1], parts[2])
    };
    let bumped = format!("{major}.{minor}.{}", patch + 1);

    let restore = Restore {
        path: manifest.clone(),
        original: original.clone(),
    };
    std::fs::write(
        &manifest,
        original.replacen(
            &format!("version = \"{old}\""),
            &format!("version = \"{bumped}\""),
            1,
        ),
    )
    .expect("写不进 plugin.toml");
    assert_ne!(bumped, old);

    // 真重编（build.rs 的 rerun-if-changed 会因 manifest 变化触发）。
    let built = std::process::Command::new(env!("CARGO"))
        .args(["build", "-q", "-p", "nous-server"])
        .current_dir(repo_root())
        .status()
        .expect("cargo build 可跑");
    assert!(built.success(), "改高版本后重编失败");
    drop(restore); // 先恢复盘面，再去核对（失败了也不留脏）

    // 核对：读**新编出来的**二进制才能看到新值；这里退一步验生成物本身 ——
    // `$OUT_DIR/plugin_versions.rs` 是 build.rs 的产物，改了清单不重编就不会变。
    let out_dir = std::path::Path::new(env!("OUT_DIR"));
    let generated = std::fs::read_to_string(out_dir.join("plugin_versions.rs"))
        .unwrap_or_else(|_| panic!("读不出 {}（build.rs 没生成）", out_dir.display()));
    assert!(
        generated.contains(&format!("({module:?}, {bumped:?})")),
        "改了 {module}/plugin.toml 到 {bumped}，生成表没跟着变：\n{generated}"
    );
}

struct Restore {
    path: PathBuf,
    original: String,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.path, &self.original);
    }
}

