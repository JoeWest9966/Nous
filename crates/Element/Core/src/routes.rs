//! 本插件自己的 8 条端点：`/v1/elements*`（组 / 包的读写与锁）。
//!
//! Magento 2 那种「模块自带 controller」：handler 住在插件里，插件不认识 `server` 的
//! `AppState`，只声明它需要什么（[`Host`]：元素库私库），由宿主实现并注入。
//!
//! 搬家前后行为一致的几条：
//!
//! - **回包形状逐字不变**（`wire.md`「元素库」）：`{kind, repo, groups[], packages[], resolved}` /
//!   `{kind, items[]}` / `{kind, id, fields, etag}` / 写回执 / `{kind, pushed, repo}`。
//!   NousDesk 不动。
//! - **状态码跟着语义走**：参数错 = 400，文件不在 = 404，`If-Match` 不符 = 409，
//!   锁着的元素 = 423，私库没装配 = 409。**不在这里降级成 400**。
//! - **未知字段原样透传**：`fields` 进什么样、出什么样，插件一个键都不删、不补默认值。
//! - **写一次提交一次**：落盘 → 提交 → 尽力推，全在 [`crate::put`] / [`crate::remove`] /
//!   [`crate::set_lock`] 里；这一层只做 HTTP ↔ 领域类型的翻译。
//!
//! 时钟：本插件的 8 条端点一条都不看时钟（没有 `at` 参数，git 提交时间由 `nous-repo` 取）。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put as put_route};
use axum::Json;
use nous_repo::{Pushed, Repo};
use nous_repo::Refusal;
use nous_routes::{HttpRefusal, Route};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{ElementItem, Kind, Scan};

/// 这个插件在注册表里的模块名（`docs/plugins.md` 的 module 列）。
pub const MODULE: &str = "Element_Core";

/// 这个插件在宿主 `AppState.repos` 里的键（与 `repos::REPO_KINDS` 同一处真相）。
///
/// 宿主装配私库时用这个名字登记，插件与 `element-models` 都按它取 —— 于是两边看到的
/// 必然是同一个根，不会有人另开一份 `NOUS_ELEMENTS`。
pub const REPO_KEY: &str = "elements";

/// 宿主能力：元素库私库。`None` = 没装配（读端点回诚实空表，写端点回 409）。
///
/// 只给仓库句柄、不给路径：`nous_env::ELEMENTS` 那一条路径只在宿主的装配处解析一次
/// （`repos::from_env`），插件不读环境变量，也就不会读到第二份根。
pub trait Host {
    fn elements_repo(&self) -> Option<Arc<Repo>>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn elements_repo(&self) -> Option<Arc<Repo>> {
        (**self).elements_repo()
    }
}



type EndpointResult = std::result::Result<Json<Value>, HttpRefusal>;

/// `RepoError` → HTTP 回包：先由 `nous_repo::Refusal` 判语义，再逐字搬两格。
fn http(error: nous_repo::RepoError) -> HttpRefusal {
    let refusal = Refusal::from_repo(error);
    HttpRefusal::new(refusal.status, refusal.body())
}

fn http_conflict(message: impl Into<String>) -> HttpRefusal {
    HttpRefusal::conflict(message)
}

fn http_bad_request(message: impl Into<String>) -> HttpRefusal {
    HttpRefusal::bad_request(message)
}


/// 私库没装配：写端点诚实 409（编辑器和「没有文件」是两件事）。
fn repo<H: Host>(host: &H) -> std::result::Result<Arc<Repo>, HttpRefusal> {
    host.elements_repo().ok_or_else(|| {
        http_conflict(format!(
            "元素库私库未装配（{} 指向的目录不存在？）",
            nous_env::ELEMENTS.var
        ))
    })
}

