//! 资源仓管理面：`/v1/extensions/{files,file,publish}`。
//!
//! **技能的六条已经搬进 `nous-skills` 插件，模板的六条搬进 `nous-prompts` 插件**
//! （`crates/Nous/{Skills,Prompts}/src/routes.rs`），盘面规矩（名字 / 路径校验、递归清单、
//! 骨架落盘、发布）下沉到 `nous_repo::entries`，`RepoError → 状态码` 的翻译下沉到
//! `nous_routes::Refusal`；这里只剩扩展一仓，用的是同一份下沉后的实现。
//!
//! 只做 HTTP ↔ 盘面翻译：文件读写 / etag 撞闸 / git 提交 / push 都在 `nous_repo`
//! （platform 层），这里不直接 spawn `git`，也不另建一套锁。
//!
//! 一件事只做一遍：publish 自己不拼 `git add`，先问 `Repo::pending` 拿 dirty 路径，
//! 再交给 `Repo::commit`（内容闸与 pathspec 防搭车都在那一层）。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use nous_repo::{Repo, RepoError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::error::{Result, ServerError};

// 盘面规矩（名字 / 路径校验、递归清单、骨架落盘、发布）住在 `nous_repo::entries`，
// 技能插件与这里共用同一份。这个模块只留 HTTP 翻译，不再有第二套常量。

/// 还留在这里的资源仓。`repo` 是 `AppState` 里的私库键（`repos::from_env` 的名字），
/// `segment` 是 URL 里那一段 —— 扩展的私库键是 `extend`，URL 却是 `extensions`。
///
/// 技能那一支搬去了 `nous-skills`，模板那一支搬去了 `nous-prompts`；这里只剩扩展。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Extensions,
}

impl Kind {
    pub fn segment(self) -> &'static str {
        match self {
            Self::Extensions => "extensions",
        }
    }

    fn repo_key(self) -> &'static str {
        match self {
            Self::Extensions => "extend",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Extensions => "扩展",
        }
    }

    /// 发布时的默认文案（与旧面板 `prompts-api.js` 一致）。
    fn default_message(self) -> &'static str {
        match self {
            Self::Extensions => "extend: 更新扩展仓库",
        }
    }
}

pub fn attach(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    // 技能的六条已搬进 `nous-skills` 插件，模板的六条已搬进 `nous-prompts` 插件
    // （含 `GET /v1/prompts` 那条只读投影）。这里只剩扩展一仓。
    router
        .route(
            "/v1/extensions/files",
            axum::routing::get(|s, q| files(Kind::Extensions, s, q)),
        )
        .route(
            "/v1/extensions/file",
            axum::routing::get(|s, q| file(Kind::Extensions, s, q))
                .put(|s, b| save(Kind::Extensions, s, b)),
        )
        // 扩展的 create 已经在 `ext_admin`（#84，带骨架与运行时），这里不再造第二条。
        .route(
            "/v1/extensions/publish",
            axum::routing::post(|s, b| publish(Kind::Extensions, s, b)),
        )
}

