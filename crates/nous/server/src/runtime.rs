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

/// #70：模型解析的**唯一顺序**：请求体 → 会话 → 编目 default → 503。
///
/// 会话里的 model 是 wire 的字符串（`<provider>/<modelId>`，第一个 `/` 切开）；
/// 请求体与编目 default 是 `{provider, modelId}`。解析结果只描述「是谁」，
/// 真正的 provider 绑定由组装根持有（这里是 `assemble_with_provider` 注入的那一个）。
pub fn resolve_model_ref(
    body: Option<&nous_device::ModelRef>,
    session: Option<&str>,
    catalog: &ModelsResponse,
) -> Option<nous_device::ModelRef> {
    if let Some(model) = body {
        if !model.provider.is_empty() && !model.model_id.is_empty() {
            return Some(model.clone());
        }
    }
    if let Some(text) = session {
        if let Some((provider, id)) = text.split_once('/') {
            if !provider.is_empty() && !id.is_empty() {
                return Some(nous_device::ModelRef {
                    provider: provider.to_string(),
                    model_id: id.to_string(),
                });
            }
        }
    }
    catalog.default.clone()
}

/// 把 wire 的模型引用变成轮次用的 `nous_ai::Model`。
/// faux 用 `Api::Faux`，真 provider 用 `Api::OpenaiCompletions` 作为协议占位：
/// 本票只决定「用哪个模型」，凭证/端点绑定由组装根的 provider 完成。
pub fn model_from_ref(reference: &nous_device::ModelRef) -> nous_ai::Model {
    let (api, base_url) = if reference.provider == "faux" {
        (nous_ai::Api::Faux, "http://localhost:0")
    } else {
        (nous_ai::Api::OpenaiCompletions, "http://localhost:1")
    };
    nous_ai::Model::new(
        &reference.model_id,
        &reference.model_id,
        api,
        &reference.provider,
        base_url,
    )
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
    /// #70：一轮结束时广播它的 submission id。waitMs 等的是这个信号，不是 sleep 轮询。
    pub completions: tokio::sync::broadcast::Sender<(i64, serde_json::Value)>,
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
    assemble_in(cwd, values, session, nous_env::ELEMENTS.resolve())
}

/// 同 [`assemble`]，但元素库的位置由调用方给定（测试用临时库，生产走 `NOUS_ELEMENTS`）。
pub fn assemble_with_elements(
    cwd: impl AsRef<Path>,
    values: &dyn ValueSource,
    session: Arc<nous_durable::session::Session>,
    elements: impl AsRef<Path>,
) -> Result<AssembledRuntime> {
    assemble_in(cwd, values, session, elements.as_ref().to_path_buf())
}

