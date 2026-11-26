//! `repo` 模块自己的端点：`/v1/repos/{repo}/*` 那 30 条（#75 / #76 / #77）。
//!
//! handler 住在插件里（Magento 2 那种「模块自带 controller」）。插件不认识
//! `server` 的 `AppState`，只声明它需要什么（[`Host`]），由宿主实现并注入。
//!
//! 这里只做 HTTP ↔ 本 crate 的翻译：错误码映射、查询参数、JSON 形状。
//! 文件 CRUD、git 语义、变体绑定全在本 crate 的其它模块里，这里**不碰进程**。
//!
//! 三组端点共用同一个 `:repo` 前缀，`REPO_KINDS` 是那六个值的唯一真相：
//!
//! - 前 20 条是私库本体（status / pending / history / archive / variants）；
//! - 后 10 条是上游追踪（sources / upstream/*），只挂在 `skills` 上。

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::routing::{get, post};
use axum::Json;
use nous_routes::HttpRefusal;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{Refusal, Repo, RepoError, Source, Upstream, VariantFamily};

/// 模块名（规则 12 的全名是小写 `repo`）：`/v1/capabilities` 的 `modules[].name`、
/// `/v1/_routes` 的 `module` 标签都用它。
pub const MODULE: &str = "repo";

/// 六套私库的名字。`:repo` 不是任意字符串，未知值必须 400。
pub const REPO_KINDS: &[&str] = &["presets", "skills", "prompts", "extend", "triggers", "elements"];

/// 宿主能力：这个插件只认识四件事。
///
/// 每个 `Option` 返回 `None` 都是「宿主没装配」（目录不存在 / 没配 data 根），
/// 端点据此诚实回 409/500，**不退回空 200**。
pub trait Host {
    /// 某套私库的仓库句柄（`None` = 那套没装配）。
    fn repo(&self, kind: &str) -> Option<Arc<Repo>>;

    /// 变体绑定表的落盘路径（只有 `skills` / `extend` 有）。
    fn variant_table(&self, kind: &str) -> Option<PathBuf>;

    /// 上游镜像的 `data/` 根（`NOUS_DATA`）。没有它上游无处落盘。
    fn upstream_data(&self) -> Option<PathBuf>;

    /// `variants/usage` 的反查：谁在引用这个变体。会话记录归 durable，插件不自己开库。
    fn source_usage(&self, field: &str) -> Result<Value, HttpRefusal>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn repo(&self, kind: &str) -> Option<Arc<Repo>> {
        (**self).repo(kind)
    }
    fn variant_table(&self, kind: &str) -> Option<PathBuf> {
        (**self).variant_table(kind)
    }
    fn upstream_data(&self) -> Option<PathBuf> {
        (**self).upstream_data()
    }
    fn source_usage(&self, field: &str) -> Result<Value, HttpRefusal> {
        (**self).source_usage(field)
    }
}

type EndpointResult = std::result::Result<Json<Value>, HttpRefusal>;

/// `RepoError` → HTTP 回包：先由本 crate 的 [`Refusal`] 判语义，再两格搬运。
///
/// **搬家前后逐字一致**：409 的 `{content, etag}` 富化就在 `Refusal::body()` 里。
fn http(error: RepoError) -> HttpRefusal {
    let refusal = Refusal::from_repo(error);
    HttpRefusal::new(refusal.status, refusal.body())
}

fn json_result<T: serde::Serialize>(result: std::result::Result<T, RepoError>) -> EndpointResult {
    Ok(Json(serde_json::to_value(result.map_err(http)?).unwrap_or(Value::Null)))
}

/// 校验 `:repo` 并取句柄。未知值 400，未装配 500 —— 两件事分开。
fn select<H: Host>(host: &H, kind: &str) -> std::result::Result<Arc<Repo>, HttpRefusal> {
    if !REPO_KINDS.contains(&kind) {
        return Err(HttpRefusal::bad_request(format!("未知私库：{kind}")));
    }
    host.repo(kind)
        .ok_or_else(|| HttpRefusal::internal(format!("私库未装配：{kind}")))
}

// ---------------------------------------------------------------- 请求体

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

// ---------------------------------------------------------------- 私库本体

