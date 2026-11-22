//! 元素库（组 / 包）：`/v1/elements*`。
//!
//! 盘上真相、校验与「写一次提交一次」的事务都在 `nous-elements`（app 层）；
//! 这一层只做 HTTP ↔ 领域类型的翻译，不 spawn git、不自己拼路径。
//!
//! 形状（`wire.md`「元素库」）：
//!
//! | 端点 | 回包 |
//! |---|---|
//! | `GET /v1/elements` | `{ kind, repo, groups[], packages[], resolved }`，坏的那份 `state:"problem"` |
//! | `GET /v1/elements/{kind}` | `{ kind, items[] }`（只扫这一段） |
//! | `GET /v1/elements/{kind}/{id}` | `{ kind, id, fields, etag }` |
//! | `PUT /v1/elements/{kind}/{id}` | 建或覆盖；`{ fields, base?, requestId? }` |
//! | `POST /v1/elements/{kind}` | 同 PUT，id 从 `fields.id` 取 |
//! | `DELETE /v1/elements/{kind}/{id}` | 删掉并提交（`?base=&requestId=`） |
//! | `POST /v1/elements/push` | 手动补推（网络恢复后页面上的「重试推送」） |

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use nous_elements::{ElementItem, Kind, Scan};
use nous_repo::{Pushed, Repo};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::{Result, ServerError};
use crate::resource_admin::repo_error;

/// `AppState.repos` 里的私库键（与 `repos::REPO_KINDS` / `from_env` 同一处真相）。
pub const REPO_KEY: &str = "elements";

pub fn attach(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/v1/elements", axum::routing::get(state))
        .route("/v1/elements/push", axum::routing::post(push))
        .route(
            "/v1/elements/{kind}",
            axum::routing::get(list).post(create),
        )
        .route(
            "/v1/elements/{kind}/{id}",
            axum::routing::get(read)
                .put(put)
                .delete(remove),
        )
}

/// 私库没装配：写端点诚实 409（编辑器和「没有文件」是两件事）。
fn repo(state: &AppState) -> Result<Arc<Repo>> {
    state.repo(REPO_KEY).ok_or_else(|| {
        ServerError::Conflict(format!(
            "元素库私库未装配（{} 指向的目录不存在？）",
            nous_env::ELEMENTS.var
        ))
    })
}

fn parse_kind(raw: &str) -> Result<Kind> {
    Kind::parse(raw).ok_or_else(|| {
        ServerError::BadRequest(format!("未知类别：{raw}（只有 groups / packages）"))
    })
}

/// 一份读得出来的元素。
fn ok_item(kind: Kind, item: ElementItem) -> Value {
    json!({
        "kind": kind.dir(),
        "id": item.id,
        "fields": item.fields,
        "etag": item.etag,
        "state": "ok",
        "detail": Value::Null,
    })
}

/// 一份读不出来的元素：只标红它，同一目录里别的照列。
fn broken_item(kind: Kind, broken: nous_elements::BrokenFile) -> Value {
    json!({
        "kind": kind.dir(),
        "id": broken.id,
        "fields": Value::Null,
        "etag": Value::Null,
        "state": "problem",
        "detail": broken.detail,
        "path": broken.path,
    })
}

fn scan_items(kind: Kind, scan: Scan) -> Vec<Value> {
    let mut out: Vec<Value> = scan.items.into_iter().map(|item| ok_item(kind, item)).collect();
    out.extend(scan.broken.into_iter().map(|broken| broken_item(kind, broken)));
    out.sort_by(|a, b| {
        a["id"].as_str().unwrap_or_default().cmp(b["id"].as_str().unwrap_or_default())
    });
    out
}

/// 仓库那一块：根路径 + 在不在 + git 三态。未装配时 `present:false` 并指路。
fn repo_block(repo: Option<&Repo>) -> Value {
    let root = nous_env::ELEMENTS.resolve();
    let Some(repo) = repo else {
        return json!({
            "root": root.to_string_lossy(),
            "present": false,
            "missing": format!(
                "私库路径不存在：{}（设 {} 指到你的副本）",
                nous_env::ELEMENTS.default,
                nous_env::ELEMENTS.var
            ),
            "head": Value::Null,
            "branch": Value::Null,
            "remote": Value::Null,
            "dirty": Value::Null,
            "ahead": Value::Null,
            "behind": Value::Null,
        });
    };
    let status = repo.status().ok();
    let remote = repo.remote_url().ok().flatten();
    json!({
        "root": repo.root().to_string_lossy(),
        "present": true,
        "missing": Value::Null,
        "head": status.as_ref().and_then(|s| s.head.clone()),
        "branch": status.as_ref().and_then(|s| s.branch.clone()),
        "remote": remote,
        "dirty": status.as_ref().map(|s| s.dirty),
        "ahead": status.as_ref().map(|s| s.ahead),
        "behind": status.as_ref().map(|s| s.behind),
    })
}

/// `GET /v1/elements`：两段全量 + 引用解析。目录不存在 = 空表（不 404、不编数据）。
async fn state(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let repo = state.repo(REPO_KEY);
    let block = repo_block(repo.as_deref());
    let Some(repo) = repo else {
        return Ok(Json(json!({
            "kind": "elements",
            "repo": block,
            "groups": [],
            "packages": [],
            "resolved": { "groups": {}, "missing": [] },
        })));
    };

    let groups = nous_elements::scan(&repo, Kind::Groups).map_err(repo_error)?;
    let packages = nous_elements::scan(&repo, Kind::Packages).map_err(repo_error)?;
    let resolved = nous_elements::resolve(&groups.items, &packages.items);
    Ok(Json(json!({
        "kind": "elements",
        "repo": block,
        "groups": scan_items(Kind::Groups, groups),
        "packages": scan_items(Kind::Packages, packages),
        "resolved": resolved,
    })))
}

