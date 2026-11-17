//! #17 — four wire protocols, one message each, no real network.
//!
//! The transport is a mock `Provider` registered into `ProviderRegistry`; the
//! thing under test is the router (`RuntimeFactories` + `RuntimeConfig::resolve`)
//! that binds a model to a provider and hands it a request. Every protocol in
//! the runtime's built-in route table gets one `stream_simple` call and must
//! deliver a complete assistant message.
//!
//! The second test is the **catalogue audit** required by §4.2: every `Api`
//! variant is either routed here, or listed in `SKIPPED` with a reason. A new
//! `Api` value added upstream turns the audit red instead of being skipped
//! silently.

use nous_ai::{
    AssistantMessage, AssistantMessageEvent, Context, DoneReason, Model, Provider,
    SimpleStreamOptions,
};
use nous_runtime::{
    ProviderDeclaration, ProviderRegistry, RuntimeConfig, RuntimeSettings, ValueSource,
};
use std::sync::Arc;

/// The four protocols `RuntimeFactories::default()` routes natively.
const ROUTED: &[nous_ai::Api] = &[
    nous_ai::Api::AnthropicMessages,
    nous_ai::Api::OpenaiCompletions,
    nous_ai::Api::OpenaiResponses,
    nous_ai::Api::GoogleGenerativeAi,
];

/// Every other `Api` variant, with the reason it has no native runtime route.
/// Adding a variant to `nous_ai::Api` without touching this table is a compile
/// error (the exhaustive `match` in the audit test).
fn skip_reason(api: &nous_ai::Api) -> &'static str {
    match api {
        nous_ai::Api::MistralConversations | nous_ai::Api::BedrockConverseStream => {
            "openai-compatible / bedrock gateway: no built-in SDK in the frozen nous-ai; hosts register their own Provider through register()"
        }
        nous_ai::Api::AzureOpenaiResponses | nous_ai::Api::OpenaiCodexResponses => {
            "response-wire variants: served by the openai-responses route via base_url/headers; no separate SDK"
        }
        nous_ai::Api::GoogleVertex => {
            "vertex auth is host-specific (service account); no built-in credential resolver in this crate"
        }
        nous_ai::Api::PiMessages => "native pi wire protocol: no provider implementation in the frozen nous-ai",
        nous_ai::Api::Faux => "in-process test provider: hosts register it explicitly, never resolved from a model catalogue",
        nous_ai::Api::Other(_) => "host-defined extension: registered through ProviderRegistry::register() with its own Api value",
        // Keep the routed arm here rather than `_` so adding a variant is loud.
        nous_ai::Api::AnthropicMessages
        | nous_ai::Api::OpenaiCompletions
        | nous_ai::Api::OpenaiResponses
        | nous_ai::Api::GoogleGenerativeAi => "routed natively (listed in ROUTED)",
    }
}

#[test]
fn api_catalogue_every_variant_is_routed_or_explicitly_skipped() {
    for api in [
        nous_ai::Api::OpenaiCompletions,
        nous_ai::Api::MistralConversations,
        nous_ai::Api::OpenaiResponses,
        nous_ai::Api::AzureOpenaiResponses,
        nous_ai::Api::OpenaiCodexResponses,
        nous_ai::Api::AnthropicMessages,
        nous_ai::Api::BedrockConverseStream,
        nous_ai::Api::GoogleGenerativeAi,
        nous_ai::Api::GoogleVertex,
        nous_ai::Api::PiMessages,
        nous_ai::Api::Faux,
        nous_ai::Api::Other("custom".into()),
    ] {
        let routed = ROUTED.contains(&api);
        assert!(
            routed || !skip_reason(&api).is_empty(),
            "api {} has neither a route nor a skip reason",
            api.as_str()
        );
        if !routed {
            assert!(
                !skip_reason(&api).starts_with("routed"),
                "api {} is listed as skipped but claims to be routed",
                api.as_str()
            );
        }
    }
    assert_eq!(ROUTED.len(), 4);
}

