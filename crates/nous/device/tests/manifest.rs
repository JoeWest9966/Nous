//! #65 判据 1/2：device 的清单归属与依赖方向。
//!
//! 接缝：`cargo metadata` / `cargo tree` / `scripts/check-plugins.sh`（都是公开命令行面），
//! 不在 crate 内部查源码。这里故意不引 `nous_server` —— `cargo tree` 一遍就能看见。

use std::path::Path;
use std::process::Command;

fn root() -> &'static str {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::to_str)
        .expect("crates/nous/device → 仓库根")
}

fn repo_cmd(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(root());
    cmd
}

#[test]
fn device_is_a_registered_app_plugin_and_check_plugins_counts_one_more() {
    let meta: serde_json::Value = String::from_utf8(
        repo_cmd("cargo")
            .args(["metadata", "--format-version", "1"])
            .output()
            .expect("cargo metadata 可跑")
            .stdout,
    )
    .ok()
    .and_then(|text| serde_json::from_str(&text).ok())
    .expect("cargo metadata 输出是 JSON");
    let device = meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pkg| pkg["name"] == "nous-device")
        .expect("nous-device 必须在 workspace 里");
    let manifest = Path::new(device["manifest_path"].as_str().unwrap());
    assert_eq!(manifest.parent().unwrap().file_name().unwrap(), "device");
    let plugin = std::fs::read_to_string(manifest.parent().unwrap().join("plugin.toml"))
        .expect("device/plugin.toml 必须存在");
    assert!(plugin.contains("name = \"device\""), "plugin.toml 的 name");
    assert!(plugin.contains("layer = \"app\""), "plugin.toml 的 layer=app");

    let checked = repo_cmd("bash")
        .args(["scripts/check-plugins.sh"])
        .output()
        .expect("check-plugins.sh 可跑");
    let summary = String::from_utf8_lossy(&checked.stdout);
    let last = summary.lines().last().unwrap_or_default();
    // device 自己让 checked 从 17 → 18；别的票再加插件（如 #62 的 stats）会继续 +1，
    // 所以只钉「至少 18 且 0 处不符」，不写死数字（与票面判据一致）。
    let checked_n: u32 = last
        .split("checked=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert!(
        checked_n >= 18 && last.contains("mismatched=0"),
        "清单应至少 18 行且 0 处不符：{last}"
    );

    let doc = std::fs::read_to_string(Path::new(root()).join("docs/plugins.md")).unwrap();
    // layer-paths.sh 只读同一张表的四列，跳过 (待建)/— 行；四层之和就是 checked。
    let mut expected = 0usize;
    for layer in ["kernel", "platform", "app", "dev"] {
        let out = repo_cmd("bash").args(["scripts/layer-paths.sh", layer]).output().expect("layer-paths.sh 可跑");
        let text = String::from_utf8_lossy(&out.stdout);
        expected += text.split_whitespace().count();
    }
    assert!(summary.contains(&format!("checked={expected} mismatched=0")),
        "清单应为 {expected} 行且 0 处不符：{}", summary.lines().last().unwrap_or_default());

    assert!(
        doc.contains("| `device` | nous | `nous-device` | app |"),
        "docs/plugins.md 必须有 device 行"
    );
}

#[test]
fn device_does_not_depend_on_server() {
    let output = repo_cmd("cargo")
        .args(["tree", "-p", "nous-device"])
        .output()
        .expect("cargo tree 可跑");
    let tree = String::from_utf8_lossy(&output.stdout);
    assert!(
        !tree.contains("nous-server"),
        "device 不许反依赖 server（判据 2）：\n{tree}"
    );
}
