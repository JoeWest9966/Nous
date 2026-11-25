//! #77：`/v1/repos/{repo}/sources` 与 `/v1/repos/{repo}/upstream/*` 的 HTTP 面。
//!
//! 只做 HTTP ↔ `nous_repo::Upstream` 翻译：`:repo` 校验、查询参数、JSON 形状。
//! 所有 git 进程调用仍在 `nous-repo`（platform 层）。

use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::{Json, Router};
use nous_repo::{Source, Upstream};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ServerError;
use crate::repos::{repo_error, select, REPO_KINDS};

#[derive(Debug, Deserialize)]
pub struct SourcesBody {
    sources: Vec<Source>,
}

#[derive(Debug, Deserialize, Default)]
pub struct NameQuery {
    name: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct NameBody {
    name: String,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    to: Option<String>,
    /// 协议字段：客户端可带，服务端只接收不解释（AGENTS.md「协议是哑的」）。
    #[allow(dead_code)]
    #[serde(default)]
    request_id: Option<String>,
}

/// 上游只挂在 `skills` 私库上：sources.json 住技能仓，镜像来自 data/upstream。
fn upstream(state: &AppState, kind: &str) -> Result<Upstream, ServerError> {
    if kind != "skills" {
        return Err(ServerError::BadRequest(format!("上游追踪只支持 skills，不支持 {kind}")));
    }
    let repo = select(state, kind)?;
    let data = state.upstream_data().ok_or_else(|| {
        ServerError::Conflict("没有 data 根：上游镜像无处落盘（设 NOUS_DATA）".into())
    })?;
    std::fs::create_dir_all(&data).map_err(|e| ServerError::Internal(e.to_string()))?;
    Upstream::open(repo.root(), data).map_err(repo_error)
}

fn json_result<T: serde::Serialize>(result: Result<T, nous_repo::RepoError>) -> Result<Json<Value>, ServerError> {
    Ok(Json(serde_json::to_value(result.map_err(repo_error)?).unwrap_or(Value::Null)))
}

pub fn attach(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/v1/repos/{repo}/sources", axum::routing::get(sources).post(write_sources))
        .route("/v1/repos/{repo}/upstream", axum::routing::get(overview))
        .route("/v1/repos/{repo}/upstream/refresh", axum::routing::post(refresh))
        .route("/v1/repos/{repo}/upstream/pin", axum::routing::post(pin))
        .route("/v1/repos/{repo}/upstream/check", axum::routing::post(check))
        .route("/v1/repos/{repo}/upstream/sync", axum::routing::post(sync))
        .route("/v1/repos/{repo}/upstream/tree", axum::routing::get(tree))
        .route("/v1/repos/{repo}/upstream/file", axum::routing::get(file))
        .route("/v1/repos/{repo}/upstream/files", axum::routing::get(files))
        .route("/v1/repos/{repo}/upstream/copy", axum::routing::post(copy))
        .route("/v1/repos/{repo}/upstream/adopt", axum::routing::post(adopt))
}

pub async fn sources(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    Ok(Json(up.sources_doc().map_err(repo_error)?))
}

pub async fn write_sources(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<SourcesBody>,
) -> Result<Json<Value>, ServerError> {
    if !REPO_KINDS.contains(&kind.as_str()) {
        return Err(ServerError::BadRequest(format!("未知私库：{kind}")));
    }
    let up = upstream(&state, &kind)?;
    let sources = up.write_sources(&body.sources).map_err(repo_error)?;
    Ok(Json(json!({ "ok": true, "sources": sources })))
}

pub async fn overview(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    let (sources, errors) = up.status_with_errors();
    Ok(Json(json!({ "sources": sources, "errors": errors })))
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    let head = up.refresh(&body.name).map_err(repo_error)?;
    Ok(Json(json!({ "ok": true, "name": body.name, "head": head })))
}

pub async fn pin(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    let commit = up.pin(&body.name, body.commit.as_deref()).map_err(repo_error)?;
    Ok(Json(json!({ "ok": true, "name": body.name, "commit": commit })))
}

pub async fn check(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    body: Option<Json<NameBody>>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    let name = body.and_then(|Json(body)| if body.name.is_empty() { None } else { Some(body.name) });
    let checks = up.check(name.as_deref()).map_err(repo_error)?;
    Ok(Json(json!({ "ok": true, "checks": checks })))
}

pub async fn sync(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    let head = up.sync(&body.name).map_err(repo_error)?;
    Ok(Json(json!({ "ok": true, "name": body.name, "head": head })))
}

pub async fn tree(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<NameQuery>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    let items = up.tree_items(
        q.name.as_deref().unwrap_or_default(),
        q.path.as_deref().unwrap_or_default(),
    ).map_err(repo_error)?;
    Ok(Json(json!({ "items": items })))
}

pub async fn file(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<NameQuery>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    json_result(up.file(
        q.name.as_deref().unwrap_or_default(),
        q.path.as_deref().unwrap_or_default(),
    ))
}

pub async fn files(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<NameQuery>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    let files = up.compare(
        q.name.as_deref().unwrap_or_default(),
        q.path.as_deref().unwrap_or_default(),
    ).map_err(repo_error)?;
    Ok(Json(json!({ "files": files })))
}

pub async fn copy(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    json_result(up.adopt(
        &body.name,
        body.path.as_deref().unwrap_or_default(),
        body.to.as_deref(),
        false,
    ))
}

pub async fn adopt(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> Result<Json<Value>, ServerError> {
    let up = upstream(&state, &kind)?;
    json_result(up.adopt(
        &body.name,
        body.path.as_deref().unwrap_or_default(),
        body.to.as_deref(),
        true,
    ))
}
