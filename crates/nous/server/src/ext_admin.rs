//! #84：扩展管理面的 HTTP 翻译层（create / duplicate / install / uninstall / 类别）。
//!
//! 文件动作在 `nous_ext::AdminStore` 的私库盘面上；运行动作经 #81 的同一个 `Runtime`，
//! 不在这一层另存注册表。鉴权、错误形状、状态码都走 `ServerError`。

use axum::extract::State;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::app::AppState;
use crate::error::{Result, ServerError};
use nous_ext::{CreateOptions, DuplicateOptions, ExtError};

fn bad(error: ExtError) -> ServerError {
    ServerError::BadRequest(error.to_string())
}

fn conflict(error: ExtError) -> ServerError {
    let message = error.to_string();
    if message.contains("已存在") || message.contains("已在源码根") {
        ServerError::Conflict(message)
    } else {
        ServerError::NotFound(message)
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DuplicateBody {
    pub name: String,
    pub to: String,
    #[serde(default)]
    pub unbind: bool,
}

#[derive(Debug, Deserialize)]
pub struct NameBody {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct CategoryBody {
    pub name: String,
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CategoriesBody {
    pub op: String,
    pub name: String,
    #[serde(default)]
    pub to: Option<String>,
}

pub fn attach(router: Router<Arc<AppState>>) -> Router<Arc<AppState>> {
    router
        .route("/v1/extensions/create", axum::routing::post(create))
        .route("/v1/extensions/duplicate", axum::routing::post(duplicate))
        .route("/v1/extensions/install", axum::routing::post(install))
        .route("/v1/extensions/uninstall", axum::routing::post(uninstall))
        .route("/v1/extensions/categories", axum::routing::get(get_categories).post(post_categories))
        .route("/v1/extensions/category", axum::routing::post(category))
}

fn store(state: &AppState) -> Result<&nous_ext::AdminStore> {
    state.ext_admin().ok_or_else(|| ServerError::Conflict("扩展管理面未装配".into()))
}

fn json<T: serde::Serialize>(value: std::result::Result<T, ExtError>) -> Result<Json<Value>> {
    let value = value.map_err(bad)?;
    Ok(Json(serde_json::to_value(value).unwrap_or(Value::Null)))
}

pub async fn create(State(state): State<Arc<AppState>>, Json(body): Json<CreateBody>) -> Result<(axum::http::StatusCode, Json<Value>)> {
    let store = store(&state)?;
    let result = store.create(&CreateOptions { name: body.name, category: body.category });
    match result {
        Ok(value) => Ok((axum::http::StatusCode::CREATED, Json(value))),
        Err(error) => Err(conflict(error)),
    }
}

pub async fn duplicate(State(state): State<Arc<AppState>>, Json(body): Json<DuplicateBody>) -> Result<(axum::http::StatusCode, Json<Value>)> {
    let store = store(&state)?;
    let result = store.duplicate(&DuplicateOptions { name: body.name, to: body.to, unbind: body.unbind });
    match result {
        Ok(value) => Ok((axum::http::StatusCode::CREATED, Json(value))),
        Err(error) => Err(conflict(error)),
    }
}

pub async fn uninstall(State(state): State<Arc<AppState>>, Json(body): Json<NameBody>) -> Result<Json<Value>> {
    let store = store(&state)?;
    let result = store.uninstall(&body.name).map_err(bad)?;
    if let Some(runtime) = state.ext() {
        let _ = runtime.uninstall(&body.name);
    }
    Ok(Json(result))
}

pub async fn install(State(state): State<Arc<AppState>>, Json(body): Json<NameBody>) -> Result<Json<Value>> {
    let store = store(&state)?;
    let mut result = store.install(&body.name).map_err(bad)?;
    let Some(runtime) = state.ext() else {
        result["state"] = json!("stopped");
        return Ok(Json(result));
    };
    let loaded = nous_ext::LoadedManifest::load(&store.root.join(&body.name)).map_err(bad)?;
    runtime.start_one(loaded).map_err(bad)?;
    // 运行时内部五态的 normal 在 wire 上叫 running（与 /v1/extensions 的 items[] 同规矩）。
    let state = match runtime.state_of(&body.name).map(|state| state.as_str()) {
        Some("normal") => "running",
        Some(other) => other,
        None => "stopped",
    };
    result["state"] = json!(state);
    Ok(Json(result))
}

pub async fn get_categories(State(state): State<Arc<AppState>>) -> Result<Json<Value>> {
    Ok(Json(json!({ "categories": store(&state)?.categories() })))
}

pub async fn post_categories(State(state): State<Arc<AppState>>, Json(body): Json<CategoriesBody>) -> Result<Json<Value>> {
    let store = store(&state)?;
    json(store.edit_categories(&body.op, &body.name, body.to.as_deref()))
}

pub async fn category(State(state): State<Arc<AppState>>, Json(body): Json<CategoryBody>) -> Result<Json<Value>> {
    let store = store(&state)?;
    json(store.set_category(&body.name, body.category.as_deref()))
}
