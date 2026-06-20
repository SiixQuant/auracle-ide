//! The "Auracle Agent" language-model provider (PRD #169 slice 2).
//!
//! This provider does not hold a DeepSeek key and does not talk to
//! `api.deepseek.com`. Instead it routes every completion through the
//! operator's own Houston engine over loopback: `POST /ui/api/agent/chat`.
//! The engine owns the vaulted DeepSeek key, applies the Auracle harness
//! (system prompt + tools, placeholder today), and proxies to DeepSeek. The
//! IDE therefore never has to store or transmit the DeepSeek secret — it only
//! carries the per-seat engine key that `auracle_connect` already manages.
//!
//! Transport is the shared loopback client from `auracle_connections`
//! (`X-API-Key` + `auracle_session` cookie + CSRF double-submit). The gateway
//! is non-streaming in v1, so its single JSON response is mapped to a small
//! burst of `LanguageModelCompletionEvent`s (text, optional tool calls, usage,
//! a terminal stop). Streaming is a documented fast-follow: when the gateway
//! grows an SSE shape, `map_response` can be swapped for the streamed mapper
//! the DeepSeek provider already uses, without changing this provider's seam.
//!
//! Authentication is derived from the saved engine connection plus a health
//! probe (`GET /ui/api/agent/health`): authenticated means the engine is
//! reachable and reports `key_configured == true`. There is no paste-a-key
//! card — the configuration view points the user at the launcher (the sole
//! DeepSeek-key entry surface) and the Connect-to-Auracle flow.

use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use auracle_connect::load_config;
use auracle_connections::{get_json, post_json};
use futures::{FutureExt, StreamExt, future::BoxFuture, stream::BoxStream};
use gpui::{AnyView, App, AsyncApp, Context, Entity, Task, Window};
use http_client::HttpClient;
use language_model::{
    AuthenticateError, IconOrSvg, LanguageModel, LanguageModelCompletionError,
    LanguageModelCompletionEvent, LanguageModelId, LanguageModelName, LanguageModelProvider,
    LanguageModelProviderId, LanguageModelProviderName, LanguageModelProviderState,
    LanguageModelRequest, LanguageModelToolChoice, LanguageModelToolResultContent,
    LanguageModelToolUse, MessageContent, RateLimiter, Role, StopReason, TokenUsage,
};
use serde::Deserialize;
use ui::{ConfiguredApiCard, List, ListBulletItem, prelude::*};
use util::ResultExt as _;

use language_model::util::parse_tool_arguments;

const PROVIDER_ID: LanguageModelProviderId = LanguageModelProviderId::new("auracle-agent");
const PROVIDER_NAME: LanguageModelProviderName = LanguageModelProviderName::new("Auracle Agent");

const CHAT_PATH: &str = "/ui/api/agent/chat";
const HEALTH_PATH: &str = "/ui/api/agent/health";

/// The DeepSeek model ids the gateway exposes. The gateway resolves the actual
/// upstream model at build time (see PRD "model naming drift"); these are the
/// labels the IDE offers, and the chosen id is passed through to the gateway as
/// the requested `model`. The engine gateway resolves the concrete DeepSeek
/// model and can override it via AURACLE_DEEPSEEK_MODEL, so a single tool-capable
/// model is exposed here; deepseek-reasoner is intentionally not offered because
/// it does not support function calling, which the agent requires.
const DEFAULT_MODEL_ID: &str = "deepseek-chat";
const FAST_MODEL_ID: &str = "deepseek-chat";

#[derive(Clone, Copy)]
struct AgentModel {
    id: &'static str,
    display_name: &'static str,
    max_tokens: u64,
    max_output_tokens: u64,
}

const MODELS: &[AgentModel] = &[AgentModel {
    id: DEFAULT_MODEL_ID,
    display_name: "Auracle Agent",
    max_tokens: 65_536,
    max_output_tokens: 8_192,
}];

fn model_by_id(id: &str) -> AgentModel {
    MODELS
        .iter()
        .copied()
        .find(|model| model.id == id)
        .unwrap_or(MODELS[0])
}

pub struct AuracleAgentLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: Entity<State>,
}

