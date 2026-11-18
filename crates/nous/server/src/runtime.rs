//! 组装根：server 构造 ProviderRegistry / ToolRegistry，交给 sessions。
//!
//! 这里只做「装配 + 传参」：provider 的认证/模型绑定在 `nous-runtime`，工具实现在
//! `nous-tools`，轮次编排在 `nous-sessions`。server 不实现任何一家的业务状态。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use nous_ai::tool::AgentTool;
use nous_runtime::{ProviderRegistry, ValueSource};
use nous_sessions::turn::{AbortWatcher, HarnessTurnDriver, TurnDriver, TurnRuntime};
use nous_tools::tools::tool_context::ExecutionToolContext as ToolsExecutionToolContext;
use nous_tools::OsExecutionEnv;

use crate::error::Result;

/// server 构造好的轮次驱动器，备给路由/命令层使用。
pub struct AssembledRuntime {
    pub driver: Arc<dyn TurnDriver>,
    pub provider_registry: ProviderRegistry,
    pub watcher: Arc<AbortWatcher>,
    pub tool_names: Vec<String>,
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
    let registry: ProviderRegistry = nous_runtime::ProviderRegistry::default();
    let config = nous_runtime::RuntimeConfig::default();
    let (_models, diagnostics) = config.catalog(&registry, values);
    for note in diagnostics {
        eprintln!("nous-server: model catalog: {note}");
    }
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
    Ok(AssembledRuntime {
        driver: Arc::new(HarnessTurnDriver::new(session.clone(), TurnRuntime {
            providers: crate::runtime::offline_provider(),
            tools: Arc::new(tools.clone()),
        })),
        provider_registry: registry,
        watcher: Arc::new(AbortWatcher::new(nous_sessions::turn::abort_token(&session))),
        tool_names: tools.keys().cloned().collect(),
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
        driver: Arc::new(HarnessTurnDriver::new(session, runtime)),
        provider_registry: registry,
        watcher,
        tool_names,
    }
}

/// 进程环境变量的只读视图。宿主显式传进来，runtime 自己不读环境。
pub struct ProcessEnvValues;
impl ValueSource for ProcessEnvValues {
    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// 离线/测试用的 provider。生产走 RuntimeConfig::resolve 绑定真实供应商。
pub fn offline_provider() -> Arc<dyn nous_ai::Provider> {
    nous_ai::providers::faux::FauxProvider::new(nous_ai::providers::faux::FauxScript::new())
}
