//! 进程内的装配点：数据源 + 广播通道，以及「一次提交 → 这个会话的订阅者」这一跳。
//!
//! 生产里 `advance` 由 `nous_durable::session::Session` 的 `CommitObserver` 同步调用。
//! 那一层不许 await（上游契约），所以这里整条路径是同步的：投影 → `broadcast::send`。
//! 合帧在连接侧（[`crate::coalesce`]），不在提交侧 —— 提交侧攒帧会让「工具启动」晚到。

use std::sync::Arc;

use nous_durable::session::publish::{CommitChange, CommitPublication, DocumentCommitChange};
use nous_wire::RemoteEvent;

use crate::commands::{CommandSource, DurableCommands, ReadOnlyCommands};
use crate::runtime::AssembledRuntime;
use crate::error::Result;
use crate::hub::Hub;
use crate::observer::SessionObserver;
use crate::project;
use crate::source::{ImportedSource, SessionSource};

/// 路由共享的状态。
pub struct AppState {
    source: Arc<dyn SessionSource>,
    commands: Arc<dyn CommandSource>,
    hub: Arc<Hub>,
    models: nous_device::ModelsResponse,
    /// #79：最近一次远端刷新的编目（成功时才写，失败保留旧表）。
    refreshed: std::sync::Mutex<Option<nous_device::ModelsResponse>>,
    /// #65：模型编目的来源快照（`NOUS_MODELS` / `NOUS_MODELS_FILE`），由入口读一次。
    catalog_env: nous_device::CatalogEnvironment,
    /// 显式注入的编目（faux 验收 / 测试快照）优先于文件来源。
    injected_models: bool,
    runtime: std::sync::Mutex<Option<Arc<AssembledRuntime>>>,
    /// `capabilities.services[engine].base` 的单一来源：环境变量优先，绑定后由 `main` 覆盖。
    origin: String,
    /// #57：点火路径的装配（store + 同一个 Session + 模板读取）。未装配的测试路由回 503。
    triggers: Option<nous_triggers::TriggerRuntime>,
    /// #77：镜像缓存根（`data/upstream/<name>/`）。`None` = 没有 data 根，上游路由如实报错。
    upstream_data: Option<std::path::PathBuf>,
    /// #75：四套私库（presets / skills / prompts / extend）的读写与发布面。
    repos: std::collections::BTreeMap<String, std::sync::Arc<nous_repo::Repo>>,
    /// #76：变体绑定表路径（skills / extend）。
    variant_tables: std::collections::BTreeMap<String, std::path::PathBuf>,
    /// #60：模板私库 + 修订表的装配来源（整段串行锁在插件里）。按 env 的路径解析，进程内不留 map。
    presets: Option<nous_presets_admin::routes::Sources>,
    /// #62：stats 的只读句柄（两个库路径）。没有装配时端点回 409，不假装空数据。
    /// #62/搬家：data-manager 插件的只读句柄（两个库路径）。
    stats: Option<data_data_manager::routes::Paths>,
    /// #81：扩展运行时。只有一个入口可 spawn 扩展子进程，装配一次后由端点读观测。
    ext: Option<Arc<library_extend::Runtime>>,
    /// #84：扩展源码管理面（私库根 + 垃圾站）。运行时装配时一并注入。
    ext_admin: Option<library_extend::AdminStore>,
    /// #61：组合名册的盘面根 + 触发器名册根。未装配的测试路由回 409（不是空 200）。
    combos: Option<nous_combos::ComboStore>,
    /// #82：试跑判定用的注入时钟与「时钟许可」这一外部事实，生产由 `main` 装配一次。
    combo_action: Option<nous_combos::Clock>,
}

impl AppState {
    pub fn new(
        source: Arc<dyn SessionSource>,
        commands: Arc<dyn CommandSource>,
        hub: Arc<Hub>,
    ) -> Arc<Self> {
        Arc::new(Self {
            source,
            commands,
            hub,
            models: nous_device::ModelsResponse::default(),
            refreshed: std::sync::Mutex::new(None),
            catalog_env: nous_device::CatalogEnvironment::from_env(),
            injected_models: false,
            runtime: std::sync::Mutex::new(None),
            origin: std::env::var("NOUS_ORIGIN")
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:4789".to_string()),
            triggers: None,
            upstream_data: std::env::var_os("NOUS_DATA").map(std::path::PathBuf::from),
            repos: crate::repos::from_env(),
            variant_tables: crate::repos::variant_tables_from_env(),
            presets: None,
            stats: None,
            ext: None,
            ext_admin: None,
            combos: None,
            combo_action: None,
        })
    }

