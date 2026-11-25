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

/// 六套私库的名字。`:repo` 不是任意字符串，未知值必须 400。
pub const REPO_KINDS: &[&str] = &["presets", "skills", "prompts", "extend", "triggers", "elements"];

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

#[derive(Debug, Clone, Deserialize)]
pub struct VariantIdBody {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VariantBindBody {
    pub id: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VariantBranchBody {
    pub id: String,
    pub branch: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VariantPublishBody {
    pub id: String,
    pub message: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VariantPruneBody {
    pub id: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VariantUsageQuery {
    pub id: String,
}

/// 把私库错误映射成 wire 的 `{error}`；409 额外带磁盘当前正文（票面要求）。
pub(crate) fn repo_error(error: RepoError) -> ServerError {
    match error {
        RepoError::BadRequest(m) | RepoError::InvalidPath(m) => ServerError::BadRequest(m),
        RepoError::Validation { field, problem } => ServerError::Validation { field, problem },
        RepoError::NotFound(m) => ServerError::NotFound(m),
        RepoError::Busy(m) => ServerError::Conflict(m),
        RepoError::Locked(m) => ServerError::Locked(m),
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
        .route("/v1/repos/{repo}/archive", axum::routing::get(archived).post(archive))
        .route("/v1/repos/{repo}/restore", axum::routing::post(restore))
        .route("/v1/repos/{repo}/purge", axum::routing::post(purge))
        // #76：变体（只给 skills / extend；其余四值回 400）。
        .route("/v1/repos/{repo}/variants", axum::routing::get(variants))
        .route("/v1/repos/{repo}/variants/bind", axum::routing::post(variant_bind))
        .route("/v1/repos/{repo}/variants/unbind", axum::routing::post(variant_unbind))
        .route("/v1/repos/{repo}/variants/branch", axum::routing::post(variant_branch))
        .route("/v1/repos/{repo}/variants/publish", axum::routing::post(variant_publish))
        .route("/v1/repos/{repo}/variants/prune", axum::routing::post(variant_prune))
        .route("/v1/repos/{repo}/variants/usage", axum::routing::get(variant_usage))
}

/// 变体只给技能 / 扩展；其余私库（presets / prompts）回 400。
fn variant_kind(kind: &str) -> std::result::Result<(&'static str, nous_repo::VariantFamily), ServerError> {
    match kind {
        "skills" => Ok(("skills", nous_repo::VariantFamily::Skill)),
        "extend" => Ok(("extend", nous_repo::VariantFamily::Extension)),
        other if REPO_KINDS.contains(&other) => Err(ServerError::BadRequest(format!("私库 {other} 没有变体"))),
        other => Err(ServerError::BadRequest(format!("未知私库：{other}"))),
    }
}

fn table_path(state: &AppState, kind: &str) -> std::result::Result<std::path::PathBuf, ServerError> {
    state.variant_table(kind).ok_or_else(|| ServerError::Conflict("变体绑定表未装配".into()))
}

async fn variants(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
) -> std::result::Result<Json<Value>, ServerError> {
    let (kind, _) = variant_kind(&kind)?;
    let repo = select(&state, kind)?;
    json_result(repo.variants(&table_path(&state, kind)?))
}

async fn variant_bind(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantBindBody>,
) -> std::result::Result<Json<Value>, ServerError> {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&state, kind)?;
    let paths: Vec<&str> = body.paths.iter().map(String::as_str).collect();
    json_result(repo.bind(family, &table_path(&state, kind)?, nous_repo::BindVariant {
        id: &body.id, paths: &paths,
    }))
}

async fn variant_unbind(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantIdBody>,
) -> std::result::Result<Json<Value>, ServerError> {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&state, kind)?;
    let result = repo.unbind(family, &table_path(&state, kind)?, &body.id).map_err(repo_error)?;
    let _ = result;
    Ok(Json(json!({ "id": body.id, "removed": true })))
}

async fn variant_branch(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantBranchBody>,
) -> std::result::Result<Json<Value>, ServerError> {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&state, kind)?;
    let table = table_path(&state, kind)?;
    let branch = repo.switch_branch(family, &table, &body.id, &body.branch).map_err(repo_error)?;
    let root = repo.variants(&table).ok()
        .and_then(|v| v["variants"][family_key(family)][&body.id]["root"].as_str().map(str::to_string))
        .unwrap_or_default();
    Ok(Json(json!({ "id": body.id, "branch": branch, "root": root })))
}

fn family_key(family: nous_repo::VariantFamily) -> &'static str {
    // 与私库名一致（同 `nous_repo::VariantFamily::key`）：`skills` / `extend`。
    match family { nous_repo::VariantFamily::Skill => "skills", nous_repo::VariantFamily::Extension => "extend" }
}

async fn variant_publish(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantPublishBody>,
) -> std::result::Result<Json<Value>, ServerError> {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&state, kind)?;
    json_result(repo.publish(family, &table_path(&state, kind)?, &body.id, &body.message, body.request_id.as_deref()))
}

async fn variant_prune(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantPruneBody>,
) -> std::result::Result<Json<Value>, ServerError> {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&state, kind)?;
    let result = repo.prune(family, &table_path(&state, kind)?, &body.id,
        nous_repo::PruneVariant { force: body.force }).map_err(repo_error)?;
    Ok(Json(json!({ "unmerged": result.unmerged, "remoteBranch": result.remote_branch, "removed": true })))
}

async fn variant_usage(
    State(state): State<Arc<AppState>>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<VariantUsageQuery>,
) -> std::result::Result<Json<Value>, ServerError> {
    let (kind, _) = variant_kind(&kind)?;
    let _ = select(&state, kind)?;
    let _ = table_path(&state, kind)?;
    let field = if kind == "skills" { "skills" } else { "extensions" };
    let usage = state.source().usage(field)?;
    Ok(Json(usage.get(&q.id).cloned().unwrap_or(Value::Null).as_array()
        .map(|ids| json!(ids)).unwrap_or(json!([]))))
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

/// `GET /v1/repos/{repo}/archive`：`.archive/` 里的单元名（归档区的读面）。
///
/// 写面早在（`archive` / `restore` / `purge`）：没有这条，客户端只能「猜着恢复」。
async fn archived(State(state): State<Arc<AppState>>, AxumPath(kind): AxumPath<String>) -> std::result::Result<Json<Value>, ServerError> {
    let repo = select(&state, &kind)?;
    let items = repo.archived().map_err(repo_error)?;
    Ok(Json(json!({ "items": items })))
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
        ("triggers", nous_env::TRIGGERS.resolve()),

        ("elements", nous_env::ELEMENTS.resolve()),
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

/// #76：生产装配的变体绑定表路径。位于各私库根的 `data/` 下，沿用 env 的四条路径解析。
pub fn variant_tables_from_env() -> std::collections::BTreeMap<String, std::path::PathBuf> {
    [
        ("skills", nous_env::SKILLS.resolve().join("data/skills.local.json")),
        ("extend", nous_env::EXTEND.resolve().join("data/extensions.local.json")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}
