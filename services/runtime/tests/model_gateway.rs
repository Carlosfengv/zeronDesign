use anydesign_runtime::model_gateway::{
    model_client_from_config, HttpModelGatewayClient, ModelClient, ModelRequest, ModelResponse,
    ModelToolDefinition, OpenAiCompatibleModelClient,
};
use anydesign_runtime::{
    config::{ModelProvider, RuntimeConfig},
    tools::registry::ToolLoadingPolicy,
    types::AgentPhase,
};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::{net::TcpListener, sync::Mutex};

#[tokio::test]
async fn http_model_gateway_client_posts_turn_request_and_maps_tool_calls() {
    let captured_request = Arc::new(Mutex::new(None));
    let app = Router::new()
        .route("/v1/agent/turn", post(capture_turn_request))
        .with_state(captured_request.clone());
    let base_url = spawn_gateway(app).await;
    let client = HttpModelGatewayClient::new(base_url);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 2,
            model: "internal-balanced".to_string(),
            phase: AgentPhase::Brief,
            agent_profile: "brief".to_string(),
            system_prompt: "Use the provided tools.".to_string(),
            messages: vec![json!({ "role": "assistant", "text": "hello" })],
            tools: vec![ModelToolDefinition {
                name: "brief.write_draft".to_string(),
                input_schema: json!({ "type": "object" }),
                input_json_schema: None,
                output_schema: None,
                loading_policy: ToolLoadingPolicy::Eager,
                mcp_info: None,
            }],
            deferred_tools: vec![],
        })
        .await
        .unwrap();

    let request = captured_request.lock().await.clone().unwrap();
    assert_eq!(request["runId"], "run-1");
    assert_eq!(request["turn"], 2);
    assert_eq!(request["model"], "internal-balanced");
    assert_eq!(request["phase"], "brief");
    assert_eq!(request["agentProfile"], "brief");
    assert_eq!(request["systemPrompt"], "Use the provided tools.");
    assert_eq!(request["messages"][0]["text"], "hello");
    assert_eq!(request["tools"][0]["name"], "brief.write_draft");
    assert_eq!(
        response,
        ModelResponse::ToolCalls(vec![anydesign_runtime::model_gateway::ToolCall::new(
            "tool-1",
            "brief.write_draft",
            json!({ "projectType": "website" }),
        )])
    );
}

#[tokio::test]
async fn http_model_gateway_client_reports_non_success_status() {
    let app = Router::new().route("/v1/agent/turn", post(failing_turn_request));
    let base_url = spawn_gateway(app).await;
    let client = HttpModelGatewayClient::new(base_url);

    let error = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 1,
            model: "internal-balanced".to_string(),
            phase: AgentPhase::Brief,
            agent_profile: "brief".to_string(),
            system_prompt: "Use the provided tools.".to_string(),
            messages: vec![],
            tools: vec![],
            deferred_tools: vec![],
        })
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("model gateway request failed"));
    assert!(error.contains("503 Service Unavailable"));
    assert!(error.contains("gateway unavailable"));
}

#[tokio::test]
async fn openai_compatible_client_posts_chat_completion_and_maps_tool_calls() {
    let captured_request = Arc::new(Mutex::new(None));
    let app = Router::new()
        .route("/v1/chat/completions", post(capture_chat_completion))
        .with_state(captured_request.clone());
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 2,
            model: "deepseek-v4-pro".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "You are a runtime agent.".to_string(),
            messages: vec![
                json!({
                    "role": "assistant",
                    "turn": 1,
                    "toolCalls": [
                        {
                            "id": "call-prev",
                            "name": "content.list_sources",
                            "input": {}
                        }
                    ]
                }),
                json!({
                    "role": "tool",
                    "turn": 1,
                    "toolUseId": "call-prev",
                    "toolName": "content.list_sources",
                    "isError": false,
                    "content": { "sources": [] }
                }),
            ],
            tools: vec![ModelToolDefinition {
                name: "run.complete".to_string(),
                input_schema: json!({ "type": "object", "properties": { "summary": { "type": "string" } } }),
                input_json_schema: None,
                output_schema: None,
                loading_policy: ToolLoadingPolicy::Eager,
                mcp_info: None,
            }],
            deferred_tools: vec![],
        })
        .await
        .unwrap();

    let request = captured_request.lock().await.clone().unwrap();
    assert_eq!(request["model"], "deepseek-v4-pro");
    assert_eq!(request["stream"], false);
    assert_eq!(request["messages"][0]["role"], "system");
    assert_eq!(
        request["messages"][0]["content"],
        "You are a runtime agent."
    );
    assert_eq!(request["messages"][1]["role"], "assistant");
    assert_eq!(
        request["messages"][1]["tool_calls"][0]["function"]["name"],
        "content.list_sources"
    );
    assert_eq!(request["messages"][2]["role"], "tool");
    assert_eq!(request["messages"][2]["tool_call_id"], "call-prev");
    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["function"]["name"], "run.complete");
    assert_eq!(
        response,
        ModelResponse::ToolCalls(vec![anydesign_runtime::model_gateway::ToolCall::new(
            "call-1",
            "run.complete",
            json!({ "status": "completed", "summary": "done" }),
        )])
    );
}

