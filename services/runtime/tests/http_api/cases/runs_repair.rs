use super::*;

#[tokio::test]
async fn start_repair_run_requires_finding_ids() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-parent-no-finding".to_string(),
            "sandbox-claim-parent-no-finding".to_string(),
            "workspace-sandbox-claim-parent-no-finding".to_string(),
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
    let parent = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .bind_run_to_sandbox(&parent.id, &binding.id)
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
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": parent.id
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
        .contains("repair run requires at least one finding"));
}

#[tokio::test]
async fn start_repair_run_can_target_review_child_finding() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-review-repair".to_string(),
            "sandbox-claim-review-repair".to_string(),
            "workspace-sandbox-claim-review-repair".to_string(),
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
    let build = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .bind_run_to_sandbox(&build.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &build.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-review".to_string()),
            Some("file:///workspace/snapshots/review-candidate.tar".to_string()),
        )
        .await;
    let review = store
        .create_child_run(
            &build.id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{}", candidate.id)),
            vec![],
        )
        .await
        .unwrap();
    let finding = store
        .record_review_finding(
            "project-1",
            &review.id,
            &candidate.id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "First viewport is blank",
            None,
            true,
        )
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
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
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": review.id,
                            "findingIds": [finding.id.clone()]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let repair = store
        .get_run(payload["runId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(repair.parent_run_id.as_deref(), Some(review.id.as_str()));
    assert_eq!(repair.sandbox_id.as_deref(), Some(binding.id.as_str()));
    assert_eq!(repair.finding_ids, Some(vec![finding.id.clone()]));
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );
    assert_eq!(
        store.get_review_finding(&finding.id).await.unwrap().status,
        ReviewFindingStatus::Repairing
    );
}

#[tokio::test]
async fn start_run_rejects_unknown_parent_or_sandbox_binding() {
    let store = RuntimeStore::new();
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
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let missing_parent = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": "run-missing"
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_parent.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(missing_parent.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["error"], "parent run not found: run-missing");

    let missing_sandbox = app
        .clone()
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
                            "sandboxBindingId": "sandbox-binding-missing"
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_sandbox.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(missing_sandbox.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["error"],
        "sandbox binding not found: sandbox-binding-missing"
    );

    let cross_project_binding = store
        .create_sandbox_binding(
            "project-2",
            "sandbox-project-2".to_string(),
            "sandbox-claim-project-2".to_string(),
            "workspace-sandbox-claim-project-2".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&cross_project_binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let cross_project = app
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
                            "sandboxBindingId": cross_project_binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross_project.status(), StatusCode::CONFLICT);
    let body = to_bytes(cross_project.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("sandbox binding project mismatch"));
}

#[tokio::test]
async fn start_run_rejects_sandbox_phase_without_workspace_binding() {
    let store = RuntimeStore::new();
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
                        "agentProfile": "build"
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
        .contains("Build run requires a confirmed briefId"));
}

#[tokio::test]
async fn start_run_rejects_sandbox_phase_before_binding_ready() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-claiming".to_string(),
            "sandbox-claim-claiming".to_string(),
            "workspace-sandbox-claim-claiming".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
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
                            "sandboxBindingId": binding.id
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
    let error = payload["error"].as_str().unwrap();
    assert!(error.contains("is not ready"));
    assert!(error.contains("wait_ready must complete"));
}

#[tokio::test]
async fn start_run_rejects_child_workspace_binding_mismatch() {
    let store = RuntimeStore::new();
    let parent_binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-parent".to_string(),
            "sandbox-claim-parent-mismatch".to_string(),
            "workspace-sandbox-claim-parent-mismatch".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&parent_binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let child_binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-child".to_string(),
            "sandbox-claim-child-mismatch".to_string(),
            "workspace-sandbox-claim-child-mismatch".to_string(),
            "anydesign-astro-website-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&child_binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let parent = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .bind_run_to_sandbox(&parent.id, &parent_binding.id)
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
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": parent.id,
                            "sandboxBindingId": child_binding.id
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
        .contains("child run must use parent sandbox binding"));
}
