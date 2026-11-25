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
    all.extend(data_archive::routes::routes::<Arc<AppState>>());
    all.extend(nous_elements::routes::routes::<Arc<AppState>>());
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
        self.repo(nous_elements::routes::REPO_KEY)
    }
}

/// `elements` 插件要的那一件事：元素库仓库。
///
/// 与 `element-models` 要的是同一个仓库 —— 两条能力各自声明，因为它们是两个插件：
/// `elements` 读写元素本身，`element-models` 只把它投影成编目。共用一份 `AppState.repos`
/// 里的 `"elements"` 键，所以看到的必然是同一个根。
impl nous_elements::routes::Host for AppState {
    fn elements_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        self.repo(nous_elements::routes::REPO_KEY)
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

/// `archive` 插件要的三件事：归档 / 列世代 / 打捞。
///
/// 全部落在同一个 `CommandSource`（同一个 `Session` 写者）上 —— 插件不另开写通道。
/// 错误语义只在 `commands.rs` 的 `command_error` 里判一次（409 与 400 的分界），
/// 这里只把 `ServerError` 翻成插件认识的状态码 + 文案，不重算。
#[async_trait::async_trait]
impl data_archive::routes::Host for AppState {
    async fn archive_session(
        &self,
        session: i64,
        at: i64,
    ) -> Result<nous_durable::session::commands::ArchiveResult, data_archive::routes::Refusal> {
        self.commands()
            .archive(session, at, None)
            .await
            .map_err(refusal)
    }

    async fn archive_generations(
        &self,
    ) -> Result<Vec<nous_durable::storage::archive::Generation>, data_archive::routes::Refusal> {
        self.commands().archives().await.map_err(refusal)
    }

    async fn restore_session(
        &self,
        session: i64,
        gen: String,
        at: i64,
    ) -> Result<nous_durable::session::commands::RestoreResult, data_archive::routes::Refusal> {
        self.commands()
            .restore(session, gen, at)
            .await
            .map_err(refusal)
    }
}

/// `ServerError` → 插件侧的拒绝理由：状态码与文案逐字搬过去（不在这里换口径）。
fn refusal(error: crate::ServerError) -> data_archive::routes::Refusal {
    data_archive::routes::Refusal::new(error.status(), error.message())
}
