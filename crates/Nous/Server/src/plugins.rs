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
    all.extend(library_skill::routes::routes::<Arc<AppState>>());
    all.extend(nous_env::routes::routes::<Arc<AppState>>());
    all.extend(nous_combos::routes::<Arc<AppState>>());
    all.extend(nous_prompts::routes::routes::<Arc<AppState>>());
    all.extend(library_extend::routes::routes::<Arc<AppState>>());
    all.extend(nous_repo::routes::routes::<Arc<AppState>>());
    all.extend(nous_sessions::routes::routes::<Arc<AppState>>());
    all.extend(nous_triggers::routes::routes::<Arc<AppState>>());
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
/// 只读投影要路径（`library_skill::scan` 只吃路径）。第三个走 `SessionSource::usage` ——
/// 会话记录归 `nous-durable`，插件不自己开库。
impl library_skill::routes::Host for AppState {
    fn skills_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        self.repo(library_skill::routes::REPO_KEY)
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

/// `Library_Extend` 插件要的四件事：编辑页的私库句柄、管理面的盘面、运行时、以及「谁在用」。
///
/// 前三件是同一套扩展系统的三种取法，不是三份状态：`ext_repo` 是 git 面（私库键
/// `extend`，URL 段却是 `extensions`）、`ext_admin` 是目录动作的盘面、`ext_runtime`
/// 是进程监管。它们都由 `AppState` 装配一次注入，插件不自己造第二份。
impl library_extend::routes::Host for AppState {
    fn ext_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        self.repo(library_extend::routes::REPO_KEY)
    }

    fn ext_admin(&self) -> Option<&library_extend::AdminStore> {
        self.ext_admin()
    }

    fn ext_runtime(&self) -> Option<&Arc<library_extend::Runtime>> {
        self.ext()
    }

    /// 反查走 `SessionSource::usage`（会话记录归 `nous-durable`）。
    /// 这里返回 `Result` 而不是 `Option`：搬前是 `?` 直传状态码，折成 `None`
    /// 会把 500 变成「未装配」，那是一句假话。
    fn ext_usage(&self) -> std::result::Result<
        serde_json::Value,
        nous_routes::HttpRefusal,
    > {
        self.source().usage("extensions").map_err(refusal)
    }
}

/// `triggers` 插件的两条宿主能力 —— 也就是搬走之后 `server` 还欠它的全部东西。
///
/// 一个是**库里的行**（点火运行时：账本 + 同一个 durable 会话），一个是**盘上的目录**
/// （`triggers` 私库的写锁与 git）。凑成一条「触发器宿主」大对象看着更整齐，但那等于说
/// 「能点火的人自然也拿到了 git 写锁」—— 假的。两条就两条。
impl nous_triggers::routes::Host for AppState {
    fn triggers_runtime(&self) -> Option<&nous_triggers::TriggerRuntime> {
        self.triggers()
    }

