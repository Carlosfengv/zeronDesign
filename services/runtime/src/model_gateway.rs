use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;

use crate::{
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
struct ModelGatewayToolCall {
    id: String,
    name: String,
    #[serde(default)]
    input: Value,
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
