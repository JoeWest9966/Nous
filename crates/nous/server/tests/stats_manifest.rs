//! #62 判据 1 的 cargo-test 面：plugin.toml / docs/plugins.md / ROUTES 三处同源。
//! `scripts/check-plugins.sh` 是外部判据（checked +1），这里钉住「本票的最小增量」。

use std::path::Path;

#[test]
fn stats_plugin_manifest_module_layer_and_routes_are_registered() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let plugin = std::fs::read_to_string(root.join("../stats/plugin.toml")).unwrap();
    assert!(plugin.contains("name = \"stats\""));
    assert!(plugin.contains("layer = \"app\""));
    assert!(plugin.contains("crate = \"nous-stats\""));

    let plugins_doc = std::fs::read_to_string(root.join("../../../docs/plugins.md")).unwrap();
    let stats_row = plugins_doc.lines()
        .find(|line| line.starts_with("| `stats` |"))
        .expect("docs/plugins.md 缺 stats 行");
    assert!(stats_row.contains("`nous-stats`"), "{stats_row}");
    assert!(stats_row.contains("| app |"), "{stats_row}");
    assert!(stats_row.contains("编译期"), "{stats_row}");

    let stats_routes: Vec<_> = nous_server::meta::ROUTES.iter()
        .filter(|route| route.module == "stats")
        .collect();
    assert_eq!(stats_routes.len(), 3);
    for path in ["/v1/stats/usage", "/v1/stats/tasks", "/v1/stats/db"] {
        assert!(stats_routes.iter().any(|r| r.method == "GET" && r.path == path));
    }
    assert!(nous_server::meta::route_enabled("stats"));
}
