use anydesign_runtime::model_gateway::{
    model_client_from_config, HttpModelGatewayClient, ModelClient, ModelGatewayScope, ModelRequest,
    ModelResponse, ModelToolDefinition, OpenAiCompatibleModelClient,
};
use anydesign_runtime::{
    config::{ModelProvider, RuntimeConfig},
    tools::registry::ToolLoadingPolicy,
    types::AgentPhase,
};
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use futures::stream;
use provider_gateway::{
    router as provider_gateway_router, AutomaticSwitchPolicy, DirectSelectionPolicy, GatewayConfig,
    GatewayService, ModelCandidate, ModelDefaults, ModelResource, ModelResourceKind,
    ModelSelectionLimits, ModelSelectionPolicy, PolicyApplicability, PolicyScope, ProviderAuth,
    ProviderCapabilities, ProviderEndpoint, MODEL_RESOURCE_SCHEMA, MODEL_SELECTION_POLICY_SCHEMA,
};
use serde_json::{json, Value};
use std::{
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
    time::{sleep, Duration},
};

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
async fn http_model_gateway_client_posts_versioned_scoped_turn_request() {
    let captured_request = Arc::new(Mutex::new(None));
    let app = Router::new()
        .route("/v1/agent/turn", post(capture_versioned_turn_request))
        .with_state(captured_request.clone());
    let base_url = spawn_gateway(app).await;
    let client = HttpModelGatewayClient::new(base_url)
        .with_runtime_bearer_token(Some("runtime-token".to_string()));

    let response = client
        .next_response_scoped(
            ModelRequest {
                run_id: "run-1".to_string(),
                turn: 2,
                model: "internal-balanced".to_string(),
                phase: AgentPhase::Build,
                agent_profile: "website-builder".to_string(),
                system_prompt: "Use the provided tools.".to_string(),
                messages: vec![],
                tools: vec![],
                deferred_tools: vec![],
            },
            ModelGatewayScope {
                organization_id: "org-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                project_id: "project-1".to_string(),
            },
        )
        .await
        .unwrap();

    let request = captured_request.lock().await.clone().unwrap();
    assert_eq!(request["schemaVersion"], "provider-gateway-turn-request@1");
    assert_eq!(request["scope"]["organizationId"], "org-1");
    assert_eq!(request["scope"]["workspaceId"], "workspace-1");
    assert_eq!(request["scope"]["projectId"], "project-1");
    assert_eq!(request["scope"]["runId"], "run-1");
    assert_eq!(request["routing"]["modelResourceId"], Value::Null);
    assert_eq!(
        response,
        ModelResponse::TextOnly("gateway text".to_string())
    );
}

#[tokio::test]
async fn http_model_gateway_client_passes_explicit_model_resource_to_gateway_policy() {
    let captured_request = Arc::new(Mutex::new(None));
    let app = Router::new()
        .route("/v1/agent/turn", post(capture_versioned_turn_request))
        .with_state(captured_request.clone());
    let base_url = spawn_gateway(app).await;
    let client = HttpModelGatewayClient::new(base_url);

    client
        .next_response_scoped(
            ModelRequest {
                run_id: "run-1".to_string(),
                turn: 3,
                model: "resource:quality-edit-model".to_string(),
                phase: AgentPhase::Edit,
                agent_profile: "website-editor".to_string(),
                system_prompt: "Use the provided tools.".to_string(),
                messages: vec![],
                tools: vec![],
                deferred_tools: vec![],
            },
            ModelGatewayScope {
                organization_id: "org-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                project_id: "project-1".to_string(),
            },
        )
        .await
        .unwrap();

    let request = captured_request.lock().await.clone().unwrap();
    assert_eq!(request["routing"]["modelResourceId"], "quality-edit-model");
    assert!(request["routing"].get("endpoint").is_none());
    assert!(request["routing"].get("apiKey").is_none());
}

