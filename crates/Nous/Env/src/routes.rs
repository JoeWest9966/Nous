//! 本插件自己的端点：`GET /v1/fs/dirs`（#68 本机 / #78 远端）。
//!
//! Magento 2 那种「模块自带 controller」：handler 住在插件里，插件不认识 `server` 的
//! `AppState`，只声明它需要什么（[`Host`]：一条远端浏览命令），由宿主实现并注入。
//!
//! ## 为什么远端是宿主能力
//!
//! `nous-env-ssh` 的 `requires` 里就有 `nous-env`（它要实现这里的 `ExecutionEnv`），
//! 所以本 crate 直接调它会成环。远端那条分支只能反过来由宿主注入 —— 这也是它本来就
//! 该有的方向：本 crate 是契约层，不知道有几个实现，更不知道远端用什么连。
//!
//! ## 搬家前后一字未改的几条
//!
//! - **路径规则只在这里**：缺省起点取 `NOUS_START_DIRS` 第一段（[`crate::start_dir`]），
//!   `~` 一律 400，符号链接不算目录，点开头默认跳过。
//! - **状态码跟着语义走**：相对路径 400、不存在 404、没权限 403、不是目录 400、
//!   连不上远端 502、编目里没有这台机器 404。**绝不静默退回本机目录。**
//! - **回包形状**：`{path, dirs:[{name, path, hidden}], truncated}`，最多 500 条。

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use nous_routes::HttpRefusal;

use crate::dirs::{is_local_machine, list_subdirs, BrowseDirs, BrowseError};

/// 这个插件在注册表里的模块名（`docs/plugins.md` 的 module 列）。
pub const MODULE: &str = "env";

/// 目录浏览上限。超出的项不计入 `dirs`，只把 `truncated` 翻成 true。
pub const FS_DIRS_LIMIT: usize = 500;

/// 宿主能力：远端目录浏览。**一个方法一个能力**。
///
/// 只有 `?machine=` 指向**别的机器**时才被调用；本机名 / 省略 / 空串一律走本地分支，
/// 宿主不会看到这些请求。远端实现的 `requires` 里有本 crate，所以只能由宿主注入。
#[async_trait]
pub trait Host: Send + Sync {
    async fn remote_subdirs(
        &self,
        machine: &str,
        path: &str,
        include_hidden: bool,
        limit: usize,
    ) -> Result<BrowseDirs, HttpRefusal>;
}

/// `Arc<T>` 也是宿主：路由装进 `Router<Arc<AppState>>` 时不用再写一层转发。
#[async_trait]
impl<T: Host + ?Sized> Host for Arc<T> {
    async fn remote_subdirs(
        &self,
        machine: &str,
        path: &str,
        include_hidden: bool,
        limit: usize,
    ) -> Result<BrowseDirs, HttpRefusal> {
        (**self)
            .remote_subdirs(machine, path, include_hidden, limit)
            .await
    }
}

impl From<BrowseError> for HttpRefusal {
    /// 本地分支的五种失败。文案与搬家前 `server::routes::fs_dirs_error` 逐字一致。
    fn from(error: BrowseError) -> Self {
        match error {
            BrowseError::RelativePath => Self::new(StatusCode::BAD_REQUEST, "path 必须是绝对路径"),
            BrowseError::NotFound => Self::new(StatusCode::NOT_FOUND, "目录不存在"),
            BrowseError::PermissionDenied => {
                Self::new(StatusCode::FORBIDDEN, "没有权限读这个目录")
            }
            BrowseError::NotDirectory => Self::new(StatusCode::BAD_REQUEST, "path 不是目录"),
            BrowseError::Other => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "目录读不出来")
            }
        }
    }
}

type EndpointResult = std::result::Result<Json<Value>, HttpRefusal>;

/// 本插件的端点表。
pub fn routes<H>() -> Vec<nous_routes::Route<H>>
where
    H: Host + Clone + Send + Sync + 'static,
{
    vec![nous_routes::Route::new(
        "GET",
        "/v1/fs/dirs",
        MODULE,
        || get(browse::<H>),
    )]
}

/// `GET /v1/fs/dirs` 的查询参数。未知字段由 axum 忽略，协议保持哑的。
#[derive(Debug, Deserialize)]
pub struct FsDirsQuery {
    pub path: Option<String>,
    pub machine: Option<String>,
    pub hidden: Option<String>,
}

/// `GET /v1/fs/dirs`：只读列子目录。
///
/// 显式远端 `machine` 走宿主能力；编目里没有 → 404，连不上 → 502，
/// **绝不静默退回本机**。
async fn browse<H>(State(host): State<H>, Query(query): Query<FsDirsQuery>) -> EndpointResult
where
    H: Host + Clone + Send + Sync + 'static,
{
    let hidden = query.hidden.as_deref() == Some("1");
    if let Some(machine) = query.machine.as_deref().filter(|name| !name.is_empty()) {
        if !is_local_machine(Some(machine)) {
            let result = host
                .remote_subdirs(
                    machine,
                    query.path.as_deref().unwrap_or(""),
                    hidden,
                    FS_DIRS_LIMIT,
                )
                .await?;
            return Ok(Json(serde_json::to_value(result).unwrap_or(Value::Null)));
        }
    }
    let result = list_subdirs(query.path.as_deref().unwrap_or(""), hidden, FS_DIRS_LIMIT)?;
    Ok(Json(serde_json::to_value(result).unwrap_or(Value::Null)))
}
