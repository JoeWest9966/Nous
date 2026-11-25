//! #62 判据 1 的 cargo-test 面：plugin.toml / docs/plugins.md / ROUTES 三处同源。
//! `scripts/check-plugins.sh` 是外部判据（checked +1），这里钉住「本票的最小增量」。

use std::path::Path;

#[test]
fn data_manager_plugin_manifest_module_layer_and_routes_are_registered() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let plugin = std::fs::read_to_string(root.join("../../data/data-manager/plugin.toml")).unwrap();
    assert!(plugin.contains("name = \"data-manager\""));
    assert!(plugin.contains("layer = \"app\""));
    assert!(plugin.contains("crate = \"data-data-manager\""));

    let plugins_doc = std::fs::read_to_string(root.join("../../../docs/plugins.md")).unwrap();
    let stats_row = plugins_doc.lines()
        .find(|line| line.starts_with("| `data-manager` |"))
        .expect("docs/plugins.md 缺 data-manager 行");
    assert!(stats_row.contains("`data-data-manager`"), "{stats_row}");
    assert!(stats_row.contains("| app |"), "{stats_row}");
    assert!(stats_row.contains("编译期"), "{stats_row}");

    let stats_routes: Vec<_> = nous_server::meta::ROUTES.iter()
        .filter(|route| route.module == "data-manager")
        .collect();
    assert_eq!(stats_routes.len(), 8);
    for path in [
        "/v1/stats/usage",
        "/v1/stats/tasks",
        "/v1/stats/db",
        "/v1/stats/overview",
        "/v1/stats/conversations",
        "/v1/stats/conversation/:id",
        "/v1/stats/cost",
        "/v1/stats/health",
    ] {
        assert!(stats_routes.iter().any(|r| r.method == "GET" && r.path == path), "缺 {path}");
    }
    assert!(nous_server::meta::route_enabled("data-manager"));
}