/// `GET /v1/elements/{kind}`：只扫这一段（不会顺带读另一段、也不解析引用）。
async fn list(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
) -> Result<Json<Value>> {
    let kind = parse_kind(&kind)?;
    let repo = repo(&state)?;
    let scan = nous_elements::scan(&repo, kind).map_err(repo_error)?;
    Ok(Json(json!({ "kind": kind.dir(), "items": scan_items(kind, scan) })))
}

/// `GET /v1/elements/{kind}/{id}`：整份字段 + 内容指纹（写回时当 `base`）。
async fn read(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<Value>> {
    let kind = parse_kind(&kind)?;
    let repo = repo(&state)?;
    let item = nous_elements::get(&repo, kind, &id).map_err(repo_error)?;
    Ok(Json(json!({
        "kind": kind.dir(),
        "id": item.id,
        "fields": item.fields,
        "etag": item.etag,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteBody {
    /// 整份元素 JSON（含 `id` / `name` 与任何未知字段）。
    pub fields: Value,
    /// 内容指纹；给了就是 If-Match，不符回 409。
    #[serde(default)]
    pub base: Option<String>,
    /// 幂等锚；同一个值只提交一次。
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeleteQuery {
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
}

fn request_id(value: Option<String>) -> Option<String> {
    value.filter(|id| !id.trim().is_empty())
}

fn write_receipt(
    kind: Kind,
    id: &str,
    created: bool,
    outcome: nous_elements::PutOutcome,
) -> Value {
    let commit = outcome.commit.as_ref();
    json!({
        "kind": kind.dir(),
        "id": id,
        "created": created,
        "etag": outcome.etag,
        "commit": commit.map(|c| c.commit.clone()),
        "pushed": commit.map(|c| c.pushed).unwrap_or(Pushed::Unknown),
        "refs": commit.map(|c| c.refs.clone()).unwrap_or_default(),
        "replay": outcome.replay,
    })
}

/// `PUT /v1/elements/{kind}/{id}`：落盘 + 提交 + 尽力推，一次一个 `requestId`。
async fn put(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
    Json(body): Json<WriteBody>,
) -> Result<Json<Value>> {
    let kind = parse_kind(&kind)?;
    let repo = repo(&state)?;
    let outcome = nous_elements::put(
        &repo,
        kind,
        &id,
        body.fields,
        body.base.as_deref(),
        request_id(body.request_id).as_deref(),
    )
    .map_err(repo_error)?;
    let created = outcome.created;
    let id = outcome.id.clone();
    Ok(Json(write_receipt(kind, &id, created, outcome)))
}

/// `POST /v1/elements/{kind}`：新建（id 从 `fields.id` 取），201。已有的 id 会被覆盖。
async fn create(
    State(state): State<Arc<AppState>>,
    Path(kind): Path<String>,
    Json(body): Json<WriteBody>,
) -> Result<(StatusCode, Json<Value>)> {
    let kind = parse_kind(&kind)?;
    let repo = repo(&state)?;
    let Some(id) = body.fields.get("id").and_then(Value::as_str).map(str::to_string) else {
        return Err(ServerError::BadRequest(
            "缺 id（放在 body.fields.id 里）".into(),
        ));
    };
    let outcome = nous_elements::put(
        &repo,
        kind,
        &id,
        body.fields,
        None,
        request_id(body.request_id).as_deref(),
    )
    .map_err(repo_error)?;
    let created = outcome.created;
    let written_id = outcome.id.clone();
    Ok((
        StatusCode::CREATED,
        Json(write_receipt(kind, &written_id, created, outcome)),
    ))
}

/// `DELETE /v1/elements/{kind}/{id}`：删掉并提交（`?base=` 不符回 409）。
async fn remove(
    State(state): State<Arc<AppState>>,
    Path((kind, id)): Path<(String, String)>,
    Query(query): Query<DeleteQuery>,
) -> Result<Json<Value>> {
    let kind = parse_kind(&kind)?;
    let repo = repo(&state)?;
    let outcome = nous_elements::remove(
        &repo,
        kind,
        &id,
        query.base.as_deref(),
        request_id(query.request_id).as_deref(),
    )
    .map_err(repo_error)?;
    let commit = outcome.commit.as_ref();
    Ok(Json(json!({
        "kind": kind.dir(),
        "id": id,
        "commit": commit.map(|c| c.commit.clone()),
        "pushed": commit.map(|c| c.pushed).unwrap_or(Pushed::Unknown),
        "refs": commit.map(|c| c.refs.clone()).unwrap_or_default(),
        "replay": outcome.replay,
    })))
}

/// `POST /v1/elements/push`：手动补推 —— 不重复写文件、不制造提交。
async fn push(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    let repo = repo(&state)?;
    let pushed: Pushed = nous_elements::push(&repo).map_err(repo_error)?;
    Ok(Json(json!({
        "kind": "elements",
        "pushed": pushed,
        "repo": repo_block(Some(&repo)),
    })))
}
