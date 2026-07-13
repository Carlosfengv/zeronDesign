use super::*;

#[tokio::test]
async fn required_unsupported_extended_token_blocks_build_with_capability_state() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });
    let mut create_request = design_profile_request("project-capability", vec!["astro-website"]);
    create_request["profile"]["schemaVersion"] = json!("design-profile@2");
    create_request["profile"]["extendedTokenMapping"] =
        json!({ "imagery.unsupportedShader": "required" });
    create_request["profile"]["signatureRules"] = json!([{
        "id": "unsupported-required-token",
        "category": "imagery",
        "statement": "The unsupported shader token is required.",
        "priority": "required",
        "appliesTo": ["website"],
        "verification": {
            "kind": "token",
            "token": "imagery.unsupportedShader",
            "expected": "required",
            "comparator": { "kind": "exact" }
        }
    }]);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(create_request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = to_bytes(created.into_body(), 32_768).await.unwrap();
    let created_json: Value = serde_json::from_slice(&created_body).unwrap();
    let profile_id = created_json["designProfile"]["id"].as_str().unwrap();

    let brief_run = store
        .create_run(
            "project-capability".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "builders".to_string(),
                content_hierarchy: vec!["Home".to_string()],
                page_structure: json!([]),
                visual_direction: "specific".to_string(),
                recommended_template: "astro-website".to_string(),
                assumptions: vec![],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    store
        .update_run_status(&brief_run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let started = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-capability",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "briefId": brief_id,
                            "designProfileId": profile_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let started_status = started.status();
    let started_body = to_bytes(started.into_body(), 16_384).await.unwrap();
    assert_eq!(
        started_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&started_body)
    );
    let started_json: Value = serde_json::from_slice(&started_body).unwrap();
    assert_eq!(started_json["status"], "needs_user_input");
    let run_id = started_json["runId"].as_str().unwrap();
    let run = store.get_run(run_id).await.unwrap();
    assert_eq!(
        run.design_profile_blocking_capability_rule_ids,
        vec!["unsupported-required-token"]
    );
    assert!(store.events(run_id).await.iter().any(|event| matches!(
        event,
        AgentEvent::StateChanged { state, .. }
            if state == "needs_user_input:design_profile_capability_gap"
    )));
}

#[tokio::test]
async fn design_profile_rejects_multiple_active_profiles_for_same_project() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request("project-unique", vec!["astro-website"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = to_bytes(first.into_body(), 16384).await.unwrap();
    let first_payload: Value = serde_json::from_slice(&first_body).unwrap();
    let first_profile_id = first_payload["designProfile"]["id"].as_str().unwrap();

    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "Second active profile",
                        "profile": design_profile_request("project-unique", vec!["astro-website"])["profile"].clone()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let second_body = to_bytes(second.into_body(), 4096).await.unwrap();
    let second_payload: Value = serde_json::from_slice(&second_body).unwrap();
    assert!(second_payload["error"]
        .as_str()
        .unwrap()
        .contains("already has active design profile"));

    let archived = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/design-profiles/{first_profile_id}/archive"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);

    let second_after_archive = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "Second active profile",
                        "profile": design_profile_request("project-unique", vec!["astro-website"])["profile"].clone()
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_after_archive.status(), StatusCode::OK);
}

#[tokio::test]
async fn start_run_resolves_design_profile_by_workspace_then_project_precedence() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let workspace_created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request_for_scope(
                        None,
                        json!({ "workspaceId": "workspace-1" }),
                        vec!["astro-website"],
                    )
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let workspace_body = to_bytes(workspace_created.into_body(), 16384)
        .await
        .unwrap();
    let workspace_payload: Value = serde_json::from_slice(&workspace_body).unwrap();
    let workspace_profile_id = workspace_payload["designProfile"]["id"].as_str().unwrap();

    let project_created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request("project-1", vec!["astro-website"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let project_body = to_bytes(project_created.into_body(), 16384).await.unwrap();
    let project_payload: Value = serde_json::from_slice(&project_body).unwrap();
    let project_profile_id = project_payload["designProfile"]["id"].as_str().unwrap();

    let workspace_run = app
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
                            "workspaceId": "workspace-1",
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
    assert_eq!(workspace_run.status(), StatusCode::OK);
    let workspace_run_body = to_bytes(workspace_run.into_body(), 4096).await.unwrap();
    let workspace_run_payload: Value = serde_json::from_slice(&workspace_run_body).unwrap();
    let workspace_run_id = workspace_run_payload["runId"].as_str().unwrap();
    assert_eq!(
        store
            .get_run(workspace_run_id)
            .await
            .unwrap()
            .design_profile_id
            .as_deref(),
        Some(workspace_profile_id)
    );
    store
        .update_run_status(workspace_run_id, AgentRunStatus::Completed)
        .await
        .unwrap();

    let bound = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/project-1/design-profile")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "designProfileId": project_profile_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bound.status(), StatusCode::OK);

    let project_run = app
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
                            "workspaceId": "workspace-1",
                            "contentSources": [
                                ContentSource::readable("source-2", "prompt", "Make another website")
                            ]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(project_run.status(), StatusCode::OK);
    let project_run_body = to_bytes(project_run.into_body(), 4096).await.unwrap();
    let project_run_payload: Value = serde_json::from_slice(&project_run_body).unwrap();
    let project_run_id = project_run_payload["runId"].as_str().unwrap();
    assert_eq!(
        store
            .get_run(project_run_id)
            .await
            .unwrap()
            .design_profile_id
            .as_deref(),
        Some(project_profile_id)
    );
}

#[tokio::test]
async fn start_run_resolves_design_profile_by_organization_fallback() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request_for_scope(
                        None,
                        json!({ "organizationId": "org-1" }),
                        vec!["astro-website"],
                    )
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let created_body = to_bytes(created.into_body(), 16384).await.unwrap();
    let created_payload: Value = serde_json::from_slice(&created_body).unwrap();
    let profile_id = created_payload["designProfile"]["id"].as_str().unwrap();

    let started = app
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
                            "organizationId": "org-1",
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
    assert_eq!(started.status(), StatusCode::OK);
    let started_body = to_bytes(started.into_body(), 4096).await.unwrap();
    let started_payload: Value = serde_json::from_slice(&started_body).unwrap();
    let run_id = started_payload["runId"].as_str().unwrap();
    assert_eq!(
        store
            .get_run(run_id)
            .await
            .unwrap()
            .design_profile_id
            .as_deref(),
        Some(profile_id)
    );
}

#[tokio::test]
async fn start_run_with_missing_explicit_design_profile_returns_not_found() {
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: RuntimeStore::new(),
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
                        "phase": "brief",
                        "agentProfile": "brief",
                        "inputContext": {
                            "designProfileId": "design-profile-missing",
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

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn start_run_design_profile_template_conflict_enters_needs_user_input() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    store
        .update_run_status(&brief_run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request("project-1", vec!["fumadocs-docs"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(created.into_body(), 16384).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let profile_id = payload["designProfile"]["id"].as_str().unwrap();

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
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": {
                            "briefId": brief_id,
                            "designProfileId": profile_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let response_body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected start run response: {}",
        String::from_utf8_lossy(&response_body)
    );
    let response_payload: Value = serde_json::from_slice(&response_body).unwrap();
    assert_eq!(response_payload["status"], "needs_user_input");
    let run_id = response_payload["runId"].as_str().unwrap();
    let run = store.get_run(run_id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::NeedsUserInput);
    assert_eq!(run.design_profile_id.as_deref(), Some(profile_id));
    assert!(store.events(run_id).await.iter().any(|event| {
        matches!(
            event,
            AgentEvent::StateChanged { state, .. }
                if state == "needs_user_input:design_profile_conflict"
        )
    }));
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(
            |item| item.kind == "approval_request" && item.text.contains("DesignProfile conflict")
        ));
}

#[tokio::test]
async fn continue_edit_run_design_profile_conflict_enters_needs_user_input() {
    let store = RuntimeStore::new();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(
                    design_profile_request("project-1", vec!["astro-website"]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(created.into_body(), 16384).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let profile_id = payload["designProfile"]["id"].as_str().unwrap();
    let profile = store.get_design_profile(profile_id).await.unwrap();
    let edit_run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
            None,
            Some("version-1".to_string()),
        )
        .await;
    let edit_run = store
        .attach_run_design_profile(&edit_run.id, &profile)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", edit_run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Make the page flashy and loud." }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_body = to_bytes(response.into_body(), 4096).await.unwrap();
    let response_payload: Value = serde_json::from_slice(&response_body).unwrap();
    assert_eq!(response_payload["status"], "needs_user_input");
    let run = store.get_run(&edit_run.id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::NeedsUserInput);
    assert!(store.events(&edit_run.id).await.iter().any(|event| {
        matches!(
            event,
            AgentEvent::StateChanged { state, .. }
                if state == "needs_user_input:design_profile_conflict"
        )
    }));
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "approval_request"
            && item.text.contains("visual keyword \"flashy\"")));

    let override_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/continue", edit_run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "临时覆盖 DesignProfile，继续执行" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(override_response.status(), StatusCode::OK);
    let override_body = to_bytes(override_response.into_body(), 4096).await.unwrap();
    let override_payload: Value = serde_json::from_slice(&override_body).unwrap();
    assert_eq!(override_payload["status"], "running");
    assert!(store
        .conversation_items("project-1")
        .await
        .iter()
        .any(|item| item.kind == "design_profile_override"
            && item.text.contains("override accepted")
            && item
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata["designProfileId"] == profile_id)));
}