/// Cached reachability of the engine gateway. `is_authenticated` is synchronous,
/// so the async health probe in [`State::refresh_health`] writes its verdict
/// here and the UI reads it. A saved engine key is the precondition; the probe
/// confirms the engine answers and holds a DeepSeek key.
pub struct State {
    /// Whether a saved engine connection key exists at all (read from
    /// `auracle_connect::load_config`). Without it the agent can't be reached.
    has_engine_key: bool,
    /// Whether the most recent health probe found the engine reachable.
    engine_reachable: bool,
    /// Whether the most recent health probe reported a vaulted DeepSeek key.
    key_configured: bool,
    http_client: Arc<dyn HttpClient>,
}

impl State {
    fn is_authenticated(&self) -> bool {
        self.has_engine_key && self.engine_reachable && self.key_configured
    }

    /// Refresh the cached reachability by hitting `GET /ui/api/agent/health`.
    /// A missing engine key short-circuits to "not authenticated" without a
    /// network call. Any error (engine down, off-box, no key) is recorded as
    /// not-authenticated rather than propagated, because the UI only needs a
    /// boolean verdict; the honest message is surfaced in the configuration
    /// view and at stream time.
    fn refresh_health(&mut self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        let has_engine_key = !load_config()
            .api_key
            .as_deref()
            .unwrap_or_default()
            .is_empty();
        self.has_engine_key = has_engine_key;
        if !has_engine_key {
            self.engine_reachable = false;
            self.key_configured = false;
            cx.notify();
            return Task::ready(Ok(()));
        }
        let http_client = self.http_client.clone();
        cx.spawn(async move |this, cx| {
            let health = fetch_health(http_client).await;
            this.update(cx, |this, cx| {
                match health {
                    Ok(health) => {
                        this.engine_reachable = true;
                        this.key_configured = health.key_configured;
                    }
                    Err(_) => {
                        this.engine_reachable = false;
                        this.key_configured = false;
                    }
                }
                cx.notify();
            })
            .log_err();
            Ok(())
        })
    }
}

impl AuracleAgentLanguageModelProvider {
    pub fn new(http_client: Arc<dyn HttpClient>, cx: &mut App) -> Self {
        let state = cx.new(|cx| {
            // Re-probe whenever the saved engine connection changes so the
            // agent flips to authenticated the moment the launcher (or the
            // Connect dialog) saves a working key.
            cx.observe_global::<auracle_connect::ConnectGeneration>(|this: &mut State, cx| {
                this.refresh_health(cx).detach();
            })
            .detach();
            State {
                has_engine_key: false,
                engine_reachable: false,
                key_configured: false,
                http_client: http_client.clone(),
            }
        });

        Self { http_client, state }
    }

    fn create_language_model(&self, model: AgentModel) -> Arc<dyn LanguageModel> {
        Arc::new(AuracleAgentLanguageModel {
            id: LanguageModelId::from(model.id.to_string()),
            model,
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }
}

impl LanguageModelProviderState for AuracleAgentLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for AuracleAgentLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn icon(&self) -> IconOrSvg {
        IconOrSvg::Icon(IconName::AuracleAgent)
    }

    fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model(model_by_id(DEFAULT_MODEL_ID)))
    }

    fn default_fast_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model(model_by_id(FAST_MODEL_ID)))
    }

    fn provided_models(&self, _cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        MODELS
            .iter()
            .copied()
            .map(|model| self.create_language_model(model))
            .collect()
    }

    fn recommended_models(&self, _cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        vec![self.create_language_model(model_by_id(DEFAULT_MODEL_ID))]
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        self.state.update(cx, |state, cx| state.refresh_health(cx))
    }

    fn configuration_view(
        &self,
        _target_agent: language_model::ConfigurationViewTargetAgent,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|cx| ConfigurationView::new(self.state.clone(), window, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        // There is no IDE-stored credential to reset; the DeepSeek key lives in
        // the engine vault and the engine key is managed by the launcher /
        // Connect dialog. Re-probe so the panel reflects current reachability.
        self.state.update(cx, |state, cx| {
            state.refresh_health(cx).detach();
        });
        Task::ready(Ok(()))
    }
}

