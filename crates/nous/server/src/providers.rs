//! 元素库的模型入口组 → 能真发请求的 provider。
//!
//! 这里只做三件事，全部在组装根（[`crate::runtime::assemble`]）里发生一次：
//!
//! 1. **密钥引用由宿主解成值**：元素包的 `apiKey` 只存 `$ENV` / `!cmd` / `secret:<id>`
//!    引用，取值面由 [`HostKeyLookup`] 决定；解不出来就整条供应商跳过，绝不把引用串
//!    当密钥发出去。密钥只进请求头，不进日志、不进响应体。
//! 2. **每个供应商各自 `resolve` 一次**：拿到的是 runtime 已经绑好执行面的运行时
//!    （baseUrl / 协议 / 请求头都在里面），一条坏供应商不影响别条。
//! 3. **合成一个句柄**：轮次驱动器只认一个 `Arc<dyn Provider>`
//!    （`nous_sessions::turn::TurnRuntime`），而元素库的模型天然跨多个供应商，
//!    所以 [`ProviderSet`] 在边界上按 `model.provider` 分派。

use std::sync::Arc;
use std::time::{Duration, Instant};

use nous_ai::{AssistantMessageEvent, AssistantMessageEventStream, Context, Model, Provider};
use nous_ai::{SimpleStreamOptions, StopReason};
use nous_element_models::KeyLookup;
use nous_runtime::{ModelRequest, ProviderRegistry, RuntimeConfig, ValueSource};

/// `!cmd` 的超时：装配只在启动时跑一次，但一条挂住的命令会把启动卡死。
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// `secret:<id>` 落在宿主的密钥面：环境变量 `NOUS_SECRET_<ID>`（大写下划线）。
///
/// 与 `$ENV` 的区别只在命名约定：`secret:` 是元素库自己的密钥空间，部署时由
/// `EnvironmentFile` 提供，元素包里不出现明文。
fn secret_env_name(id: &str) -> String {
    let mut name = String::from("NOUS_SECRET_");
    for character in id.chars() {
        name.push(if character.is_ascii_alphanumeric() {
            character.to_ascii_uppercase()
        } else {
            '_'
        });
    }
    name
}

/// 宿主的密钥取值面：环境变量 / 命令 / 密钥空间都走宿主给的 [`ValueSource`]。
pub struct HostKeyLookup<'a> {
    values: &'a dyn ValueSource,
    timeout: Duration,
}

