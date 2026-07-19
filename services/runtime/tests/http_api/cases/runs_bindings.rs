use super::*;

#[tokio::test]
async fn start_run_input_context_binds_existing_sandbox() {
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
        .update_sandbox_binding_runtime_identity_with_uids(
            &binding.id,
            binding.sandbox_name.clone(),
            Some(binding.sandbox_name.clone()),
            Some("sandbox-uid-1".to_string()),
            Some("pod-uid-1".to_string()),
        )
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
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-1",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
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
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let run_id = payload["runId"].as_str().unwrap();
    assert_eq!(
        store.get_run(run_id).await.unwrap().sandbox_id,
        Some(binding.id.clone())
    );
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );
}

#[tokio::test]
async fn start_build_run_auto_provisions_sandbox_workspace_from_brief() {
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
        config: phase_a_contract_config(),
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

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let run = store
        .get_run(payload["runId"].as_str().unwrap())
        .await
        .unwrap();
    let binding_id = run.sandbox_id.as_deref().unwrap();
    let binding = store.get_sandbox_binding(binding_id).await.unwrap();
    assert_eq!(binding.project_id, "project-1");
    assert_eq!(binding.status, SandboxBindingStatus::Busy);
    assert_eq!(binding.warm_pool_name, "anydesign-astro-website-pool");
    assert_eq!(
        binding.workspace_pvc_name,
        format!("workspace-{}", binding.sandbox_claim_name)
    );
}

#[tokio::test]
async fn build_run_rejects_unconfirmed_brief_until_continue_confirms_it() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-unconfirmed-brief".to_string(),
            "sandbox-claim-unconfirmed-brief".to_string(),
            "workspace-sandbox-claim-unconfirmed-brief".to_string(),
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
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![ContentSource::readable(
                "design-md",
                "design_md",
                "# Visual rules\nUse a polished product website style.",
            )],
        )
        .await;
    let brief_id = store
        .write_brief_draft(&brief_run.id, website_brief())
        .await
        .unwrap();
    assert!(store
        .content_sources_for_brief(&brief_id)
        .await
        .iter()
        .any(|source| source.id == "design-md" && source.kind == "design_md"));
    store
        .update_run_status(&brief_run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let rejected = app
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
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let body = to_bytes(rejected.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("requires a confirmed brief"));

    let confirmed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", brief_run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "确认这个 brief，可以开始生成 website" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirmed.status(), StatusCode::OK);
    let body = to_bytes(confirmed.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["status"], "completed");
    assert_eq!(
        store.brief_status(&brief_id).await,
        Some(BriefStatus::Confirmed)
    );
    let inherited_sources = store.content_sources_for_brief(&brief_id).await;
    assert!(inherited_sources
        .iter()
        .any(|source| source.id == "design-md" && source.kind == "design_md"));
    let checkpoint_id = store
        .get_run(&brief_run.id)
        .await
        .unwrap()
        .checkpoint_id
        .unwrap();
    let checkpoint = store.get_checkpoint(&checkpoint_id).await.unwrap();
    assert_eq!(checkpoint.brief_version.as_deref(), Some(brief_id.as_str()));

    let accepted = app
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
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    let body = to_bytes(accepted.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let build_run_id = payload["runId"].as_str().unwrap();
    let build_sources = store.content_sources(build_run_id).await;
    assert!(build_sources
        .iter()
        .any(|source| source.id == "design-md" && source.kind == "design_md"));
}

#[tokio::test]
async fn sandbox_binding_is_exclusive_until_run_terminal() {
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
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-exclusive".to_string(),
            "sandbox-claim-exclusive".to_string(),
            "workspace-sandbox-claim-exclusive".to_string(),
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
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: phase_a_contract_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let first = app
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
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let body = to_bytes(first.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let first_run_id = payload["runId"].as_str().unwrap().to_string();
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Busy
    );

    let second = app
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
                            "sandboxBindingId": binding.id,
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let body = to_bytes(second.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("already in use by active run"));

    let cancelled = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{first_run_id}/cancel"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Idle
    );
}

#[tokio::test]
async fn start_run_input_context_creates_child_run_with_findings() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-1",
            "sandbox-parent".to_string(),
            "sandbox-claim-parent".to_string(),
            "workspace-sandbox-claim-parent".to_string(),
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
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &parent.id,
            "http://preview.local/preview/project-1/current".to_string(),
            Some("shot-1".to_string()),
            Some("file:///workspace/snapshots/candidate.tar".to_string()),
        )
        .await;
    let first_finding = store
        .record_review_finding(
            "project-1",
            &parent.id,
            &candidate.id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Build,
            "Build fails with TS2304",
            None,
            true,
        )
        .await
        .unwrap();
    let second_finding = store
        .record_review_finding(
            "project-1",
            &parent.id,
            &candidate.id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "Hero section is blank",
            None,
            true,
        )
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
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": parent.id,
                            "findingIds": [first_finding.id.clone(), second_finding.id.clone()]
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
    let run = store
        .get_run(payload["runId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(run.parent_run_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(run.sandbox_id.as_deref(), Some(binding.id.as_str()));
    assert_eq!(
        run.finding_ids,
        Some(vec![first_finding.id.clone(), second_finding.id.clone()])
    );
    assert_eq!(run.project_id, "project-1");
    assert_eq!(
        store
            .get_review_finding(&first_finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Repairing
    );
    assert_eq!(
        store
            .get_review_finding(&second_finding.id)
            .await
            .unwrap()
            .status,
        ReviewFindingStatus::Repairing
    );
}
