//! 本插件唯一的一条端点：`GET /v1/modules/graph`。
//!
//! 声明与 handler 在同一处（[`routes`] 里那条 `Route` 指向下面的函数），所以
//! 「注册表里写着有」和「真能打到」不会分叉（`docs/plugins.md` 规则 11）。
//!
//! 为什么要一个有状态的插件：没有。handler 返回的是编译期内嵌的静态表投影
//! （[`crate::graph`]），不读盘、不碰 `AppState`、不看时钟 —— 所以 `routes()` 对
//! 状态类型只有 `Clone + Send + Sync` 这一个要求，宿主给什么都能装。

use axum::routing::get;
use axum::Json;
use nous_routes::Route;
use serde_json::Value;

/// 这条端点的路径（axum 写法）。注册表里显示成 `/v1/modules/graph`（没有参数）。
pub const PATH: &str = "/v1/modules/graph";

/// `GET /v1/modules/graph`：整张依赖图一次给全。
///
/// 不分页、不按模块取：整张图 30 来个模块、几十条边（几十 KB 以内），一次给完比
/// 「先拿列表再逐个查依赖」少 30 次往返，也不会出现「查了 A 之后 B 变了」的半个视图。
pub async fn graph_handler() -> Json<Value> {
    Json(crate::graph())
}

pub fn routes<S>() -> Vec<Route<S>>
where
    S: Clone + Send + Sync + 'static,
{
    vec![Route::new("GET", PATH, crate::MODULE, || get(graph_handler))]
}