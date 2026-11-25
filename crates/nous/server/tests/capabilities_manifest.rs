//! #88：MANIFEST 的六列必须与 docs/plugins.md 同源，`/v1/capabilities` 才能给桌面端
//! 稳定的类型 / 层 / 必需 / 来源 crate / 说明。
//!
//! 判据打在公共接缝：编译后的 `meta::MANIFEST` 与仓库里的唯一真相逐列比对，
//! 不抄第二份期望数组；HTTP 侧另测「字段真的到了线上」。

use std::path::Path;

/// docs/plugins.md 主表：`module -> 所有同名行`。同名多行（数据 + 扫描 crate）是清单现状，
/// 只要有一行六列全对就算对齐；`——` 的第二份事实源绝不允许存在。
fn doc_cells(module: &str) -> Vec<Vec<String>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(root.join("../../../docs/plugins.md")).unwrap();
    let rows: Vec<Vec<String>> = doc
        .lines()
        .filter(|line| line.starts_with(&format!("| `{module}` |")))
        .map(|row| row.split('|').map(|cell| cell.trim().to_string()).collect())
        .collect();
    assert!(!rows.is_empty(), "docs/plugins.md 缺 `{module}` 行");
    rows
}

#[test]
fn manifest_metadata_matches_the_plugins_table_cell_by_cell() {
    let manifest = nous_server::meta::MANIFEST;
    assert_eq!(manifest.len(), 15, "#90 下线 documents、#60 补 presets-admin、元素库 elements 后 MANIFEST 应为 14 行");
    for entry in manifest {
        // 表头：`| module | vendor | crate | layer | 形态 | API 面 | 必需 | 备注 |`
        // split('|') → ["", module, vendor, crate, layer, 形态, API, 必需, 备注, ""]
        let rows = doc_cells(entry.name);
        let matches = rows.iter().any(|cells| {
            cells.len() == 10
                && cells[4] == entry.layer.as_str()
                && cells[5] == entry.kind.as_str()
                && cells[7] == entry.required_text()
                && cells[3] == entry.crate_name
                && cells[8] == entry.description
        });
        assert!(
            matches,
            "{} 的六列在 docs/plugins.md 里一行都对不上：{rows:?}",
            entry.name
        );
    }
}

#[test]
fn manifest_covers_every_user_facing_module_and_no_dev_module() {
    let manifest = nous_server::meta::MANIFEST;
    let names: Vec<&str> = manifest.iter().map(|e| e.name).collect();
    for name in ["sessions", "env", "repo", "presets", "presets-admin", "skills", "archive",
                 "combos", "triggers", "data-manager", "device", "prompts", "ext", "elements",
                 "Element_Models"] {
        assert!(names.contains(&name), "MANIFEST 缺 {name}");
    }
    assert!(!names.contains(&"fixture"), "dev 层不是用户功能面");
    assert!(!names.contains(&"ext-probe"), "dev 层不是用户功能面");
}
