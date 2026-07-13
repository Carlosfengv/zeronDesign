use super::*;

#[tokio::test]
async fn start_run_and_stream_events() {
    let store = RuntimeStore::new();
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![ToolCall::new(
        "tool-1",
        "run.complete",
        json!({ "status": "completed", "summary": "Brief ready" }),
    )])]);
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(model),
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "brief",
                        "agentProfile": "brief",
                        "inputContext": {
                            "contentSources": [
                                ContentSource::readable("source-1", "prompt", "Make a website")
                            ]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected start run response: {}",
        String::from_utf8_lossy(&body)
    );
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let run_id = payload["runId"].as_str().unwrap().to_string();

    for _ in 0..200 {
        if store
            .get_run(&run_id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{run_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("run.started"));
    assert!(body.contains("agent.message"));
    assert!(body.contains("run.completed"));
    assert!(body.contains("id:"));
    assert!(body.contains(&format!("id: {run_id}/1")));
}

#[tokio::test]
async fn start_run_rejects_empty_contract_identifiers() {
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": " ",
                        "phase": "brief",
                        "agentProfile": "brief"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["error"], "projectId must not be empty");
}

#[tokio::test]
async fn continue_run_rejects_empty_user_message() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", run.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "userMessage": " " }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["error"], "userMessage must not be empty");
}

#[tokio::test]
async fn stream_events_exposes_tool_input_parse_failure_error_kind_without_raw_arguments() {
    let store = RuntimeStore::new();
    let model = MockModelClient::new(vec![
        ModelResponse::ToolInputParseFailed {
            parsed_calls: vec![],
            failures: vec![ToolInputParseFailure {
                tool_call_id: "tool-bad-json".to_string(),
                tool_name: "fs.write".to_string(),
                raw_len: 54,
                raw_sha256: "abc123".to_string(),
                ends_with_json_close: false,
                bracket_balance: 1,
                quote_closed: false,
                likely_truncated: true,
            }],
        },
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "tool-1",
            "run.complete",
            json!({ "status": "completed", "summary": "Recovered after parse failure" }),
        )]),
    ]);
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(model),
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "brief",
                        "agentProfile": "brief",
                        "inputContext": {
                            "contentSources": [
                                ContentSource::readable("source-1", "prompt", "Make a website")
                            ]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected start run response: {}",
        String::from_utf8_lossy(&body)
    );
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let run_id = payload["runId"].as_str().unwrap().to_string();

    for _ in 0..20 {
        if store
            .get_run(&run_id)
            .await
            .is_some_and(|run| run.status.is_terminal())
        {
            break;
        }
        tokio::task::yield_now().await;
    }

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{run_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 16384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("tool.failed"));
    assert!(body.contains("tool.input_json_parse_failed"));
    assert!(body.contains("tool_input_json_parse_failed"));
    assert!(body.contains("rawSha256"));
    assert!(!body.contains("rawArguments"));
    assert!(!body.contains("<html"));
    assert!(!body.contains("fs.write requires path"));
}

#[tokio::test]
async fn start_run_uses_configured_agent_model_for_real_provider_runs() {
    let store = RuntimeStore::new();
    let mut config = public_auth_disabled_config();
    config.agent_model = "deepseek-chat".to_string();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Brief ready" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "brief",
                        "agentProfile": "brief",
                        "inputContext": {
                            "contentSources": [
                                ContentSource::readable("source-1", "prompt", "Make a website")
                            ]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    if status != StatusCode::OK {
        panic!(
            "unexpected start run response {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let run = store
        .get_run(payload["runId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(run.model, "deepseek-chat");
}
