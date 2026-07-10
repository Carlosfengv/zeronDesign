use anydesign_runtime::model_gateway::{
    model_client_from_config, HttpModelGatewayClient, ModelClient, ModelRequest, ModelResponse,
    ModelToolDefinition, OpenAiCompatibleModelClient,
};
use anydesign_runtime::{
    config::{ModelProvider, RuntimeConfig},
    tools::registry::ToolLoadingPolicy,
    types::AgentPhase,
};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
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
                    "role": "tool",
                    "turn": 1,
                    "toolUseId": "orphan-call",
                    "toolName": "fs.list",
                    "isError": false,
                    "content": { "entries": [] }
                }),
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
        "content_list_sources"
    );
    assert_eq!(request["messages"][2]["role"], "tool");
    assert_eq!(request["messages"][2]["tool_call_id"], "call-prev");
    assert_eq!(request["messages"].as_array().unwrap().len(), 3);
    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["function"]["name"], "run_complete");
    assert!(request["tools"][0]["function"].get("strict").is_none());
    assert_eq!(
        response,
        ModelResponse::ToolCalls(vec![anydesign_runtime::model_gateway::ToolCall::new(
            "call-1",
            "run.complete",
            json!({ "status": "completed", "summary": "done" }),
        )])
    );
}

#[tokio::test]
async fn openai_compatible_client_can_enable_strict_tool_definitions() {
    let captured_request = Arc::new(Mutex::new(None));
    let app = Router::new()
        .route("/v1/chat/completions", post(capture_chat_completion))
        .with_state(captured_request.clone());
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None)
        .with_strict_tools(true);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 1,
            model: "deepseek-v4-pro".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "Use strict tool calling.".to_string(),
            messages: vec![],
            tools: vec![ModelToolDefinition {
                name: "run.complete".to_string(),
                input_schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "status": { "type": "string" },
                        "summary": { "type": "string" }
                    },
                    "required": ["status", "summary"]
                }),
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
    assert_eq!(request["tools"][0]["function"]["name"], "run_complete");
    assert_eq!(request["tools"][0]["function"]["strict"], true);
    assert_eq!(
        request["tools"][0]["function"]["parameters"]["additionalProperties"],
        false
    );
    assert_eq!(
        response,
        ModelResponse::ToolCalls(vec![anydesign_runtime::model_gateway::ToolCall::new(
            "call-1",
            "run.complete",
            json!({ "status": "completed", "summary": "done" }),
        )])
    );
}

#[tokio::test]
async fn openai_compatible_client_streams_tool_call_argument_deltas() {
    let captured_request = Arc::new(Mutex::new(None));
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(capture_streaming_chat_completion),
        )
        .with_state(captured_request.clone());
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None)
        .with_streaming(true);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 2,
            model: "deepseek-chat".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "You are a runtime agent.".to_string(),
            messages: vec![],
            tools: vec![ModelToolDefinition {
                name: "fs.write".to_string(),
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
    assert_eq!(request["stream"], true);
    assert_eq!(
        response,
        ModelResponse::ToolCalls(vec![anydesign_runtime::model_gateway::ToolCall::new(
            "call-stream",
            "fs.write",
            json!({ "path": "project/src/pages/index.astro", "text": "ok" }),
        )])
    );
}

#[tokio::test]
async fn openai_compatible_streaming_reports_tool_arguments_over_budget() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(capture_streaming_oversized_tool_arguments),
    );
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None)
        .with_streaming(true);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 2,
            model: "deepseek-chat".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "You are a runtime agent.".to_string(),
            messages: vec![],
            tools: vec![ModelToolDefinition {
                name: "fs.write".to_string(),
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

    match response {
        ModelResponse::ToolInputTooLarge {
            parsed_calls,
            failures,
        } => {
            assert!(parsed_calls.is_empty());
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].tool_call_id, "call-large");
            assert_eq!(failures[0].tool_name, "fs.write");
            assert!(failures[0].input_chars > 96_000);
            assert_eq!(failures[0].max_input_chars, 96_000);
            assert_eq!(failures[0].raw_sha256.len(), 64);
        }
        other => panic!("expected ToolInputTooLarge, got {other:?}"),
    }
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
                                "name": "run_complete",
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

async fn capture_streaming_chat_completion(
    State(captured_request): State<Arc<Mutex<Option<Value>>>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    *captured_request.lock().await = Some(body);
    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-stream\",\"type\":\"function\",\"function\":{\"name\":\"fs_write\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"project/src/pages/index.astro\\\",\\\"text\\\":\\\"ok\\\"}\"}}]}}]}\n\n",
        "data: [DONE]\n\n"
    );
    ([("content-type", "text/event-stream")], stream)
}

