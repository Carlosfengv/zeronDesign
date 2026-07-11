use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
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
}

pub fn model_client_from_config(config: &RuntimeConfig) -> Result<ConfiguredModelClient> {
    match config.model_provider {
        ModelProvider::InternalGateway => Ok(ConfiguredModelClient::InternalGateway(
            HttpModelGatewayClient::new(config.model_gateway_url.clone())
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
}

#[derive(Debug, Clone)]
pub struct HttpModelGatewayClient {
    endpoint: String,
    client: reqwest::Client,
    request_timeout: Duration,
}

impl HttpModelGatewayClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let endpoint = format!("{}/v1/agent/turn", base_url.trim_end_matches('/'));
        Self {
            endpoint,
            client: reqwest::Client::new(),
            request_timeout: Duration::from_secs(180),
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

    async fn execute(&self, request: ModelRequest) -> Result<ModelResponse> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "model gateway request failed: status={} body={}",
                status,
                body
            ));
        }
        let response = response.json::<ModelGatewayTurnResponse>().await?;
        Ok(response.into())
    }
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
        let (mut body, tool_name_map) = openai_chat_request(&request, self.strict_tools);
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
) -> (Value, HashMap<String, String>) {
    let tool_name_map = openai_tool_name_map(request);
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system_prompt,
    })];
    messages.extend(openai_messages_from_runtime(&request.messages));
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
    (body, tool_name_map)
}

fn openai_messages_from_runtime(messages: &[Value]) -> Vec<Value> {
    let mut output = Vec::new();
    let mut pending_tool_call_ids = HashSet::new();
    for message in messages {
        let Some(openai_message) = openai_message_from_runtime(message) else {
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
    output
}

fn openai_message_from_runtime(message: &Value) -> Option<Value> {
    let role = message.get("role").and_then(Value::as_str)?;
    match role {
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
            "content": message_text(message),
        })),
        "system" | "model" | "runtime" => Some(json!({
            "role": "system",
            "content": message_text(message),
        })),
        _ => None,
    }
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
        if !input.as_object().is_some_and(|object| object.len() == 1) {
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
