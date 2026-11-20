//! 进程内的装配点：数据源 + 广播通道，以及「一次提交 → 这个会话的订阅者」这一跳。
//!
//! 生产里 `advance` 由 `nous_durable::session::Session` 的 `CommitObserver` 同步调用。
//! 那一层不许 await（上游契约），所以这里整条路径是同步的：投影 → `broadcast::send`。
//! 合帧在连接侧（[`crate::coalesce`]），不在提交侧 —— 提交侧攒帧会让「工具启动」晚到。

use std::sync::Arc;

use nous_durable::session::publish::{CommitChange, CommitPublication, DocumentCommitChange};
use nous_wire::RemoteEvent;

use crate::commands::{CommandSource, DurableCommands, ReadOnlyCommands};
use crate::documents::{DurableDocumentBackend, EmptyBackend};
use crate::runtime::AssembledRuntime;
use nous_documents::service::DocumentService;
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
    documents: Option<DocumentService>,
    /// `capabilities.services[engine].base` 的单一来源：环境变量优先，绑定后由 `main` 覆盖。
    origin: String,
    /// #57：点火路径的装配（store + 同一个 Session + 模板读取）。未装配的测试路由回 503。
    triggers: Option<crate::triggers::TriggerRuntime>,
    /// #60：模板私库 + 修订表的写者（整段串行锁）。按 env 的路径解析，进程内不留 map。
    presets: Option<crate::presets::SharedStore>,
    /// #62：stats 的只读句柄（两个库路径）。没有装配时端点回 409，不假装空数据。
    stats: Option<crate::stats::StatsRuntime>,
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
            documents: Some(DocumentService::new(
                Arc::new(EmptyBackend),
                Some(Arc::new(EmptyBackend)),
            )),
            origin: std::env::var("NOUS_ORIGIN")
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:4789".to_string()),
            triggers: None,
            presets: None,
            stats: None,
        })
    }

    /// 把 presets 写者接上（main 装配一次；测试按临时目录注入）。
    pub fn with_presets(self: Arc<Self>, store: crate::presets::SharedStore) -> Arc<Self> {
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
            documents: self.documents.clone(),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            presets: Some(store),
            stats: self.stats.clone(),
        })
    }

    pub fn presets(&self) -> Option<&crate::presets::SharedStore> {
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
            documents: None,
            origin: std::env::var("NOUS_ORIGIN")
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:4789".to_string()),
            triggers: None,
            presets: None,
            stats: None,
        });
        commands.attach(SessionObserver::new(state.clone()));
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
            documents: self.documents.as_ref().map(|_| DocumentService::new(
                Arc::new(EmptyBackend),
                Some(Arc::new(EmptyBackend)),
            )),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
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
            documents: self.documents.as_ref().map(|_| DocumentService::new(
                Arc::new(EmptyBackend),
                Some(Arc::new(EmptyBackend)),
            )),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
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
            documents: self.documents.clone(),
            origin,
            triggers: self.triggers.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
        })
    }

    /// 启动时补写孤儿终态：走**已经持有的那个写者**，不另开 `Session`。
    pub async fn reclaim_orphans(&self) -> Result<usize> {
        self.commands.reclaim_orphans().await
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

    /// documents 端点的泛型服务。生产装配里它持有同一个 `Session` 的写者。
    pub fn document_service(&self) -> Option<DocumentService> {
        self.documents.clone()
    }

    /// #57 的点火装配（只有 main 注入一次）。
    pub fn with_triggers(self: Arc<Self>, triggers: crate::triggers::TriggerRuntime) -> Arc<Self> {
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            documents: self.documents.clone(),
            origin: self.origin.clone(),
            triggers: Some(triggers),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
        })
    }

    pub fn triggers(&self) -> Option<&crate::triggers::TriggerRuntime> {
        self.triggers.as_ref()
    }

    pub fn stats(&self) -> Option<&crate::stats::StatsRuntime> {
        self.stats.as_ref()
    }

    /// 装上 stats 的只读句柄（生产装配只调一次；测试用它打真 Router）。
    pub fn with_stats(self: Arc<Self>, runtime: crate::stats::StatsRuntime) -> Arc<Self> {
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
            documents: self.documents.clone(),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            presets: self.presets.clone(),
            stats: Some(runtime),
        })
    }

    /// 把 documents 的读写后端接到已有的库路径与同一条 `Session` 上。
    pub fn with_documents_backend(self: Arc<Self>, path: std::path::PathBuf, session: Arc<nous_durable::session::Session>) -> Arc<Self> {
        let backend = DurableDocumentBackend::new(path, session);
        Arc::new(Self {
            source: self.source.clone(),
            commands: self.commands.clone(),
            hub: self.hub.clone(),
            models: self.models.clone(),
            refreshed: std::sync::Mutex::new(self.refreshed.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            catalog_env: self.catalog_env.clone(),
            injected_models: self.injected_models,
            runtime: std::sync::Mutex::new(self.runtime.lock().unwrap_or_else(|p| p.into_inner()).clone()),
            documents: Some(DocumentService::new(
                Arc::new(backend.clone()) as Arc<dyn nous_documents::store::DocumentReader>,
                Some(Arc::new(backend) as Arc<dyn nous_documents::service::DocumentWriter>),
            )),
            origin: self.origin.clone(),
            triggers: self.triggers.clone(),
            presets: self.presets.clone(),
            stats: self.stats.clone(),
        })
    }

    /// 一次提交 → 帧，发给**每个改动自己所属**的会话。
    ///
    /// 不是「整个 AppState 一个会话」：`Session` 不绑会话，一次提交里的 `Entry` / `Document`
    /// 各自带 `conversationId`（session 作用域的文档在 `record.scope` 里）。按改动分派，
    /// 免得把 A 会话的 delta 推到 B 会话的 SSE 上。
    pub fn publish_publication(&self, publication: &CommitPublication) -> Result<()> {
        for change in &publication.changes {
            let Some(session) = session_of(change) else {
                continue;
            };
            self.hub
                .publish_all(session, project::of_change(change)?);
        }
        Ok(())
    }

    /// 把一个已投成帧的事件直接发给会话。判据用这条注入**合帧器的真实输入**。
    pub fn publish_event(&self, session: i64, event: RemoteEvent) {
        self.hub.publish(session, event);
    }
}

/// 一条改动属于哪个会话。
fn session_of(change: &CommitChange) -> Option<i64> {
    match change {
        CommitChange::Conversation(record) => Some(record.id),
        CommitChange::Entry(record) => Some(record.conversation_id),
        CommitChange::Task(record) => Some(record.conversation_id),
        CommitChange::Submission(record) => Some(record.fields.conversation_id),
        CommitChange::Document(document) => match document {
            DocumentCommitChange::Change {
                conversation_id, ..
            }
            | DocumentCommitChange::Retire {
                conversation_id, ..
            } => *conversation_id,
            DocumentCommitChange::Copy {
                conversation_id, ..
            } => Some(*conversation_id),
        },
    }
}
