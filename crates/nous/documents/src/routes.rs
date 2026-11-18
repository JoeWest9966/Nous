//! documents 端点的路径模板注册表：server 侧的泛型 handler 按这张表注册路由。
//! 对外自省（`/v1/_routes` / capabilities）由 `nous-server::meta` 的唯一注册表负责；
//! 这张表是它在 documents 一族上的注册源，测试守住两者不漂移（不手抄第二份视图）。

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

use serde::Serialize;
