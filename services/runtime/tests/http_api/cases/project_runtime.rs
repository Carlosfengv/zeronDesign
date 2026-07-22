use super::*;

#[tokio::test]
async fn preview_current_returns_promoted_version_contract() {
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
    let candidate = store
        .create_project_version_candidate(
            "project-1",
            &run.id,
            "http://preview.local/project-1/current".to_string(),
            Some("shot-1".to_string()),
            None,
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
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: public_auth_disabled_config(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/preview/project-1/current")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["versionId"], candidate.id);
    assert_eq!(
        payload["previewUrl"],
        "http://preview.local/project-1/current"
    );
    assert_eq!(payload["status"], "promoted");
}

#[tokio::test]
async fn project_history_returns_recoverable_drafts_and_publishable_versions() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-history".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let draft = store
        .create_draft_snapshot(
            "project-history",
            "object://source-snapshots/project-history/draft-1.tar.zst".to_string(),
            "a".repeat(64),
            "next-app".to_string(),
            "next-app@1".to_string(),
            "runtime-dependency-policy@1".to_string(),
            "b".repeat(64),
            &run.id,
            None,
            None,
        )
        .await
        .unwrap();
    let version = store
        .create_project_version_candidate(
            "project-history",
            &run.id,
            "http://preview.local/project-history/current".to_string(),
            None,
            Some("object://source-snapshots/project-history/version-1.tar.zst".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "project-history",
        &run.id,
        &version.id,
        PromotionGateReport::passing(),
    )
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
                .uri("/projects/project-history/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["projectId"], "project-history");
    let items = payload["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let draft_item = items
        .iter()
        .find(|item| item["kind"] == "draft_snapshot")
        .unwrap();
    assert_eq!(draft_item["snapshot"]["snapshotId"], draft.snapshot_id);
    assert_eq!(draft_item["recoverable"], true);
    assert_eq!(draft_item["publishable"], false);
    let version_item = items
        .iter()
        .find(|item| item["kind"] == "work_version")
        .unwrap();
    assert_eq!(version_item["version"]["id"], version.id);
    assert_eq!(version_item["recoverable"], true);
    assert_eq!(version_item["publishable"], true);
}

#[tokio::test]
async fn project_runtime_state_exposes_editable_lifecycle_metadata() {
    let workspace = unique_temp_dir("runtime-state-lifecycle");
    let project_workspace = workspace.join("project-1");
    fs::create_dir_all(project_workspace.join("state")).unwrap();
    fs::create_dir_all(project_workspace.join("outputs/build")).unwrap();
    fs::write(
        project_workspace.join("state/style-contract.json"),
        runtime_style_contract_json(
            "project/src/styles/tokens.css",
            "project/src/styles/global.css",
            "project/src/components/ui",
        )
        .to_string(),
    )
    .unwrap();
    fs::write(
        project_workspace.join("state/dependency-state.json"),
        json!({
            "needsRestore": false,
            "packageManager": "npm",
            "success": true
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        project_workspace.join("state/preview.json"),
        json!({
            "url": "http://127.0.0.1:4321",
            "status": "running"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        project_workspace.join("outputs/build/latest.json"),
        json!({
            "status": "success",
            "buildId": "build-1",
            "sourceSnapshotUri": "runtime://snapshots/project-1/version-1"
        })
        .to_string(),
    )
    .unwrap();
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
    let run = store
        .create_run_with_context(
            "project-1".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            None,
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
            Some("runtime://snapshots/project-1/version-1".to_string()),
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
    let mut config = public_auth_disabled_config();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config.workspace_root = workspace;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["currentVersionId"], candidate.id);
    assert_eq!(payload["sandboxBindingId"], binding.id);
    assert_eq!(
        payload["sourceSnapshotUri"],
        "runtime://snapshots/project-1/version-1"
    );
    assert_eq!(payload["appRoot"], "project");
    assert_eq!(payload["templateKey"], "next-app");
    assert_eq!(
        payload["styleContractPath"],
        "/workspace/state/style-contract.json"
    );
    assert_eq!(
        payload["styleContract"]["tokens"]["color.primary"],
        "--runtime-primary"
    );
    assert_eq!(
        payload["styleContract"]["globalCssFile"],
        "project/src/styles/global.css"
    );
    assert_eq!(
        payload["styleContract"]["componentRoot"],
        "project/src/components/ui"
    );
    assert_eq!(payload["styleContract"]["tailwind"]["version"], "4");
    assert_eq!(
        payload["styleContract"]["tailwind"]["entryImport"],
        "@import \"tailwindcss\""
    );
    assert_eq!(
        payload["styleContract"]["tailwind"]["themeSource"],
        "css-variables"
    );
    assert_eq!(payload["dependencyState"]["needsRestore"], false);
    assert_eq!(payload["latestBuild"]["status"], "success");
    assert_eq!(
        payload["latestBuild"]["sourceSnapshotUri"],
        payload["sourceSnapshotUri"]
    );
    assert_eq!(payload["preview"]["status"], "running");
}

#[tokio::test]
async fn project_runtime_state_reads_phase_a_global_workspace_lifecycle_state() {
    let workspace = unique_temp_dir("runtime-state-phase-a-global");
    fs::create_dir_all(workspace.join("state")).unwrap();
    fs::create_dir_all(workspace.join("outputs/build")).unwrap();
    fs::write(
        workspace.join("state/style-contract.json"),
        runtime_style_contract_json(
            "project/src/styles/tokens.css",
            "project/src/styles/global.css",
            "project/src/components/ui",
        )
        .to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/dependency-state.json"),
        json!({ "needsRestore": true }).to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("state/preview.json"),
        json!({ "status": "running" }).to_string(),
    )
    .unwrap();
    fs::write(
        workspace.join("outputs/build/latest.json"),
        json!({
            "status": "success",
            "sourceSnapshotUri": "runtime://snapshots/phase-a-project/version-1"
        })
        .to_string(),
    )
    .unwrap();
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "phase-a-project".to_string(),
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
    let run = store
        .create_run_with_context(
            "phase-a-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            None,
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "phase-a-project",
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
            "phase-a-project",
            &run.id,
            "http://preview.local/phase-a-project/current".to_string(),
            Some("shot-1".to_string()),
            Some("runtime://snapshots/phase-a-project/version-1".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "phase-a-project",
        &run.id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    let mut config = public_auth_disabled_config();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config.workspace_root = workspace;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/phase-a-project/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["styleContract"]["tokens"]["color.primary"],
        "--runtime-primary"
    );
    assert_eq!(
        payload["styleContract"]["componentRoot"],
        "project/src/components/ui"
    );
    assert_eq!(
        payload["styleContract"]["tailwind"]["themeSource"],
        "css-variables"
    );
    assert_eq!(payload["dependencyState"]["needsRestore"], true);
    assert_eq!(payload["latestBuild"]["status"], "success");
    assert_eq!(
        payload["latestBuild"]["sourceSnapshotUri"],
        payload["sourceSnapshotUri"]
    );
    assert_eq!(payload["preview"]["status"], "running");
}

#[tokio::test(flavor = "current_thread")]
async fn project_runtime_state_reads_kubernetes_workspace_channel_lifecycle_state() {
    let _env_guard = SANDBOX_CHANNEL_ENV_LOCK.lock().await;
    let mut files = HashMap::new();
    files.insert(
        "/workspace/state/style-contract.json".to_string(),
        runtime_style_contract_json(
            "project/src/styles/tokens.css",
            "project/src/styles/global.css",
            "project/src/components/ui",
        ),
    );
    files.insert(
        "/workspace/state/dependency-state.json".to_string(),
        json!({ "needsRestore": false }),
    );
    files.insert(
        "/workspace/state/preview.json".to_string(),
        json!({ "status": "running" }),
    );
    files.insert(
        "/workspace/outputs/build/latest.json".to_string(),
        json!({
            "status": "success",
            "sourceSnapshotUri": "runtime://snapshots/k8s-project/version-1"
        }),
    );
    let (host, port, _requests, server) = start_runtime_state_workspace_channel_server(files).await;
    let _channel_env = SandboxChannelEnvOverride::set(host, port);

    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "k8s-project".to_string(),
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
    let run = store
        .create_run_with_context(
            "k8s-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            None,
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "k8s-project",
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
        .update_sandbox_binding_runtime_identity_with_uids(
            &binding.id,
            binding.sandbox_name.clone(),
            Some(binding.sandbox_name.clone()),
            Some("sandbox-uid-k8s-project".to_string()),
            Some("pod-uid-k8s-project".to_string()),
        )
        .await
        .unwrap();
    store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .unwrap();
    let candidate = store
        .create_project_version_candidate(
            "k8s-project",
            &run.id,
            "http://preview.local/k8s-project/current".to_string(),
            Some("shot-1".to_string()),
            Some("runtime://snapshots/k8s-project/version-1".to_string()),
        )
        .await;
    promote_preview(
        &store,
        "k8s-project",
        &run.id,
        &candidate.id,
        PromotionGateReport::passing(),
    )
    .await
    .unwrap();
    let mut config = public_auth_disabled_config();
    config.sandbox_backend_mode = SandboxBackendMode::Kubernetes;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/k8s-project/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    server.abort();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["styleContract"]["tokens"]["color.primary"],
        "--runtime-primary"
    );
    assert_eq!(
        payload["styleContract"]["globalCssFile"],
        "project/src/styles/global.css"
    );
    assert_eq!(payload["styleContract"]["tailwind"]["version"], "4");
    assert_eq!(payload["dependencyState"]["needsRestore"], false);
    assert_eq!(payload["latestBuild"]["status"], "success");
    assert_eq!(
        payload["latestBuild"]["sourceSnapshotUri"],
        payload["sourceSnapshotUri"]
    );
    assert_eq!(payload["preview"]["status"], "running");
    assert_eq!(_requests.lock().unwrap().len(), 4);
}