/// 变体只给技能 / 扩展；其余私库（presets / prompts）回 400。
fn variant_kind(kind: &str) -> std::result::Result<(&'static str, VariantFamily), HttpRefusal> {
    match kind {
        "skills" => Ok(("skills", VariantFamily::Skill)),
        "extend" => Ok(("extend", VariantFamily::Extension)),
        other if REPO_KINDS.contains(&other) => {
            Err(HttpRefusal::bad_request(format!("私库 {other} 没有变体")))
        }
        other => Err(HttpRefusal::bad_request(format!("未知私库：{other}"))),
    }
}

fn table_path<H: Host>(host: &H, kind: &str) -> std::result::Result<PathBuf, HttpRefusal> {
    host.variant_table(kind)
        .ok_or_else(|| HttpRefusal::conflict("变体绑定表未装配"))
}

async fn variants<H: Host>(State(host): State<H>, AxumPath(kind): AxumPath<String>) -> EndpointResult {
    let (kind, _) = variant_kind(&kind)?;
    let repo = select(&host, kind)?;
    json_result(repo.variants(&table_path(&host, kind)?))
}

async fn variant_bind<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantBindBody>,
) -> EndpointResult {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&host, kind)?;
    let paths: Vec<&str> = body.paths.iter().map(String::as_str).collect();
    json_result(repo.bind(family, &table_path(&host, kind)?, crate::BindVariant {
        id: &body.id,
        paths: &paths,
    }))
}

async fn variant_unbind<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantIdBody>,
) -> EndpointResult {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&host, kind)?;
    repo.unbind(family, &table_path(&host, kind)?, &body.id).map_err(http)?;
    Ok(Json(json!({ "id": body.id, "removed": true })))
}

async fn variant_branch<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantBranchBody>,
) -> EndpointResult {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&host, kind)?;
    let table = table_path(&host, kind)?;
    let branch = repo.switch_branch(family, &table, &body.id, &body.branch).map_err(http)?;
    let root = repo
        .variants(&table)
        .ok()
        .and_then(|v| v["variants"][family_key(family)][&body.id]["root"].as_str().map(str::to_string))
        .unwrap_or_default();
    Ok(Json(json!({ "id": body.id, "branch": branch, "root": root })))
}

fn family_key(family: VariantFamily) -> &'static str {
    // 与私库名一致（同 `VariantFamily::key`）：`skills` / `extend`。
    match family {
        VariantFamily::Skill => "skills",
        VariantFamily::Extension => "extend",
    }
}

async fn variant_publish<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantPublishBody>,
) -> EndpointResult {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&host, kind)?;
    json_result(repo.publish(
        family,
        &table_path(&host, kind)?,
        &body.id,
        &body.message,
        body.request_id.as_deref(),
    ))
}

async fn variant_prune<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<VariantPruneBody>,
) -> EndpointResult {
    let (kind, family) = variant_kind(&kind)?;
    let repo = select(&host, kind)?;
    let result = repo
        .prune(family, &table_path(&host, kind)?, &body.id, crate::PruneVariant { force: body.force })
        .map_err(http)?;
    Ok(Json(json!({
        "unmerged": result.unmerged,
        "remoteBranch": result.remote_branch,
        "removed": true,
    })))
}

async fn variant_usage<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<VariantUsageQuery>,
) -> EndpointResult {
    let (kind, _) = variant_kind(&kind)?;
    let _ = select(&host, kind)?;
    let _ = table_path(&host, kind)?;
    let field = if kind == "skills" { "skills" } else { "extensions" };
    let usage = host.source_usage(field)?;
    Ok(Json(
        usage
            .get(&q.id)
            .cloned()
            .unwrap_or(Value::Null)
            .as_array()
            .map(|ids| json!(ids))
            .unwrap_or(json!([])),
    ))
}

async fn status<H: Host>(State(host): State<H>, AxumPath(kind): AxumPath<String>) -> EndpointResult {
    let repo = select(&host, &kind)?;
    json_result(repo.status())
}

async fn pending<H: Host>(State(host): State<H>, AxumPath(kind): AxumPath<String>) -> EndpointResult {
    let repo = select(&host, &kind)?;
    let units = repo.pending().map_err(http)?;
    Ok(Json(json!({ "units": units })))
}

/// `GET /v1/repos/{repo}/archive`：`.archive/` 里的单元名（归档区的读面）。
///
/// 写面早在（`archive` / `restore` / `purge`）：没有这条，客户端只能「猜着恢复」。
async fn archived<H: Host>(State(host): State<H>, AxumPath(kind): AxumPath<String>) -> EndpointResult {
    let repo = select(&host, &kind)?;
    let items = repo.archived().map_err(http)?;
    Ok(Json(json!({ "items": items })))
}

