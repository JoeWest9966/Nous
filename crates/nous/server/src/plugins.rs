//! 插件交上来的端点表：`server` 只做「收集 → 装进 Router → 把声明交给注册表」。
//!
//! 语义见 `nous-routes`：一条声明同时是「注册」和「登记」，所以 `capabilities`、
//! `/v1/_routes`、`/v1/openapi.json` 三处读到的必然是真正装进 Router 的那些路径。
//!
//! 还没搬进插件的端点仍在 [`crate::meta::ROUTES`]（静态表）里，两处合流在
//! `meta::endpoints()`；搬一个删一行，表会越来越薄。

use std::sync::Arc;

use crate::app::AppState;

/// 所有插件自己的端点。顺序即注册顺序；同一路径注册两次会在 `Router` 那里炸，不静默。
pub fn routes() -> Vec<nous_routes::Route<Arc<AppState>>> {
    let mut all: Vec<nous_routes::Route<Arc<AppState>>> = Vec::new();
    all.extend(nous_device::routes::routes::<Arc<AppState>>());
    all.extend(element_models::routes::routes::<Arc<AppState>>());
    all.extend(data_data_manager::routes::routes::<Arc<AppState>>());
    all
}

/// 注册表视图（`:id` 形式 + 归属模块）。
pub fn declarations() -> Vec<nous_routes::Declaration> {
    routes()
        .iter()
        .map(nous_routes::Route::declaration)
        .collect()
}

/// `element-models` 插件要的那一件事：元素库仓库。
///
/// 插件不认识 `AppState`，只认识这个 trait；这里把宿主的能力显式交出去。
/// 未装配 = `None`（插件回 `state:"missing"` 的诚实空表）。
impl element_models::routes::Host for AppState {
    fn elements_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        self.repo(crate::elements::REPO_KEY)
    }
}

/// `element-models` 要的另一件事：密钥文件放哪。
///
/// `NOUS_SECRETS_FILE` 优先；没配就是 `/var/lib/nous/secrets/providers.env`
/// （`deploy/nous-app.service` 里那条 `EnvironmentFile=` 读的同一份）。引擎以 `nous` 用户跑，
/// 所以这个位置必须是 `nous` 可写的 —— `/etc/nous/providers.env` 是 `root:nous 0640`，写不进去。
impl element_models::routes::Secrets for AppState {
    fn secrets_file(&self) -> Option<std::path::PathBuf> {
        Some(match std::env::var("NOUS_SECRETS_FILE") {
            Ok(path) if !path.trim().is_empty() => std::path::PathBuf::from(path),
            _ => std::path::PathBuf::from("/var/lib/nous/secrets/providers.env"),
        })
    }
}

/// `data-manager` 插件要的那一件事：两个只读库在哪。
///
/// 路径与启动 / `--check` / `--triggers` 共用同一套推导（`main` 装配一次），
/// 插件拿到的就是那一份 —— 不复算第二次，也就不会读到另一份库。
impl data_data_manager::routes::Host for AppState {
    fn stats_paths(&self) -> Option<data_data_manager::routes::Paths> {
        self.stats().cloned()
    }
}
