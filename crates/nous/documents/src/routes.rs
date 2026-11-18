//! 路由注册表。端点由 server 侧的泛型 handler 注册；这里只定义**一张表**，
//! `/v1/_routes` 与真正的 router 都从它取，所以条数天然相等（测试见 server/tests/routes.rs）。

/// 模块清单里的一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModuleInfo {
    pub name: &'static str,
    pub enabled: bool,
}

/// 一条路由的声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RouteInfo {
    pub module: &'static str,
    pub method: &'static str,
    pub path: &'static str,
}

/// 一个 kind 的 10 条端点（ticket #48 表）。**路径模板只有这一份**。
pub const DOCUMENT_ROUTES: &[RouteInfo] = &[
    RouteInfo { module: "documents", method: "GET", path: "/v1/documents/{kind}" },
    RouteInfo { module: "documents", method: "GET", path: "/v1/documents/{kind}/{section}/{name}" },
    RouteInfo { module: "documents", method: "PUT", path: "/v1/documents/{kind}/{section}/{name}" },
    RouteInfo { module: "documents", method: "POST", path: "/v1/documents/{kind}/{section}" },
    RouteInfo { module: "documents", method: "POST", path: "/v1/documents/{kind}/{section}/{name}/rename" },
    RouteInfo { module: "documents", method: "POST", path: "/v1/documents/{kind}/{section}/{name}/publish" },
    RouteInfo { module: "documents", method: "POST", path: "/v1/documents/{kind}/{section}/{name}/reload" },
    RouteInfo { module: "documents", method: "GET", path: "/v1/documents/{kind}/{section}/{name}/validate" },
    RouteInfo { module: "documents", method: "GET", path: "/v1/documents/{kind}/{section}/{name}/history" },
    RouteInfo { module: "documents", method: "GET", path: "/v1/documents/{kind}/{section}/{name}/history/diff" },
    RouteInfo { module: "documents", method: "GET", path: "/v1/documents/{kind}/{section}/{name}/pending" },
    RouteInfo { module: "documents", method: "GET", path: "/v1/documents/{kind}/{section}/{name}/pending/diff" },
];

/// 自省的模块表。以后 #47 的 capabilities 可以从同一处再吐一份视图。
pub const MODULES: &[ModuleInfo] = &[
    ModuleInfo { name: "documents", enabled: true },
    ModuleInfo { name: "server", enabled: true },
];

/// `/v1/_routes` 的回包形状。
#[derive(Debug, Clone, Serialize)]
pub struct RoutesResponse {
    pub routes: Vec<RouteInfo>,
    pub modules: Vec<ModuleInfo>,
}

impl RoutesResponse {
    pub fn snapshot() -> Self {
        Self { routes: DOCUMENT_ROUTES.to_vec(), modules: MODULES.to_vec() }
    }
}

use serde::Serialize;
