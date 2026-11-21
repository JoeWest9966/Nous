//! 组装根：server 构造 ProviderRegistry / ToolRegistry，交给 sessions。
//!
//! 这里只做「装配 + 传参」：provider 的认证/模型绑定在 `nous-runtime`，工具实现在
//! `nous-tools`，轮次编排在 `nous-sessions`。server 不实现任何一家的业务状态。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nous_ai::tool::AgentTool;
use nous_ai::{Api, Model};
use nous_durable::actor::Actor;
use nous_runtime::{ProviderRegistry, ValueSource};
use nous_sessions::turn::{AbortWatcher, HarnessTurnDriver, TurnDriver, TurnRuntime};
use nous_tools::tools::tool_context::ExecutionToolContext as ToolsExecutionToolContext;
use nous_tools::OsExecutionEnv;

use crate::error::Result;
use nous_device::{ModelItem, ModelRef, ModelsResponse};

/// #51 钉死的假回复。测试与生产共用同一个常量，避免两边各写一份然后漂移。
pub const FAUX_MODEL_ID: &str = "echo";
pub const FAUX_REPLY: &str = "provider=faux model=echo（假回复，别当真）";

/// `NOUS_PROVIDER` 的取值。除 `faux` 外一律走真装配。
pub fn provider_kind(values: &dyn ValueSource) -> &'static str {
    match values.env("NOUS_PROVIDER").as_deref() {
        Some("faux") => "faux",
        _ => "real",
    }
}

/// 只给 faux 用的编目：一个 `faux/echo` 模型，没有凭证要求。
pub fn faux_catalog() -> ModelsResponse {
    ModelsResponse {
        default: Some(ModelRef {
            provider: "faux".into(),
            model_id: FAUX_MODEL_ID.into(),
        }),
        items: vec![ModelItem {
            provider: "faux".into(),
            model_id: FAUX_MODEL_ID.into(),
            name: "Faux Echo".into(),
            thinking_levels: Vec::new(),
            context_window: Some(128_000),
            max_tokens: Some(16_384),
        }],
        catalog: nous_device::CatalogDiagnostics {
            state: "ok",
            source: None,
            path: PathBuf::from(nous_device::DEFAULT_MODELS_FILE),
            fetched_at: None,
            error: None,
        },
    }
}

/// faux 的 provider 实例：固定文本 + 无节流的多帧流，终态 stop。
/// 用 factory 每次现造一条脚本，连发多条消息也都是同一句固定回复（不是一次性队列）。
fn faux_provider() -> Arc<dyn nous_ai::Provider> {
    use nous_ai::providers::faux::{
        faux_assistant_message, FauxProvider, FauxScript, FauxStep,
    };
    let model = Model::new(
        FAUX_MODEL_ID,
        "Faux Echo",
        Api::Faux,
        "faux",
        "http://localhost:0",
    );
    let script = FauxScript::new();
    script.append_responses(vec![FauxStep::factory(|_| {
        faux_assistant_message(FAUX_REPLY, nous_ai::StopReason::Stop)
    })]);
    FauxProvider::with_models(script, vec![model])
}

/// 一条 driver 的工厂：接 SSE sink，造一条新 driver（每条消息一条）。
pub type DriverFactory = Arc<
    dyn Fn(Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>) -> Arc<dyn TurnDriver>
        + Send
        + Sync,
>;

/// server 构造好的轮次驱动器，备给路由/命令层使用。
pub struct AssembledRuntime {
    /// 这一台组装根写进 entry / submission 的 actor。
    /// 默认 `system("nous-app")`；将来认证落地只在这里换成 principal 派生的值。
    pub actor: Actor,
    pub driver: Arc<dyn TurnDriver>,
    pub driver_with_sink: Arc<dyn Fn(Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>) -> Arc<dyn TurnDriver> + Send + Sync>,
    pub provider_registry: ProviderRegistry,
    pub watcher: Arc<AbortWatcher>,
    pub tool_names: Vec<String>,
    /// #51：只影响 `/v1/models` 与 capabilities，不参与轮次语义。
    pub models: ModelsResponse,
    pub provider_kind: &'static str,
    /// provider id → 驱动工厂。真装配时每个有凭证的 provider 一条；
    /// faux / 离线只有一条。`POST /messages` 按解析出的模型选工厂。
    pub drivers: BTreeMap<String, DriverFactory>,
    /// provider id → 它服务的模型（带 `api` / `base_url`，起轮时原样用）。
    pub models_by_provider: BTreeMap<String, Vec<Model>>,
}

