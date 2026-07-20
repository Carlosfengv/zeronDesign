use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{Duration as ChronoDuration, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;

use crate::{
    config::{ModelProvider, RuntimeConfig},
    tools::registry::{McpToolInfo, ToolLoadingPolicy},
    types::AgentPhase,
};

const MAX_STREAMING_TOOL_ARGUMENT_CHARS: usize = 96_000;
const OPENAI_TRANSPORT_ATTEMPTS: u32 = 5;
const OPENAI_TRANSPORT_RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
const PROVIDER_GATEWAY_TURN_REQUEST_SCHEMA: &str = "provider-gateway-turn-request@1";
const PROVIDER_GATEWAY_TRANSPORT_ATTEMPTS: u32 = 3;
static GATEWAY_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input: normalize_tool_input(input),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelResponse {
    ToolCalls(Vec<ToolCall>),
    ToolInputParseFailed {
        parsed_calls: Vec<ToolCall>,
        failures: Vec<ToolInputParseFailure>,
    },
    ToolInputTooLarge {
        parsed_calls: Vec<ToolCall>,
        failures: Vec<ToolInputTooLargeFailure>,
    },
    ToolCallsThenError {
        calls: Vec<ToolCall>,
        error: String,
    },
    ToolCallsThenFallback {
        calls: Vec<ToolCall>,
        reason: String,
    },
    TextOnly(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolInputParseFailure {
    pub tool_call_id: String,
    pub tool_name: String,
    pub raw_len: usize,
    pub raw_sha256: String,
    pub ends_with_json_close: bool,
    pub bracket_balance: i32,
    pub quote_closed: bool,
    pub likely_truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolInputTooLargeFailure {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input_chars: usize,
    pub max_input_chars: usize,
    pub raw_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequest {
    pub run_id: String,
    pub turn: u32,
    pub model: String,
    pub phase: AgentPhase,
    pub agent_profile: String,
    pub system_prompt: String,
    pub messages: Vec<Value>,
    pub tools: Vec<ModelToolDefinition>,
    pub deferred_tools: Vec<ModelToolDefinition>,
}

/// Trusted Runtime context used only when calling the internal Provider Gateway.
/// Direct provider clients deliberately ignore it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelGatewayScope {
    pub workspace_id: String,
    pub project_id: String,
}

/// Low-sensitivity execution record returned by the Provider Gateway. It is
/// safe to persist in a Run event and deliberately excludes prompts, tool
/// arguments, and provider credentials.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelExecutionSnapshot {
    pub id: String,
    pub model_resource_id: String,
    pub model_resource_revision: u64,
    pub provider_id: String,
    pub physical_model: String,
    pub selection_policy_id: String,
    pub selection_policy_revision: u64,
    pub capability_snapshot_hash: String,
    pub selection_reason: String,
    pub automatic_switch: Value,
    #[serde(default)]
    pub provider_request_id: Option<String>,
    #[serde(default)]
    pub provider_attempt_count: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelClientTurn {
    pub response: ModelResponse,
    pub execution: Option<ModelExecutionSnapshot>,
    pub usage: Option<ModelTokenUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelGatewayRequestError {
    pub status: u16,
    pub code: String,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
}

impl fmt::Display for ModelGatewayRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "model gateway request failed: status={} code={} retryable={} retry_after_ms={:?}",
            reqwest::StatusCode::from_u16(self.status)
                .map(|status| status.to_string())
                .unwrap_or_else(|_| self.status.to_string()),
            self.code,
            self.retryable,
            self.retry_after_ms
        )
    }
}

impl Error for ModelGatewayRequestError {}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelToolDefinition {
    pub name: String,
    pub input_schema: Value,
    pub input_json_schema: Option<Value>,
    pub output_schema: Option<Value>,
    pub loading_policy: ToolLoadingPolicy,
    pub mcp_info: Option<McpToolInfo>,
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn next_response(&self, request: ModelRequest) -> Result<ModelResponse>;

    async fn next_response_scoped(
        &self,
        request: ModelRequest,
        _scope: ModelGatewayScope,
    ) -> Result<ModelResponse> {
        self.next_response(request).await
    }

    async fn next_response_scoped_with_execution(
        &self,
        request: ModelRequest,
        scope: ModelGatewayScope,
    ) -> Result<ModelClientTurn> {
        Ok(ModelClientTurn {
            response: self.next_response_scoped(request, scope).await?,
            execution: None,
            usage: None,
        })
    }
}

pub fn model_client_from_config(config: &RuntimeConfig) -> Result<ConfiguredModelClient> {
    match config.model_provider {
        ModelProvider::InternalGateway => Ok(ConfiguredModelClient::InternalGateway(
            HttpModelGatewayClient::new(config.model_gateway_url.clone())
                .with_runtime_bearer_token(config.model_gateway_auth_token.clone())
                .with_timeout(Duration::from_secs(config.model_request_timeout_seconds)),
        )),
        ModelProvider::DeepSeek => {
            let api_key = config
                .deepseek_api_key
                .clone()
                .ok_or_else(|| anyhow!("MODEL_PROVIDER=deepseek requires DEEPSEEK_API_KEY"))?;
            Ok(ConfiguredModelClient::OpenAiCompatible(
                OpenAiCompatibleModelClient::new(
                    config.deepseek_base_url.clone(),
                    api_key,
                    Some("deepseek"),
                )
                .with_streaming(config.model_streaming)
                .with_strict_tools(config.model_strict_tools)
                .with_timeout(Duration::from_secs(config.model_request_timeout_seconds)),
            ))
        }
        ModelProvider::KimiGlobal => {
            let api_key = config.kimi_api_key.clone().ok_or_else(|| {
                anyhow!("MODEL_PROVIDER=kimi_global requires KIMI_API_KEY or MOONSHOT_API_KEY")
            })?;
            Ok(ConfiguredModelClient::OpenAiCompatible(
                OpenAiCompatibleModelClient::new(
                    config.kimi_base_url.clone(),
                    api_key,
                    Some("kimi_global"),
                )
                .with_streaming(config.model_streaming)
                .with_strict_tools(config.model_strict_tools)
                .with_timeout(Duration::from_secs(config.model_request_timeout_seconds)),
            ))
        }
        ModelProvider::KimiChina => {
            let api_key = config
                .kimi_cn_api_key
                .clone()
                .or_else(|| config.kimi_api_key.clone())
                .ok_or_else(|| anyhow!("MODEL_PROVIDER=kimi_cn requires KIMI_CN_API_KEY, KIMI_API_KEY, or MOONSHOT_API_KEY"))?;
            Ok(ConfiguredModelClient::OpenAiCompatible(
                OpenAiCompatibleModelClient::new(
                    config.kimi_cn_base_url.clone(),
                    api_key,
                    Some("kimi_cn"),
                )
                .with_streaming(config.model_streaming)
                .with_strict_tools(config.model_strict_tools)
                .with_timeout(Duration::from_secs(config.model_request_timeout_seconds)),
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConfiguredModelClient {
    InternalGateway(HttpModelGatewayClient),
    OpenAiCompatible(OpenAiCompatibleModelClient),
}

impl ConfiguredModelClient {
    pub fn provider_id(&self) -> &str {
        match self {
            Self::InternalGateway(_) => "internal_gateway",
            Self::OpenAiCompatible(client) => client.provider_id(),
        }
    }

    pub fn endpoint(&self) -> &str {
        match self {
            Self::InternalGateway(client) => client.endpoint(),
            Self::OpenAiCompatible(client) => client.endpoint(),
        }
    }
}

#[async_trait]
impl ModelClient for ConfiguredModelClient {
    async fn next_response(&self, request: ModelRequest) -> Result<ModelResponse> {
        match self {
            Self::InternalGateway(client) => client.next_response(request).await,
            Self::OpenAiCompatible(client) => client.next_response(request).await,
        }
    }

    async fn next_response_scoped(
        &self,
        request: ModelRequest,
        scope: ModelGatewayScope,
    ) -> Result<ModelResponse> {
        match self {
            Self::InternalGateway(client) => client.next_response_scoped(request, scope).await,
            Self::OpenAiCompatible(client) => client.next_response(request).await,
        }
    }

    async fn next_response_scoped_with_execution(
        &self,
        request: ModelRequest,
        scope: ModelGatewayScope,
    ) -> Result<ModelClientTurn> {
        match self {
            Self::InternalGateway(client) => {
                client
                    .next_response_scoped_with_execution(request, scope)
                    .await
            }
            Self::OpenAiCompatible(client) => Ok(ModelClientTurn {
                response: client.next_response(request).await?,
                execution: None,
                usage: None,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpModelGatewayClient {
    endpoint: String,
    client: reqwest::Client,
    request_timeout: Duration,
    runtime_bearer_token: Option<String>,
}

impl HttpModelGatewayClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let endpoint = format!("{}/v1/agent/turn", base_url.trim_end_matches('/'));
        Self {
            endpoint,
            client: reqwest::Client::new(),
            request_timeout: Duration::from_secs(180),
            runtime_bearer_token: None,
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self.client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("model gateway HTTP client");
        self
    }

    pub fn with_runtime_bearer_token(mut self, token: Option<String>) -> Self {
        self.runtime_bearer_token = token.filter(|token| !token.trim().is_empty());
        self
    }

    async fn execute(&self, request: ModelRequest) -> Result<ModelResponse> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.json::<Value>().await.unwrap_or_else(|_| json!({}));
            let code = body
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("gateway_request_failed");
            let retryable = body
                .get("error")
                .and_then(|error| error.get("retryable"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return Err(anyhow!(
                "model gateway request failed: status={} code={} retryable={}",
                status,
                code,
                retryable
            ));
        }
        let response = response.json::<ModelGatewayTurnResponse>().await?;
        Ok(response.into())
    }

    async fn execute_scoped(
        &self,
        request: ModelRequest,
        scope: ModelGatewayScope,
    ) -> Result<ModelClientTurn> {
        let request_id = format!(
            "runtime-{}-{}-{}",
            request.run_id,
            request.turn,
            GATEWAY_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let phase = serde_json::to_value(request.phase)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| "build".to_string());
        let payload = json!({
            "schemaVersion": PROVIDER_GATEWAY_TURN_REQUEST_SCHEMA,
            "requestId": request_id,
            "idempotencyKey": format!("{}:turn-{}", request.run_id, request.turn),
            "deadlineAt": (Utc::now() + ChronoDuration::from_std(self.request_timeout).unwrap_or_else(|_| ChronoDuration::seconds(180))).to_rfc3339(),
            "scope": {
                "workspaceId": scope.workspace_id,
                "projectId": scope.project_id,
                "runId": request.run_id,
                "turn": request.turn,
                "phase": phase,
                "agentProfile": request.agent_profile,
            },
            "routing": {
                "modelResourceId": explicit_model_resource_id(&request.model)
                    .map(|id| Value::String(id.to_string()))
                    .unwrap_or(Value::Null),
                "requiredCapabilities": {
                    "toolCalls": !request.tools.is_empty() || !request.deferred_tools.is_empty(),
                    "strictToolSchema": false,
                    "streaming": false,
                    "vision": runtime_messages_require_vision(&request.messages),
                }
            },
            "input": {
                "systemPrompt": request.system_prompt,
                "messages": request.messages,
                "tools": request.tools,
                "deferredTools": request.deferred_tools,
            }
        });
        for attempt in 1..=PROVIDER_GATEWAY_TRANSPORT_ATTEMPTS {
            let mut call = self
                .client
                .post(&self.endpoint)
                .header(
                    "idempotency-key",
                    payload["idempotencyKey"].as_str().unwrap_or_default(),
                )
                .header(
                    "x-request-id",
                    payload["requestId"].as_str().unwrap_or_default(),
                )
                .json(&payload);
            if let Some(token) = &self.runtime_bearer_token {
                call = call.bearer_auth(token);
            }
            let response = match call.send().await {
                Ok(response) => response,
                Err(error)
                    if provider_gateway_transport_error_retryable(&error)
                        && attempt < PROVIDER_GATEWAY_TRANSPORT_ATTEMPTS =>
                {
                    tokio::time::sleep(Duration::from_millis(
                        250u64.saturating_mul(u64::from(attempt)),
                    ))
                    .await;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let status = response.status();
            let body = response.json::<Value>().await.unwrap_or_else(|_| json!({}));
            if !status.is_success() {
                let failure = ModelGatewayRequestError {
                    status: status.as_u16(),
                    code: body
                        .get("error")
                        .and_then(|error| error.get("code"))
                        .and_then(Value::as_str)
                        .unwrap_or("gateway_request_failed")
                        .to_string(),
                    retryable: body
                        .get("error")
                        .and_then(|error| error.get("retryable"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    retry_after_ms: body
                        .get("error")
                        .and_then(|error| error.get("retryAfterMs"))
                        .and_then(Value::as_u64),
                };
                if failure.retryable && attempt < PROVIDER_GATEWAY_TRANSPORT_ATTEMPTS {
                    let fallback_delay = 250u64.saturating_mul(u64::from(attempt));
                    tokio::time::sleep(Duration::from_millis(
                        failure.retry_after_ms.unwrap_or(fallback_delay).min(5_000),
                    ))
                    .await;
                    continue;
                }
                return Err(anyhow::Error::new(failure));
            }
            if body.get("schemaVersion").and_then(Value::as_str)
                == Some("provider-gateway-turn-response@1")
            {
                let response = serde_json::from_value::<VersionedGatewayTurnResponse>(body)?;
                let execution = response.model_execution.clone();
                let usage = response.usage;
                return Ok(ModelClientTurn {
                    response: response.into(),
                    execution: Some(execution),
                    usage: Some(usage),
                });
            }
            // Fixture and legacy gateway compatibility during the staged migration.
            return Ok(ModelClientTurn {
                response: serde_json::from_value::<ModelGatewayTurnResponse>(body)?.into(),
                execution: None,
                usage: None,
            });
        }
        unreachable!("Provider Gateway transport attempts are at least one")
    }
}

fn provider_gateway_transport_error_retryable(error: &reqwest::Error) -> bool {
    !error.is_builder() && (error.is_connect() || error.is_timeout() || error.is_request())
}

fn explicit_model_resource_id(model: &str) -> Option<&str> {
    model.strip_prefix("resource:").filter(|id| !id.is_empty())
}

#[async_trait]
impl ModelClient for HttpModelGatewayClient {
    async fn next_response(&self, request: ModelRequest) -> Result<ModelResponse> {
        tokio::time::timeout(self.request_timeout, self.execute(request))
            .await
            .map_err(|_| {
                anyhow!(
                    "model gateway turn timed out after {}ms",
                    self.request_timeout.as_millis()
                )
            })?
    }

    async fn next_response_scoped(
        &self,
        request: ModelRequest,
        scope: ModelGatewayScope,
    ) -> Result<ModelResponse> {
        Ok(
            tokio::time::timeout(self.request_timeout, self.execute_scoped(request, scope))
                .await
                .map_err(|_| {
                    anyhow!(
                        "model gateway turn timed out after {}ms",
                        self.request_timeout.as_millis()
                    )
                })??
                .response,
        )
    }

    async fn next_response_scoped_with_execution(
        &self,
        request: ModelRequest,
        scope: ModelGatewayScope,
    ) -> Result<ModelClientTurn> {
        tokio::time::timeout(self.request_timeout, self.execute_scoped(request, scope))
            .await
            .map_err(|_| {
                anyhow!(
                    "model gateway turn timed out after {}ms",
                    self.request_timeout.as_millis()
                )
            })?
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleModelClient {
    provider_id: String,
    endpoint: String,
    api_key: String,
    streaming: bool,
    strict_tools: bool,
    client: reqwest::Client,
    request_timeout: Duration,
    transport_attempts: u32,
    transport_retry_base_delay: Duration,
}

impl OpenAiCompatibleModelClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        provider_id: Option<&str>,
    ) -> Self {
        let base_url = base_url.into();
        let endpoint = chat_completions_endpoint(&base_url);
        Self {
            provider_id: provider_id.unwrap_or("openai_compatible").to_string(),
            endpoint,
            api_key: api_key.into(),
            streaming: false,
            strict_tools: false,
            client: reqwest::Client::new(),
            request_timeout: Duration::from_secs(180),
            transport_attempts: OPENAI_TRANSPORT_ATTEMPTS,
            transport_retry_base_delay: OPENAI_TRANSPORT_RETRY_BASE_DELAY,
        }
    }

    pub fn with_streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    pub fn with_strict_tools(mut self, strict_tools: bool) -> Self {
        self.strict_tools = strict_tools;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self.client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("OpenAI-compatible HTTP client");
        self
    }

    pub fn with_transport_retry(mut self, attempts: u32, base_delay: Duration) -> Self {
        self.transport_attempts = attempts.max(1);
        self.transport_retry_base_delay = base_delay;
        self
    }

    async fn execute(&self, request: ModelRequest) -> Result<ModelResponse> {
        let (mut body, tool_name_map) = openai_chat_request(&request, self.strict_tools)?;
        if self.streaming {
            body["stream"] = Value::Bool(true);
        }
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| openai_transport_error(&self.provider_id, "sending request", error))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "{} model request failed: status={} body={}",
                self.provider_id,
                status,
                body
            ));
        }
        if self.streaming {
            return openai_streaming_model_response(response, &tool_name_map, &self.provider_id)
                .await;
        }
        let response_body = response.text().await.map_err(|error| {
            openai_transport_error(&self.provider_id, "reading response", error)
        })?;
        let response = serde_json::from_str::<OpenAiChatCompletionResponse>(&response_body)
            .map_err(|error| {
                anyhow!(
                    "{} model response decode failed: {error}; body={}",
                    self.provider_id,
                    response_body.chars().take(2_000).collect::<String>()
                )
            })?;
        Ok(response.into_model_response(&tool_name_map))
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[async_trait]
impl ModelClient for OpenAiCompatibleModelClient {
    async fn next_response(&self, request: ModelRequest) -> Result<ModelResponse> {
        let execute_with_retry = async {
            for attempt in 1..=self.transport_attempts {
                match self.execute(request.clone()).await {
                    Err(error)
                        if error.downcast_ref::<OpenAiTransportError>().is_some()
                            && attempt < self.transport_attempts =>
                    {
                        let multiplier = 1_u32 << (attempt - 1).min(30);
                        tokio::time::sleep(
                            self.transport_retry_base_delay.saturating_mul(multiplier),
                        )
                        .await;
                    }
                    result => return result,
                }
            }
            unreachable!("transport retry loop always returns")
        };
        tokio::time::timeout(self.request_timeout, execute_with_retry)
            .await
            .map_err(|_| {
                anyhow!(
                    "{} model turn timed out after {}ms",
                    self.provider_id,
                    self.request_timeout.as_millis()
                )
            })?
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ModelGatewayTurnResponse {
    ToolCalls {
        #[serde(rename = "toolCalls")]
        #[serde(default)]
        tool_calls: Vec<ModelGatewayToolCall>,
    },
    ToolCallsThenError {
        #[serde(rename = "toolCalls")]
        #[serde(default)]
        tool_calls: Vec<ModelGatewayToolCall>,
        error: String,
    },
    ToolCallsThenFallback {
        #[serde(rename = "toolCalls")]
        #[serde(default)]
        tool_calls: Vec<ModelGatewayToolCall>,
        reason: String,
    },
    Text {
        text: String,
    },
    Error {
        error: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionedGatewayTurnResponse {
    #[serde(rename = "type")]
    response_type: String,
    #[serde(default)]
    tool_calls: Vec<ModelGatewayToolCall>,
    #[serde(default)]
    text: Option<String>,
    model_execution: ModelExecutionSnapshot,
    #[serde(default)]
    usage: ModelTokenUsage,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatCompletionResponse {
    #[serde(default)]
    choices: Vec<OpenAiChatChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatChoice {
    message: OpenAiChatMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatMessage {
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    #[serde(default)]
    id: Option<String>,
    function: OpenAiToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallFunction {
    name: String,
    arguments: Value,
}

impl OpenAiChatCompletionResponse {
    fn into_model_response(self, tool_name_map: &HashMap<String, String>) -> ModelResponse {
        let Some(choice) = self.choices.into_iter().next() else {
            return ModelResponse::Error("model returned no choices".to_string());
        };
        let tool_calls = choice.message.tool_calls.unwrap_or_default();
        if !tool_calls.is_empty() {
            return openai_tool_calls_to_model_response(
                tool_calls.into_iter().enumerate().map(|(index, call)| {
                    let arguments = match call.function.arguments {
                        Value::String(arguments) => arguments,
                        arguments => arguments.to_string(),
                    };
                    (
                        call.id.unwrap_or_else(|| format!("tool-call-{index}")),
                        call.function.name,
                        arguments,
                    )
                }),
                tool_name_map,
            );
        }
        let text = match choice.message.content {
            Some(Value::String(text)) => text,
            Some(value) => value.to_string(),
            None => String::new(),
        };
        ModelResponse::TextOnly(text)
    }
}

async fn openai_streaming_model_response(
    response: reqwest::Response,
    tool_name_map: &HashMap<String, String>,
    provider_id: &str,
) -> Result<ModelResponse> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut text = String::new();
    let mut tool_calls = BTreeMap::<u64, OpenAiStreamingToolCall>::new();

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|error| openai_transport_error(provider_id, "reading stream", error))?;
        buffer.push_str(std::str::from_utf8(&chunk)?);
        while let Some(boundary) = buffer.find("\n\n") {
            let event = buffer.drain(..boundary + 2).collect::<String>();
            if process_openai_stream_event(&event, &mut text, &mut tool_calls)? {
                return Ok(openai_stream_accumulators_to_model_response(
                    text,
                    tool_calls,
                    tool_name_map,
                ));
            }
            if let Some(failure) = streaming_tool_input_too_large(&tool_calls, tool_name_map) {
                return Ok(ModelResponse::ToolInputTooLarge {
                    parsed_calls: Vec::new(),
                    failures: vec![failure],
                });
            }
        }
    }
    if !buffer.trim().is_empty() {
        process_openai_stream_event(&buffer, &mut text, &mut tool_calls)?;
        if let Some(failure) = streaming_tool_input_too_large(&tool_calls, tool_name_map) {
            return Ok(ModelResponse::ToolInputTooLarge {
                parsed_calls: Vec::new(),
                failures: vec![failure],
            });
        }
    }
    Ok(openai_stream_accumulators_to_model_response(
        text,
        tool_calls,
        tool_name_map,
    ))
}

fn openai_transport_error(
    provider_id: &str,
    operation: &str,
    error: reqwest::Error,
) -> anyhow::Error {
    let message = if error.is_timeout() {
        format!("{provider_id} model turn timed out while {operation}")
    } else {
        format!("{provider_id} model transport failed while {operation}: {error}")
    };
    anyhow!(OpenAiTransportError(message))
}

#[derive(Debug)]
struct OpenAiTransportError(String);

impl std::fmt::Display for OpenAiTransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for OpenAiTransportError {}

#[derive(Debug, Default)]
struct OpenAiStreamingToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

fn process_openai_stream_event(
    event: &str,
    text: &mut String,
    tool_calls: &mut BTreeMap<u64, OpenAiStreamingToolCall>,
) -> Result<bool> {
    for line in event.lines() {
        let line = line.trim();
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            return Ok(true);
        }
        if data.is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(data)?;
        for choice in value
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(delta) = choice.get("delta") else {
                continue;
            };
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                text.push_str(content);
            }
            for call in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                let accumulator = tool_calls.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    accumulator.id = Some(id.to_string());
                }
                if let Some(function) = call.get("function") {
                    if let Some(name) = function.get("name").and_then(Value::as_str) {
                        accumulator.name = Some(name.to_string());
                    }
                    if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                        accumulator.arguments.push_str(arguments);
                    }
                }
            }
        }
    }
    Ok(false)
}

fn streaming_tool_input_too_large(
    tool_calls: &BTreeMap<u64, OpenAiStreamingToolCall>,
    tool_name_map: &HashMap<String, String>,
) -> Option<ToolInputTooLargeFailure> {
    tool_calls.iter().find_map(|(index, call)| {
        let input_chars = call.arguments.chars().count();
        if input_chars <= MAX_STREAMING_TOOL_ARGUMENT_CHARS {
            return None;
        }
        let api_name = call
            .name
            .clone()
            .unwrap_or_else(|| "unknown_tool".to_string());
        let tool_name = tool_name_map.get(&api_name).cloned().unwrap_or(api_name);
        Some(ToolInputTooLargeFailure {
            tool_call_id: call
                .id
                .clone()
                .unwrap_or_else(|| format!("stream-tool-call-{index}")),
            tool_name,
            input_chars,
            max_input_chars: MAX_STREAMING_TOOL_ARGUMENT_CHARS,
            raw_sha256: sha256_hex(call.arguments.as_bytes()),
        })
    })
}

fn openai_stream_accumulators_to_model_response(
    text: String,
    tool_calls: BTreeMap<u64, OpenAiStreamingToolCall>,
    tool_name_map: &HashMap<String, String>,
) -> ModelResponse {
    if tool_calls.is_empty() {
        return ModelResponse::TextOnly(text);
    }
    openai_tool_calls_to_model_response(
        tool_calls.into_iter().map(|(index, call)| {
            (
                call.id
                    .unwrap_or_else(|| format!("stream-tool-call-{index}")),
                call.name.unwrap_or_else(|| "unknown_tool".to_string()),
                call.arguments,
            )
        }),
        tool_name_map,
    )
}

fn openai_tool_calls_to_model_response(
    calls: impl IntoIterator<Item = (String, String, String)>,
    tool_name_map: &HashMap<String, String>,
) -> ModelResponse {
    let mut parsed_calls = Vec::new();
    let mut failures = Vec::new();
    for (id, api_name, arguments) in calls {
        let name = tool_name_map.get(&api_name).cloned().unwrap_or(api_name);
        match serde_json::from_str::<Value>(&arguments) {
            Ok(input) => {
                parsed_calls.push(ToolCall::new(id, name, input));
            }
            Err(_) => {
                failures.push(tool_input_parse_failure(id, name, &arguments));
            }
        }
    }
    if !failures.is_empty() {
        return ModelResponse::ToolInputParseFailed {
            parsed_calls,
            failures,
        };
    }
    ModelResponse::ToolCalls(parsed_calls)
}

fn tool_input_parse_failure(
    tool_call_id: String,
    tool_name: String,
    raw_arguments: &str,
) -> ToolInputParseFailure {
    let (bracket_balance, quote_closed) = json_shape_diagnostics(raw_arguments);
    let trimmed = raw_arguments.trim_end();
    let ends_with_json_close = trimmed.ends_with('}') || trimmed.ends_with(']');
    let has_large_write_signal = ["\"text\"", "<html", "---", "<style", "class="]
        .iter()
        .any(|signal| raw_arguments.contains(signal));
    ToolInputParseFailure {
        tool_call_id,
        tool_name,
        raw_len: raw_arguments.len(),
        raw_sha256: sha256_hex(raw_arguments.as_bytes()),
        ends_with_json_close,
        bracket_balance,
        quote_closed,
        likely_truncated: !ends_with_json_close
            || !quote_closed
            || bracket_balance != 0
            || has_large_write_signal,
    }
}

fn json_shape_diagnostics(raw: &str) -> (i32, bool) {
    let mut balance = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_string {
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => balance += 1,
            '}' | ']' => balance -= 1,
            _ => {}
        }
    }
    (balance, !in_string)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelGatewayToolCall {
    id: String,
    name: String,
    #[serde(default)]
    input: Value,
}

fn chat_completions_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn openai_chat_request(
    request: &ModelRequest,
    strict_tools: bool,
) -> Result<(Value, HashMap<String, String>)> {
    let tool_name_map = openai_tool_name_map(request);
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system_prompt,
    })];
    messages.extend(openai_messages_from_runtime(&request.messages)?);
    let tools = request
        .tools
        .iter()
        .map(|tool| openai_tool_definition(tool, strict_tools))
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": false,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    Ok((body, tool_name_map))
}

fn openai_messages_from_runtime(messages: &[Value]) -> Result<Vec<Value>> {
    let mut output = Vec::new();
    let mut pending_tool_call_ids = HashSet::new();
    for message in messages {
        let Some(openai_message) = openai_message_from_runtime(message)? else {
            continue;
        };
        match openai_message.get("role").and_then(Value::as_str) {
            Some("assistant") => {
                pending_tool_call_ids.clear();
                if let Some(tool_calls) = openai_message.get("tool_calls").and_then(Value::as_array)
                {
                    pending_tool_call_ids.extend(tool_calls.iter().filter_map(|call| {
                        call.get("id").and_then(Value::as_str).map(str::to_string)
                    }));
                }
                output.push(openai_message);
            }
            Some("tool") => {
                let Some(tool_call_id) = openai_message.get("tool_call_id").and_then(Value::as_str)
                else {
                    continue;
                };
                if pending_tool_call_ids.remove(tool_call_id) {
                    output.push(openai_message);
                }
            }
            _ => {
                pending_tool_call_ids.clear();
                output.push(openai_message);
            }
        }
    }
    Ok(output)
}

fn openai_message_from_runtime(message: &Value) -> Result<Option<Value>> {
    let Some(role) = message.get("role").and_then(Value::as_str) else {
        return Ok(None);
    };
    Ok(match role {
        "assistant" => {
            let tool_calls = message
                .get("toolCalls")
                .and_then(Value::as_array)
                .map(|calls| {
                    calls
                        .iter()
                        .filter_map(openai_tool_call_from_runtime)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if tool_calls.is_empty() {
                Some(json!({
                    "role": "assistant",
                    "content": message_text(message),
                }))
            } else {
                Some(json!({
                    "role": "assistant",
                    "content": Value::Null,
                    "tool_calls": tool_calls,
                }))
            }
        }
        "tool" => Some(json!({
            "role": "tool",
            "tool_call_id": message
                .get("toolUseId")
                .and_then(Value::as_str)
                .unwrap_or("unknown-tool-call"),
            "content": message_content_string(message),
        })),
        "user" => Some(json!({
            "role": "user",
            "content": openai_user_content(message)?,
        })),
        "system" | "model" | "runtime" => Some(json!({
            "role": "system",
            "content": message_text(message),
        })),
        _ => None,
    })
}

fn runtime_messages_require_vision(messages: &[Value]) -> bool {
    messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("image"))
            })
    })
}

fn openai_user_content(message: &Value) -> Result<Value> {
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        return Ok(Value::String(message_text(message)));
    };
    let mut content = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => content.push(json!({
                "type": "text",
                "text": block.get("text").and_then(Value::as_str).unwrap_or_default(),
            })),
            Some("json") => content.push(json!({
                "type": "text",
                "text": serde_json::to_string(block.get("value").unwrap_or(&Value::Null))?,
            })),
            Some("image") => {
                let media_type = block
                    .get("mediaType")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("image content block requires mediaType"))?;
                let data = block
                    .get("dataBase64")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        anyhow!(
                            "direct provider image content requires Runtime-resolved dataBase64"
                        )
                    })?;
                content.push(json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{media_type};base64,{data}"),
                        "detail": "high",
                    },
                }));
            }
            Some(other) => return Err(anyhow!("unsupported content block type: {other}")),
            None => return Err(anyhow!("content block requires type")),
        }
    }
    Ok(Value::Array(content))
}

