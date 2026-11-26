//! 本插件自己的端点：`GET /v1/models/elements`。
//!
//! handler 住在插件里（Magento 2 那种「模块自带 controller」）。插件不认识
//! `server` 的 `AppState`，只声明它需要什么（[`Host`]），由宿主实现并注入。
//!
//! 三条不越过的线（和搬家之前一样）：
//!
//! - **密钥只回形状**（`env` / `cmd` / `secret` / `plaintext` / `missing`），值不进响应、
//!   不进日志；元素库也不收明文（出现明文只标红，不回显）。
//! - **缺什么说什么**：缺包、悬空覆盖、默认模型不在表里，都在 `missing[]` / `problems[]` 点名。
//! - **只读**：编目端点不改元素库一个字节（写面走 `/v1/elements`）。
//!
//! 另有 `PUT /v1/providers/{id}/secret`：把一条供应商密钥写进宿主的密钥文件
//! （元素库里仍然只有 `$NAME` 引用，见 [`crate::secret`]）。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::Json;
use serde_json::{json, Value};

use crate::Snapshot;

/// 模块全名（规则 12）：`/v1/capabilities` 的 `modules[].name`、`/v1/_routes` 的
/// `module` 标签都用它 —— 面板靠这个标签把端点挂到模块名下。
///
/// 2026-11-27 由 `Element_Models` 改成 `Models`（目录 `crates/Element/Models`、包名
/// `element-models`、capability `models`、元素库组 id `element-models` 都没动）。
pub const MODULE: &str = "Models";

/// 宿主能力：这个插件只认识「给我元素库仓库」这一件事。
///
/// 未装配（目录不存在）回 `None` —— 那是「还没有」，不是「没这个端点」。
pub trait Host {
    fn elements_repo(&self) -> Option<Arc<nous_repo::Repo>>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
impl<T: Host + ?Sized> Host for Arc<T> {
    fn elements_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        (**self).elements_repo()
    }
}

/// 宿主能力：密钥文件放哪。`None` = 宿主没开这个能力（端点直说，不假装写成功）。
///
/// 写什么进文件由插件决定（`NAME=value`、原子替换、`0600`）；**放哪由宿主决定**。
pub trait Secrets {
    fn secrets_file(&self) -> Option<std::path::PathBuf>;
}

/// `Arc<T>` 同样转发。
impl<T: Secrets + ?Sized> Secrets for Arc<T> {
    fn secrets_file(&self) -> Option<std::path::PathBuf> {
        (**self).secrets_file()
    }
}

/// 本插件的端点表。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Secrets + Clone + Send + Sync + 'static,
{
    vec![
        nous_routes::Route::new("GET", "/v1/models/elements", MODULE, || {
            get(handler::<H>)
        }),
        nous_routes::Route::new("PUT", "/v1/providers/{id}/secret", MODULE, || {
            put(store_secret::<H>)
        }),
    ]
}

/// 元素库 → 约定组投影。私库扫不动 / 组不存在都回 `None`。
pub fn snapshot(repo: &nous_repo::Repo) -> Option<Snapshot> {
    let groups = nous_elements::scan(repo, nous_elements::Kind::Groups).ok()?;
    let group = crate::find_group(&groups.items)?;
    let packages = nous_elements::scan(repo, nous_elements::Kind::Packages).ok()?;
    Some(crate::load(group, &packages.items))
}

/// 元素库（组 / 包两段）→ `GET /v1/models/elements` 的整份回包。
///
/// 纯函数：只读仓库，不联网、不看时钟。扫不动就 `Err`（宿主的问题，不是「还没有」）。
pub fn projection(repo: &nous_repo::Repo) -> Result<Value, String> {
    let groups = nous_elements::scan(repo, nous_elements::Kind::Groups)
        .map_err(|error| format!("扫元素库失败：{error}"))?;
    let packages = nous_elements::scan(repo, nous_elements::Kind::Packages)
        .map_err(|error| format!("扫元素库失败：{error}"))?;

    let repo_block = json!({ "root": repo.root().to_string_lossy(), "present": true });
    let Some(group) = crate::find_group(&groups.items) else {
        let problems: Vec<Value> = groups
            .broken
            .iter()
            .filter(|broken| broken.id == crate::GROUP_ID)
            .map(|broken| {
                json!({
                    "element": broken.id,
                    "field": "file",
                    "problem": broken.detail,
                })
            })
            .collect();
        // 组读不出来时别假装「没配」：把解析原文带上。
        let detail = if problems.is_empty() {
            format!("没有 {} 这个组", crate::GROUP_ID)
        } else {
            format!("{} 这份组读不出来（见 problems）", crate::GROUP_ID)
        };
        return Ok(json!({
            "kind": "models",
            "state": "missing",
            "repo": repo_block,
            "items": [],
            "providers": [],
            "packages": [],
            "missing": [crate::GROUP_ID],
            "problems": problems,
            "detail": detail,
        }));
    };

    let snapshot = crate::load(group, &packages.items);
    let mut body = serde_json::to_value(&snapshot).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut body {
        map.insert("kind".into(), json!("models"));
        map.insert("repo".into(), repo_block);
        // 同一目录里坏掉的包也列出来：它们没进 items，但不该悄悄消失。
        if !packages.broken.is_empty() {
            let mut problems = map
                .get("problems")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            problems.extend(packages.broken.iter().map(|broken| {
                json!({ "element": broken.id, "field": "file", "problem": broken.detail })
            }));
            map.insert("problems".into(), Value::Array(problems));
            map.insert("state".into(), json!("problem"));
        }
    }
    Ok(body)
}