    /// 把 presets 写者接上（main 装配一次；测试按临时目录注入）。
    pub fn with_presets(self: Arc<Self>, sources: nous_presets_admin::routes::Sources) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            refreshed: std::sync::Mutex::new(
                self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: Some(sources),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
        })
    }

    pub fn presets(&self) -> Option<&nous_presets_admin::routes::Sources> {
        self.presets.as_ref()
    }

    /// 只读装配：读端点可用，写端点一律 409（不假装能写）。
    pub fn read_only(source: Arc<dyn SessionSource>) -> Arc<Self> {
        Self::new(source.clone(), ReadOnlyCommands::new(source), Hub::new())
    }

    /// 读 + 写都接在**同一份库**上。写走 `Session`，`CommitObserver` 挂到同一个 hub ——
    /// 提交落库后帧立刻进 SSE，端点不用自己补发。
    pub fn durable_with_commands(
        source: Arc<dyn SessionSource>,
        commands: Arc<dyn CommandSource>,
    ) -> Arc<Self> {
        Self::new(source, commands, Hub::new())
    }

    pub fn durable(path: impl AsRef<std::path::Path>) -> Result<Arc<Self>> {
        let path = path.as_ref();
        // 先开写后端：空路径由它的迁移建表（`ImportedSource::open` 会立刻读一次，
        // 文件还不存在时先读会报 "unable to open database file"）。
        let commands = DurableCommands::open(path)?;
        let source = ImportedSource::open(path)?;
        let state = Arc::new(Self {
            source,
            commands: commands.clone(),
            hub: Hub::new(),
            models: nous_device::ModelsResponse::default(),
            refreshed: std::sync::Mutex::new(None),
            catalog_env: nous_device::CatalogEnvironment::from_env(),
            injected_models: false,
            runtime: std::sync::Mutex::new(None),
            origin: std::env::var("NOUS_ORIGIN")
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:4789".to_string()),
            triggers: None,
            upstream_data: std::env::var_os("NOUS_DATA").map(std::path::PathBuf::from),
            repos: crate::repos::from_env(),
            variant_tables: crate::repos::variant_tables_from_env(),
            presets: None,
            stats: None,
            ext: None,
            ext_admin: None,
            combos: None,
            combo_action: None,
        });
        commands.attach(SessionObserver::new(state.hub.clone()));
        Ok(state)
    }

    /// 用默认 `Hub` 的只读装配（判据里读快照/SSE 用）。
    pub fn with_source(source: Arc<dyn SessionSource>) -> Arc<Self> {
        Self::read_only(source)
    }

    pub fn source(&self) -> &Arc<dyn SessionSource> {
        &self.source
    }

    pub fn commands(&self) -> &Arc<dyn CommandSource> {
        &self.commands
    }

    /// 只读投影（origins / unread）用的 `Session`：只读装配没有。
    pub fn durable_session(&self) -> Option<Arc<nous_durable::session::Session>> {
        self.commands.session()
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    /// 换入真编目：默认 catalog 是空的，宿主填进来才有内容。
    pub fn with_models(self: Arc<Self>, models: nous_device::ModelsResponse) -> Arc<Self> {
        self.source.set_model_levels(&models);
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models,
            catalog_env: self.catalog_env.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            injected_models: true,
            runtime: std::sync::Mutex::new(
                self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
        })
    }

    /// 最近一次远端刷新的编目优先；没有就回静态来源的那份。
    pub fn models(&self) -> nous_device::ModelsResponse {
        self.refreshed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_else(|| self.models.clone())
    }

    /// 只看远端刷新那一份（没有就是 None），给路由判断要不要读文件来源。
    pub fn refreshed_models(&self) -> Option<nous_device::ModelsResponse> {
        self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// 远端刷新成功后换入新编目；失败不调用，旧表原样保留。
    pub fn set_models(&self, models: nous_device::ModelsResponse) {
        *self.refreshed.lock().unwrap_or_else(|p| p.into_inner()) = Some(models);
    }

    pub fn injected_models(&self) -> bool {
        self.injected_models
    }

    /// 换入编目来源快照（测试/入口一次性注入；运行时不读全局环境）。
    pub fn with_catalog_env(self: Arc<Self>, catalog_env: nous_device::CatalogEnvironment) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            catalog_env,
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(
                self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
        })
    }

    pub fn catalog_env(&self) -> &nous_device::CatalogEnvironment {
        &self.catalog_env
    }

    /// `NOUS_ORIGIN` 优先；绑到实际端口后由 `main` 用真地址覆盖。
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// 绑定完成后把实际监听地址告诉 state；返回新的 `Arc` 供 router 使用。
    pub fn with_origin(self: Arc<Self>, origin: impl Into<String>) -> Arc<Self> {
        let origin = origin.into();
        if origin == self.origin {
            return self;
        }
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(
                self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            origin,
            triggers: self.triggers.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
        })
    }

    /// 启动时补写孤儿终态：走**已经持有的那个写者**，不另开 `Session`。
    pub async fn reclaim_orphans(&self) -> Result<usize> {
        self.commands.reclaim_orphans().await.map_err(Into::into)
    }

    /// 装上组装根（provider / tool 注册表），只在 `main` 里做一次。
    pub fn with_runtime(self: Arc<Self>, runtime: Arc<AssembledRuntime>) -> Arc<Self> {
        *self.runtime.lock().unwrap_or_else(|p| p.into_inner()) = Some(runtime);
        self
    }

    pub fn runtime(&self) -> Option<Arc<AssembledRuntime>> {
        self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// faux 轮次用的帧出口：把驱动器产出的状态帧送进**本进程唯一**的 Hub。
    /// 会话 id 由调用点给出，不让事件自己猜路由。
    pub fn turn_event_sink(
        self: &Arc<Self>,
        session: i64,
    ) -> Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync> {
        let state = self.clone();
        Arc::new(move |event| state.publish_event(session, event.clone()))
    }

    /// faux 装配的 provider 种类（`"faux"` / `"real"`）。只读诊断，不参与轮次。
    pub fn provider_kind(&self) -> &'static str {
        self.runtime()
            .map(|runtime| runtime.provider_kind)
            .unwrap_or("real")
    }

    /// faux 装配的编目；没有装配时为空（诚实）。
    pub fn assembled_models(&self) -> Option<nous_device::ModelsResponse> {
        self.runtime().map(|runtime| runtime.models.clone())
    }

    /// #75：按私库名取 repository（未装配返回 None，由路由报 400/500）。
    pub fn repo(&self, kind: &str) -> Option<std::sync::Arc<nous_repo::Repo>> {
        self.repos.get(kind).cloned()
    }

    /// #77：镜像缓存根；测试显式注入，生产从 `NOUS_DATA` 读一次。
    pub fn upstream_data(&self) -> Option<std::path::PathBuf> {
        self.upstream_data.clone()
    }

    /// #77：测试注入 data 根（只用于进程内 Router 测试）。
    pub fn with_upstream_data_for_test(self: Arc<Self>, data: impl AsRef<std::path::Path>) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(), commands: self.commands.clone(), hub: self.hub.clone(),
            models: self.models.clone(), catalog_env: self.catalog_env.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            origin: self.origin.clone(), triggers: self.triggers.clone(),
            repos: self.repos.clone(), upstream_data: Some(data.as_ref().to_path_buf()),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
        })
    }

    /// #75：测试注入指定私库根（只用于进程内 Router 测试）。
    pub fn with_repos_for_test(self: Arc<Self>, kind: &str, root: impl AsRef<std::path::Path>) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(), commands: self.commands.clone(), hub: self.hub.clone(),
            models: self.models.clone(), catalog_env: self.catalog_env.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            origin: self.origin.clone(), triggers: self.triggers.clone(),
            repos: crate::repos::from_path(kind, root.as_ref()),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
        })
    }

    /// #76：测试注入变体绑定表路径（只用于进程内 Router 测试）。
    pub fn with_variant_table_for_test(
        self: Arc<Self>,
        path: impl AsRef<std::path::Path>,
    ) -> Arc<Self> {
        let mut tables = self.variant_tables.clone();
        tables.insert("skills".into(), path.as_ref().to_path_buf());
        tables.insert("extend".into(), path.as_ref().to_path_buf());
        Arc::new(Self {
            source: self.source.clone(), commands: self.commands.clone(), hub: self.hub.clone(),
            models: self.models.clone(), catalog_env: self.catalog_env.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            origin: self.origin.clone(), triggers: self.triggers.clone(),
            repos: self.repos.clone(), upstream_data: self.upstream_data.clone(), variant_tables: tables,
            presets: self.presets.clone(), stats: self.stats.clone(), ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
        })
    }

    /// #76：变体绑定表路径（未装配返回 None，由路由回 409）。
    pub fn variant_table(&self, kind: &str) -> Option<std::path::PathBuf> {
        self.variant_tables.get(kind).cloned()
    }

    /// #57 的点火装配（只有 main 注入一次）。
    pub fn with_triggers(self: Arc<Self>, triggers: nous_triggers::TriggerRuntime) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            origin: self.origin.clone(),
            triggers: Some(triggers),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
        })
    }

    /// #61：装配组合名册的两个盘面根（生产由 main 注入一次）。
    pub fn with_combos_root(self: Arc<Self>, root: impl Into<std::path::PathBuf>) -> Arc<Self> {
        self.with_combos_roots(root, std::path::PathBuf::from("/opt/Nous-Trigger"))
    }

    /// 同上，但触发器根可显式指定（判据要能改名触发器的目录）。
    pub fn with_combos_roots(
        self: Arc<Self>,
        root: impl Into<std::path::PathBuf>,
        triggers: impl Into<std::path::PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: Some(nous_combos::ComboStore::new(root, triggers)),
            combo_action: self.combo_action.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
        })
    }

    /// #82：注入试跑判定的时钟与时钟许可（生产在 `main` 装配一次，测试用假时钟）。
    pub fn with_combos_action_clock(
        self: Arc<Self>,
        clock: Arc<dyn nous_durable::harness::clock::Clock>,
        clock_allowed: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            refreshed: std::sync::Mutex::new(
                self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(
                self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: Some(nous_combos::Clock { clock, clock_allowed }),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
        })
    }

    pub fn triggers(&self) -> Option<&nous_triggers::TriggerRuntime> {
        self.triggers.as_ref()
    }

    /// #82：试跑判定的装配（未装配时动作端点回 503，不假装判定过）。
    pub fn combo_action(&self) -> Option<&nous_combos::Clock> {
        self.combo_action.as_ref()
    }

    pub fn stats(&self) -> Option<&data_data_manager::routes::Paths> {
        self.stats.as_ref()
    }

    /// #81：把扩展运行时接到端点层（生产入口只调一次）。
    pub fn with_ext(self: Arc<Self>, runtime: Arc<library_extend::Runtime>) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            runtime: std::sync::Mutex::new(
                self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            refreshed: std::sync::Mutex::new(
                self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            ext: Some(runtime),
            ext_admin: self.ext_admin.clone(),
        })
    }

    pub fn ext(&self) -> Option<&Arc<library_extend::Runtime>> {
        self.ext.as_ref()
    }

    /// #84：装配扩展管理面的盘面根（生产由 main 注入一次，测试用临时目录）。
    pub fn with_ext_admin(
        self: Arc<Self>,
        root: impl Into<std::path::PathBuf>,
        trash: impl Into<std::path::PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(), commands: self.commands.clone(), hub: self.hub.clone(),
            models: self.models.clone(), catalog_env: self.catalog_env.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            origin: self.origin.clone(), triggers: self.triggers.clone(),
            repos: self.repos.clone(), upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(), stats: self.stats.clone(),
            ext: self.ext.clone(),
            ext_admin: Some({
                let mut admin = library_extend::AdminStore::new(root, trash);
                admin.head_restore = Some(library_extend::HeadRestore(std::sync::Arc::new(|root: &std::path::Path, name: &str| {
                    nous_repo::materialize_head_dir(root, name).map_err(|e| e.to_string())
                })));
                admin
            }),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
        })
    }

    pub fn ext_admin(&self) -> Option<&library_extend::AdminStore> {
        self.ext_admin.as_ref()
    }

    /// 装上 stats 的只读句柄（生产装配只调一次；测试用它打真 Router）。
    pub fn with_stats(self: Arc<Self>, runtime: data_data_manager::routes::Paths) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            runtime: std::sync::Mutex::new(
                self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            refreshed: std::sync::Mutex::new(
                self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone(),
            ),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            ext: self.ext.clone(),
            ext_admin: self.ext_admin.clone(),
            repos: self.repos.clone(),
            upstream_data: self.upstream_data.clone(),
            variant_tables: self.variant_tables.clone(),
            presets: self.presets.clone(),
            stats: Some(runtime),
            combos: self.combos.clone(),
            combo_action: self.combo_action.clone(),
        })
    }

    pub fn combos(&self) -> Option<&nous_combos::ComboStore> {
        self.combos.as_ref()
    }

    /// 一次提交 → 帧，发给**每个改动自己所属**的会话。
    ///
    /// 不是「整个 AppState 一个会话」：`Session` 不绑会话，一次提交里的 `Entry` / `Document`
    /// 各自带 `conversationId`（session 作用域的文档在 `record.scope` 里）。按改动分派，
    /// 免得把 A 会话的 delta 推到 B 会话的 SSE 上。
    pub fn publish_publication(&self, publication: &CommitPublication) -> Result<()> {
        nous_sessions::observer::publish_publication(&self.hub, publication).map_err(Into::into)
    }

    /// 把一个已投成帧的事件直接发给会话。判据用这条注入**合帧器的真实输入**。
    pub fn publish_event(&self, session: i64, event: RemoteEvent) {
        self.hub.publish(session, event);
    }
}

