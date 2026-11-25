//! 本插件自己的 8 条只读端点：`GET /v1/stats/{usage,tasks,db,overview,conversations,
//! conversation/:id,cost,health}`。
//!
//! Magento 2 那种「模块自带 controller」：handler 住在插件里，插件不认识 `server` 的
//! `AppState`，只声明它需要什么（[`Host`]：两个库路径），由宿主实现并注入。
//!
//! 搬家前后行为一致的三条：
//!
//! - **没装配就直说**：宿主没给库路径 → `409 {"error":"stats not configured"}`，
//!   不假装空数据（空库与没库是两件事，空库判据是「三条端点都 200」）。
//! - **参数错就 400，且说明白**：`from >= to`、`group_by` 不在闭集合、`id` 不是数字。
//! - **只读**：这里一次写都不做（写面根本不归这个插件）。
//!
//! 时钟只在这一层取：聚合层零时钟，`health` 的「卡住」判据要一个「现在」。

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use nous_routes::Route;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{GroupBy, UsageWindow};

/// 这个插件在注册表里的模块名（`docs/plugins.md` 的 module 列）。
pub const MODULE: &str = "data-manager";

/// 宿主能力：两个只读库在哪。`None` = 没装配（端点回 409，不假装空数据）。
///
/// 只给路径、不给连接：插件自己按「只读」打开，写操作在 SQLite 层就失败。
pub trait Host {
    fn stats_paths(&self) -> Option<Paths>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn stats_paths(&self) -> Option<Paths> {
        (**self).stats_paths()
    }
}

/// 只读句柄：两个库路径，没有连接、没有缓存。
#[derive(Debug, Clone)]
pub struct Paths {
    /// durable 库（`nous.db`）：会话 / 任务 / 提交 / 文档。
    pub durable: PathBuf,
    /// plugins 库：`triggers` / `fires`。
    pub plugins: PathBuf,
}

/// 一条端点的拒绝理由。状态码与文案与搬家前逐字一致。
#[derive(Debug)]
pub enum Refusal {
    /// 宿主没装配库路径。
    NotConfigured,
    /// 参数不合法。
    BadRequest(String),
    /// 查得到但不存在。
    NotFound(String),
    /// 读库失败（原文带路径，能贴进 issue）。
    Internal(String),
}

impl Refusal {
    fn into_response(self) -> (StatusCode, Json<Value>) {
        let (status, message) = match self {
            Self::NotConfigured => (StatusCode::CONFLICT, "stats not configured".to_string()),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(json!({ "error": message })))
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }
}

type EndpointResult = std::result::Result<Json<Value>, Refusal>;

fn paths<H: Host>(host: &H) -> std::result::Result<Paths, Refusal> {
    host.stats_paths().ok_or(Refusal::NotConfigured)
}

/// `?from=&to=&group_by=`：epoch 毫秒，闭开区间 `[from, to)`；缺省全量。
#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub group_by: Option<String>,
}

impl UsageQuery {
    fn window(&self) -> std::result::Result<UsageWindow, Refusal> {
        if let (Some(from), Some(to)) = (self.from, self.to) {
            if from >= to {
                return Err(Refusal::BadRequest("from 必须小于 to".to_string()));
            }
        }
        Ok(UsageWindow { from: self.from, to: self.to })
    }

    fn group_by(&self) -> std::result::Result<GroupBy, Refusal> {
        let raw = self.group_by.as_deref().unwrap_or("day");
        GroupBy::parse(raw).ok_or_else(|| {
            Refusal::BadRequest(format!(
                "group_by 只能是 day|model|conversation，收到 {raw:?}"
            ))
        })
    }
}

/// `GET /v1/stats/usage`：按 `day` / `model` / `conversation` 归桶的用量。
async fn usage<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Query(query): Query<UsageQuery>,
) -> EndpointResult {
    let paths = paths(&host)?;
    let window = query.window()?;
    let group_by = query.group_by()?;
    crate::usage(&paths.durable, window, group_by)
        .map(Json)
        .map_err(Refusal::internal)
}

/// `GET /v1/stats/tasks`：任务按状态计数 + 最近样本。
async fn tasks<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>) -> EndpointResult {
    let paths = paths(&host)?;
    crate::tasks(&paths.durable).map(Json).map_err(Refusal::internal)
}

/// `GET /v1/stats/db`：两个库的字节 / 行数 / quickCheck / schema。
async fn db<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>) -> EndpointResult {
    let paths = paths(&host)?;
    crate::db(&paths.durable, &paths.plugins)
        .map(Json)
        .map_err(Refusal::internal)
}

/// `GET /v1/stats/overview`：durable 库的 X 光片（解剖屏）。
async fn overview<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>) -> EndpointResult {
    let paths = paths(&host)?;
    crate::overview(&paths.durable).map(Json).map_err(Refusal::internal)
}

/// `GET /v1/stats/conversations`：会话树。
async fn conversations<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>) -> EndpointResult {
    let paths = paths(&host)?;
    crate::conversations(&paths.durable)
        .map(Json)
        .map_err(Refusal::internal)
}