pub struct AuracleAgentLanguageModel {
    id: LanguageModelId,
    model: AgentModel,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl LanguageModel for AuracleAgentLanguageModel {
    fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName::from(self.model.display_name.to_string())
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn supports_streaming_tools(&self) -> bool {
        true
    }

    fn supports_tool_choice(&self, _choice: LanguageModelToolChoice) -> bool {
        true
    }

    fn supports_images(&self) -> bool {
        false
    }

    fn telemetry_id(&self) -> String {
        format!("auracle-agent/{}", self.model.id)
    }

    fn max_token_count(&self) -> u64 {
        self.model.max_tokens
    }

    fn max_output_tokens(&self) -> Option<u64> {
        Some(self.model.max_output_tokens)
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        _cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<
            BoxStream<'static, Result<LanguageModelCompletionEvent, LanguageModelCompletionError>>,
            LanguageModelCompletionError,
        >,
    > {
        let body = into_gateway_body(request, self.model.id);
        let http_client = self.http_client.clone();

        let future = self.request_limiter.stream(async move {
            // Fail closed before the network call if there is no saved engine
            // connection: without it the gateway is unreachable and the only
            // honest verdict is "not authenticated".
            if load_config()
                .api_key
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                return Err(LanguageModelCompletionError::NoApiKey {
                    provider: PROVIDER_NAME,
                });
            }
            let response = post_json(http_client, CHAT_PATH, body)
                .await
                .context(
                    "couldn't reach your Auracle engine — start it, or add your \
                     DeepSeek key in the launcher",
                )
                .map_err(LanguageModelCompletionError::from)?;
            let chat: ChatResponse = serde_json::from_value(response)
                .context("the Auracle engine returned an unexpected agent response")
                .map_err(LanguageModelCompletionError::from)?;
            let events = map_response(chat);
            Ok(futures::stream::iter(events))
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }
}

// ── Request mapping: IDE completion request → gateway body ────────────────

/// Build the gateway request body. The gateway speaks an OpenAI-compatible
/// chat shape (`{model, messages, tools?}`); the IDE messages and tools are
/// flattened to that shape here. The DeepSeek key, the system prompt, and the
/// tool execution all live engine-side — the IDE only forwards the
/// conversation and the tool *schemas* the agent may call.
fn into_gateway_body(request: LanguageModelRequest, model_id: &str) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    for message in request.messages {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        };
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<serde_json::Value> = Vec::new();

        for content in message.content {
            match content {
                MessageContent::Text(text) => {
                    if !text.is_empty() {
                        text_parts.push(text);
                    }
                }
                MessageContent::Thinking { .. } | MessageContent::RedactedThinking(_) => {}
                MessageContent::Image(_) => {}
                MessageContent::ToolUse(tool_use) => {
                    tool_calls.push(serde_json::json!({
                        "id": tool_use.id.to_string(),
                        "type": "function",
                        "function": {
                            "name": tool_use.name.to_string(),
                            "arguments": serde_json::to_string(&tool_use.input)
                                .unwrap_or_default(),
                        },
                    }));
                }
                MessageContent::ToolResult(tool_result) => {
                    let mut parts: Vec<String> = Vec::new();
                    for part in &tool_result.content {
                        match part {
                            LanguageModelToolResultContent::Text(text) => {
                                parts.push(text.to_string());
                            }
                            LanguageModelToolResultContent::Image(_) => {
                                parts.push("[Tool responded with an image]".to_string());
                            }
                        }
                    }
                    let content = if parts.is_empty() {
                        "<Tool returned an empty string>".to_string()
                    } else {
                        parts.join("\n")
                    };
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "content": content,
                        "tool_call_id": tool_result.tool_use_id.to_string(),
                    }));
                }
            }
        }

        if text_parts.is_empty() && tool_calls.is_empty() {
            continue;
        }
        let mut entry = serde_json::Map::new();
        entry.insert("role".into(), serde_json::Value::String(role.to_string()));
        entry.insert(
            "content".into(),
            serde_json::Value::String(text_parts.join("\n")),
        );
        if !tool_calls.is_empty() {
            entry.insert("tool_calls".into(), serde_json::Value::Array(tool_calls));
        }
        messages.push(serde_json::Value::Object(entry));
    }

    let tools: Vec<serde_json::Value> = request
        .tools
        .into_iter()
        .map(|tool| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                },
            })
        })
        .collect();

    let mut body = serde_json::Map::new();
    body.insert(
        "model".into(),
        serde_json::Value::String(model_id.to_string()),
    );
    body.insert("messages".into(), serde_json::Value::Array(messages));
    if !tools.is_empty() {
        body.insert("tools".into(), serde_json::Value::Array(tools));
    }
    serde_json::Value::Object(body)
}

// ── Response mapping: gateway JSON → completion events ────────────────────

/// The non-streaming gateway response. The engine returns the full assistant
/// message in one shot; `usage` and `finish_reason` are optional.
#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    message: ResponseMessage,
    #[serde(default)]
    usage: Option<ResponseUsage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Debug, Deserialize)]