/// 私库未装配时的那份回包：`state:"missing"` + 点名环境变量。
pub fn unassembled() -> Value {
    json!({
        "kind": "models",
        "state": "missing",
        "repo": { "root": nous_env::ELEMENTS.resolve().to_string_lossy(), "present": false },
        "items": [],
        "providers": [],
        "packages": [],
        "missing": [nous_env::ELEMENTS.var],
        "problems": [],
        "detail": format!(
            "元素库私库未装配：{} 指向的目录不存在？",
            nous_env::ELEMENTS.var
        ),
    })
}

/// `GET /v1/models/elements`：约定组 → 编目（`items[]` / `default`）+ 诊断。
///
/// 私库没装配 / 组不存在 = `state:"missing"` 空表（不是 404：这是「还没有」，不是「没这个端点」）。
async fn handler<H>(State(state): State<H>) -> Result<Json<Value>, (StatusCode, Json<Value>)>
where
    H: Host + Clone + Send + Sync + 'static,
{
    let Some(repo) = state.elements_repo() else {
        return Ok(Json(unassembled()));
    };
    projection(&repo).map(Json).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error })),
        )
    })
}

/// `PUT /v1/providers/{id}/secret { value }`：把供应商密钥写进宿主的密钥文件。
///
/// 元素库**不改**：它只存 `$NAME` 引用，值落在那份 `EnvironmentFile` 里。
/// 回执不带值，只回「写了哪个变量、写到哪、要不要重启」。
async fn store_secret<H>(
    State(state): State<H>,
    Path(provider): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)>
where
    H: Host + Secrets + Clone + Send + Sync + 'static,
{
    let refuse = |code: StatusCode, message: String| (code, Json(json!({ "error": message })));

    let Some(repo) = state.elements_repo() else {
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("元素库私库未装配：{} 指向的目录不存在？", nous_env::ELEMENTS.var),
        ));
    };
    let Some(path) = state.secrets_file() else {
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "宿主没开密钥写入能力：NOUS_SECRETS_FILE 没配".to_string(),
        ));
    };
    let value = match body.get("value").and_then(Value::as_str) {
        Some(value) => value,
        None => {
            return Err(refuse(
                StatusCode::UNPROCESSABLE_ENTITY,
                "missing field value —— 请求体要是 { \"value\": \"...\" }".to_string(),
            ))
        }
    };

    let groups = nous_elements::scan(&repo, nous_elements::Kind::Groups)
        .map_err(|error| refuse(StatusCode::INTERNAL_SERVER_ERROR, format!("扫元素库失败：{error}")))?;
    let packages = nous_elements::scan(&repo, nous_elements::Kind::Packages)
        .map_err(|error| refuse(StatusCode::INTERNAL_SERVER_ERROR, format!("扫元素库失败：{error}")))?;
    let Some(group) = crate::find_group(&groups.items) else {
        return Err(refuse(
            StatusCode::NOT_FOUND,
            format!("没有 {} 这个组", crate::GROUP_ID),
        ));
    };

    match crate::secret::store(group, &packages.items, &provider, value, &path) {
        Ok(stored) => Ok(Json(json!({
            "kind": "secret",
            "provider": stored.provider,
            "package": stored.package,
            "name": stored.env,
            "path": stored.path.to_string_lossy(),
            "replaced": stored.replaced,
            "restartRequired": true,
            "detail": "引擎只在启动时读密钥：重启后生效（密钥文件换了位置也要重启一次）",
        }))),
        Err(refusal) => {
            let code = match &refusal {
                crate::secret::Refusal::UnknownProvider(_) => StatusCode::NOT_FOUND,
                crate::secret::Refusal::UnknownPackage(_) => StatusCode::NOT_FOUND,
                crate::secret::Refusal::NotEnv { .. } => StatusCode::CONFLICT,
                crate::secret::Refusal::NoKey(_) => StatusCode::UNPROCESSABLE_ENTITY,
                crate::secret::Refusal::Plaintext(_) => StatusCode::UNPROCESSABLE_ENTITY,
                crate::secret::Refusal::Bad(_) => StatusCode::UNPROCESSABLE_ENTITY,
                crate::secret::Refusal::Unavailable(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err(refuse(code, refusal.message()))
        }
    }
}