fn openai_tool_call_from_runtime(call: &Value) -> Option<Value> {
    let id = call.get("id").and_then(Value::as_str)?;
    let name = call.get("name").and_then(Value::as_str)?;
    let api_name = openai_tool_name(name);
    let arguments = call.get("input").cloned().unwrap_or_else(|| json!({}));
    Some(json!({
        "id": id,
        "type": "function",
        "function": {
            "name": api_name,
            "arguments": arguments.to_string(),
        }
    }))
}

fn openai_tool_definition(tool: &ModelToolDefinition, strict_tools: bool) -> Value {
    let name = openai_tool_name(&tool.name);
    let mut definition = json!({
        "type": "function",
        "function": {
            "name": name,
            "parameters": tool
                .input_json_schema
                .as_ref()
                .unwrap_or(&tool.input_schema),
        }
    });
    if strict_tools {
        definition["function"]["strict"] = Value::Bool(true);
    }
    definition
}

fn openai_tool_name_map(request: &ModelRequest) -> HashMap<String, String> {
    request
        .tools
        .iter()
        .chain(request.deferred_tools.iter())
        .map(|tool| (openai_api_tool_name(&tool.name), tool.name.clone()))
        .collect()
}

fn openai_tool_name(runtime_name: &str) -> String {
    openai_api_tool_name(runtime_name)
}