#[tokio::test]
async fn runtime_gateway_provider_round_trip_preserves_governed_selection_boundary() {
    let captured_provider_request = Arc::new(Mutex::new(None));
    let provider = Router::new()
        .route(
            "/v1/chat/completions",
            post(capture_cross_service_provider_request),
        )
        .with_state(captured_provider_request.clone());
    let provider_url = spawn_gateway(provider).await;
    let secret_path = std::env::temp_dir().join(format!(
        "provider-gateway-runtime-contract-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&secret_path, "provider-test-key").unwrap();
    unsafe { std::env::set_var("PROVIDER_GATEWAY_ALLOW_LOOPBACK", "true") };

    let resource = ModelResource {
        schema_version: MODEL_RESOURCE_SCHEMA.to_string(),
        id: "edit-model".to_string(),
        display_name: "Governed edit model".to_string(),
        kind: ModelResourceKind::OpenaiCompatible,
        enabled: true,
        revision: 3,
        endpoint: ProviderEndpoint {
            base_url: provider_url,
            chat_completions_path: "/v1/chat/completions".to_string(),
        },
        auth: ProviderAuth {
            auth_type: "bearer".to_string(),
            secret_ref: format!("file:{}", secret_path.display()),
        },
        physical_model: "provider-edit-physical".to_string(),
        capabilities: ProviderCapabilities {
            tool_calls: true,
            strict_tool_schema: false,
            streaming: false,
            vision: false,
        },
        defaults: ModelDefaults::default(),
    };
    let policy = ModelSelectionPolicy {
        schema_version: MODEL_SELECTION_POLICY_SCHEMA.to_string(),
        id: "website-edit-policy".to_string(),
        revision: 2,
        scope: PolicyScope {
            organization_ids: vec!["org-1".to_string()],
            workspace_ids: vec!["workspace-1".to_string()],
            project_ids: vec!["project-1".to_string()],
        },
        applies_to: PolicyApplicability {
            phases: vec!["edit".to_string()],
            agent_profiles: vec!["website-editor".to_string()],
        },
        candidates: vec![ModelCandidate {
            model_resource_id: "edit-model".to_string(),
            priority: 10,
            weight: 100,
        }],
        automatic_switch: AutomaticSwitchPolicy::default(),
        direct_selection: DirectSelectionPolicy {
            allowed_model_resource_ids: vec!["edit-model".to_string()],
        },
        limits: ModelSelectionLimits::default(),
    };
    let gateway = GatewayService::new(GatewayConfig {
        listen: "127.0.0.1:0".to_string(),
        database_url: Some(":memory:".to_string()),
        runtime_bearer_token: Some("runtime-contract-token".to_string()),
        admin_bearer_token: None,
        resources: vec![resource],
        policies: vec![policy],
    })
    .unwrap();
    let gateway_url = spawn_gateway(provider_gateway_router(gateway)).await;
    let client = HttpModelGatewayClient::new(gateway_url)
        .with_runtime_bearer_token(Some("runtime-contract-token".to_string()))
        .with_timeout(Duration::from_secs(5));

    let turn = client
        .next_response_scoped_with_execution(
            ModelRequest {
                run_id: "run-contract-1".to_string(),
                turn: 1,
                model: "resource:edit-model".to_string(),
                phase: AgentPhase::Edit,
                agent_profile: "website-editor".to_string(),
                system_prompt: "Apply a focused visual edit.".to_string(),
                messages: vec![json!({ "role": "user", "content": "adjust spacing" })],
                tools: vec![],
                deferred_tools: vec![],
            },
            ModelGatewayScope {
                organization_id: "org-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                project_id: "project-1".to_string(),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        turn.response,
        ModelResponse::TextOnly("provider ok".to_string())
    );
    let usage = turn.usage.unwrap();
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 4);
    assert_eq!(usage.cached_input_tokens, 0);
    let execution = turn.execution.unwrap();
    assert_eq!(execution.model_resource_id, "edit-model");
    assert_eq!(execution.model_resource_revision, 3);
    assert_eq!(execution.physical_model, "provider-edit-physical");
    assert_eq!(execution.selection_policy_id, "website-edit-policy");
    assert_eq!(execution.selection_policy_revision, 2);
    assert_eq!(execution.selection_reason, "explicit_resource");
    assert_eq!(
        execution.provider_request_id.as_deref(),
        Some("provider-request-contract-1")
    );
    assert_eq!(execution.provider_attempt_count, 1);

    let (headers, provider_body): (HeaderMap, Value) =
        captured_provider_request.lock().await.clone().unwrap();
    assert_eq!(provider_body["model"], "provider-edit-physical");
    assert_eq!(
        headers.get(header::AUTHORIZATION).unwrap(),
        "Bearer provider-test-key"
    );
    assert!(headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("runtime-run-contract-1-1-")));
    assert!(provider_body.get("scope").is_none());
    assert!(provider_body
        .to_string()
        .contains("Apply a focused visual edit."));
    assert!(!provider_body.to_string().contains("runtime-contract-token"));
    let _ = std::fs::remove_file(secret_path);
    unsafe { std::env::remove_var("PROVIDER_GATEWAY_ALLOW_LOOPBACK") };
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
    assert!(error.contains("code=gateway_request_failed"));
    assert!(!error.contains("gateway unavailable"));
}

#[tokio::test]
async fn http_model_gateway_client_retries_retryable_gateway_failures() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/v1/agent/turn", post(retryable_gateway_then_success))
        .with_state(calls.clone());
    let base_url = spawn_gateway(app).await;
    let client = HttpModelGatewayClient::new(base_url).with_timeout(Duration::from_secs(2));
    let response = client
        .next_response_scoped(
            ModelRequest {
                run_id: "run-retryable".to_string(),
                turn: 1,
                model: "internal-balanced".to_string(),
                phase: AgentPhase::Build,
                agent_profile: "website-builder".to_string(),
                system_prompt: "retry safely".to_string(),
                messages: vec![],
                tools: vec![],
                deferred_tools: vec![],
            },
            ModelGatewayScope {
                organization_id: "org-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                project_id: "project-1".to_string(),
            },
        )
        .await
        .unwrap();
    assert_eq!(response, ModelResponse::TextOnly("recovered".to_string()));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn http_model_gateway_client_retries_internal_transport_errors() {
    let (base_url, calls) = spawn_flaky_transport_gateway().await;
    let client = HttpModelGatewayClient::new(base_url).with_timeout(Duration::from_secs(3));
    let response = client
        .next_response_scoped(
            ModelRequest {
                run_id: "run-transport-retry".to_string(),
                turn: 1,
                model: "internal-balanced".to_string(),
                phase: AgentPhase::Build,
                agent_profile: "website-builder".to_string(),
                system_prompt: "retry an internal transport interruption".to_string(),
                messages: vec![],
                tools: vec![],
                deferred_tools: vec![],
            },
            ModelGatewayScope {
                organization_id: "org-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                project_id: "project-1".to_string(),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        response,
        ModelResponse::TextOnly("transport recovered".to_string())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn http_model_gateway_client_bounds_the_complete_provider_turn() {
    let app = Router::new().route(
        "/v1/agent/turn",
        post(|| async {
            sleep(Duration::from_millis(250)).await;
            Json(json!({ "type": "text", "text": "late" }))
        }),
    );
    let base_url = spawn_gateway(app).await;
    let client = HttpModelGatewayClient::new(base_url).with_timeout(Duration::from_millis(50));
    let error = client
        .next_response(ModelRequest {
            run_id: "run-timeout".to_string(),
            turn: 1,
            model: "internal-balanced".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "bounded turn".to_string(),
            messages: vec![],
            tools: vec![],
            deferred_tools: vec![],
        })
        .await
        .unwrap_err();
    let message = error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
        .to_lowercase();
    assert!(
        message.contains("timeout") || message.contains("timed out"),
        "unexpected timeout error: {message}"
    );
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
async fn openai_compatible_client_retries_interrupted_response_streams() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/v1/chat/completions", post(interrupt_twice_then_complete))
        .with_state(attempts.clone());
    let base_url = spawn_gateway(app).await;
    let client = OpenAiCompatibleModelClient::new(format!("{base_url}/v1"), "test-key", None)
        .with_streaming(true)
        .with_transport_retry(3, Duration::from_millis(10));

    let response = client
        .next_response(ModelRequest {
            run_id: "run-retry".to_string(),
            turn: 1,
            model: "deepseek-chat".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            system_prompt: "retry interrupted streams".to_string(),
            messages: vec![],
            tools: vec![],
            deferred_tools: vec![],
        })
        .await
        .unwrap();

    assert_eq!(response, ModelResponse::TextOnly("OK".to_string()));
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

async fn interrupt_twice_then_complete(State(attempts): State<Arc<AtomicUsize>>) -> Response {
    if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
        let body = Body::from_stream(stream::once(async {
            Err::<String, io::Error>(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "simulated response interruption",
            ))
        }));
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(body)
            .unwrap();
    }
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from(
            "data: {\"choices\":[{\"delta\":{\"content\":\"OK\"}}]}\n\ndata: [DONE]\n\n",
        ))
        .unwrap()
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

async fn retryable_gateway_then_success(State(calls): State<Arc<AtomicUsize>>) -> Response {
    if calls.fetch_add(1, Ordering::SeqCst) < 2 {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "schemaVersion": "provider-gateway-error@1",
                "requestId": "gateway-retry",
                "error": {
                    "code": "gateway_overloaded",
                    "message": "retry later",
                    "retryable": true,
                    "retryAfterMs": 1
                }
            })),
        )
            .into_response();
    }
    Json(json!({
        "schemaVersion": "provider-gateway-turn-response@1",
        "requestId": "gateway-retry",
        "type": "text",
        "toolCalls": [],
        "text": "recovered",
        "finishReason": "stop",
        "modelExecution": {
            "id": "model-execution-retry",
            "modelResourceId": "balanced",
            "modelResourceRevision": 1,
            "providerId": "balanced",
            "physicalModel": "physical",
            "selectionPolicyId": "default",
            "selectionPolicyRevision": 1,
            "capabilitySnapshotHash": "hash",
            "selectionReason": "automatic_selection",
            "automaticSwitch": { "used": false }
        },
        "usage": {
            "inputTokens": 1,
            "outputTokens": 1,
            "cachedInputTokens": 0
        },
        "provider": { "requestId": null, "attemptCount": 1 }
    }))
    .into_response()
}

