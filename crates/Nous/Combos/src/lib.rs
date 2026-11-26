//! 组合插件：终极模板里每条组合（触发器 + 会话模板）的名册投影与开关写穿。
//!
//! ## 这个插件管什么
//!
//! `GET /v1/combos/:section` 每次请求都重扫盘面，`enabled` 由引擎算（`spec.enabled !== false`
//! 且引用的 trigger / session 都在）；`PATCH` 只写穿 `spec.enabled` 一位，幂等回执落
//! `<root>/.combos-receipts/`。`fire` 先问一次 `nous_dispatcher::evaluate`，判定不 fire
//! 就不建会话；`launch` 跳过判定直接起一条。两者共用同一条 launch 路径，只在 `forced`
//! 一位上分叉。
//!
//! ## 宿主能力只有三件事
//!
//! [`Host`]：两个盘面根（模板私库 / 触发器名册）、触发器定义快照、起会话 + 写血缘。
//! 插件不认识 `AppState`，也不自己开库 —— 起会话那条命令在 `nous_durable` 的
//! `session::commands`（活库写锁与补偿方向都在那里）。
//!
//! ## 两条不越过的线
//!
//! - **状态码跟着语义走**：忙 / 引用不全 = 409（「现在不行」），参数错 = 400，
//!   组合不在 = 404，版本不匹配 = 412。不在这里降级成 400。
//! - **时钟不在这里读**：`fire` 的 `now_ms` 由宿主的注入时钟给（[`Host::combo_now_ms`]），
//!   生产注入系统时钟、测试注入假时钟。

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use serde_json::{json, Value};

use nous_routes::HttpRefusal;

pub mod actions;
pub mod store;

/// 试跑判定的装配：注入时钟 + 「这条规则能不能由时钟点火」这一外部事实。
///
/// 时钟不在这里读：生产注入系统时钟、测试注入假时钟（`#82` 的既有口径）。
#[derive(Clone)]
pub struct Clock {
    pub clock: Arc<dyn nous_durable::harness::clock::Clock>,
    pub clock_allowed: bool,
}

pub use store::{ComboStore, SECTIONS};

/// 这个插件在注册表里的模块名（`docs/plugins.md` 的 module 列）。
pub const MODULE: &str = "combos";

type EndpointResult = std::result::Result<Json<Value>, HttpRefusal>;

/// 412：盘面版本号对不上（公共壳没有现成构造子，`new` + JSON 体）。
pub(crate) fn fn_precondition(message: impl Into<String>) -> HttpRefusal {
    HttpRefusal::new(412, json!({ "error": message.into() }))
}

/// 503：宿主没装配（触发器运行时 / 注入时钟），端点直说，不假装判定过。
pub(crate) fn fn_service_unavailable(message: impl Into<String>) -> HttpRefusal {
    HttpRefusal::new(503, json!({ "error": message.into() }))
}

/// 本插件的端点表。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        nous_routes::Route::new("GET", "/v1/combos/{section}", MODULE, || {
            get(actions::list::<H>)
        }),
        nous_routes::Route::new("PATCH", "/v1/combos/{section}/{name}", MODULE, || {
            axum::routing::patch(actions::patch::<H>)
        }),
        nous_routes::Route::new("POST", "/v1/combos/{section}/{name}/fire", MODULE, || {
            post(actions::fire::<H>)
        }),
        // 终极模板的手动 launch 挂在 presets 前缀下（客户端照旧），归属仍是 combos。
        nous_routes::Route::new("POST", "/v1/presets/Ultimate-Template/launch", MODULE, || {
            post(actions::launch_handler::<H>)
        }),
    ]
}

/// 宿主未装配时的统一拒绝（搬家前是 `409 {"error":"combos not configured"}`）。
pub(crate) fn store_of<H: Host + ?Sized>(host: &H) -> Result<ComboStore, HttpRefusal> {
    host.combo_roots()
        .ok_or_else(|| HttpRefusal::conflict("combos not configured"))
}

/// 内容版本号：`v` + sha256 前 12 个 hex。与搬家前 `version::sha256` 逐字同源。
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    // 复用 `nous_repo` 的 sha256（git 面里已经有一份，不另造第二套）。
    nous_repo::sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
