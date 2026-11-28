//! `AppState.repos` 的**装配**：六套私库从哪来、变体绑定表落哪。
//!
//! 端点（`/v1/repos/{repo}/*` 30 条）已经搬进 `nous_repo::routes`（`repo` 模块
//! 自带 controller）。这个文件只剩「进程启动时怎么把仓库打开」这一件事，
//! 属于宿主装配，不是插件职责 —— 插件拿到的永远是句柄（`Host::repo`）。
//!
//! **这里不再有任何错误翻译。** 最后一份 `repo_error` 是给
//! `server/src/trigger_projects.rs` 用的，那份文件已整份搬进
//! `trigger-core`（`crates/Trigger/Core/src/projects.rs`），它在那儿有自己的
//! 一行翻译（`nous_repo::Refusal::from_repo` → `HttpRefusal`）。语义唯一真相是
//! `nous_repo::Refusal`，`ServerError` 这一侧不需要第二份表。

use std::path::Path;
use std::sync::Arc;

use nous_repo::Repo;

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
