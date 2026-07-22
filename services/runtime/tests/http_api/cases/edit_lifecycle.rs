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
            "anydesign-next-app-pool".to_string(),
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
        config: public_auth_disabled_config(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
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
async fn start_edit_rejects_cross_project_base_version_before_creating_a_run() {
    let store = RuntimeStore::new();
    let source_run = store
        .create_run(
            "project-2".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let foreign_version = store
        .create_project_version_candidate(
            "project-2",
            &source_run.id,
            "http://preview.local/project-2/current".to_string(),
            Some("project-2-shot".to_string()),
            Some("runtime://snapshots/project-2/current".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "project-2",
        &source_run.id,
        &foreign_version.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    store
        .update_run_status(&source_run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-1".to_string(),
            "sandbox-claim-1".to_string(),
            "workspace-sandbox-claim-1".to_string(),
            "anydesign-next-app-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
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
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "baseVersionId": foreign_version.id
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
        "Edit run baseVersionId belongs to a different project"
    );
    assert!(
        store
            .active_mutable_run_for_project("project-1")
            .await
            .is_none(),
        "the invalid cross-project request must not create a mutable Run"
    );
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
    store
        .upsert_project_runtime_state_with_template_identity(
            "project-1",
            "project".to_string(),
            "next-app".to_string(),
            "next-app@1".to_string(),
            Some("919771231a9745aee050a3280518189d4b8d9f106d6ba334a896f41eac253067".to_string()),
            "next".to_string(),
            Some("next-app".to_string()),
            Some("0.1.0".to_string()),
            "npm".to_string(),
            "package-lock.json".to_string(),
            "https://registry.npmjs.org/".to_string(),
        )
        .await
        .unwrap();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store.write_brief(&run.id, website_brief()).await.unwrap();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-1".to_string(),
            "sandbox-claim-1".to_string(),
            "workspace-sandbox-claim-1".to_string(),
            "anydesign-next-app-pool".to_string(),
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
    config.enable_design_context_package = true;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model.clone()),
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
    let edit = store.get_run(edit_run_id).await.unwrap();
    assert_eq!(edit.status, AgentRunStatus::Queued);
    assert!(
        edit.design_context_manifest.is_none(),
        "a promoted legacy version must retain Edit behavior when DCP master is enabled"
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
        config: public_auth_disabled_config(),
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
