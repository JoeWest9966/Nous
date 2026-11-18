//! 组装根：本次进程装了哪些 provider、哪些工具，以及一轮怎么起。
//!
//! 这里是**唯一**该出现"具体实现"的地方：`nous-runtime` 解凭证 / 选模型，`nous-tools` 造工具表，
//! `nous-sessions` 跑轮次。server 自己不实现其中任何一家的业务状态（`AGENTS.md` 的规矩），
//! 也不在这里读环境 —— `ValueSource` 由宿主传进来。
//!
//! 三条刻意放在这里而不是散在路由里的规矩：
//!
//! 1. **provider 解析失败就明确失败**：不接受消息再假装受理（`#44` 复核那条：
//!    把 faux provider 当生产 provider，等于"发出去永远没有回复"）。
//! 2. **工具表只有一份**：路由拿不到工具名清单，也就不会有人另造一份。
//! 3. **不写 `nous_ai` 的类型**：句柄由 `nous-sessions` 提供（[`TurnHandle`]），端点层的
//!    依赖边里就没有模型层 —— 判据 1 要的是 tools/runtime/sessions 三条，不是把 `nous-ai` 也搬进来。
use std::path::Path;
use std::sync::Arc;

use nous_durable::session::Session;
use nous_runtime::{ProviderRegistry, RuntimeConfig, ValueSource};
pub use nous_sessions::turn::TurnHandle;

use crate::error::{Result, ServerError};

/// 一次成功装配的结果。`session` 与写命令共用同一个 `Arc`（取消 token 同源，`#44`）。
pub struct AssembledRuntime {
    config: RuntimeConfig,
    registry: ProviderRegistry,
    tools: Arc<nous_tools::ToolMap>,
    session: Arc<Session>,
    tool_names: Vec<String>,
}

/// 装配。
///
/// - `cwd` 是工具的工作目录（bash / read 的相对路径根）；
/// - `values` 是宿主的环境视图（凭证从这里读，本层不碰 `std::env`）；
/// - `session` 必须与写命令是同一个 `Arc<Session>`。
pub fn assemble(
    cwd: impl AsRef<Path>,
    values: &dyn ValueSource,
    session: Arc<Session>,
) -> Result<AssembledRuntime> {
    let registry = ProviderRegistry::default();
    let config = RuntimeConfig::default();

    // 工具表的**唯一构造点**在 `nous-tools`：这里只拿一张现成的表，不重复 `insert`。
    let tools: nous_tools::ToolMap = nous_tools::builtin_tools(cwd);

    // 编目只用来诊断（缺凭证的 provider 在这里报一句），真正的解析在 `turn_for`。
    let (_models, diagnostics) = config.catalog(&registry, values);
    for note in diagnostics {
        eprintln!("nous-server: model catalog: {note}");
    }

    Ok(AssembledRuntime {
        config,
        registry,
        tool_names: tools.keys().cloned().collect(),
        tools: Arc::new(tools),
        session,
    })
}

/// 起一轮的工厂。`AppState` 只认这条 trait：
///
/// - 生产实现是 [`AssembledRuntime`]（真 provider，凭证从宿主环境读）；
/// - 端到端判据注入一个 faux 实现（`nous_sessions::turn::TurnHandle::faux`），
///   于是 server 这个 crate **不用**依赖 `nous-ai`，也不必在测试里认识 FauxProvider。
pub trait TurnFactory: Send + Sync {
    fn turn_for(&self, model: &str) -> Result<TurnHandle>;
}

impl TurnFactory for AssembledRuntime {
    fn turn_for(&self, model: &str) -> Result<TurnHandle> {
        // 生产：凭证只在请求这一刻读，`ProcessEnvValues` 是宿主显式给的视图。
        self.turn_for_with(model, &ProcessEnvValues)
    }
}

impl AssembledRuntime {
    pub fn tool_names(&self) -> &[String] {
        &self.tool_names
    }

    /// 起一轮的句柄：把 `"provider/modelId"` 解析成真 provider，和工具表一起交给 sessions。
    ///
    /// 解析不出来就是 [`ServerError::Unavailable`]（503）—— **先受理再假装**是禁止的。
    pub fn turn_for_with(&self, model: &str, values: &dyn ValueSource) -> Result<TurnHandle> {
        let (provider, model_id) = model.split_once('/').ok_or_else(|| {
            ServerError::BadRequest(format!("model must be 'provider/modelId', got {model:?}"))
        })?;
        let request = nous_runtime::ModelRequest {
            provider: Some(provider),
            api: None,
            model: Some(model_id),
            thinking: None,
            api_key: None,
            base_url: None,
        };
        let resolved = self
            .config
            .resolve(&request, &self.registry, values)
            .map_err(|error| ServerError::Unavailable(format!("provider unavailable: {error}")))?;
        Ok(TurnHandle::new(
            Arc::clone(&self.session),
            resolved.provider,
            Arc::clone(&self.tools),
            resolved.model,
        ))
    }
}

/// 进程环境变量的只读视图（宿主显式装配，runtime 自己不读环境）。
pub struct ProcessEnvValues;

impl ValueSource for ProcessEnvValues {
    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}