async fn pending_diff<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<DiffQuery>,
) -> EndpointResult {
    let repo = select(&host, &kind)?;
    json_result(repo.pending_diff(&q.path.unwrap_or_default()))
}

async fn pending_commit<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<CommitQuery>,
) -> EndpointResult {
    let repo = select(&host, &kind)?;
    let hash = q.hash.unwrap_or_default();
    json_result(repo.pending_commit(&hash))
}

async fn commit<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<CommitBody>,
) -> EndpointResult {
    let repo = select(&host, &kind)?;
    json_result(repo.commit(&body.paths, &body.message, body.request_id.as_deref()))
}

async fn history<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<HistoryQuery>,
) -> EndpointResult {
    let repo = select(&host, &kind)?;
    let items = repo
        .history(&q.name.unwrap_or_default(), q.limit.unwrap_or(50))
        .map_err(http)?;
    Ok(Json(json!({ "items": items })))
}

async fn history_diff<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<HistoryDiffQuery>,
) -> EndpointResult {
    let repo = select(&host, &kind)?;
    json_result(repo.history_diff(
        &q.name.unwrap_or_default(),
        &q.hash.unwrap_or_default(),
        &q.path.unwrap_or_default(),
    ))
}

async fn archive<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> EndpointResult {
    lifecycle(select(&host, &kind)?, |r| r.archive(&body.name, body.request_id.as_deref()))
}

async fn restore<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> EndpointResult {
    lifecycle(select(&host, &kind)?, |r| r.restore(&body.name, body.request_id.as_deref()))
}

async fn purge<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<NameBody>,
) -> EndpointResult {
    lifecycle(select(&host, &kind)?, |r| r.purge(&body.name, body.request_id.as_deref()))
}

fn lifecycle<F>(repo: Arc<Repo>, f: F) -> EndpointResult
where
    F: FnOnce(&Repo) -> std::result::Result<crate::LifecycleResult, RepoError>,
{
    json_result(f(&repo))
}

// ---------------------------------------------------------------- 上游追踪（#77）

#[derive(Debug, Deserialize)]
pub struct SourcesBody {
    pub sources: Vec<Source>,
}

#[derive(Debug, Deserialize, Default)]
pub struct UpstreamNameQuery {
    pub name: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct UpstreamBody {
    pub name: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    /// 协议字段：客户端可带，服务端只接收不解释（AGENTS.md「协议是哑的」）。
    #[allow(dead_code)]
    #[serde(default)]
    pub request_id: Option<String>,
}

/// 上游只挂在 `skills` 私库上：sources.json 住技能仓，镜像来自 data/upstream。
fn upstream<H: Host>(host: &H, kind: &str) -> std::result::Result<Upstream, HttpRefusal> {
    if kind != "skills" {
        return Err(HttpRefusal::bad_request(format!("上游追踪只支持 skills，不支持 {kind}")));
    }
    let repo = select(host, kind)?;
    let data = host.upstream_data().ok_or_else(|| {
        HttpRefusal::conflict("没有 data 根：上游镜像无处落盘（设 NOUS_DATA）")
    })?;
    std::fs::create_dir_all(&data).map_err(|e| HttpRefusal::internal(e.to_string()))?;
    Upstream::open(repo.root(), data).map_err(http)
}

async fn sources<H: Host>(State(host): State<H>, AxumPath(kind): AxumPath<String>) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    Ok(Json(up.sources_doc().map_err(http)?))
}

async fn write_sources<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<SourcesBody>,
) -> EndpointResult {
    if !REPO_KINDS.contains(&kind.as_str()) {
        return Err(HttpRefusal::bad_request(format!("未知私库：{kind}")));
    }
    let up = upstream(&host, &kind)?;
    let sources = up.write_sources(&body.sources).map_err(http)?;
    Ok(Json(json!({ "ok": true, "sources": sources })))
}

async fn overview<H: Host>(State(host): State<H>, AxumPath(kind): AxumPath<String>) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    let (sources, errors) = up.status_with_errors();
    Ok(Json(json!({ "sources": sources, "errors": errors })))
}

async fn refresh<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<UpstreamBody>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    let head = up.refresh(&body.name).map_err(http)?;
    Ok(Json(json!({ "ok": true, "name": body.name, "head": head })))
}

