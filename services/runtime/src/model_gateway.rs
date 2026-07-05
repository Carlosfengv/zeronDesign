use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;

use crate::{
    config::{ModelProvider, RuntimeConfig},
    tools::registry::{McpToolInfo, ToolLoadingPolicy},
    types::AgentPhase,
};

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
            input,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelResponse {
    ToolCalls(Vec<ToolCall>),
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
            HttpModelGatewayClient::new(config.model_gateway_url.clone()),
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
                ),
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
                ),
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
                ),
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
}

impl HttpModelGatewayClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let endpoint = format!("{}/v1/agent/turn", base_url.trim_end_matches('/'));
        Self {
            endpoint,
            client: reqwest::Client::new(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[async_trait]
impl ModelClient for HttpModelGatewayClient {
    async fn next_response(&self, request: ModelRequest) -> Result<ModelResponse> {
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

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleModelClient {
    provider_id: String,
    endpoint: String,
    api_key: String,
    client: reqwest::Client,
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
            client: reqwest::Client::new(),
        }
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
        let body = openai_chat_request(&request);
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
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
        let response = response.json::<OpenAiChatCompletionResponse>().await?;
        Ok(response.into_model_response())
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
    tool_calls: Vec<OpenAiToolCall>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiToolCallFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallFunction {
    name: String,
    arguments: String,
}

impl OpenAiChatCompletionResponse {
    fn into_model_response(self) -> ModelResponse {
        let Some(choice) = self.choices.into_iter().next() else {
            return ModelResponse::Error("model returned no choices".to_string());
        };
        if !choice.message.tool_calls.is_empty() {
            let calls = choice
                .message
                .tool_calls
                .into_iter()
                .map(|call| {
                    let input = serde_json::from_str(&call.function.arguments)
                        .unwrap_or_else(|_| json!({ "arguments": call.function.arguments }));
                    ToolCall::new(call.id, call.function.name, input)
                })
                .collect();
            return ModelResponse::ToolCalls(calls);
        }
        let text = match choice.message.content {
            Some(Value::String(text)) => text,
            Some(value) => value.to_string(),
            None => String::new(),
        };
        ModelResponse::TextOnly(text)
    }
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

fn openai_chat_request(request: &ModelRequest) -> Value {
    let mut messages = vec![json!({
        "role": "system",
        "content": request.system_prompt,
    })];
    messages.extend(
        request
            .messages
            .iter()
            .filter_map(openai_message_from_runtime),
    );
    let tools = request
        .tools
        .iter()
        .map(openai_tool_definition)
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": false,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    body
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
    let arguments = call.get("input").cloned().unwrap_or_else(|| json!({}));
    Some(json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments.to_string(),
        }
    }))
}

fn openai_tool_definition(tool: &ModelToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "parameters": tool
                .input_json_schema
                .as_ref()
                .unwrap_or(&tool.input_schema),
        }
    })
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
        Self {
            id: call.id,
            name: call.name,
            input: call.input,
        }
    }
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
