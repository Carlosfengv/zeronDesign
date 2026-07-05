use anydesign_runtime::model_gateway::{
    HttpModelGatewayClient, ModelClient, ModelRequest, ModelResponse, ModelToolDefinition,
};
use anydesign_runtime::{tools::registry::ToolLoadingPolicy, types::AgentPhase};
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