    fn triggers_repo(&self) -> Option<Arc<nous_repo::Repo>> {
        self.repo(nous_triggers::routes::REPO_KEY)
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

/// 任何宿主层错误 → 公共壳 [`nous_routes::HttpRefusal`]：状态码与文案逐字搬过去
/// （不在这里换口径）。泛型是因为插件各有自己的错误类型（`SessionError` 等），
/// 它们都有 `Into<ServerError>` —— 状态码与文案仍由 `ServerError` 一处说了算。
fn refusal(error: impl Into<crate::ServerError>) -> nous_routes::HttpRefusal {
    let error: crate::ServerError = error.into();
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

/// `sessions` 插件要的四件事：读投影、写命令、活库句柄、SSE 扇出。
///
/// 四件事都已经是 `AppState` 自己的字段（`source` / `commands` / `hub`），这里只是
/// 换个句柄交出去 —— 会话的语义在插件里，宿主只负责「进程启动时装配了什么」。
impl nous_sessions::routes::Host for AppState {
    fn source(&self) -> Arc<dyn nous_sessions::SessionSource> {
        AppState::source(self).clone()
    }

    fn commands(&self) -> Arc<dyn nous_sessions::commands::CommandSource> {
        AppState::commands(self).clone()
    }

    fn durable_session(&self) -> Option<Arc<nous_durable::session::Session>> {
        AppState::durable_session(self)
    }

    fn hub(&self) -> Arc<nous_sessions::Hub> {
        AppState::hub(self).clone()
    }

    fn models(&self) -> nous_device::ModelsResponse {
        AppState::models(self)
    }

    fn assembled_models(&self) -> Option<nous_device::ModelsResponse> {
        AppState::assembled_models(self)
    }
}

/// `repo` 插件要的四件事：仓库句柄、变体绑定表、上游镜像根、变体引用反查。
///
/// 全同步、全返回 `Option`/`Result`：插件据此自己决定回 400/409/500，
/// 宿主只负责「进程启动时打开了哪些仓库」这一件事。
impl nous_repo::routes::Host for AppState {
    fn repo(&self, kind: &str) -> Option<Arc<nous_repo::Repo>> {
        AppState::repo(self, kind)
    }

    fn variant_table(&self, kind: &str) -> Option<std::path::PathBuf> {
        AppState::variant_table(self, kind)
    }

    fn upstream_data(&self) -> Option<std::path::PathBuf> {
        AppState::upstream_data(self)
    }

    fn source_usage(&self, field: &str) -> Result<serde_json::Value, nous_routes::HttpRefusal> {
        self.source().usage(field).map_err(refusal)
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
        let refused = |status: u16, message: &str| {
            nous_routes::HttpRefusal::new(status, serde_json::json!({ "error": message }))
        };
        nous_env_ssh::remote_subdirs(machine, path, include_hidden, limit)
            .await
            .map_err(|error| match error {
                RemoteDirsError::MachineNotFound => refused(404, "机器不在编目里".into()),
                RemoteDirsError::Unreachable(message) => refused(502, &message),
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
            nous_combos::service_unavailable_refusal("triggers not configured")
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
            nous_combos::service_unavailable_refusal("combo action clock not configured")
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

/// `Device` 插件要的十四件事：编目四份、注册表两份、产物身份三份、运行时自述两份。
///
/// 插件不认识 `AppState`，只认识 `nous_device::routes::Host`。这里每行都是一次显式的
/// 「宿主交出自己的什么」，不是给插件开一扇 `&AppState` 的后门 —— 后者等于让插件
/// 偷偷反依赖 server，`tests/manifest.rs` 的 `cargo tree` 判据会当场红。
impl nous_device::routes::Host for AppState {
    fn models(&self) -> nous_device::ModelsResponse {
        AppState::models(self)
    }

    fn injected_models(&self) -> bool {
        AppState::injected_models(self)
    }

    fn assembled_models(&self) -> Option<nous_device::ModelsResponse> {
        AppState::assembled_models(self)
    }

    fn refreshed_models(&self) -> Option<nous_device::ModelsResponse> {
        AppState::refreshed_models(self)
    }

    fn set_models(&self, models: nous_device::ModelsResponse) {
        AppState::set_models(self, models)
    }

    fn catalog_env(&self) -> nous_device::CatalogEnvironment {
        AppState::catalog_env(self).clone()
    }

    /// 元素库约定组的投影：组不存在 / 私库未装配 → `None`，调用方回落到文件来源。
    fn group_catalog(&self) -> Option<nous_device::ModelsResponse> {
        crate::models::group_catalog(self)
    }

    fn manifest(&self) -> Vec<nous_device::routes::ManifestLine> {
        crate::meta::manifest_lines()
    }

    fn registered(&self) -> Vec<nous_routes::Declaration> {
        crate::meta::registered()
    }

    fn build_id(&self) -> &'static str {
        crate::version::build_id()
    }

    fn target_triple(&self) -> &'static str {
        crate::version::target_triple()
    }

    fn server_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn provider_kind(&self) -> &'static str {
        AppState::provider_kind(self)
    }

    fn origin(&self) -> String {
        AppState::origin(self).to_string()
    }
}