fn assemble_in(
    cwd: impl AsRef<Path>,
    values: &dyn ValueSource,
    session: Arc<nous_durable::session::Session>,
    elements: PathBuf,
) -> Result<AssembledRuntime> {
    // 构造点：模型注册表从宿主配置解析，工具注册表从本地执行环境装箱。
    // 轮次运行时拿走这两张表，server 保留句柄供编目/诊断使用。
    let mut registry: ProviderRegistry = nous_runtime::ProviderRegistry::default();
    // 模型入口组优先：元素库里有 `element-models` 组就用它的组/包投影当宿主配置
    // （provider 键 / baseUrl / apiKey 引用 / models[]）；没有组就退回空配置，
    // 行为与没有这条线时一致（编目空表 + 凭证链只认 auth.json / env）。
    let (config, elements_note) = elements_runtime_config_at(elements);
    let config = config.unwrap_or_else(nous_runtime::RuntimeConfig::default);
    if let Some(note) = elements_note {
        eprintln!("nous-server: model catalog: {note}");
    }
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
    let driver_session = session.clone();
    let driver_tools = Arc::new(tools);
    // 真装配的供应商从元素组的投影里建：密钥引用在这里解成值，解不出来的供应商
    // 被跳选（日志点名），一个都没成就退回离线 provider（轮次会失败，不假装有回答）。
    let (real_provider, provider_notes) = if kind == "faux" {
        (None, Vec::new())
    } else {
        crate::providers::build(&config, &registry, values)
    };
    for note in provider_notes {
        eprintln!("nous-server: provider: {note}");
    }
    let faux_or_offline: Arc<dyn nous_ai::Provider> = if kind == "faux" {
        faux_provider()
    } else {
        real_provider.unwrap_or_else(|| {
            eprintln!(
                "nous-server: provider: 没有可用的真供应商（元素包的 apiKey 引用都解不出来），\
                 轮次会走离线 provider，不会有回答"
            );
            crate::runtime::offline_provider()
        })
    };
    // #54：actor 从组装根显式传进每一条 driver（包括带 SSE sink 的那条），
    // 不经全局/线程局部。#51 的 provider 工厂语义原样保留：factory 每次现造脚本，
    // 连发多条消息都是同一句固定回复，不是一次性队列。
    let driver_actor = actor.clone();
    let make_driver = Arc::new({
        let providers = faux_or_offline.clone();
        move |sink: Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>| {
            Arc::new(
                HarnessTurnDriver::new(
                    driver_session.clone(),
                    TurnRuntime {
                        providers: providers.clone(),
                        tools: driver_tools.clone(),
                    },
                )
                .with_actor(driver_actor.clone())
                .with_event_sink(sink),
            ) as Arc<dyn TurnDriver>
        }
    }) as Arc<dyn Fn(Arc<dyn Fn(&nous_wire::RemoteEvent) + Send + Sync>) -> Arc<dyn TurnDriver> + Send + Sync>;
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
        completions: tokio::sync::broadcast::channel(64).0,
    })
}

/// 用一个 faux provider 组装一套可跑的轮次（测试/离线用）。
pub fn assemble_with_provider(
    providers: Arc<dyn nous_ai::Provider>,
    tools: Arc<BTreeMap<String, Arc<dyn AgentTool>>>,
    session: Arc<nous_durable::session::Session>,
) -> AssembledRuntime {
    let registry = ProviderRegistry::default();
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
        models: faux_catalog(),
        provider_kind: "faux",
        completions: tokio::sync::broadcast::channel(64).0,
    }
}

/// 元素库的模型入口组 → 宿主 `RuntimeConfig`。组不存在 / 私库打不开 → `(None, None)`。
/// 密钥只在这里保持 `$ENV` / `!cmd` 引用字符串；展开发生在 runtime 的凭证链里。
fn elements_runtime_config_at(
    root: PathBuf,
) -> (Option<nous_runtime::RuntimeConfig>, Option<String>) {
    let repo = match nous_repo::Repo::open(root) {
        Ok(repo) => repo,
        Err(_) => return (None, None),
    };
    let (Ok(groups), Ok(packages)) = (
        nous_elements::scan(&repo, nous_elements::Kind::Groups),
        nous_elements::scan(&repo, nous_elements::Kind::Packages),
    ) else {
        return (None, None);
    };
    let Some(group) = nous_element_models::find_group(&groups.items) else {
        return (None, None);
    };
    let models_config = nous_element_models::runtime_config(group, &packages.items);
    let note = format!(
        "elements group `{}`: {} providers / {} models",
        group.id,
        models_config.providers.len(),
        models_config.providers.values().map(|p| p.models.len()).sum::<usize>(),
    );
    (
        Some(nous_runtime::RuntimeConfig {
            models: models_config,
            credentials: Default::default(),
            settings: Default::default(),
        }),
        Some(note),
    )
}

/// 进程环境变量的只读视图。宿主显式传进来，runtime 自己不读环境。
pub struct ProcessEnvValues;
impl ValueSource for ProcessEnvValues {
    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// 离线/测试用的 provider。生产走 [`crate::providers::build`] 绑定真实供应商。
pub fn offline_provider() -> Arc<dyn nous_ai::Provider> {
    nous_ai::providers::faux::FauxProvider::new(nous_ai::providers::faux::FauxScript::new())
}
