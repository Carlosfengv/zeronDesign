use super::*;

#[tokio::test]
async fn cancel_run_marks_terminal_cancelled() {
    let store = RuntimeStore::new();
    let mut run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-cancel".to_string(),
            "claim-cancel".to_string(),
            "workspace-cancel".to_string(),
            "pool-cancel".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    store
        .update_sandbox_binding_runtime_identity_with_uids(
            &binding.id,
            "sandbox-cancel".to_string(),
            Some("sandbox-cancel".to_string()),
            Some("sandbox-uid-cancel".to_string()),
            Some("pod-uid-cancel".to_string()),
        )
        .await
        .unwrap();
    run = store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Busy)
        .await
        .unwrap();
    let lease = store
        .create_preview_lease(&run.id, "build-cancel".to_string(), "a".repeat(64), 900)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/cancel", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::OK, "cancel response: {payload}");
    assert_eq!(payload["status"], "cancelled");
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Cancelled
    );
    assert_eq!(
        store.get_preview_lease(&lease.id).await.unwrap().status,
        PreviewLeaseStatus::Stopped
    );
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Idle
    );
}

#[tokio::test]
async fn cancel_run_cleans_staged_chunk_sessions_for_run() {
    let workspace_root = unique_temp_dir("http-cancel-staged-writes");
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let session_dir = workspace_root
        .join("project-1")
        .join("outputs/staged-writes/session-to-clean");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("manifest.json"),
        json!({
            "runId": run.id.clone(),
            "path": "/workspace/project/large.next",
            "total": 1,
            "chunks": [0]
        })
        .to_string(),
    )
    .unwrap();
    fs::write(session_dir.join("chunk-0.txt"), "large page").unwrap();
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace_root;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/cancel", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(!session_dir.exists());
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Cancelled
    );
}

#[tokio::test]
async fn cancel_run_rejects_terminal_run_without_reopening_it() {
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
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/cancel", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("already terminal"));
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}

#[tokio::test]
async fn continue_run_records_user_message_and_resumes() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Resumed" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", run.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "userMessage": "Continue" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        wait_for_terminal_with_timeout(&store, &run.id, 10).await,
        "run did not become terminal; status={:?}",
        store.get_run(&run.id).await.map(|run| run.status)
    );
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation.iter().any(|item| item.text == "Continue"));
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}

#[tokio::test]
async fn continue_run_rejects_terminal_run_without_recording_message() {
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
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Should not resume" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Reopen it" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(store.conversation_items("project-1").await.is_empty());
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}

#[tokio::test]
async fn continue_run_on_running_run_queues_message_without_reentrant_session() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::Running)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Should not start" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Queue this edit after the current tool" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["runId"], run.id);
    assert_eq!(payload["status"], "running");
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Running
    );
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "user_message"
            && item.text == "Queue this edit after the current tool"));
    assert!(store.events(&run.id).await.iter().any(|event| matches!(
        event,
        AgentEvent::StateChanged { state, .. } if state == "running:continue_queued"
    )));
    assert!(store.continue_interrupt_requested(&run.id).await);
}

#[tokio::test]
async fn resolve_permission_allow_resumes_run() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;
    let supervisor = http_api::RuntimeSupervisor::new();
    let app = http_api::router_with_state(AppState {
        supervisor: supervisor.clone(),
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Permission resolved" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "decision": "allow",
                        "updatedInput": { "command": ["git", "status"] }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        wait_for_terminal_with_timeout(&store, &run.id, 10).await,
        "supervisor readiness: {:?}",
        supervisor.readiness()
    );
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.decision == "allow" && record.tool == "shell.run"));
    assert_eq!(
        store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .resolved_input
            .unwrap()["command"],
        json!(["git", "status"])
    );
}

#[tokio::test]
async fn resolve_permission_after_restart_resumes_same_run() {
    let checkpoint_dir = unique_temp_dir("http-permission-restart");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;

    let reloaded_store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let supervisor = http_api::RuntimeSupervisor::new();
    let app = http_api::router_with_state(AppState {
        supervisor: supervisor.clone(),
        config: phase_a_contract_config(),
        store: reloaded_store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Permission resolved after restart" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "allow" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        wait_for_terminal_with_timeout(&reloaded_store, &run.id, 10).await,
        "supervisor readiness: {:?}",
        supervisor.readiness()
    );
    assert_eq!(
        reloaded_store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert_eq!(
        reloaded_store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .status,
        "allow"
    );
}

#[tokio::test]
async fn resolve_permission_ask_keeps_run_waiting_for_user_input() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "package.install")
        .await;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Should not resume" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "decision": "ask", "updatedInput": { "question": "Which registry?" } })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["runId"], run.id);
    assert_eq!(payload["status"], "needs_user_input");
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::NeedsUserInput
    );
    assert_eq!(
        store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .status,
        "ask"
    );
    assert!(store.events(&run.id).await.iter().any(|event| matches!(
        event,
        AgentEvent::StateChanged { state, .. }
            if state == "needs_user_input:permission_ask"
    )));
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.decision == "ask" && record.tool == "package.install"));
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| {
            item.kind == "permission_resolved"
                && item.metadata.as_ref().is_some_and(|metadata| {
                    metadata["permissionId"] == permission.id && metadata["decision"] == "ask"
                })
        }));
}

#[tokio::test]
async fn resolve_permission_deny_blocks_run_and_writes_conversation_item() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "deny" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
    assert!(store
        .audit_records()
        .await
        .iter()
        .any(|record| record.decision == "deny" && record.tool == "shell.run"));
    let conversation = store.conversation_items("project-1").await;
    assert!(conversation.iter().any(|item| {
        item.kind == "permission_denied"
            && item.text.contains("shell.run")
            && item.metadata.as_ref().is_some_and(|metadata| {
                metadata["tool"] == "shell.run" && metadata["permissionId"] == permission.id
            })
    }));
}

#[tokio::test]
async fn resolve_permission_rejects_expired_terminal_run_permission() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let permission = store
        .create_permission_request("project-1", &run.id, "shell.run")
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "tool-1",
                "run.complete",
                json!({ "status": "completed", "summary": "Should not resume" }),
            ),
        ])])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/permissions/{}/decision", permission.id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "decision": "allow" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        store
            .pending_permission(&permission.id)
            .await
            .unwrap()
            .status,
        "expired"
    );
    assert_eq!(
        store.get_run(&run.id).await.unwrap().status,
        AgentRunStatus::Completed
    );
}