/// 从宿主配置和环境变量构造注册表。
///
/// - `ProviderRegistry::default()` 装四种内置协议工厂；
/// - `ToolRegistry` 是工具名 → `AgentTool` 的具名槽，用本地文件系统执行环境；
/// - `TurnRuntime` 把两张表交给 sessions，轮次循环只在那里。
pub fn assemble(
    cwd: impl AsRef<Path>,
    values: &dyn ValueSource,
    session: Arc<nous_durable::session::Session>,
) -> Result<AssembledRuntime> {
    // 构造点：模型注册表从宿主配置解析，工具注册表从本地执行环境装箱。
    // 轮次运行时拿走这两张表，server 保留句柄供编目/诊断使用。
    let mut registry: ProviderRegistry = nous_runtime::ProviderRegistry::default();
    let config = nous_runtime::RuntimeConfig::default();
    // `NOUS_PROVIDER=faux` 时进入验收模式：跳过真凭证解析，用一个固定脚本的 provider。
    // 只在 runtime 的组装根里做一次，路由和消息层完全不认识 provider。
    let kind = provider_kind(values);
    let (models, diagnostics) = if kind == "faux" {
        // 显式入口：不看凭证、不解析真供应商；只挂一个 faux/echo。
        let model = Model::new(
            FAUX_MODEL_ID,
            "Faux Echo",
            Api::Faux,
            "faux",
            "http://localhost:0",
        );
        registry.declare(nous_runtime::ProviderDeclaration {
            id: "faux".into(),
            models: vec![model],
            api_key_env: None,
            requires_auth: false,
        });
        eprintln!("nous-server: {FAUX_REPLY}");
        (Vec::<Model>::new(), Vec::<String>::new())
    } else {
        config.catalog(&registry, values)
    };
    for note in diagnostics {
        eprintln!("nous-server: model catalog: {note}");
    }
    let catalog = if kind == "faux" {
        let _ = models;
        faux_catalog()
    } else {
        // 真装配沿用原来的空编目：本票不替 #47/#42 决定真模型清单。
        let _ = models;
        ModelsResponse::default()
    };
    let env = Arc::new(OsExecutionEnv::with_cwd(cwd.as_ref().to_path_buf()));
    let tool_ctx = ToolsExecutionToolContext::new(
        env.clone(),
        Some(env.clone() as Arc<dyn nous_tools::file_mutation_queue::MutatingEnv>),
    );
    let mut tools: BTreeMap<String, Arc<dyn AgentTool>> = BTreeMap::new();
    tools.insert(
        "read".into(),
        nous_tools::tools::create_read_tool(&tool_ctx, None),
    );
    tools.insert(
        "bash".into(),
        nous_tools::tools::create_bash_tool(&tool_ctx, None),
    );
    tools.insert(
        "grep".into(),
        nous_tools::tools::create_grep_tool(&tool_ctx, None),
    );
    tools.insert(
        "find".into(),
        nous_tools::tools::create_find_tool(&tool_ctx, None),
    );
    tools.insert(
        "ls".into(),
        nous_tools::tools::create_ls_tool(&tool_ctx, None),
    );
    let actor = Actor::system("nous-app");
    let tool_names: Vec<String> = tools.keys().cloned().collect();
    let driver_tools = Arc::new(tools);
    // 每个 provider 一个实例：faux 一个；真装配按编目里出现过的 provider 逐个解析凭证。
    // 解析失败的 provider 打一行诊断、不进池子（模型在 /v1/models 里仍是「待凭证」）。
    let mut providers: Vec<Arc<dyn nous_ai::Provider>> = Vec::new();
    if kind == "faux" {
        providers.push(faux_provider());
    } else {
        let ids: std::collections::BTreeSet<String> =
            models.iter().map(|model| model.provider.clone()).collect();
        for id in ids {
            let request = nous_runtime::ModelRequest {
                provider: Some(id.as_str()),
                ..Default::default()
            };
            match config.resolve(&request, &registry, values) {
                Ok(resolved) => providers.push(resolved.provider.clone()),
                Err(error) => {
                    eprintln!("nous-server: provider {id} 未装配（{error}）");
                }
            }
        }
    }
    let (drivers, models_by_provider) =
        build_drivers(&providers, &driver_tools, &session, &actor);
    // 默认工厂：faux 就一条；真装配取字典序第一个（触发器点火等老路径照旧用它）。
    let make_driver = drivers
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| offline_driver_factory(&driver_tools, &session, &actor));
    let driver = make_driver(Arc::new(|_| {}));
    Ok(AssembledRuntime {
        actor,
        driver,
        driver_with_sink: make_driver,
        provider_registry: registry,
        watcher: Arc::new(AbortWatcher::new(nous_sessions::turn::abort_token(&session))),
        tool_names,
        models: catalog,
        provider_kind: kind,
        drivers,
        models_by_provider,
    })
}

/// 每个 provider 一条 driver 工厂 + 它服务的模型表。
///
/// #54 的规矩不变：actor 从组装根显式传进每一条 driver（包括带 SSE sink 的那条），
/// 不经全局/线程局部。
fn build_drivers(
    providers: &[Arc<dyn nous_ai::Provider>],
    tools: &Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
    session: &Arc<nous_durable::session::Session>,
    actor: &Actor,
) -> (BTreeMap<String, DriverFactory>, BTreeMap<String, Vec<Model>>) {
    let mut drivers: BTreeMap<String, DriverFactory> = BTreeMap::new();
    let mut models_by_provider: BTreeMap<String, Vec<Model>> = BTreeMap::new();
    for provider in providers {
        let id = provider.id().to_string();
        models_by_provider.insert(id.clone(), provider.models().to_vec());
        let provider = provider.clone();
        let tools = tools.clone();
        let session = session.clone();
        let actor = actor.clone();
        drivers.insert(
            id,
            Arc::new(
                move |sink: Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>| {
                    Arc::new(
                        HarnessTurnDriver::new(
                            session.clone(),
                            TurnRuntime {
                                providers: provider.clone(),
                                tools: tools.clone(),
                            },
                        )
                        .with_actor(actor.clone())
                        .with_event_sink(sink),
                    ) as Arc<dyn TurnDriver>
                },
            ),
        );
    }
    (drivers, models_by_provider)
}

