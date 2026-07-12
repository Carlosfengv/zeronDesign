use super::*;

#[tokio::test]
async fn start_edit_rejects_stale_base_version() {
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
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-1".to_string(),
            "sandbox-claim-1".to_string(),
            "workspace-sandbox-claim-1".to_string(),
            "anydesign-astro-website-pool".to_string(),
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
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("runtime://snapshots/project-1/current".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &run.id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store,
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
                        "projectId": "project-1",
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "baseVersionId": "version-stale"
                        }
                    })
                    .to_string(),
                ))
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
        .contains("baseVersionId version-stale is stale"));
}

#[tokio::test]
async fn start_edit_waits_for_continue_before_spawning_agent() {
    let workspace = unique_temp_dir("http-edit-waits-restore");
    fs::create_dir_all(
        workspace
            .join("project-1")
            .join("outputs/build/source-snapshots/current"),
    )
    .unwrap();
    fs::write(
        workspace
            .join("project-1")
            .join("outputs/build/source-snapshots/current/package.json"),
        "{}",
    )
    .unwrap();
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
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-1".to_string(),
            "sandbox-claim-1".to_string(),
            "workspace-sandbox-claim-1".to_string(),
            "anydesign-astro-website-pool".to_string(),
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
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/outputs/build/source-snapshots/current".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &run.id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let model = MockModelClient::new(vec![ModelResponse::Error(
        "edit agent should wait for continue".to_string(),
    )]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model.clone()),
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
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "baseVersionId": candidate.id
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
            "unexpected start edit response {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let edit_run_id = payload["runId"].as_str().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        store.get_run(edit_run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );
}

#[tokio::test]
async fn start_mutable_run_rejects_existing_project_mutation() {
    let store = RuntimeStore::new();
    let active = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    assert_eq!(active.status, AgentRunStatus::Queued);
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store,
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
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["error"],
        format!(
            "project project-1 already has active mutable run {}",
            active.id
        )
    );
}