fn parse_kind(raw: &str) -> std::result::Result<Kind, HttpRefusal> {
    Kind::parse(raw)
        .ok_or_else(|| http_bad_request(format!("未知类别：{raw}（只有 groups / packages）")))
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
fn broken_item(kind: Kind, broken: crate::BrokenFile) -> Value {
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
async fn state<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
) -> EndpointResult {
    let repo = host.elements_repo();
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

    let groups = crate::scan(&repo, Kind::Groups).map_err(http)?;
    let packages = crate::scan(&repo, Kind::Packages).map_err(http)?;
    let resolved = crate::resolve(&groups.items, &packages.items);
    Ok(Json(json!({
        "kind": "elements",
        "repo": block,
        "groups": scan_items(Kind::Groups, groups),
        "packages": scan_items(Kind::Packages, packages),
        "resolved": resolved,
    })))
}

/// `GET /v1/elements/{kind}`：只扫这一段（不会顺带读另一段、也不解析引用）。
async fn list<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path(kind): Path<String>,
) -> EndpointResult {
    let kind = parse_kind(&kind)?;
    let repo = repo(&host)?;
    let scan = crate::scan(&repo, kind).map_err(http)?;
    Ok(Json(json!({ "kind": kind.dir(), "items": scan_items(kind, scan) })))
}

/// `GET /v1/elements/{kind}/{id}`：整份字段 + 内容指纹（写回时当 `base`）。
async fn read<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path((kind, id)): Path<(String, String)>,
) -> EndpointResult {
    let kind = parse_kind(&kind)?;
    let repo = repo(&host)?;
    let item = crate::get(&repo, kind, &id).map_err(http)?;
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockBody {
    /// `true` 上锁、`false` 解锁。必填：缺了是 400，不猜「反正是切换」。
    #[serde(default)]
    pub locked: Option<bool>,
    /// 内容指纹；给了就是 If-Match，不符回 409。
    #[serde(default)]
    pub base: Option<String>,
    /// 幂等锚；同一个值只提交一次。
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
    outcome: crate::PutOutcome,
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
async fn write<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path((kind, id)): Path<(String, String)>,
    Json(body): Json<WriteBody>,
) -> EndpointResult {
    let kind = parse_kind(&kind)?;
    let repo = repo(&host)?;
    let outcome = crate::put(
        &repo,
        kind,
        &id,
        body.fields,
        body.base.as_deref(),
        request_id(body.request_id).as_deref(),
    )
    .map_err(http)?;
    let created = outcome.created;
    let id = outcome.id.clone();
    Ok(Json(write_receipt(kind, &id, created, outcome)))
}

/// `POST /v1/elements/{kind}`：新建（id 从 `fields.id` 取），201。已有的 id 会被覆盖。
async fn create<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path(kind): Path<String>,
    Json(body): Json<WriteBody>,
) -> std::result::Result<(StatusCode, Json<Value>), HttpRefusal> {
    let kind = parse_kind(&kind)?;
    let repo = repo(&host)?;
    let Some(id) = body.fields.get("id").and_then(Value::as_str).map(str::to_string) else {
        return Err(http_bad_request("缺 id（放在 body.fields.id 里）"));
    };
    let outcome = crate::put(
        &repo,
        kind,
        &id,
        body.fields,
        None,
        request_id(body.request_id).as_deref(),
    )
    .map_err(http)?;
    let created = outcome.created;
    let written_id = outcome.id.clone();
    Ok((
        StatusCode::CREATED,
        Json(write_receipt(kind, &written_id, created, outcome)),
    ))
}

/// `DELETE /v1/elements/{kind}/{id}`：删掉并提交（`?base=` 不符回 409）。
async fn remove<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path((kind, id)): Path<(String, String)>,
    Query(query): Query<DeleteQuery>,
) -> EndpointResult {
    let kind = parse_kind(&kind)?;
    let repo = repo(&host)?;
    let outcome = crate::remove(
        &repo,
        kind,
        &id,
        query.base.as_deref(),
        request_id(query.request_id).as_deref(),
    )
    .map_err(http)?;
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

/// `POST /v1/elements/{kind}/{id}/lock`：上锁 / 解锁，改的是盘上那一份 JSON。
///
/// 它是**唯一**能改 `locked` 的入口（`PUT` / `DELETE` 遇到锁着的元素直接 423），
/// 也正因此它自己不被锁拦住 —— 否则锁死就再也解不开了。
async fn lock<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
    Path((kind, id)): Path<(String, String)>,
    Json(body): Json<LockBody>,
) -> EndpointResult {
    let kind = parse_kind(&kind)?;
    let Some(locked) = body.locked else {
        return Err(http_bad_request("缺 locked（true 上锁 / false 解锁）"));
    };
    let repo = repo(&host)?;
    let outcome = crate::set_lock(
        &repo,
        kind,
        &id,
        locked,
        body.base.as_deref(),
        request_id(body.request_id).as_deref(),
    )
    .map_err(http)?;
    let commit = outcome.commit.as_ref();
    Ok(Json(json!({
        "kind": kind.dir(),
        "id": outcome.id,
        "locked": outcome.locked,
        "changed": outcome.changed,
        "etag": outcome.etag,
        "commit": commit.map(|c| c.commit.clone()),
        "pushed": commit.map(|c| c.pushed).unwrap_or(Pushed::Unknown),
        "refs": commit.map(|c| c.refs.clone()).unwrap_or_default(),
        "replay": outcome.replay,
    })))
}