struct ResponseToolCall {
    #[serde(default)]
    id: String,
    #[serde(default)]
    function: ResponseFunction,
}

#[derive(Debug, Default, Deserialize)]
struct ResponseFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ResponseUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

/// Flatten the single gateway response into the burst of events the agent loop
/// expects from a streaming provider: text, then any tool calls, then a usage
/// update, then a terminal stop reason. This is the non-streaming→event wrap;
/// when the gateway gains SSE this is the seam to replace.
fn map_response(
    response: ChatResponse,
) -> Vec<Result<LanguageModelCompletionEvent, LanguageModelCompletionError>> {
    let mut events = Vec::new();

    if let Some(content) = response.message.content
        && !content.is_empty()
    {
        events.push(Ok(LanguageModelCompletionEvent::Text(content)));
    }

    let has_tool_calls = !response.message.tool_calls.is_empty();
    for tool_call in response.message.tool_calls {
        let event = match parse_tool_arguments(&tool_call.function.arguments) {
            Ok(input) => Ok(LanguageModelCompletionEvent::ToolUse(
                LanguageModelToolUse {
                    id: tool_call.id.clone().into(),
                    name: tool_call.function.name.as_str().into(),
                    is_input_complete: true,
                    input,
                    raw_input: tool_call.function.arguments.clone(),
                    thought_signature: None,
                },
            )),
            Err(error) => Ok(LanguageModelCompletionEvent::ToolUseJsonParseError {
                id: tool_call.id.clone().into(),
                tool_name: tool_call.function.name.as_str().into(),
                raw_input: tool_call.function.arguments.into(),
                json_parse_error: error.to_string(),
            }),
        };
        events.push(event);
    }

    if let Some(usage) = response.usage {
        events.push(Ok(LanguageModelCompletionEvent::UsageUpdate(TokenUsage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        })));
    }

    let stop_reason = match response.finish_reason.as_deref() {
        Some("tool_calls") => StopReason::ToolUse,
        Some("stop") | None => {
            if has_tool_calls {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        }
        Some(other) => {
            log::error!("Unexpected Auracle Agent finish_reason: {other:?}");
            StopReason::EndTurn
        }
    };
    events.push(Ok(LanguageModelCompletionEvent::Stop(stop_reason)));

    events
}

// ── Health probe ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct HealthResponse {
    #[serde(default)]
    key_configured: bool,
}

async fn fetch_health(http_client: Arc<dyn HttpClient>) -> Result<HealthResponse> {
    let value = get_json(http_client, HEALTH_PATH).await?;
    serde_json::from_value(value).map_err(|error| anyhow!(error))
}

// ── Configuration view ─────────────────────────────────────────────────────

struct ConfigurationView {
    state: Entity<State>,
    load_task: Option<Task<()>>,
}

impl ConfigurationView {
    fn new(state: Entity<State>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();

        let load_task = Some(cx.spawn({
            let state = state.clone();
            async move |this, cx| {
                state
                    .update(cx, |state, cx| state.refresh_health(cx))
                    .await
                    .log_err();
                this.update(cx, |this, cx| {
                    this.load_task = None;
                    cx.notify();
                })
                .log_err();
            }
        }));

        Self { state, load_task }
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.load_task.is_some() {
            return div()
                .child(Label::new("Checking your Auracle engine…"))
                .into_any_element();
        }

        let state = self.state.read(cx);
        if state.is_authenticated() {
            return ConfiguredApiCard::new("Connected to your Auracle engine")
                .disabled(true)
                .into_any_element();
        }

        // Honest, specific guidance for the two distinct failure modes.
        let detail = if !state.has_engine_key || !state.engine_reachable {
            "Connect to your Auracle engine first (the launcher does this \
             automatically, or use Connect to Auracle)."
        } else {
            "Add your DeepSeek key in the launcher — the Auracle Agent reuses \
             that one vaulted key, so there's nothing to paste here."
        };

        v_flex()
            .size_full()
            .child(Label::new(
                "The Auracle Agent runs on your own engine, using the DeepSeek \
                 key you already entered in the launcher.",
            ))
            .child(
                List::new()
                    .child(ListBulletItem::new(detail))
                    .child(ListBulletItem::new(
                        "Nothing leaves your machine except the call your engine \
                         makes to DeepSeek itself.",
                    )),
            )
            .into_any_element()
    }
}