#[derive(Debug, Deserialize)]
pub struct NameQuery {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct SaveBody {
    pub name: String,
    pub path: String,
    pub text: String,
    /// 省掉 = 硬写（新建 / 覆盖）；给上 = If-Match，不符回 409。
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PublishBody {
    #[serde(default)]
    pub message: Option<String>,
}

/// 资源私库没装配（`NOUS_EXTEND` 指到的目录不存在）
/// 时诚实地 409，不退回空 200：编辑器和「没有文件」是两件事。
fn repo(state: &AppState, kind: Kind) -> Result<Arc<Repo>> {
    state.repo(kind.repo_key()).ok_or_else(|| {
        ServerError::Conflict(format!(
            "{}私库未装配（{} 指向的目录不存在？）",
            kind.label(),
            match kind {
                Kind::Extensions => "NOUS_EXTEND",
            }
        ))
    })
}

/// 旧面板 `checkName` 的同一套规矩 —— **实现已下沉到 `nous_repo::entries`**
/// （技能的插件与这里共用同一份，抄第二份就是两处真相）。
pub fn check_name(raw: &str) -> Result<String> {
    nous_repo::check_name(raw).map_err(repo_error)
}

/// 条目目录里的相对路径 —— 同样下沉到 `nous_repo::entries`。
pub fn check_path(raw: &str) -> Result<String> {
    nous_repo::check_path(raw).map_err(repo_error)
}

pub(crate) fn repo_error(error: RepoError) -> ServerError {
    match error {
        RepoError::BadRequest(m) | RepoError::InvalidPath(m) => ServerError::BadRequest(m),
        RepoError::Validation { field, problem } => ServerError::Validation { field, problem },
        RepoError::NotFound(m) => ServerError::NotFound(m),
        // 409 的 `{error}` 只留一句人话：Desk 原样显示它，JSON 塞进字符串里没人看得懂。
        RepoError::Conflict { message, .. } => {
            ServerError::Conflict(format!("{message} —— 先重新打开这个文件，再存一次"))
        }
        RepoError::Busy(m) => ServerError::Conflict(m),
        RepoError::Locked(m) => ServerError::Locked(m),
        RepoError::Io(m) => ServerError::Internal(m),
        RepoError::LockPoisoned => ServerError::Internal("私库写锁中毒".into()),
    }
}

/// `GET /v1/{kind}/files?name=`：递归清单，正文与附件一起给（不按扩展名筛）。
async fn files(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Query(query): Query<NameQuery>,
) -> Result<Json<Value>> {
    let repo = repo(&state, kind)?;
    let name = check_name(&query.name)?;
    let list = nous_repo::list_entry_files(&repo, &name).map_err(repo_error)?;
    Ok(Json(json!({ "name": name, "files": list })))
}

/// `GET /v1/{kind}/file?name=&path=`：正文 + `etag`（写回时当 `base`）。
async fn file(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileQuery>,
) -> Result<Json<Value>> {
    let repo = repo(&state, kind)?;
    let name = check_name(&query.name)?;
    let path = check_path(&query.path)?;
    nous_repo::require_entry(&repo, &name, kind.label()).map_err(repo_error)?;
    let (text, etag) =
        nous_repo::read_text(&repo, &format!("{name}/{path}")).map_err(repo_error)?;
    Ok(Json(json!({ "name": name, "path": path, "text": text, "etag": etag })))
}

/// `PUT /v1/{kind}/file`：只落盘、不提交。`base` 不符时 `nous_repo` 回 409。
async fn save(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveBody>,
) -> Result<Json<Value>> {
    let repo = repo(&state, kind)?;
    let name = check_name(&body.name)?;
    let path = check_path(&body.path)?;
    nous_repo::require_entry(&repo, &name, kind.label()).map_err(repo_error)?;
    let created = body.base.is_none();
    let etag = nous_repo::write_text(
        &repo,
        &format!("{name}/{path}"),
        &body.text,
        body.base.as_deref(),
    )
    .map_err(repo_error)?;
    Ok(Json(json!({ "name": name, "path": path, "etag": etag, "created": created })))
}

/// `POST /v1/{kind}/publish`：把私库里 dirty 的路径提交（并尝试 push）。
/// body 可以省；`message` 空着就用旧面板那句默认文案（随仓变）。
async fn publish(
    kind: Kind,
    State(state): State<Arc<AppState>>,
    Json(body): Json<PublishBody>,
) -> Result<Json<Value>> {
    let repo = repo(&state, kind)?;
    let message = body
        .message
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(kind.default_message());
    let published = nous_repo::publish_entry(&repo, message).map_err(repo_error)?;
    Ok(Json(serde_json::to_value(published).unwrap_or(Value::Null)))
}