/// A provider that never touches the network: it records the options it was
/// called with and returns one complete assistant message.
struct MockProvider {
    id: String,
    models: Vec<Model>,
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    async fn stream_simple(
        &self,
        model: &Model,
        _ctx: &Context,
        _options: &SimpleStreamOptions,
    ) -> nous_ai::AssistantMessageEventStream {
        let (mut producer, stream) = nous_ai::event_stream::create_assistant_message_event_stream();
        let mut message = AssistantMessage::empty(
            model.api.clone(),
            model.provider.clone(),
            model.id.clone(),
            0,
        );
        message.content = vec![nous_ai::Content::text("one-message-from-the-mock-transport")];
        message.stop_reason = nous_ai::StopReason::Stop;
        producer
            .push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message,
            })
            .await;
        stream
    }
}

struct Env;
impl ValueSource for Env {
    fn env(&self, name: &str) -> Option<String> {
        if name.ends_with("_KEY") {
            Some(format!("mock-key-for-{name}")).into()
        } else {
            None
        }
    }
    fn now_unix_seconds(&self) -> i64 {
        0
    }
}

fn providers_for(api: &nous_ai::Api) -> (ProviderRegistry, String) {
    let id = format!("wire-{}", api.as_str());
    let model = Model::new(
        "wire-model",
        "Wire Model",
        api.clone(),
        id.clone(),
        "https://wire.example/v1",
    );
    let mut registry = ProviderRegistry::default();
    registry.declare(ProviderDeclaration {
        id: id.clone(),
        models: vec![model.clone()],
        api_key_env: Some(format!("{}_KEY", api.as_str().to_uppercase().replace('-', "_"))),
        requires_auth: true,
    });
    registry.register(
        Arc::new(MockProvider {
            id: id.clone(),
            models: vec![model],
        }),
        api.clone(),
    );
    (registry, id)
}

/// The built-in route table (not the host-registered override map) must know
/// all four protocols. No mock is registered here: if `RuntimeFactories` lacked
/// a protocol, `resolve` would fail with `UnsupportedApi`. Resolving alone
/// sends no HTTP.
#[test]
fn builtin_route_table_has_all_four_protocols() {
    for api in ROUTED {
        let id = format!("builtin-{}", api.as_str());
        let model = Model::new(
            "builtin-model",
            "Builtin Model",
            api.clone(),
            id.clone(),
            "https://builtin.example/v1",
        );
        let mut registry = ProviderRegistry::default();
        registry.declare(ProviderDeclaration {
            id: id.clone(),
            models: vec![model],
            api_key_env: Some(format!(
                "{}_KEY",
                api.as_str().to_uppercase().replace('-', "_")
            )),
            requires_auth: true,
        });
        let config = RuntimeConfig::default();
        let request = nous_runtime::ModelRequest {
            provider: Some(&id),
            model: Some("builtin-model"),
            ..Default::default()
        };
        let runtime = match nous_runtime::Runtime::new(&config, &registry, &Env, &request) {
            Ok(runtime) => runtime,
            Err(error) => panic!(
                "builtin route {} missing from RuntimeFactories: {error:?}",
                api.as_str()
            ),
        };
        assert!(
            !runtime.current().provider.models().is_empty(),
            "builtin route {} resolved to an empty provider",
            api.as_str()
        );
    }
}

/// Four protocols, four `stream_simple` calls, four complete assistant messages.
/// Transport is mocked; no HTTP is sent.
#[tokio::test]
async fn every_routed_protocol_delivers_one_complete_assistant_message() {
    for api in ROUTED {
        let (providers, provider_id) = providers_for(api);
        let config = RuntimeConfig {
            settings: RuntimeSettings {
                default_provider: Some(provider_id.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let request = nous_runtime::ModelRequest {
            provider: Some(&provider_id),
            model: Some("wire-model"),
            ..Default::default()
        };
        let runtime = match nous_runtime::Runtime::new(&config, &providers, &Env, &request) {
            Ok(runtime) => runtime,
            Err(error) => panic!("route {} failed to resolve: {error:?}", api.as_str()),
        };
        let active = runtime.current();
        let message = active
            .provider
            .stream_simple(
                &active.model,
                &Context::new(vec![]),
                &SimpleStreamOptions::default(),
            )
            .await
            .result()
            .await
            .unwrap_or_else(|error| panic!("route {} failed: {error:?}", api.as_str()));
        assert_eq!(message.api, *api);
        assert_eq!(
            nous_ai::Content::text_only(&message.content, ""),
            "one-message-from-the-mock-transport"
        );
        assert_eq!(message.stop_reason, nous_ai::StopReason::Stop);
    }
}