async fn spawn_flaky_transport_gateway() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let server_calls = calls.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let attempt = server_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt < 3 {
                drop(socket);
                continue;
            }

            let mut request = vec![0u8; 64 * 1024];
            let _ = socket.read(&mut request).await.unwrap();
            let body = json!({
                "schemaVersion": "provider-gateway-turn-response@1",
                "requestId": "gateway-transport-retry",
                "type": "text",
                "toolCalls": [],
                "text": "transport recovered",
                "finishReason": "stop",
                "modelExecution": {
                    "id": "model-execution-transport-retry",
                    "modelResourceId": "balanced",
                    "modelResourceRevision": 1,
                    "providerId": "balanced",
                    "physicalModel": "physical",
                    "selectionPolicyId": "default",
                    "selectionPolicyRevision": 1,
                    "capabilitySnapshotHash": "hash",
                    "selectionReason": "automatic_selection",
                    "automaticSwitch": { "used": false }
                },
                "usage": {
                    "inputTokens": 1,
                    "outputTokens": 1,
                    "cachedInputTokens": 0
                },
                "provider": { "requestId": null, "attemptCount": 1 }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
            break;
        }
    });
    (format!("http://{address}"), calls)
}

async fn capture_versioned_turn_request(
    State(captured): State<Arc<Mutex<Option<Value>>>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    *captured.lock().await = Some(request);
    Json(json!({
        "schemaVersion": "provider-gateway-turn-response@1",
        "requestId": "gateway-request-1",
        "type": "text",
        "toolCalls": [],
        "text": "gateway text",
        "finishReason": "stop",
        "modelExecution": {
            "id": "model-execution-1",
            "modelResourceId": "deepseek-design-balanced",
            "modelResourceRevision": 1,
            "providerId": "deepseek-design-balanced",
            "physicalModel": "deepseek-chat",
            "selectionPolicyId": "website-default",
            "selectionPolicyRevision": 1,
            "capabilitySnapshotHash": "hash",
            "selectionReason": "automatic_selection",
            "automaticSwitch": { "used": false }
        },
        "usage": {},
        "provider": { "attemptCount": 1 }
    }))
}

async fn capture_cross_service_provider_request(
    State(captured): State<Arc<Mutex<Option<(HeaderMap, Value)>>>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    *captured.lock().await = Some((headers, request));
    (
        [("x-request-id", "provider-request-contract-1")],
        Json(json!({
            "choices": [{
                "message": { "content": "provider ok" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 4 }
        })),
    )
        .into_response()
}

async fn spawn_gateway(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}
