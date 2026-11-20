//! #88：MANIFEST 的六列必须与 docs/plugins.md 同源，`/v1/capabilities` 才能给桌面端
//! 稳定的类型 / 层 / 必需 / 来源 crate / 说明。
//!
//! 判据打在公共接缝：编译后的 `meta::MANIFEST` 与仓库里的唯一真相逐列比对，
//! 不抄第二份期望数组；HTTP 侧另测「字段真的到了线上」。

use std::path::Path;

/// docs/plugins.md 主表：`module -> 原始行`。只读 `| \`name\` |` 开头的行。
fn doc_cells(module: &str) -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(root.join("../../../docs/plugins.md")).unwrap();
    let row = doc
        .lines()
        .find(|line| line.starts_with(&format!("| `{module}` |")))
        .unwrap_or_else(|| panic!("docs/plugins.md 缺 `{module}` 行"));
    row.split('|').map(|cell| cell.trim().to_string()).collect()
}

#[test]
fn manifest_metadata_matches_the_plugins_table_cell_by_cell() {
    let manifest = nous_server::meta::MANIFEST;
    assert_eq!(manifest.len(), 12, "MANIFEST 行数变了要同步核对 docs/plugins.md");
    for entry in manifest {
        // 表头：`| module | vendor | crate | layer | 形态 | API 面 | 必需 | 备注 |`
        // split('|') → ["", module, vendor, crate, layer, 形态, API, 必需, 备注, ""]
        let cells = doc_cells(entry.name);
        assert_eq!(cells.len(), 10, "{} 的表行列数不对：{cells:?}", entry.name);
        assert_eq!(cells[4], entry.layer.as_str(), "{} 的 layer 与清单不一致", entry.name);
        assert_eq!(cells[5], entry.kind.as_str(), "{} 的 kind 与清单不一致", entry.name);
        assert_eq!(cells[7], entry.required_text(), "{} 的 required 与清单不一致", entry.name);
        assert_eq!(cells[3], entry.crate_name, "{} 的 crate 与清单不一致", entry.name);
        assert_eq!(cells[8], entry.description, "{} 的 description 与清单不一致", entry.name);
    }
}

#[test]
fn manifest_covers_every_user_facing_module_and_no_dev_module() {
    let manifest = nous_server::meta::MANIFEST;
    let names: Vec<&str> = manifest.iter().map(|e| e.name).collect();
    for name in ["sessions", "env", "documents", "presets", "skills", "archive",
                 "combos", "triggers", "stats", "device", "prompts", "ext"] {
        assert!(names.contains(&name), "MANIFEST 缺 {name}");
    }
    assert!(!names.contains(&"fixture"), "dev 层不是用户功能面");
    assert!(!names.contains(&"ext-probe"), "dev 层不是用户功能面");
}
