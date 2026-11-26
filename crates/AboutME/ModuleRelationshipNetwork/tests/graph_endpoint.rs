//! `GET /v1/modules/graph`：真 Router 上打一次（不走 `server`，也不需要 `AppState`）。
//!
//! 验三件事：端点真装进 Router（路径 / 方法对）、回包形状对、图里的边两端都在 `nodes` 里。
//! 形状的另一半（环、重复边、`unmapped` 诚实性）在 `src/lib.rs` 的单元测试里。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use tower::ServiceExt;

fn app() -> Router {
    let mut router = Router::new();
    for route in aboutme_module_relationship_network::routes::routes::<()>() {
        router = route.mount(router);
    }
    router
}

#[test]
fn endpoint_is_declared_under_this_module() {
    let declared: Vec<(String, String, &'static str)> =
        aboutme_module_relationship_network::routes::routes::<()>()
            .iter()
            .map(|route| (route.method().to_string(), route.declared(), route.module()))
            .collect();
    assert_eq!(
        declared,
        vec![(
            "GET".to_string(),
            "GET /v1/modules/graph".to_string(),
            "AboutME_ModuleRelationshipNetwork",
        )]
    );
}

#[tokio::test]
async fn graph_endpoint_returns_the_whole_graph() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/v1/modules/graph")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), 512 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).expect("回包是 JSON");

    for key in ["generatedFor", "nodes", "edges", "unmapped"] {
        assert!(!body[key].is_null(), "回包缺 {key}（wire.md 的形状）");
    }
    let nodes: Vec<&str> = body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect();
    assert!(nodes.contains(&"AboutME_ModuleRelationshipNetwork"));

    let edges = body["edges"].as_array().unwrap();
    assert!(!edges.is_empty(), "一条边都没有，图没生成");
    for edge in edges {
        for key in ["from", "to", "via"] {
            assert!(edge[key].is_string(), "边缺 {key}：{edge}");
        }
        assert!(nodes.contains(&edge["from"].as_str().unwrap()));
        assert!(nodes.contains(&edge["to"].as_str().unwrap()));
    }

    // 未知路径不该被这条路吞掉（Router 只有这一条）。
    let missing = app()
        .oneshot(
            Request::builder()
                .uri("/v1/modules/nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}