/// `GET /v1/stats/conversation/:id`：一条会话的明细（回放屏，含 prompt 重建）。
async fn conversation<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path(id): Path<String>,
) -> EndpointResult {
    let paths = paths(&host)?;
    let id: i64 = id
        .parse()
        .map_err(|_| Refusal::BadRequest(format!("session id is not a number: {id:?}")))?;
    match crate::conversation(&paths.durable, id).map_err(Refusal::internal)? {
        Some(value) => Ok(Json(value)),
        None => Err(Refusal::NotFound(format!("session {id} not found"))),
    }
}

/// `GET /v1/stats/cost`：账单屏（四组桶 + note）。
async fn cost<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>) -> EndpointResult {
    let paths = paths(&host)?;
    crate::cost(&paths.durable).map(Json).map_err(Refusal::internal)
}

/// `GET /v1/stats/health`：维修屏（任务 / 提交 / 文档 / WAL / 空闲页 / 工单）。
async fn health<H: Host + Clone + Send + Sync + 'static>(State(host): State<H>) -> EndpointResult {
    let paths = paths(&host)?;
    crate::health(&paths.durable, now_ms())
        .map(Json)
        .map_err(Refusal::internal)
}

/// 健康屏的「卡住」判据要一个时钟；时间由端点注入（聚合层零时钟）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// 本插件的端点表。路径与搬家前逐字一致（`wire.md` 与 Desk 都不动）。
pub fn routes<H>() -> Vec<Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        Route::new("GET", "/v1/stats/usage", MODULE, || get(usage::<H>)),
        Route::new("GET", "/v1/stats/tasks", MODULE, || get(tasks::<H>)),
        Route::new("GET", "/v1/stats/db", MODULE, || get(db::<H>)),
        Route::new("GET", "/v1/stats/overview", MODULE, || get(overview::<H>)),
        Route::new("GET", "/v1/stats/conversations", MODULE, || {
            get(conversations::<H>)
        }),
        Route::new("GET", "/v1/stats/conversation/{id}", MODULE, || {
            get(conversation::<H>)
        }),
        Route::new("GET", "/v1/stats/cost", MODULE, || get(cost::<H>)),
        Route::new("GET", "/v1/stats/health", MODULE, || get(health::<H>)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假宿主：只给两条路径，没有别的能力。
    #[derive(Clone)]
    struct FakeHost {
        paths: Option<Paths>,
    }

    impl Host for FakeHost {
        fn stats_paths(&self) -> Option<Paths> {
            self.paths.clone()
        }
    }

    fn empty_paths(dir: &std::path::Path) -> Paths {
        Paths {
            durable: dir.join("nous.db"),
            plugins: dir.join("plugins.db"),
        }
    }

    /// 注册表里的形状：8 条、路径与模块名都对得上（`:id` 写法由 `nous-routes` 折）。
    #[test]
    fn routes_declare_the_eight_stats_endpoints_under_this_module() {
        let declared: Vec<String> = routes::<FakeHost>()
            .iter()
            .map(|route| format!("{} [{}]", route.declared(), route.module()))
            .collect();
        assert_eq!(
            declared,
            vec![
                "GET /v1/stats/usage [data-manager]",
                "GET /v1/stats/tasks [data-manager]",
                "GET /v1/stats/db [data-manager]",
                "GET /v1/stats/overview [data-manager]",
                "GET /v1/stats/conversations [data-manager]",
                "GET /v1/stats/conversation/:id [data-manager]",
                "GET /v1/stats/cost [data-manager]",
                "GET /v1/stats/health [data-manager]",
            ]
        );
    }

    /// 没装配 → 409 且文案是那句熟话（不是空 200，也不是 404）。
    #[tokio::test]
    async fn an_unassembled_host_is_a_conflict_not_empty_data() {
        let refusal = paths(&FakeHost { paths: None }).unwrap_err();
        let (status, body) = refusal.into_response();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body.0, json!({ "error": "stats not configured" }));
    }

    /// `group_by` 是闭集合；`from >= to` 直接 400，不悄悄换个区间。
    #[test]
    fn query_rules_reject_bad_input_with_a_400() {
        let bad_group = UsageQuery { from: None, to: None, group_by: Some("hour".into()) };
        let (status, body) = bad_group.group_by().unwrap_err().into_response();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body.0,
            json!({ "error": "group_by 只能是 day|model|conversation，收到 \"hour\"" })
        );

        let bad_window = UsageQuery { from: Some(10), to: Some(10), group_by: None };
        let (status, body) = bad_window.window().unwrap_err().into_response();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body.0, json!({ "error": "from 必须小于 to" }));
    }

    /// 空库也是 200：库存在但没数据，不是「坏了」，也不是「没装配」。
    #[tokio::test]
    async fn an_empty_database_answers_200_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let paths = empty_paths(dir.path());
        drop(nous_chord::sqlite::LocalDatabase::open(&paths.durable).unwrap());
        // plugins 库不存在也算空库（`db` 端点自己按 0 报告）。
        let host = FakeHost { paths: Some(paths) };
        let body = db(State(host)).await.unwrap();
        assert!(body.0.get("durable").is_some(), "durable 一节必须在");
    }
}