async fn pin<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<UpstreamBody>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    let commit = up.pin(&body.name, body.commit.as_deref()).map_err(http)?;
    Ok(Json(json!({ "ok": true, "name": body.name, "commit": commit })))
}

async fn check<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    body: Option<Json<UpstreamBody>>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    let name = body.and_then(|Json(body)| if body.name.is_empty() { None } else { Some(body.name) });
    let checks = up.check(name.as_deref()).map_err(http)?;
    Ok(Json(json!({ "ok": true, "checks": checks })))
}

async fn sync<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<UpstreamBody>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    let head = up.sync(&body.name).map_err(http)?;
    Ok(Json(json!({ "ok": true, "name": body.name, "head": head })))
}

async fn tree<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<UpstreamNameQuery>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    let items = up
        .tree_items(
            q.name.as_deref().unwrap_or_default(),
            q.path.as_deref().unwrap_or_default(),
        )
        .map_err(http)?;
    Ok(Json(json!({ "items": items })))
}

async fn file<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<UpstreamNameQuery>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    json_result(up.file(
        q.name.as_deref().unwrap_or_default(),
        q.path.as_deref().unwrap_or_default(),
    ))
}

async fn files<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Query(q): Query<UpstreamNameQuery>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    let files = up
        .compare(
            q.name.as_deref().unwrap_or_default(),
            q.path.as_deref().unwrap_or_default(),
        )
        .map_err(http)?;
    Ok(Json(json!({ "files": files })))
}

async fn copy<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<UpstreamBody>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    json_result(up.adopt(
        &body.name,
        body.path.as_deref().unwrap_or_default(),
        body.to.as_deref(),
        false,
    ))
}

async fn adopt<H: Host>(
    State(host): State<H>,
    AxumPath(kind): AxumPath<String>,
    Json(body): Json<UpstreamBody>,
) -> EndpointResult {
    let up = upstream(&host, &kind)?;
    json_result(up.adopt(
        &body.name,
        body.path.as_deref().unwrap_or_default(),
        body.to.as_deref(),
        true,
    ))
}

// ---------------------------------------------------------------- 声明

/// 端点唯一真相：`/v1/_routes`、`/v1/capabilities`、`docs/plugin-versions.md`
/// 三处都从这里读，`plugin.toml` 里**没有**端点表（规则 11）。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        // 私库本体（20 条）
        nous_routes::Route::new("GET", "/v1/repos/{repo}/status", MODULE, || get(status::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/pending", MODULE, || get(pending::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/pending/diff", MODULE, || get(pending_diff::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/pending/commit", MODULE, || get(pending_commit::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/history", MODULE, || get(history::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/history/diff", MODULE, || get(history_diff::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/commit", MODULE, || post(commit::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/archive", MODULE, || get(archived::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/archive", MODULE, || post(archive::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/restore", MODULE, || post(restore::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/purge", MODULE, || post(purge::<H>)),
        // #76：变体（只给 skills / extend；其余四值回 400）。
        nous_routes::Route::new("GET", "/v1/repos/{repo}/variants", MODULE, || get(variants::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/variants/bind", MODULE, || post(variant_bind::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/variants/unbind", MODULE, || post(variant_unbind::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/variants/branch", MODULE, || post(variant_branch::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/variants/publish", MODULE, || post(variant_publish::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/variants/prune", MODULE, || post(variant_prune::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/variants/usage", MODULE, || get(variant_usage::<H>)),
        // 上游追踪（#77，10 条）
        nous_routes::Route::new("GET", "/v1/repos/{repo}/sources", MODULE, || get(sources::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/sources", MODULE, || post(write_sources::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/upstream", MODULE, || get(overview::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/upstream/refresh", MODULE, || post(refresh::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/upstream/pin", MODULE, || post(pin::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/upstream/check", MODULE, || post(check::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/upstream/sync", MODULE, || post(sync::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/upstream/tree", MODULE, || get(tree::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/upstream/file", MODULE, || get(file::<H>)),
        nous_routes::Route::new("GET", "/v1/repos/{repo}/upstream/files", MODULE, || get(files::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/upstream/copy", MODULE, || post(copy::<H>)),
        nous_routes::Route::new("POST", "/v1/repos/{repo}/upstream/adopt", MODULE, || post(adopt::<H>)),
    ]
}
