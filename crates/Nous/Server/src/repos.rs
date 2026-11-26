//! `AppState.repos` 的**装配**：六套私库从哪来、变体绑定表落哪。
//!
//! 端点（`/v1/repos/{repo}/*` 30 条）已经搬进 `nous_repo::routes`（`repo` 模块
//! 自带 controller）。这个文件只剩「进程启动时怎么把仓库打开」这一件事，
//! 属于宿主装配，不是插件职责 —— 插件拿到的永远是句柄（`Host::repo`）。
//!
//! [`repo_error`] 还留在这儿：它只剩 `trigger_projects.rs` 一个调用方，而那个
//! 文件正在 `feat/triggers-plugin` 里搬到 `trigger-core`。等那票落地后它连同
//! 调用点一起收进 `nous_repo::Refusal`，这里就彻底空了。

use std::path::Path;
use std::sync::Arc;

use nous_repo::{Repo, RepoError};
use serde_json::json;

use crate::error::ServerError;

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
            if let Some(etag) = etag {
                value["etag"] = json!(etag);
            }
            // 用统一的 409 响应，但把字段带出去。
            ServerError::Conflict(serde_json::to_string(&value).unwrap())
        }
        RepoError::Io(m) => ServerError::Internal(m),
        RepoError::LockPoisoned => ServerError::Internal("私库写锁中毒".into()),
    }
}

/// 生产装配：从 `nous_env` 的六条路径解析（不另读环境变量）。
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
    if let Ok(repo) = Repo::open(root) {
        map.insert(kind.to_string(), Arc::new(repo));
    }
    map
}

/// #76：生产装配的变体绑定表路径。位于各私库根的 `data/` 下，沿用 env 的路径解析。
pub fn variant_tables_from_env() -> std::collections::BTreeMap<String, std::path::PathBuf> {
    [
        ("skills", nous_env::SKILLS.resolve().join("data/skills.local.json")),
        ("extend", nous_env::EXTEND.resolve().join("data/extensions.local.json")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}
