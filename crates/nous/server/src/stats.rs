//! #62：三条 stats 端点的 HTTP 装配（只读）。
//!
//! 聚合逻辑在 `data-data-manager`；这里只做三件事：解析查询参数、取库路径、把 `{error}` 变成
//! 语义状态码。库路径来自 `AppState` 的 `stats` 装配（生产由 `main` 注入 `NOUS_DB` 与
//! `plugins_db_path()` 的同一套推导），没有装配时回 409，不假装空数据。

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::{Result, ServerError};

/// 只读句柄：两个库路径，没有连接、没有缓存。
#[derive(Debug, Clone)]
pub struct StatsRuntime {
    pub durable_db: PathBuf,
    pub plugins_db: PathBuf,
}

fn runtime(state: &AppState) -> Result<&StatsRuntime> {
    state.stats().ok_or_else(|| ServerError::Conflict("stats not configured".to_string()))
}

/// `?from=&to=&group_by=`：epoch 毫秒，闭开区间 `[from, to)`；缺省全量。
#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub group_by: Option<String>,
}

impl UsageQuery {
    fn window(&self) -> Result<data_data_manager::UsageWindow> {
        if let (Some(from), Some(to)) = (self.from, self.to) {
            if from >= to {
                return Err(ServerError::BadRequest("from 必须小于 to".to_string()));
            }
        }
        Ok(data_data_manager::UsageWindow { from: self.from, to: self.to })
    }

    fn group_by(&self) -> Result<data_data_manager::GroupBy> {
        let raw = self.group_by.as_deref().unwrap_or("day");
        data_data_manager::GroupBy::parse(raw).ok_or_else(|| {
            ServerError::BadRequest(format!("group_by 只能是 day|model|conversation，收到 {raw:?}"))
        })
    }
}

/// `GET /v1/stats/usage`。
pub async fn usage(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<Value>> {
    let runtime = runtime(&state)?;
    let window = query.window()?;
    let group_by = query.group_by()?;
    data_data_manager::usage(&runtime.durable_db, window, group_by)
        .map(Json)
        .map_err(|error| ServerError::Internal(error.to_string()))
}

/// `GET /v1/stats/tasks`。
pub async fn tasks(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let runtime = runtime(&state)?;
    data_data_manager::tasks(&runtime.durable_db)
        .map(Json)
        .map_err(|error| ServerError::Internal(error.to_string()))
}

/// `GET /v1/stats/db`。
pub async fn db(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let runtime = runtime(&state)?;
    data_data_manager::db(&runtime.durable_db, &runtime.plugins_db)
        .map(Json)
        .map_err(|error| ServerError::Internal(error.to_string()))
}

/// `GET /v1/stats/overview`：durable 库的 X 光片（解剖屏）。
pub async fn overview(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let runtime = runtime(&state)?;
    data_data_manager::overview(&runtime.durable_db)
        .map(Json)
        .map_err(|error| ServerError::Internal(error.to_string()))
}

/// `GET /v1/stats/conversations`：会话树（会话树屏）。
pub async fn conversations(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let runtime = runtime(&state)?;
    data_data_manager::conversations(&runtime.durable_db)
        .map(Json)
        .map_err(|error| ServerError::Internal(error.to_string()))
}

/// `GET /v1/stats/conversation/:id`：一条会话的明细（回放屏，含 prompt 重建）。
pub async fn conversation(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Value>> {
    let runtime = runtime(&state)?;
    let id: i64 = id
        .parse()
        .map_err(|_| ServerError::BadRequest(format!("session id is not a number: {id:?}")))?;
    match data_data_manager::conversation(&runtime.durable_db, id)
        .map_err(|error| ServerError::Internal(error.to_string()))?
    {
        Some(value) => Ok(Json(value)),
        None => Err(ServerError::NotFound(format!("session {id} not found"))),
    }
}

/// `GET /v1/stats/cost`：账单屏（四组桶 + note）。
pub async fn cost(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let runtime = runtime(&state)?;
    data_data_manager::cost(&runtime.durable_db)
        .map(Json)
        .map_err(|error| ServerError::Internal(error.to_string()))
}

/// `GET /v1/stats/health`：维修屏（任务 / 提交 / 文档 / WAL / 空闲页 / 工单）。
pub async fn health(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let runtime = runtime(&state)?;
    data_data_manager::health(&runtime.durable_db, now_ms())
        .map(Json)
        .map_err(|error| ServerError::Internal(error.to_string()))
}

/// 健康屏的「卡住」判据要一个时钟；时间由端点注入（`data-data-manager` 层零时钟）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// `GET /v1/openapi.json`：从 `meta::ROUTES` 生成的只读机读视图。
pub async fn openapi() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": { "title": "nous /v1", "version": env!("CARGO_PKG_VERSION") },
        "paths": crate::meta::openapi_paths(),
    }))
}
