//! 插件交上来的端点表：`server` 只做「收集 → 装进 Router → 把声明交给注册表」。
//!
//! 语义见 `nous-routes`：一条声明同时是「注册」和「登记」，所以 `capabilities`、
//! `/v1/_routes`、`/v1/openapi.json` 三处读到的必然是真正装进 Router 的那些路径。
//!
//! 还没搬进插件的端点仍在 [`crate::meta::ROUTES`]（静态表）里，两处合流在
//! `meta::endpoints()`；搬一个删一行，表会越来越薄。

use std::sync::Arc;

use axum::http::StatusCode;

use crate::app::AppState;

/// 所有插件自己的端点。顺序即注册顺序；同一路径注册两次会在 `Router` 那里炸，不静默。
pub fn routes() -> Vec<nous_routes::Route<Arc<AppState>>> {
    let mut all: Vec<nous_routes::Route<Arc<AppState>>> = Vec::new();
    all.extend(nous_device::routes::routes::<Arc<AppState>>());
    all.extend(element_models::routes::routes::<Arc<AppState>>());
    all.extend(data_data_manager::routes::routes::<Arc<AppState>>());
    all.extend(data_archive::routes::routes::<Arc<AppState>>());
    all.extend(nous_elements::routes::routes::<Arc<AppState>>());
    all.extend(nous_presets_admin::routes::routes::<Arc<AppState>>());
    all.extend(nous_skills::routes::routes::<Arc<AppState>>());
    all.extend(nous_env::routes::routes::<Arc<AppState>>());
    all.extend(nous_combos::routes::<Arc<AppState>>());
    all.extend(nous_prompts::routes::routes::<Arc<AppState>>());
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

/// `skills` 插件要的三件事：技能私库句柄、它的根目录、以及「谁在用」的反查。
///
/// 前两个是同一份私库的两种取法：管理面要 [`nous_repo::Repo`]（写锁与 git 提交在里面），
/// 只读投影要路径（`nous_skills::scan` 只吃路径）。第三个走 `SessionSource::usage` ——
/// 会话记录归 `nous-durable`，插件不自己开库。
impl nous_skills::routes::Host for AppState {
    fn skills_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        self.repo(nous_skills::routes::REPO_KEY)
    }

    fn skills_root(&self) -> std::path::PathBuf {
        nous_env::SKILLS.resolve()
    }

    fn skills_usage(&self) -> Option<serde_json::Value> {
        self.source().usage("skills").ok()
    }
}

/// `prompts` 插件要的两件事：模板私库句柄、它的根目录。
///
/// 与 `skills` 同一个形状 —— 管理面要 [`nous_repo::Repo`]（写锁与 git 提交在里面），
/// 只读投影要路径（`nous_prompts::catalog_value` 只吃路径）。
impl nous_prompts::routes::Host for AppState {
    fn prompts_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        self.repo(nous_prompts::routes::REPO_KEY)
    }

    fn prompts_root(&self) -> std::path::PathBuf {
        nous_env::PROMPTS.resolve()
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
    ) -> Result<nous_durable::session::commands::ArchiveResult, nous_routes::HttpRefusal> {
        self.commands()
            .archive(session, at, None)
            .await
            .map_err(refusal)
    }

    async fn archive_generations(
        &self,
    ) -> Result<Vec<nous_durable::storage::archive::Generation>, nous_routes::HttpRefusal> {
        self.commands().archives().await.map_err(refusal)
    }

    async fn restore_session(
        &self,
        session: i64,
        gen: String,
        at: i64,
    ) -> Result<nous_durable::session::commands::RestoreResult, nous_routes::HttpRefusal> {
        self.commands()
            .restore(session, gen, at)
            .await
            .map_err(refusal)
    }
}

/// `ServerError` → 公共壳 [`nous_routes::HttpRefusal`]：状态码与文案逐字搬过去（不在这里换口径）。
fn refusal(error: crate::ServerError) -> nous_routes::HttpRefusal {
    nous_routes::HttpRefusal::new(
        error.status().as_u16(),
        serde_json::json!({ "error": error.message() }),
    )
}

/// `Nous_PresetsAdmin` 要的那一件事：模板私库根 + 插件库 + 外部名册根。
///
/// 搬家前这几条路径由 `server::presets::Sources::from_env()` 自己读环境；现在由宿主
/// 装配一次（`main` 的 `with_presets`），插件只接 —— 路径解析仍然只有 `env` 插件一处。
impl nous_presets_admin::routes::Host for AppState {
    fn preset_sources(&self) -> Option<nous_presets_admin::routes::Sources> {
        self.presets().cloned()
    }
}

