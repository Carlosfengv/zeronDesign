use super::*;
use anydesign_runtime::{
    artifact_publisher::{ArtifactFile, FileArtifactPublisher},
    design_context::{
        compile_website_design_context, DesignContextCompileOptions, VerifierRegistry,
    },
    templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
    types::DesignProfile,
};
use chrono::Utc;

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
async fn start_edit_inherits_frozen_dcp_restores_and_rematerializes_the_promoted_snapshot() {
    let workspace = unique_temp_dir("http-edit-inherits-dcp");
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.enable_design_context_package = true;
    let source_snapshot = workspace
        .join("project-1")
        .join("outputs/build/source-snapshots/dcp-source");
    fs::create_dir_all(source_snapshot.join("src/pages")).unwrap();
    fs::write(source_snapshot.join("package.json"), "{}\n").unwrap();
    fs::write(
        source_snapshot.join("src/pages/index.astro"),
        "<main><h1>Frozen DCP source</h1></main>\n",
    )
    .unwrap();
    let source_snapshot_uri = FileArtifactPublisher::new(&config.runtime_storage_dir)
        .publish_source_snapshot(
            "project-1",
            "dcp-source",
            vec![
                ArtifactFile {
                    path: "package.json".into(),
                    bytes: fs::read(source_snapshot.join("package.json")).unwrap(),
                },
                ArtifactFile {
                    path: "src/pages/index.astro".into(),
                    bytes: fs::read(source_snapshot.join("src/pages/index.astro")).unwrap(),
                },
            ],
        )
        .await
        .unwrap();

    let store = RuntimeStore::with_checkpoint_dir(config.runtime_storage_dir.clone());
    let source = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief = website_brief();
    store.write_brief(&source.id, brief.clone()).await.unwrap();
    let profile = dcp_observe_profile("project-1");
    store.create_design_profile(profile.clone()).await.unwrap();
    let source = store
        .attach_run_effective_design_profile(
            &source.id,
            &profile,
            Some("website"),
            Some("astro-website"),
        )
        .await
        .unwrap();
    let template = BuiltInTemplateRegistry::built_in()
        .current(&TemplateId::parse("astro-website").unwrap())
        .unwrap();
    let dcp = compile_website_design_context(
        &profile.effective_for("website", "astro-website").unwrap(),
        &brief,
        &template,
        &DesignContextCompileOptions::default(),
    )
    .unwrap();
    let source = store
        .attach_run_design_context(&source.id, &dcp, &VerifierRegistry::discover())
        .await
        .unwrap();
    store
        .record_run_design_context_materialization(
            &source.id,
            &dcp.manifest.payload.artifact_manifest_hash,
        )
        .await
        .unwrap();
    store
        .set_run_design_context_style_contract_verified(&source.id, true)
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
        .bind_run_to_sandbox(&source.id, &binding.id)
        .await
        .unwrap();
    let version = store
        .create_project_version_candidate(
            "project-1",
            &source.id,
            "http://preview.local/project-1/dcp-source".to_string(),
            Some("dcp-shot".to_string()),
            Some(source_snapshot_uri.clone()),
        )
        .await;
    promote_preview(
        &store,
        "project-1",
        &source.id,
        &version.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    store
        .update_run_status(&source.id, AgentRunStatus::Completed)
        .await
        .unwrap();

    // A restarted Runtime must recover the frozen source Run/version/binding
    // from durable state; it must not resolve a fresh DCP from the current
    // profile when the Edit starts.
    drop(store);
    let store = RuntimeStore::with_checkpoint_dir(config.runtime_storage_dir.clone());
    let restored_workspace = unique_temp_dir("http-edit-inherits-dcp-restored-workspace");
    config.workspace_root = restored_workspace.clone();
    let recovered_source = store.get_run(&source.id).await.unwrap();
    assert_eq!(
        recovered_source.design_context_content_hash,
        source.design_context_content_hash
    );
    assert_eq!(
        recovered_source.design_context_artifacts,
        source.design_context_artifacts
    );
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        version.id
    );
    assert_eq!(
        store.get_sandbox_binding(&binding.id).await.unwrap().status,
        SandboxBindingStatus::Ready
    );
    let model = MockModelClient::new(vec![ModelResponse::ToolCalls(vec![
        ToolCall::new(
            "edit-dcp-profile",
            "fs.read",
            json!({ "path": "inputs/design-profile.json" }),
        ),
        ToolCall::new(
            "edit-dcp-usage",
            "fs.read",
            json!({ "path": "inputs/design-profile-usage.md" }),
        ),
        ToolCall::new(
            "edit-dcp-recipes",
            "fs.read",
            json!({ "path": "inputs/component-recipes.json" }),
        ),
        ToolCall::new(
            "edit-dcp-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "DCP inherited and read" }),
        ),
    ])]);
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
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
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "baseVersionId": version.id,
                            "sandboxBindingId": binding.id
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
            "unexpected DCP edit start response {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let payload: Value = serde_json::from_slice(&body).unwrap();
    let edit_run_id = payload["runId"].as_str().unwrap();
    let edit = store.get_run(edit_run_id).await.unwrap();

    assert_eq!(edit.status, AgentRunStatus::Queued);
    assert_eq!(
        edit.design_context_content_hash,
        source.design_context_content_hash
    );
    assert_eq!(edit.design_context_manifest, source.design_context_manifest);
    assert_eq!(
        edit.design_context_artifacts,
        source.design_context_artifacts
    );
    assert_eq!(
        edit.design_profile_effective_hash,
        source.design_profile_effective_hash
    );
    assert_eq!(edit.brief_version, source.brief_version);
    assert!(edit.design_context_materialization_hash.is_none());
    assert!(edit.design_context_read_files.is_empty());
    assert_eq!(edit.design_context_style_contract_verified, None);
    assert_eq!(
        fs::read_to_string(restored_workspace.join("project-1/project/src/pages/index.astro"),)
            .unwrap(),
        "<main><h1>Frozen DCP source</h1></main>\n"
    );
    // This case isolates inherited DCP materialization/read behavior. A separate lifecycle
    // fixture owns preview publication, so seed the already-promoted version solely to let
    // `run.complete` terminate this read-only agent turn.
    store
        .set_run_output_version(edit_run_id, version.id.clone())
        .await
        .unwrap();
    store
        .append_conversation_item(
            "project-1",
            Some(edit_run_id),
            "design_profile_fidelity_checked",
            Some("system"),
            "Seeded passing fidelity evidence for inherited-DCP read test.",
            Some(json!({ "requiredFailedRuleIds": [] })),
        )
        .await;

    let continue_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{edit_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Keep the source and complete the DCP read pass" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let continue_status = continue_response.status();
    let continue_body = to_bytes(continue_response.into_body(), 4096).await.unwrap();
    if continue_status != StatusCode::OK {
        panic!(
            "unexpected DCP edit continue response {continue_status}: {}",
            String::from_utf8_lossy(&continue_body)
        );
    }
    wait_for_terminal(&store, edit_run_id).await;
    let completed = store.get_run(edit_run_id).await.unwrap();
    assert_eq!(
        completed.status,
        AgentRunStatus::Completed,
        "DCP edit run failed: {completed:?} events={:?}",
        store.events(edit_run_id).await
    );
    assert!(completed.design_context_materialization_hash.is_some());
    for path in [
        "inputs/design-profile.json",
        "inputs/design-profile-usage.md",
        "inputs/component-recipes.json",
    ] {
        assert!(
            completed
                .design_context_read_files
                .iter()
                .any(|read| read == path),
            "missing inherited DCP read: {path}"
        );
    }
}

fn dcp_observe_profile(project_id: &str) -> DesignProfile {
    DesignProfile {
        id: "http-edit-dcp-profile".to_string(),
        schema_version: "design-profile@1".to_string(),
        name: "HTTP Edit DCP Profile".to_string(),
        status: "active".to_string(),
        version: 1,
        scope: json!({ "projectId": project_id }),
        source: json!({ "kind": "manual" }),
        product: json!({ "name": "HTTP DCP", "category": "test fixture" }),
        brand: json!({}),
        visual: json!({ "direction": "quiet operational interface" }),
        tokens: json!({}),
        runtime_token_mapping: json!({
            "color.background": "#ffffff",
            "color.surface": "#f8fafc",
            "color.surfaceStrong": "#e2e8f0",
            "color.text": "#0f172a",
            "color.muted": "#475569",
            "color.primary": "#2563eb",
            "color.primaryContrast": "#ffffff",
            "color.border": "#cbd5e1",
            "radius.card": "8px",
            "radius.control": "6px",
            "font.sans": "Inter, sans-serif",
            "shadow.soft": "0 1px 2px rgba(15, 23, 42, 0.12)"
        }),
        extended_token_mapping: json!({}),
        components: json!({}),
        website_context: json!({ "enforcementMode": "observe" }),
        content: json!({}),
        accessibility: json!({}),
        technical: json!({ "allowedTemplates": ["astro-website"] }),
        governance: json!({ "conflictBehavior": "ask" }),
        signature_rules: Vec::new(),
        overrides: json!({}),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
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