#[test]
fn model_client_from_config_selects_deepseek_and_kimi_regions() {
    let mut config = RuntimeConfig::from_env();

    config.model_provider = ModelProvider::DeepSeek;
    config.deepseek_api_key = Some("deepseek-key".to_string());
    let deepseek = model_client_from_config(&config).unwrap();
    assert_eq!(deepseek.provider_id(), "deepseek");
    assert_eq!(
        deepseek.endpoint(),
        "https://api.deepseek.com/chat/completions"
    );

    config.model_provider = ModelProvider::KimiGlobal;
    config.kimi_api_key = Some("kimi-key".to_string());
    let kimi_global = model_client_from_config(&config).unwrap();
    assert_eq!(kimi_global.provider_id(), "kimi_global");
    assert_eq!(
        kimi_global.endpoint(),
        "https://api.moonshot.ai/v1/chat/completions"
    );

    config.model_provider = ModelProvider::KimiChina;
    let kimi_cn = model_client_from_config(&config).unwrap();
    assert_eq!(kimi_cn.provider_id(), "kimi_cn");
    assert_eq!(
        kimi_cn.endpoint(),
        "https://api.moonshot.cn/v1/chat/completions"
    );
}

#[test]
fn model_client_from_config_requires_provider_api_key() {
    let mut config = RuntimeConfig::from_env();
    config.model_provider = ModelProvider::DeepSeek;
    config.deepseek_api_key = None;

    let error = model_client_from_config(&config).unwrap_err().to_string();
    assert!(error.contains("DEEPSEEK_API_KEY"));
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY and network access"]
async fn real_deepseek_chat_completion_returns_text() {
    let api_key =
        std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY must be set for this test");
    let base_url =
        std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".into());
    let model = std::env::var("DEEPSEEK_TEST_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into());
    let client = OpenAiCompatibleModelClient::new(base_url, api_key, Some("deepseek"));

    let response = client
        .next_response(ModelRequest {
            run_id: "real-deepseek-smoke".to_string(),
            turn: 1,
            model,
            phase: AgentPhase::Brief,
            agent_profile: "brief".to_string(),
            system_prompt: "Reply with a short plain text answer.".to_string(),
            messages: vec![json!({
                "role": "user",
                "text": "Say OK in English."
            })],
            tools: vec![],
            deferred_tools: vec![],
        })
        .await
        .unwrap();

    match response {
        ModelResponse::TextOnly(text) => assert!(!text.trim().is_empty()),
        other => panic!("expected text response from DeepSeek, got {other:?}"),
    }
}

async fn capture_turn_request(
    State(captured_request): State<Arc<Mutex<Option<Value>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    *captured_request.lock().await = Some(body);
    Json(json!({
        "type": "tool_calls",
        "toolCalls": [
            {
                "id": "tool-1",
                "name": "brief.write_draft",
                "input": { "projectType": "website" }
            }
        ]
    }))
}

async fn capture_chat_completion(
    State(captured_request): State<Arc<Mutex<Option<Value>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    *captured_request.lock().await = Some(body);
    Json(json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "run.complete",
                                "arguments": "{\"status\":\"completed\",\"summary\":\"done\"}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }
        ]
    }))
}

async fn failing_turn_request() -> (StatusCode, &'static str) {
    (StatusCode::SERVICE_UNAVAILABLE, "gateway unavailable")
}

async fn spawn_gateway(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}