/// `env` 插件要的那一件事：远端目录浏览。
///
/// 本地分支不走这里（插件自己读 `NOUS_START_DIRS`、自己判本机名）；只有 `?machine=`
/// 指向别的机器时才落到这条。远端实现的 `requires` 里有 `nous-env`，所以只能由宿主
/// 注入 —— 这也是它本来就该有的方向：契约层不知道有几个实现、更不知道远端用什么连。
///
/// 状态码的分界（编目里没有 404 / 连不上 502 / 路径相对 400 / 没权限 403）在这里判一次，
/// 装进公共壳 [`nous_routes::HttpRefusal`]。
#[async_trait::async_trait]
impl nous_env::routes::Host for AppState {
    async fn remote_subdirs(
        &self,
        machine: &str,
        path: &str,
        include_hidden: bool,
        limit: usize,
    ) -> Result<nous_env::BrowseDirs, nous_routes::HttpRefusal> {
        use nous_env_ssh::RemoteDirsError;
        let refused = |status: u16, message: String| {
            nous_routes::HttpRefusal::new(status, serde_json::json!({ "error": message }))
        };
        nous_env_ssh::remote_subdirs(machine, path, include_hidden, limit)
            .await
            .map_err(|error| match error {
                RemoteDirsError::MachineNotFound => refused(404, "机器不在编目里".into()),
                RemoteDirsError::Unreachable(message) => refused(502, message),
                RemoteDirsError::RelativePath => refused(400, "path 必须是绝对路径".into()),
                RemoteDirsError::NotFound => refused(404, "远端目录不存在".into()),
                RemoteDirsError::PermissionDenied => {
                    refused(403, "没有权限读这个远端目录".into())
                }
                RemoteDirsError::NotDirectory => refused(400, "path 不是目录".into()),
                RemoteDirsError::Other => refused(500, "远端目录读不出来".into()),
            })
    }
}

/// `combos` 插件要的四件事：两个盘面根、触发器判定视图、注入时钟、起会话 + 写血缘。
///
/// 起会话那条命令落在 `nous_durable::session::commands`（活库写锁与补偿方向都在那里），
/// 插件不自己开库、不自己读时钟。血缘记录 `record_origin` 与搬家前同一条路径。
#[async_trait::async_trait]
impl nous_combos::Host for AppState {
    fn combo_roots(&self) -> Option<nous_combos::ComboStore> {
        self.combos().cloned()
    }

    fn trigger_spec(&self, code: &str) -> Result<nous_combos::SpecView, nous_routes::HttpRefusal> {
        let runtime = self.triggers().ok_or_else(|| {
            nous_combos::fn_service_unavailable("triggers not configured")
        })?;
        let definitions = runtime.definitions();
        let trigger = definitions
            .iter()
            .find(|definition| definition.code() == code)
            .ok_or_else(|| {
                nous_routes::HttpRefusal::not_found(format!("组合钉的触发器 `{code}` 不在册"))
            })?;
        let spec = trigger.spec();
        Ok(nous_combos::SpecView {
            enabled: trigger.enabled(),
            schedule: spec.schedule.clone(),
            on: spec.on.clone(),
        })
    }

    fn combo_clock(&self) -> Result<(i64, bool), nous_routes::HttpRefusal> {
        let action = self.combo_action().ok_or_else(|| {
            nous_combos::fn_service_unavailable("combo action clock not configured")
        })?;
        Ok((action.clock.now_ms() as i64, action.clock_allowed))
    }

    async fn launch_session(
        &self,
        ultimate: &str,
        trigger: &str,
        session_template: &str,
        forced: bool,
        at: i64,
    ) -> Result<i64, nous_routes::HttpRefusal> {
        use nous_durable::session::commands::{record_origin, OriginRecord};
        let durable = self.durable_session().ok_or_else(|| {
            nous_routes::HttpRefusal::conflict("combos actions need a durable session")
        })?;
        let created = durable
            .create_conversation(serde_json::json!({
                "name": format!("▶ {ultimate}"),
                "template": session_template,
            }))
            .await
            .map_err(|error| {
                nous_routes::HttpRefusal::internal(format!("create_conversation: {error}"))
            })?;
        let session = created
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                nous_routes::HttpRefusal::internal("create_conversation 没回 id")
            })?;
        record_origin(
            &durable,
            OriginRecord {
                id: session,
                ultimate: ultimate.to_string(),
                trigger: trigger.to_string(),
                at,
                forced,
            },
        )
        .await
        .map_err(|error| nous_routes::HttpRefusal::internal(format!("record_origin: {error}")))?;
        Ok(session)
    }
}
