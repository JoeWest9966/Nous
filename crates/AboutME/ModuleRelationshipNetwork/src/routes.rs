//! 本插件的两条端点：`GET /v1/modules/graph`（依赖图）与 `GET /v1/modules/updates`
//! （当前版本 vs 仓库最新版本）。
//!
//! 声明与 handler 在同一处（[`routes`] 里那条 `Route` 指向下面的函数），所以
//! 「注册表里写着有」和「真能打到」不会分叉（`docs/plugins.md` 规则 11）。
//!
//! 为什么不需要有状态的宿主：graph 返回的是编译期内嵌的静态表投影（[`crate::graph`]）；
//! updates 的「当前版本」同样来自编译期表，只有「最新版本」要一次外网往返（超时 3s、
//! 缓存 6h）。所以 `routes()` 对状态类型只有 `Clone + Send + Sync` 这一个要求，
//! 宿主给什么都能装。

use axum::extract::Query;
use axum::routing::get;
use axum::Json;
use nous_routes::Route;
use serde::Deserialize;
use serde_json::Value;

/// 依赖图的路径（axum 写法）。注册表里显示成 `/v1/modules/graph`（没有参数）。
pub const PATH: &str = "/v1/modules/graph";

/// 版本对照的路径。
pub const PATH_UPDATES: &str = "/v1/modules/updates";

/// `GET /v1/modules/graph`：整张依赖图一次给全。
///
/// 不分页、不按模块取：整张图 30 来个模块、几十条边（几十 KB 以内），一次给完比
/// 「先拿列表再逐个查依赖」少 30 次往返，也不会出现「查了 A 之后 B 变了」的半个视图。
pub async fn graph_handler() -> Json<Value> {
    Json(crate::graph())
}

/// `?refresh=1` 才穿透缓存；别的值一律当「不刷新」。
#[derive(Debug, Default, Deserialize)]
pub struct RefreshQuery {
    #[serde(default)]
    pub refresh: Option<String>,
}

/// `GET /v1/modules/updates[?refresh=1]`：一次给全（当前版本 + 仓库最新版本）。
///
/// 上游挂了**也是 200**（`available` / `stale` / `error` 说清楚），见 [`crate::updates`]。
pub async fn updates_handler(Query(query): Query<RefreshQuery>) -> Json<Value> {
    let refresh = crate::updates::wants_refresh(query.refresh.as_deref());
    Json(crate::updates::handle(refresh).await)
}

pub fn routes<S>() -> Vec<Route<S>>
where
    S: Clone + Send + Sync + 'static,
{
    vec![
        Route::new("GET", PATH, crate::MODULE, || get(graph_handler)),
        Route::new("GET", PATH_UPDATES, crate::MODULE, || get(updates_handler)),
    ]
}