/// `POST /v1/elements/push`：手动补推 —— 不重复写文件、不制造提交。
async fn push<H: Host + Clone + Send + Sync + 'static>(
    State(host): State<H>,
) -> EndpointResult {
    let repo = repo(&host)?;
    let pushed: Pushed = crate::push(&repo).map_err(http)?;
    Ok(Json(json!({
        "kind": "elements",
        "pushed": pushed,
        "repo": repo_block(Some(&repo)),
    })))
}

/// 本插件的端点表。路径与搬家前逐字一致（`wire.md` 与 Desk 都不动）。
pub fn routes<H>() -> Vec<Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![
        Route::new("GET", "/v1/elements", MODULE, || get(state::<H>)),
        Route::new("GET", "/v1/elements/{kind}", MODULE, || get(list::<H>)),
        Route::new("POST", "/v1/elements/{kind}", MODULE, || post(create::<H>)),
        Route::new("GET", "/v1/elements/{kind}/{id}", MODULE, || get(read::<H>)),
        Route::new("PUT", "/v1/elements/{kind}/{id}", MODULE, || {
            put_route(write::<H>)
        }),
        Route::new("DELETE", "/v1/elements/{kind}/{id}", MODULE, || {
            axum::routing::delete(remove::<H>)
        }),
        Route::new("POST", "/v1/elements/{kind}/{id}/lock", MODULE, || {
            post(lock::<H>)
        }),
        Route::new("POST", "/v1/elements/push", MODULE, || post(push::<H>)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假宿主：只给一个仓库（或干脆不给），没有别的能力。
    #[derive(Clone)]
    struct FakeHost {
        repo: Option<Arc<Repo>>,
    }

    impl Host for FakeHost {
        fn elements_repo(&self) -> Option<Arc<Repo>> {
            self.repo.clone()
        }
    }

    /// 注册表里的形状：8 条、路径与模块名都对得上（`:id` 写法由 `nous-routes` 折）。
    #[test]
    fn routes_declare_the_eight_elements_endpoints_under_this_module() {
        let declared: Vec<String> = routes::<FakeHost>()
            .iter()
            .map(|route| format!("{} [{}]", route.declared(), route.module()))
            .collect();
        assert_eq!(
            declared,
            vec![
                "GET /v1/elements [Element_Core]",
                "GET /v1/elements/:kind [Element_Core]",
                "POST /v1/elements/:kind [Element_Core]",
                "GET /v1/elements/:kind/:id [Element_Core]",
                "PUT /v1/elements/:kind/:id [Element_Core]",
                "DELETE /v1/elements/:kind/:id [Element_Core]",
                "POST /v1/elements/:kind/:id/lock [Element_Core]",
                "POST /v1/elements/push [Element_Core]",
            ]
        );
    }

    /// 未装配 = 409 且文案是那句熟话（不是空 200，也不是 404）。
    #[test]
    fn an_unassembled_repo_is_a_409_with_the_env_var_name() {
        let Err(refusal) = repo(&FakeHost { repo: None }) else {
            panic!("未装配的私库必须是 Err");
        };
        let (status, body) = refusal.split();
        assert_eq!(status, StatusCode::CONFLICT.as_u16());
        let message = body["error"].as_str().unwrap();
        assert!(message.contains("元素库私库未装配"), "{message}");
        assert!(message.contains("NOUS_ELEMENTS"), "{message}");
    }

    /// 未知类别 = 400，且把收到的值回显在 `{error}` 里（不吞成 404）。
    #[test]
    fn an_unknown_kind_is_a_400_that_echoes_the_value() {
        let (status, body) = parse_kind("skills").unwrap_err().split();
        assert_eq!(status, StatusCode::BAD_REQUEST.as_u16());
        assert_eq!(body, json!({ "error": "未知类别：skills（只有 groups / packages）" }));
    }
}
