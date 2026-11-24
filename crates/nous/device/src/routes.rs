//! device 插件自己的端点：`GET /v1/machines`。
//!
//! 声明与 handler 在同一处（[`routes`] 里那条 `Route` 指向下面的函数），所以
//! 「注册表里写着有」和「真能打到」不会再有机会分叉。

use axum::routing::get;
use axum::Json;

/// 机器编目：机器 + 注解 + 能力字段 + 兼容 target 列表。
///
/// `NOUS_MACHINES_FILE` 给了就用它（读不出来退回内置表），没给就是内置表。
/// 路径解析归宿主，这里只认一个「已经解好的文件路径」。
pub async fn machines() -> Json<serde_json::Value> {
    let file = std::env::var("NOUS_MACHINES_FILE")
        .ok()
        .filter(|text| !text.trim().is_empty())
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_file());
    Json(crate::machines(file.as_deref()))
}

/// device 插件的端点表。`S` 是宿主的状态类型：这条路由不读状态，所以与 `S` 无关。
pub fn routes<S>() -> Vec<nous_routes::Route<S>>
where
    S: Clone + Send + Sync + 'static,
{
    vec![nous_routes::Route::new("GET", "/v1/machines", "device", || {
        get(machines)
    })]
}
