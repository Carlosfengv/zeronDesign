use super::*;
use anydesign_runtime::{
    design_context::{
        compile_website_design_context, DesignContextCompileOptions, VerifierRegistry,
    },
    project::resolve_built_in_template_for_init,
};

#[tokio::test]
async fn profile_sync_plan_uses_frozen_dcp_and_actual_workspace_tokens_idempotently() {
    let project_id = "profile-sync-http";
    let mut config = phase_a_contract_config();
    config.workspace_root = unique_temp_dir("profile-sync-http-workspace");
    let store = RuntimeStore::with_checkpoint_dir(config.runtime_storage_dir.clone());
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: config.clone(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let source_profile_request = design_profile_request(project_id, vec!["astro-website"]);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-profiles")
                .header("content-type", "application/json")
                .body(Body::from(source_profile_request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_body = to_bytes(created.into_body(), 32_768).await.unwrap();
    let created_json: Value = serde_json::from_slice(&created_body).unwrap();
    let profile_id = created_json["designProfile"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let source_profile = store.get_design_profile(&profile_id).await.unwrap();

    let mut target_payload = source_profile_request["profile"].clone();
    target_payload["runtimeTokenMapping"]["color.primary"] = json!("#b91c1c");
    let updated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/design-profiles/{profile_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "Harness Calm Ops v2",
                        "profile": target_payload,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_body = to_bytes(updated.into_body(), 32_768).await.unwrap();
    let updated_json: Value = serde_json::from_slice(&updated_body).unwrap();
    let target_profile = updated_json["designProfile"].clone();
    assert_eq!(target_profile["version"], json!(2));

    let source_run = store
        .create_run(
            project_id.to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "fixture".to_string(),
            Vec::new(),
        )
        .await;
    let brief_id = store
        .write_brief(&source_run.id, website_brief())
        .await
        .unwrap();
    let source_run = store
        .attach_run_effective_design_profile(
            &source_run.id,
            &source_profile,
            Some("website"),
            Some("astro-website"),
        )
        .await
        .unwrap();
    let template = resolve_built_in_template_for_init("astro-website")
        .await
        .unwrap();
    let brief = store.get_brief(&brief_id).await.unwrap();
    let source_effective = source_profile
        .effective_for("website", "astro-website")
        .unwrap();
    let source_dcp = compile_website_design_context(
        &source_effective,
        &brief,
        &template,
        &DesignContextCompileOptions::default(),
    )
    .unwrap();
    store
        .attach_run_design_context(&source_run.id, &source_dcp, &VerifierRegistry::discover())
        .await
        .unwrap();
    store
        .record_run_design_context_materialization(
            &source_run.id,
            &source_dcp.manifest.payload.artifact_manifest_hash,
        )
        .await
        .unwrap();
    let binding = store
        .create_sandbox_binding(
            project_id,
            "profile-sync-sandbox".to_string(),
            "profile-sync-claim".to_string(),
            "profile-sync-workspace".to_string(),
            "profile-sync-pool".to_string(),
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
        .bind_run_to_sandbox(&source_run.id, &binding.id)
        .await
        .unwrap();

    let workspace = config.workspace_root.join(project_id);
    std::fs::create_dir_all(workspace.join("state")).unwrap();
    std::fs::create_dir_all(workspace.join("project")).unwrap();
    let mut style_contract: Value = serde_json::from_str(
        source_dcp
            .files
            .get("inputs/template-style-contract.json")
            .unwrap(),
    )
    .unwrap();
    let token_mappings = style_contract["tokens"].as_object().unwrap().clone();
    style_contract["tokenFile"] = json!("project/tokens.css");
    let token_css = token_mappings
        .iter()
        .map(|(token, css_variable)| {
            let value = source_dcp
                .manifest
                .payload
                .resolved_runtime_tokens
                .get(token)
                .map(String::as_str)
                .unwrap_or("fixture-default");
            format!("{}: {value};", css_variable.as_str().unwrap())
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        workspace.join("project/tokens.css"),
        format!(":root {{\n{token_css}\n}}"),
    )
    .unwrap();
    std::fs::write(
        workspace.join("state/style-contract.json"),
        serde_json::to_vec(&style_contract).unwrap(),
    )
    .unwrap();

    let manifest_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/design-context-manifest", source_run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(manifest_response.status(), StatusCode::OK);
    let manifest_body = to_bytes(manifest_response.into_body(), 32_768)
        .await
        .unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_body).unwrap();
    assert_eq!(manifest["package"]["surface"], json!("website"));
    assert_eq!(manifest["package"]["template"], json!("astro-website"));
    assert_eq!(
        manifest["package"]["designProfileId"],
        json!(profile_id.clone())
    );

    let diagnostics_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/runs/{}/design-context-diagnostics",
                    source_run.id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(diagnostics_response.status(), StatusCode::OK);
    let diagnostics_body = to_bytes(diagnostics_response.into_body(), 32_768)
        .await
        .unwrap();
    let diagnostics: Value = serde_json::from_slice(&diagnostics_body).unwrap();
    assert!(diagnostics["verification"]["capabilities"].is_object());
    assert!(diagnostics["verification"]
        .get("browserExecutable")
        .is_none());
    assert!(diagnostics["verification"].get("environment").is_none());

    let target_profile = store
        .design_profile_versions(&profile_id)
        .await
        .unwrap()
        .into_iter()
        .find(|profile| profile.version == 2)
        .unwrap();
    let target_effective = target_profile
        .effective_for("website", "astro-website")
        .unwrap();
    let request_body = json!({
        "targetDesignProfileId": profile_id.clone(),
        "targetDesignProfileVersion": 2,
        "targetEffectiveProfileHash": target_effective.effective_profile_hash.clone(),
        "expectedSourceContentHash": source_dcp.manifest.content_hash.clone(),
        "idempotencyKey": "profile-sync-plan-1",
    });
    let mut drifted_contract = style_contract.clone();
    drifted_contract["appRoot"] = json!("/workspace/not-project");
    std::fs::write(
        workspace.join("state/style-contract.json"),
        serde_json::to_vec(&drifted_contract).unwrap(),
    )
    .unwrap();
    let drifted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/design-profile-sync-plan", source_run.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "targetDesignProfileId": profile_id,
                        "targetDesignProfileVersion": 2,
                        "targetEffectiveProfileHash": target_effective.effective_profile_hash,
                        "expectedSourceContentHash": source_dcp.manifest.content_hash,
                        "idempotencyKey": "profile-sync-plan-drifted",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(drifted.status(), StatusCode::CONFLICT);
    let run_after_drift = store.get_run(&source_run.id).await.unwrap();
    assert_eq!(
        run_after_drift.design_context_style_contract_verified,
        Some(false)
    );
    std::fs::write(
        workspace.join("state/style-contract.json"),
        serde_json::to_vec(&style_contract).unwrap(),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/design-profile-sync-plan", source_run.id))
                .header("content-type", "application/json")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let response_status = response.status();
    let body = to_bytes(response.into_body(), 32_768).await.unwrap();
    assert_eq!(
        response_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let plan: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(plan["status"], json!("planned"));
    assert_eq!(plan["targetDesignProfileVersion"], json!(2));
    assert!(plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["token"] == "color.primary" && item["state"] == "apply_target"));

    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{}/design-profile-sync-plan", source_run.id))
                .header("content-type", "application/json")
                .body(Body::from(request_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay_body = to_bytes(replay.into_body(), 32_768).await.unwrap();
    let replay: Value = serde_json::from_slice(&replay_body).unwrap();
    assert_eq!(replay["operationId"], plan["operationId"]);
    assert_eq!(replay["planHash"], plan["planHash"]);

    let rejected_confirm = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/runs/{}/design-profile-sync-operations/{}/confirm",
                    source_run.id,
                    plan["operationId"].as_str().unwrap()
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "planHash": "0".repeat(64),
                        "conflictDecisions": {},
                        "idempotencyKey": "profile-sync-confirm-rejected",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rejected_confirm.status(), StatusCode::CONFLICT);
    let rejected_confirm_body = to_bytes(rejected_confirm.into_body(), 32_768)
        .await
        .unwrap();
    let rejected_confirm: Value = serde_json::from_slice(&rejected_confirm_body).unwrap();
    assert_eq!(
        rejected_confirm["errorCode"],
        json!("profile_sync_plan_mismatch")
    );

    let confirmed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/runs/{}/design-profile-sync-operations/{}/confirm",
                    source_run.id,
                    plan["operationId"].as_str().unwrap()
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "planHash": plan["planHash"],
                        "conflictDecisions": {},
                        "idempotencyKey": "profile-sync-confirm-1",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let confirmed_status = confirmed.status();
    let confirmed_body = to_bytes(confirmed.into_body(), 32_768).await.unwrap();
    assert_eq!(
        confirmed_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&confirmed_body)
    );
    let confirmed: Value = serde_json::from_slice(&confirmed_body).unwrap();
    assert_eq!(confirmed["status"], json!("applied"));
    assert!(confirmed["childRunId"].as_str().is_some());
    let token_file = std::fs::read_to_string(workspace.join("project/tokens.css")).unwrap();
    assert!(token_file.contains("#b91c1c"));

    let confirmed_replay = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/runs/{}/design-profile-sync-operations/{}/confirm",
                    source_run.id,
                    plan["operationId"].as_str().unwrap()
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "planHash": plan["planHash"],
                        "conflictDecisions": {},
                        "idempotencyKey": "profile-sync-confirm-1",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirmed_replay.status(), StatusCode::OK);
    let confirmed_replay_body = to_bytes(confirmed_replay.into_body(), 32_768)
        .await
        .unwrap();
    let confirmed_replay: Value = serde_json::from_slice(&confirmed_replay_body).unwrap();
    assert_eq!(confirmed_replay["childRunId"], confirmed["childRunId"]);
    let sync_metrics = store
        .events(&source_run.id)
        .await
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::MetricRecorded { name, metadata, .. }
                if name == "design_context_profile_sync_total" =>
            {
                metadata
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(sync_metrics.iter().any(|metric| {
        metric["stage"] == "plan"
            && metric["status"] == "rejected"
            && metric["reason"] == "profile_sync_precondition_failed"
    }));
    assert!(sync_metrics.iter().any(|metric| {
        metric["stage"] == "plan"
            && metric["status"] == "planned"
            && metric["mode"] == "observe"
            && metric["surface"] == "website"
            && metric["phase"] == "build"
    }));
    assert!(sync_metrics.iter().any(|metric| {
        metric["stage"] == "confirm"
            && metric["status"] == "rejected"
            && metric["reason"] == "profile_sync_plan_mismatch"
    }));
    assert!(sync_metrics.iter().any(|metric| {
        metric["stage"] == "confirm"
            && metric["status"] == "confirmed"
            && metric["reason"].is_null()
    }));
    assert!(sync_metrics.iter().any(|metric| {
        metric["stage"] == "apply" && metric["status"] == "applied" && metric["reason"].is_null()
    }));
    let serialized_metrics = serde_json::to_string(&sync_metrics).unwrap();
    assert!(!serialized_metrics.contains("operationId"));
    assert!(!serialized_metrics.contains("tokenName"));
    assert!(!serialized_metrics.contains("tokenValue"));
}
