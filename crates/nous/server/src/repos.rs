//! #75：`/v1/repos/:repo/*` 的统一端点（四套私库共用一套）。
//!
//! 这里只做 HTTP ↔ `nous_repo` 的翻译：错误码映射、查询参数、JSON 形状。
//! 文件 CRUD 与 git 语义都在 `nous_repo`（platform 层），这里不碰进程。

use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::{Json, Router};
use nous_repo::{Repo, RepoError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::ServerError;

/// 四套私库的名字。`:repo` 不是任意字符串，未知值必须 400。
pub const REPO_KINDS: &[&str] = &["presets", "skills", "prompts", "extend"];

#[derive(Debug, Clone, Deserialize)]
pub struct CommitBody {
    pub paths: Vec<String>,
    pub message: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NameBody {
    pub name: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DiffQuery {
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CommitQuery {
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HistoryQuery {
    pub name: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HistoryDiffQuery {
    pub name: Option<String>,
    pub hash: Option<String>,
    pub path: Option<String>,
}

/// 把私库错误映射成 wire 的 `{error}`；409 额外带磁盘当前正文（票面要求）。
pub(crate) fn repo_error(error: RepoError) -> ServerError {
    match error {
        RepoError::BadRequest(m) | RepoError::InvalidPath(m) => ServerError::BadRequest(m),
        RepoError::Validation { field, problem } => ServerError::Validation { field, problem },
        RepoError::NotFound(m) => ServerError::NotFound(m),
        RepoError::Busy(m) => ServerError::Conflict(m),
        RepoError::Conflict { message, current, etag } => {
            let mut value = json!({ "error": message });
            if let Some(content) = current {
                value["content"] = json!(String::from_utf8_lossy(&content).into_owned());
            }
            if let Some(etag) = etag { value["etag"] = json!(etag); }
            // 用统一的 409 响应，但把字段带出去。
            ServerError::Conflict(serde_json::to_string(&value).unwrap())
        }
        RepoError::Io(m) => ServerError::Internal(m),
        RepoError::LockPoisoned => ServerError::Internal("私库写锁中毒".into()),
    }
}

pub(crate) fn select(state: &AppState, kind: &str) -> std::result::Result<Arc<Repo>, ServerError> {
    if !REPO_KINDS.contains(&kind) {
        return Err(ServerError::BadRequest(format!("未知私库：{kind}")));
    }
    state.repo(kind).ok_or_else(|| ServerError::Internal(format!("私库未装配：{kind}")))
}

fn json_result<T: serde::Serialize>(result: std::result::Result<T, RepoError>) -> std::result::Result<Json<Value>, ServerError> {
    let value = serde_json::to_value(result.map_err(repo_error)?).unwrap_or(Value::Null);
    Ok(Json(value))
}

pub fn attach(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/v1/repos/{repo}/status", axum::routing::get(status))
        .route("/v1/repos/{repo}/pending", axum::routing::get(pending))
        .route("/v1/repos/{repo}/pending/diff", axum::routing::get(pending_diff))
        .route("/v1/repos/{repo}/pending/commit", axum::routing::get(pending_commit))
        .route("/v1/repos/{repo}/commit", axum::routing::post(commit))
        .route("/v1/repos/{repo}/history", axum::routing::get(history))
        .route("/v1/repos/{repo}/history/diff", axum::routing::get(history_diff))
        .route("/v1/repos/{repo}/archive", axum::routing::post(archive))
        .route("/v1/repos/{repo}/restore", axum::routing::post(restore))
        .route("/v1/repos/{repo}/purge", axum::routing::post(purge))
}

async fn status(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>) -> std::result::Result<Json<Value>, ServerError> {
    let repo = select(&state, &kind)?;
    json_result(repo.status())
}

async fn pending(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>) -> std::result::Result<Json<Value>, ServerError> {
    let repo = select(&state, &kind)?;
    let units = repo.pending().map_err(repo_error)?;
    Ok(Json(json!({ "units": units })))
}

async fn pending_diff(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>, Query(q): Query<DiffQuery>) -> std::result::Result<Json<Value>, ServerError> {
    let repo = select(&state, &kind)?;
    json_result(repo.pending_diff(&q.path.unwrap_or_default()))
}

async fn pending_commit(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>, Query(q): Query<CommitQuery>) -> std::result::Result<Json<Value>, ServerError> {
    let repo = select(&state, &kind)?;
    let hash = q.hash.unwrap_or_default();
    json_result(repo.pending_commit(&hash))
}

async fn commit(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>, Json(body): Json<CommitBody>) -> std::result::Result<Json<Value>, ServerError> {
    let repo = select(&state, &kind)?;
    json_result(repo.commit(&body.paths, &body.message, body.request_id.as_deref()))
}

async fn history(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>, Query(q): Query<HistoryQuery>) -> std::result::Result<Json<Value>, ServerError> {
    let repo = select(&state, &kind)?;
    let items = repo.history(&q.name.unwrap_or_default(), q.limit.unwrap_or(50)).map_err(repo_error)?;
    Ok(Json(json!({ "items": items })))
}

async fn history_diff(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>, Query(q): Query<HistoryDiffQuery>) -> std::result::Result<Json<Value>, ServerError> {
    let repo = select(&state, &kind)?;
    json_result(repo.history_diff(&q.name.unwrap_or_default(), &q.hash.unwrap_or_default(), &q.path.unwrap_or_default()))
}

async fn archive(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>, Json(body): Json<NameBody>) -> std::result::Result<Json<Value>, ServerError> {
    lifecycle(select(&state, &kind)?, &body, |r| r.archive(&body.name, body.request_id.as_deref()))
}

async fn restore(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>, Json(body): Json<NameBody>) -> std::result::Result<Json<Value>, ServerError> {
    lifecycle(select(&state, &kind)?, &body, |r| r.restore(&body.name, body.request_id.as_deref()))
}

async fn purge(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>, Json(body): Json<NameBody>) -> std::result::Result<Json<Value>, ServerError> {
    lifecycle(select(&state, &kind)?, &body, |r| r.purge(&body.name, body.request_id.as_deref()))
}

fn lifecycle<F>(repo: Arc<Repo>, _body: &NameBody, f: F) -> std::result::Result<Json<Value>, ServerError>
where F: FnOnce(&Repo) -> std::result::Result<nous_repo::LifecycleResult, RepoError> {
    json_result(f(&repo))
}

/// 生产装配：从 `nous_env` 的四条路径解析（不另读环境变量）。
pub fn from_env() -> std::collections::BTreeMap<String, Arc<Repo>> {
    let mut map = std::collections::BTreeMap::new();
    for (name, path) in [
        ("presets", nous_env::PRESETS.resolve()),
        ("skills", nous_env::SKILLS.resolve()),
        ("prompts", nous_env::PROMPTS.resolve()),
        ("extend", nous_env::EXTEND.resolve()),
    ] {
        if let Ok(repo) = Repo::open(&path) {
            map.insert(name.to_string(), Arc::new(repo));
        }
    }
    map
}

/// 测试装配：显式给一个仓（只用于进程内 Router 测试）。
pub fn from_path(kind: &str, root: &Path) -> std::collections::BTreeMap<String, Arc<Repo>> {
    let mut map = std::collections::BTreeMap::new();
    if let Ok(repo) = Repo::open(root) { map.insert(kind.to_string(), Arc::new(repo)); }
    map
}