impl<'a> HostKeyLookup<'a> {
    pub fn new(values: &'a dyn ValueSource) -> Self {
        Self {
            values,
            timeout: COMMAND_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl KeyLookup for HostKeyLookup<'_> {
    fn env(&self, name: &str) -> Option<String> {
        self.values.env(name)
    }

    fn run(&self, command: &str) -> Result<String, String> {
        run_command(command, self.timeout)
    }

    fn secret(&self, id: &str) -> Option<String> {
        self.values.env(&secret_env_name(id))
    }
}

/// 跑一条密钥命令，输出取 stdout。
///
/// 进程创建点不在 `server` 里：走 `nous_tools::os_env::run_capture`（S1 的唯一执行面，
/// 判据 6 / 12 守的就是这条）。这里只负责把错误翻成「密钥命令 …」那句人话。
fn run_command(command: &str, timeout: Duration) -> Result<String, String> {
    nous_tools::os_env::run_capture("sh", &["-c", command], timeout)
        .map_err(|error| format!("密钥该读的这条命令没成：{error}"))
}

/// 把一个供应商的密钥引用解成值，并只保留这一个供应商的配置。
///
/// 保留其它供应商会让 `resolve` 顺手把它们也绑一遍，诊断就分不清是谁坏了。
fn single_provider_config(
    config: &RuntimeConfig,
    id: &str,
    key: String,
) -> Option<RuntimeConfig> {
    let mut provider = config.models.providers.get(id)?.clone();
    provider.api_key = Some(key);
    let mut models = nous_runtime::config::ModelsConfig::default();
    models.providers.insert(id.to_string(), provider);
    Some(RuntimeConfig {
        models,
        credentials: Default::default(),
        settings: config.settings.clone(),
    })
}

/// 组装真供应商。`None` = 一个都没成，调用方退回离线 provider 并保持诚实（不许假装受理）。
///
/// 第二个返回值是**给人看**的诊断：哪条供应商为什么被跳过。日志里点名，不静默。
pub fn build(
    config: &RuntimeConfig,
    registry: &ProviderRegistry,
    values: &dyn ValueSource,
) -> (Option<Arc<dyn Provider>>, Vec<String>) {
    let lookup = HostKeyLookup::new(values);
    let mut notes: Vec<String> = Vec::new();
    let mut runtimes: Vec<Arc<dyn Provider>> = Vec::new();
    for (id, provider) in &config.models.providers {
        let Some(first) = provider.models.first() else {
            notes.push(format!("供应商 `{id}`：包里没有模型，跳过"));
            continue;
        };
        let key = match nous_element_models::resolve_key(
            &nous_element_models::parse_key_ref(provider.api_key.as_deref()),
            &lookup,
        ) {
            Ok(key) => key,
            Err(reason) => {
                notes.push(format!("供应商 `{id}`：{reason}，跳过"));
                continue;
            }
        };
        let Some(one) = single_provider_config(config, id, key) else {
            continue;
        };
        let request = ModelRequest {
            provider: Some(id.as_str()),
            model: Some(first.id.as_str()),
            ..Default::default()
        };
        match one.resolve(&request, registry, values) {
            Ok(resolved) => {
                for runtime in resolved.providers {
                    if runtimes
                        .iter()
                        .any(|existing| existing.id() == runtime.id())
                    {
                        continue;
                    }
                    runtimes.push(runtime);
                }
            }
            Err(error) => notes.push(format!("供应商 `{id}`：{error}")),
        }
    }
    if runtimes.is_empty() {
        return (None, notes);
    }
    let ids: Vec<&str> = runtimes.iter().map(|runtime| runtime.id()).collect();
    notes.push(format!("已绑定供应商：{}", ids.join(" / ")));
    (Some(Arc::new(ProviderSet::new(runtimes)) as Arc<dyn Provider>), notes)
}

/// 多个供应商运行时合成一个句柄，按 `model.provider` 分派。
///
/// 引用（`{provider, modelId}`）里没有 `baseUrl` / 协议，执行面以供应商自己登记的
/// 那条模型为准，所以这里会换成登记的模型再发。
pub struct ProviderSet {
    providers: Vec<Arc<dyn Provider>>,
    models: Vec<Model>,
}

impl ProviderSet {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        let models = providers
            .iter()
            .flat_map(|provider| provider.models().to_vec())
            .collect();
        Self { providers, models }
    }

    fn provider_for(&self, model: &Model) -> Option<&Arc<dyn Provider>> {
        self.providers
            .iter()
            .find(|provider| provider.id() == model.provider)
    }

    /// 登记过的模型（执行面以它为准）；没登记就 `None`。
    fn registered<'a>(provider: &'a Arc<dyn Provider>, model: &Model) -> Option<&'a Model> {
        provider
            .models()
            .iter()
            .find(|candidate| candidate.id.eq_ignore_ascii_case(&model.id))
    }
}

#[async_trait::async_trait]
impl Provider for ProviderSet {
    /// 分派认的是 `model.provider`，不是这个 id；这里只报第一条，给日志一个名字。
    fn id(&self) -> &str {
        self.providers
            .first()
            .map(|provider| provider.id())
            .unwrap_or("none")
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    async fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> AssistantMessageEventStream {
        let Some(provider) = self.provider_for(model) else {
            return unbound(
                model,
                format!(
                    "供应商 `{}` 没有可用运行时：元素包的 apiKey 引用解不出来，或该包没有模型",
                    model.provider
                ),
            )
            .await;
        };
        let Some(registered) = Self::registered(provider, model) else {
            return unbound(
                model,
                format!("供应商 `{}` 里没有模型 `{}`", model.provider, model.id),
            )
            .await;
        };
        provider.stream_simple(registered, context, options).await
    }
}

/// 分派不到的模型：回一条**错误流**，让轮次照常收敛成 `model_error`，不静默空转。
async fn unbound(model: &Model, message: String) -> AssistantMessageEventStream {
    let (mut producer, stream) = nous_ai::event_stream::create_assistant_message_event_stream();
    producer
        .push(AssistantMessageEvent::Error {
            reason: nous_ai::types::ErrorReason::Error,
            error: nous_ai::types::AssistantMessage::terminal(
                model.api.clone(),
                model.provider.clone(),
                model.id.clone(),
                StopReason::Error,
                message,
                0,
            ),
        })
        .await;
    stream
}