fn openai_api_tool_name(runtime_name: &str) -> String {
    runtime_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn message_text(message: &Value) -> String {
    message
        .get("text")
        .or_else(|| message.get("content"))
        .or_else(|| message.get("error"))
        .map(value_to_content_string)
        .unwrap_or_default()
}

fn message_content_string(message: &Value) -> String {
    message
        .get("content")
        .map(value_to_content_string)
        .unwrap_or_default()
}

fn value_to_content_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

impl From<ModelGatewayToolCall> for ToolCall {
    fn from(call: ModelGatewayToolCall) -> Self {
        Self::new(call.id, call.name, call.input)
    }
}

fn normalize_tool_input(mut input: Value) -> Value {
    for _ in 0..3 {
        if input.as_object().is_none_or(|object| object.len() != 1) {
            return input;
        };
        let Some(arguments) = input.get("arguments") else {
            return input;
        };
        match arguments {
            Value::String(arguments) => match serde_json::from_str::<Value>(arguments) {
                Ok(parsed) => input = parsed,
                Err(_) => return input,
            },
            Value::Object(_) => input = arguments.clone(),
            _ => return input,
        };
    }
    input
}

impl From<ModelGatewayTurnResponse> for ModelResponse {
    fn from(response: ModelGatewayTurnResponse) -> Self {
        match response {
            ModelGatewayTurnResponse::ToolCalls { tool_calls } => {
                ModelResponse::ToolCalls(tool_calls.into_iter().map(Into::into).collect())
            }
            ModelGatewayTurnResponse::ToolCallsThenError { tool_calls, error } => {
                ModelResponse::ToolCallsThenError {
                    calls: tool_calls.into_iter().map(Into::into).collect(),
                    error,
                }
            }
            ModelGatewayTurnResponse::ToolCallsThenFallback { tool_calls, reason } => {
                ModelResponse::ToolCallsThenFallback {
                    calls: tool_calls.into_iter().map(Into::into).collect(),
                    reason,
                }
            }
            ModelGatewayTurnResponse::Text { text } => ModelResponse::TextOnly(text),
            ModelGatewayTurnResponse::Error { error } => ModelResponse::Error(error),
        }
    }
}

impl From<VersionedGatewayTurnResponse> for ModelResponse {
    fn from(response: VersionedGatewayTurnResponse) -> Self {
        match response.response_type.as_str() {
            "tool_calls" => {
                ModelResponse::ToolCalls(response.tool_calls.into_iter().map(Into::into).collect())
            }
            "text" => ModelResponse::TextOnly(response.text.unwrap_or_default()),
            other => ModelResponse::Error(format!(
                "unsupported provider gateway response type: {other}"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MockModelClient {
    responses: Arc<Mutex<VecDeque<ModelResponse>>>,
}

impl MockModelClient {
    pub fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::from(responses))),
        }
    }

    pub async fn assert_all_consumed(&self) {
        assert!(
            self.responses.lock().await.is_empty(),
            "mock model responses should all be consumed"
        );
    }
}

#[async_trait]
impl ModelClient for MockModelClient {
    async fn next_response(&self, _request: ModelRequest) -> Result<ModelResponse> {
        self.responses
            .lock()
            .await
            .pop_front()
            .ok_or_else(|| anyhow!("mock model response queue exhausted"))
    }
}

#[derive(Debug, Clone, Default)]
pub struct EmptyModelClient;

#[async_trait]
impl ModelClient for EmptyModelClient {
    async fn next_response(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse::TextOnly(
            "No model gateway configured for this runtime skeleton.".to_string(),
        ))
    }
}

#[cfg(test)]
mod visual_content_tests {
    use super::*;

    #[test]
    fn direct_openai_provider_receives_resolved_image_bytes() {
        let message = json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "Compare this image" },
                {
                    "type": "image",
                    "artifactId": "visual-1",
                    "mediaType": "image/png",
                    "sha256": "a".repeat(64),
                    "width": 1,
                    "height": 1,
                    "dataBase64": "AQID"
                }
            ]
        });
        assert!(runtime_messages_require_vision(&[message.clone()]));
        let content = openai_user_content(&message).unwrap();
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,AQID");
    }

    #[test]
    fn direct_openai_provider_rejects_unresolved_image_references() {
        let message = json!({
            "role": "user",
            "content": [{
                "type": "image",
                "artifactId": "visual-1",
                "mediaType": "image/png",
                "sha256": "a".repeat(64),
                "width": 1,
                "height": 1
            }]
        });
        assert!(openai_user_content(&message).is_err());
    }
}
