//! `GET /v1/openapi.json`：从路由注册表生成机器可读契约（#60 第一个接入方）。
//!
//! `wire.md` 是手写宪法，会漂；这里是生成物，进判据做集合比对。路径来自
//! `meta::ROUTES`（`router()` 也用它装配真路由，`capabilities` 也用它声明），
//! 所以「声明了没挂」由 `tests/presets_openapi.rs` 对每个路径打真 Router 来守。

use axum::{Json, Router};
use utoipa::openapi::path::{Operation, PathItem};
use utoipa::openapi::path::HttpMethod;
use utoipa::openapi::{Info, OpenApi, Paths};

use crate::meta::ROUTES;

/// 生成 presets 段的 OpenAPI 路径集合（只列 presets，其它模块沿用 `wire.md`）。
pub fn presets_openapi() -> OpenApi {
    let mut paths = Paths::new();
    for route in ROUTES.iter().filter(|route| route.module == "presets") {
        let method = match route.method {
            "GET" => HttpMethod::Get,
            "PUT" => HttpMethod::Put,
            "POST" => HttpMethod::Post,
            "PATCH" => HttpMethod::Patch,
            "DELETE" => HttpMethod::Delete,
            _ => continue,
        };
        let operation = Operation::builder()
            .summary(Some(format!("{} {}", route.method, route.path)))
            .build();
        let entry = paths.paths.entry(route.path.to_string()).or_default();
        let item = PathItem::new(method.clone(), operation);
        match method {
            HttpMethod::Get => entry.get = item.get,
            HttpMethod::Put => entry.put = item.put,
            HttpMethod::Post => entry.post = item.post,
            HttpMethod::Patch => entry.patch = item.patch,
            HttpMethod::Delete => entry.delete = item.delete,
            _ => {}
        }
    }
    OpenApi::new(Info::new("nous-presets", env!("CARGO_PKG_VERSION")), paths)
}

pub fn attach(router: Router) -> Router {
    router.route("/v1/openapi.json", axum::routing::get(openapi))
}

async fn openapi() -> Json<OpenApi> {
    Json(presets_openapi())
}