async fn capture_streaming_oversized_tool_arguments(Json(_body): Json<Value>) -> impl IntoResponse {
    let oversized = "x".repeat(96_001);
    let event = json!({
        "choices": [
            {
                "delta": {
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call-large",
                            "type": "function",
                            "function": {
                                "name": "fs_write",
                                "arguments": oversized
                            }
                        }
                    ]
                }
            }
        ]
    });
    (
        [("content-type", "text/event-stream")],
        format!("data: {event}\n\ndata: [DONE]\n\n"),
    )
}

#[tokio::test]
async fn openai_compatible_client_unwraps_object_arguments_wrapper() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(capture_wrapped_object_arguments_completion),
    );
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 2,
            model: "deepseek-chat".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "You are a runtime agent.".to_string(),
            messages: vec![],
            tools: vec![ModelToolDefinition {
                name: "fs.write".to_string(),
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

    assert_eq!(
        response,
        ModelResponse::ToolCalls(vec![anydesign_runtime::model_gateway::ToolCall::new(
            "call-1",
            "fs.write",
            json!({ "path": "project/src/pages/index.astro", "text": "ok" }),
        )])
    );
}

#[tokio::test]
async fn openai_compatible_client_accepts_object_arguments_and_missing_call_id() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(capture_object_arguments_without_id_completion),
    );
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 2,
            model: "deepseek-chat".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "You are a runtime agent.".to_string(),
            messages: vec![],
            tools: vec![ModelToolDefinition {
                name: "run.complete".to_string(),
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

    assert_eq!(
        response,
        ModelResponse::ToolCalls(vec![anydesign_runtime::model_gateway::ToolCall::new(
            "tool-call-0",
            "run.complete",
            json!({ "status": "completed", "summary": "done" }),
        )])
    );
}

#[tokio::test]
async fn openai_compatible_client_accepts_null_tool_calls_for_text_response() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(capture_null_tool_calls_completion),
    );
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 1,
            model: "deepseek-chat".to_string(),
            phase: AgentPhase::Brief,
            agent_profile: "brief".to_string(),
            system_prompt: "Reply briefly.".to_string(),
            messages: vec![],
            tools: vec![],
            deferred_tools: vec![],
        })
        .await
        .unwrap();

    assert_eq!(response, ModelResponse::TextOnly("OK".to_string()));
}

#[tokio::test]
async fn openai_compatible_client_reports_invalid_tool_arguments_without_wrapping_raw_text() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(capture_invalid_tool_arguments_completion),
    );
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None);

    let response = client
        .next_response(ModelRequest {
            run_id: "run-1".to_string(),
            turn: 2,
            model: "deepseek-chat".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "You are a runtime agent.".to_string(),
            messages: vec![],
            tools: vec![ModelToolDefinition {
                name: "fs.write".to_string(),
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

    match response {
        ModelResponse::ToolInputParseFailed {
            parsed_calls,
            failures,
        } => {
            assert!(parsed_calls.is_empty());
            assert_eq!(failures.len(), 1);
            let failure = &failures[0];
            assert_eq!(failure.tool_call_id, "call-bad");
            assert_eq!(failure.tool_name, "fs.write");
            assert!(failure.raw_len > 0);
            assert_eq!(failure.raw_sha256.len(), 64);
            assert!(!failure.ends_with_json_close);
            assert!(failure.bracket_balance > 0);
            assert!(!failure.quote_closed);
            assert!(failure.likely_truncated);
        }
        other => panic!("expected ToolInputParseFailed, got {other:?}"),
    }
}

async fn capture_wrapped_object_arguments_completion(Json(_body): Json<Value>) -> Json<Value> {
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
                                "name": "fs_write",
                                "arguments": "{\"arguments\":{\"path\":\"project/src/pages/index.astro\",\"text\":\"ok\"}}"
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }
        ]
    }))
}

async fn capture_object_arguments_without_id_completion(Json(_body): Json<Value>) -> Json<Value> {
    Json(json!({
        "id": "chatcmpl-object-arguments",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "type": "function",
                            "function": {
                                "name": "run_complete",
                                "arguments": {
                                    "status": "completed",
                                    "summary": "done"
                                }
                            }
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }
        ]
    }))
}

async fn capture_null_tool_calls_completion(Json(_body): Json<Value>) -> Json<Value> {
    Json(json!({
        "id": "chatcmpl-null-tool-calls",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "OK",
                    "tool_calls": null
                },
                "finish_reason": "stop"
            }
        ]
    }))
}

async fn capture_invalid_tool_arguments_completion(Json(_body): Json<Value>) -> Json<Value> {
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
                            "id": "call-bad",
                            "type": "function",
                            "function": {
                                "name": "fs_write",
                                "arguments": "{\"path\":\"project/src/pages/index.astro\",\"text\":\"<html>"
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