/// 一个 provider 都没解析出来时的兜底工厂：照旧接离线 provider，
/// 让「没有凭证」表现为一次失败的轮次，而不是「路由 500」。
fn offline_driver_factory(
    tools: &Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
    session: &Arc<nous_durable::session::Session>,
    actor: &Actor,
) -> DriverFactory {
    let providers = offline_provider();
    let tools = tools.clone();
    let session = session.clone();
    let actor = actor.clone();
    Arc::new(move |sink: Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>| {
        Arc::new(
            HarnessTurnDriver::new(
                session.clone(),
                TurnRuntime {
                    providers: providers.clone(),
                    tools: tools.clone(),
                },
            )
            .with_actor(actor.clone())
            .with_event_sink(sink),
        ) as Arc<dyn TurnDriver>
    })
}

/// 用一个 faux provider 组装一套可跑的轮次（测试/离线用）。
pub fn assemble_with_provider(
    providers: Arc<dyn nous_ai::Provider>,
    tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
    session: Arc<nous_durable::session::Session>,
) -> AssembledRuntime {
    assemble_with_provider_kind(providers, tools, session, "faux")
}

/// 同 [`assemble_with_provider`]，但显式给 `provider_kind`。
///
/// #70 的判据要能覆盖「真 provider 装配」这条分支：生产里 `kind` 由 `NOUS_PROVIDER` 决定，
/// 测试里必须能造出 `"real"`，否则那条分支永远只有生产才走到。
pub fn assemble_with_provider_kind(
    providers: Arc<dyn nous_ai::Provider>,
    tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
    session: Arc<nous_durable::session::Session>,
    kind: &'static str,
) -> AssembledRuntime {
    let registry = ProviderRegistry::default();
    let registry_models = providers.models().to_vec();
    let (drivers, models_by_provider) = build_drivers(
        std::slice::from_ref(&providers),
        &tools,
        &session,
        &Actor::system("nous-app"),
    );
    let catalog = if kind == "faux" {
        faux_catalog()
    } else {
        let items: Vec<ModelItem> = registry_models
            .iter()
            .map(|model| ModelItem {
                provider: model.provider.clone(),
                model_id: model.id.clone(),
                name: model.name.clone(),
                thinking_levels: Vec::new(),
                context_window: None,
                max_tokens: None,
            })
            .collect();
        ModelsResponse {
            default: items.first().map(|item| ModelRef {
                provider: item.provider.clone(),
                model_id: item.model_id.clone(),
            }),
            items,
            catalog: nous_device::CatalogDiagnostics {
                state: "ok",
                source: Some("test".into()),
                path: PathBuf::from(nous_device::DEFAULT_MODELS_FILE),
                fetched_at: None,
                error: None,
            },
        }
    };
    let runtime = TurnRuntime { providers, tools };
    let tool_names = runtime.tools.keys().cloned().collect();
    let watcher = Arc::new(AbortWatcher::new(nous_sessions::turn::abort_token(&session)));
    AssembledRuntime {
        actor: Actor::system("nous-app"),
        driver: Arc::new(HarnessTurnDriver::new(session.clone(), runtime.clone()).with_actor(Actor::system("nous-app"))),
        driver_with_sink: Arc::new(move |sink| {
            Arc::new(HarnessTurnDriver::new(session.clone(), runtime.clone())
                .with_actor(Actor::system("nous-app"))
                .with_event_sink(sink)) as Arc<dyn TurnDriver>
        }),
        provider_registry: registry,
        watcher,
        tool_names,
        models: catalog,
        // 测试用的装配：一条 provider、一条 driver（跟真装配同一套建法）。
        drivers,
        models_by_provider,
        provider_kind: kind,
    }
}

/// 进程环境变量的只读视图。宿主显式传进来，runtime 自己不读环境。
pub struct ProcessEnvValues;
impl ValueSource for ProcessEnvValues {
    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// 真装配的占位 provider：没有凭证时不产生回复。
/// 注意当前路由层还没有 #44 的 resolve/503 判定，所以这里有回复就等同于「真模型在答」；
/// 绝不许把它换成返回固定文本的 provider（那会让「没配 key 也能答」变成真的）。
pub fn unconfigured_provider() -> Arc<dyn nous_ai::Provider> {
    crate::runtime::offline_provider()
}

/// 离线/测试用的 provider。生产走 RuntimeConfig::resolve 绑定真实供应商。
pub fn offline_provider() -> Arc<dyn nous_ai::Provider> {
    nous_ai::providers::faux::FauxProvider::new(nous_ai::providers::faux::FauxScript::new())
}
