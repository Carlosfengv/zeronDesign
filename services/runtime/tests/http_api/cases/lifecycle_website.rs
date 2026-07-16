use super::*;
use anydesign_runtime::types::DesignProfile;
use chrono::Utc;

fn dcp_observe_profile(project_id: &str) -> DesignProfile {
    let now = Utc::now();
    DesignProfile {
        id: "http-lifecycle-dcp-profile".to_string(),
        schema_version: "design-profile@1".to_string(),
        name: "HTTP Lifecycle DCP Profile".to_string(),
        status: "active".to_string(),
        version: 1,
        scope: json!({ "projectId": project_id }),
        source: json!({ "kind": "manual" }),
        product: json!({ "name": "HTTP DCP", "category": "test fixture" }),
        brand: json!({}),
        visual: json!({ "direction": "quiet technical confidence" }),
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
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn website_dcp_flag_matrix_is_fail_closed_and_backward_compatible() {
    let store = RuntimeStore::new();
    let cases = [
        ("off-off", false, false, false),
        ("on-off", true, false, true),
        ("on-on-empty-allowlist", true, true, true),
    ];

    for (suffix, master_enabled, enforcement_enabled, expects_dcp) in cases {
        let project_id = format!("project-dcp-flag-matrix-{suffix}");
        let brief_run = store
            .create_run(
                project_id.clone(),
                AgentPhase::Brief,
                "brief".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let brief_id = store
            .write_brief(&brief_run.id, website_brief())
            .await
            .unwrap();
        store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
        let mut profile = dcp_observe_profile(&project_id);
        profile.id = format!("profile-dcp-flag-matrix-{suffix}");
        profile.website_context = json!({
            "enforcementMode": "enforced",
            "craftPacks": ["accessibility-baseline", "responsive-layout"]
        });
        let profile = store.create_design_profile(profile).await.unwrap();
        store
            .bind_project_design_profile(&project_id, &profile.id)
            .await
            .unwrap();

        let mut config = phase_a_contract_config();
        config.enable_design_context_package = master_enabled;
        config.enable_design_context_enforcement = enforcement_enabled;
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
                    .uri("/runs")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "projectId": project_id,
                            "phase": "build",
                            "agentProfile": "build",
                            "inputContext": { "briefId": brief_id }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "flag matrix case {suffix}"
        );
        let payload: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let run = store
            .get_run(payload["runId"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            run.design_context_manifest.is_some(),
            expects_dcp,
            "flag matrix case {suffix}"
        );
        if expects_dcp {
            assert_eq!(
                run.design_context_effective_compatibility_mode.as_deref(),
                Some("observe"),
                "only an exact enforcement allowlist match may turn on required gates",
            );
        }
    }

    let mut invalid = phase_a_contract_config();
    invalid.enable_design_context_package = false;
    invalid.enable_design_context_enforcement = true;
    assert!(invalid.validate_startup().unwrap_err().contains(
        "RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_V1 requires RUNTIME_DESIGN_CONTEXT_PACKAGE_V1"
    ));
}

#[tokio::test]
async fn enforced_dcp_unavailable_verifier_blocks_and_records_low_cardinality_metric() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-dcp-verifier-unavailable".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "fixture".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, website_brief())
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let mut profile = dcp_observe_profile("project-dcp-verifier-unavailable");
    profile.id = "profile-dcp-verifier-unavailable".to_string();
    profile.website_context = json!({
        "enforcementMode": "enforced",
        "craftPacks": ["accessibility-baseline", "responsive-layout"]
    });
    let profile = store.create_design_profile(profile).await.unwrap();
    store
        .bind_project_design_profile("project-dcp-verifier-unavailable", &profile.id)
        .await
        .unwrap();
    let mut config = phase_a_contract_config();
    config.enable_design_context_package = true;
    config.enable_design_context_enforcement = true;
    config.design_context_enforcement_allowlist_json = Some(
        json!([{
            "projectId": "project-dcp-verifier-unavailable",
            "designProfileId": profile.id,
            "designProfileVersion": profile.version,
        }])
        .to_string(),
    );
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
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
                        "projectId": "project-dcp-verifier-unavailable",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": { "briefId": brief_id }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(payload["status"], "blocked");
    let run_id = payload["runId"].as_str().unwrap();
    assert_eq!(
        store.get_run(run_id).await.unwrap().status,
        AgentRunStatus::Blocked
    );
    assert!(store.events(run_id).await.into_iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded { name, metadata: Some(metadata), .. }
            if name == "design_context_verifier_unavailable_total"
                && metadata["mode"] == "enforced"
                && metadata["missingVerifierCount"].as_u64().is_some_and(|count| count > 0)
    )));
}

#[tokio::test]
async fn public_runtime_enforced_dcp_build_collects_bound_browser_evidence() {
    let browser = std::env::var_os("RUNTIME_BROWSER_EXECUTABLE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let chrome =
                PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
            chrome.is_file().then_some(chrome)
        });
    let Some(browser) = browser else {
        eprintln!("skipping enforced browser-evidence lifecycle test: Chrome is unavailable");
        return;
    };
    let workspace = unique_temp_dir("http-lifecycle-dcp-enforced-browser");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-dcp-enforced-browser".to_string(),
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let mut profile = dcp_observe_profile("project-dcp-enforced-browser");
    profile.id = "http-lifecycle-dcp-enforced-browser-profile".to_string();
    profile.website_context = json!({
        "enforcementMode": "enforced",
        "craftPacks": ["accessibility-baseline", "responsive-layout"]
    });
    let profile = store.create_design_profile(profile).await.unwrap();
    store
        .bind_project_design_profile("project-dcp-enforced-browser", &profile.id)
        .await
        .unwrap();

    let build_script = r#"const fs=require('fs'); fs.mkdirSync('dist',{recursive:true}); fs.copyFileSync('src/pages/index.astro','dist/index.html');"#;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(
            [
                "inputs/brief.md",
                "inputs/design-profile.json",
                "inputs/design-profile-usage.md",
                "inputs/component-recipes.json",
                "inputs/template-style-contract.json",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                ToolCall::new(
                    format!("enforced-browser-bootstrap-read-{index}"),
                    "fs.read",
                    json!({ "path": path }),
                )
            })
            .collect(),
        ),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "enforced-browser-init",
            "project.init",
            json!({ "template": "astro-website" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "enforced-browser-read-style-contract",
            "fs.read",
            json!({ "path": "state/style-contract.json" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "enforced-browser-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "enforced-browser-build-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "enforced-browser-page",
                "fs.write",
                json!({
                    "path": "project/src/pages/index.astro",
                    "text": "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>html,body{margin:0;max-width:100%;overflow-x:hidden;font-family:system-ui,sans-serif}main{box-sizing:border-box;min-height:100vh;padding:32px;max-width:960px;margin:auto}</style></head><body><main><h1>Enforced browser evidence</h1><p>This page has no unnamed controls, links, or images.</p></main></body></html>"
                }),
            ),
            ToolCall::new(
                "enforced-browser-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-enforced-browser-shot" }),
            ),
            ToolCall::new(
                "enforced-browser-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Enforced browser evidence website published." }),
            ),
        ]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.enable_design_context_package = true;
    config.enable_design_context_enforcement = true;
    config.design_context_browser_executable = Some(browser.clone());
    config.design_context_enforcement_allowlist_json = Some(
        json!([{
            "projectId": "project-dcp-enforced-browser",
            "designProfileId": profile.id,
            "designProfileVersion": profile.version,
        }])
        .to_string(),
    );
    let runtime_storage = config.runtime_storage_dir.clone();
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
                        "projectId": "project-dcp-enforced-browser",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": { "briefId": brief_id }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let run_id = payload["runId"].as_str().unwrap().to_string();
    assert!(
        wait_for_terminal_with_timeout(&store, &run_id, 45).await,
        "enforced browser-evidence run did not reach terminal status: {:?}",
        store.events(&run_id).await
    );
    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(
        run.status,
        AgentRunStatus::Completed,
        "enforced browser-evidence build failed: {run:?} events={:?}",
        store.events(&run_id).await
    );
    assert_eq!(
        run.design_context_effective_compatibility_mode.as_deref(),
        Some("enforced")
    );
    let enforcement_binding = run.design_context_enforcement_binding.as_ref().unwrap();
    assert_eq!(enforcement_binding.source, "config");
    assert!(enforcement_binding.enabled);
    assert_eq!(enforcement_binding.policy_revision, None);
    assert_eq!(enforcement_binding.policy_updated_by, None);
    let environment = run
        .design_context_verification_environment
        .as_ref()
        .unwrap();
    assert_eq!(environment["browserExecutable"], json!(browser));
    for kind in ["computed-style", "a11y", "viewport"] {
        assert_eq!(environment["capabilities"][kind]["available"], json!(true));
    }
    let fidelity: Value = serde_json::from_str(
        &fs::read_to_string(
            workspace.join("project-dcp-enforced-browser/state/design-profile-fidelity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(fidelity["status"], json!("passed"));
    assert_eq!(fidelity["requiredFailedRuleIds"], json!([]));
    let assertions = fidelity["assertions"].as_array().unwrap();
    assert_eq!(assertions.len(), 6);
    assert!(assertions
        .iter()
        .all(|assertion| assertion["passed"] == json!(true)));
    assert!(assertions.iter().any(|assertion| {
        assertion["ruleId"] == "craft:responsive-layout:no-horizontal-overflow:375"
            && assertion["rawActual"].as_str().is_some_and(|actual| {
                actual.contains("screenshotSha256=") && actual.contains("screenshotUri=runtime://")
            })
    }));
    assert!(runtime_storage
        .join("screenshots/project-dcp-enforced-browser")
        .exists());
    let diagnostics = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{run_id}/design-context-diagnostics"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(diagnostics.status(), StatusCode::OK);
    let diagnostics: Value =
        serde_json::from_slice(&to_bytes(diagnostics.into_body(), 64 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(diagnostics["fidelity"]["status"], json!("passed"));
    assert_eq!(
        diagnostics["package"]["enforcementPolicy"],
        json!({
            "source": "config",
            "enabled": true,
            "policyRevision": null,
            "policyUpdatedBy": null,
        })
    );
    assert_eq!(diagnostics["fidelity"]["requiredFailedRuleIds"], json!([]));
    assert_eq!(
        diagnostics["fidelity"]["assertions"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    assert!(diagnostics["fidelity"].get("previewUrl").is_none());
    assert!(diagnostics["fidelity"].get("designContext").is_none());

    fs::remove_dir_all(workspace).unwrap();
    fs::remove_dir_all(runtime_storage).unwrap();
}

#[tokio::test]
async fn public_runtime_enforced_dcp_worker_loss_keeps_candidate_unpromoted() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = unique_temp_dir("http-lifecycle-dcp-enforced-worker-loss");
    let browser = workspace.join("browser-worker-that-exits");
    fs::write(
        &browser,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo fixture-browser; fi\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&browser, fs::Permissions::from_mode(0o755)).unwrap();
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-dcp-enforced-worker-loss".to_string(),
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let mut profile = dcp_observe_profile("project-dcp-enforced-worker-loss");
    profile.id = "http-lifecycle-dcp-enforced-worker-loss-profile".to_string();
    profile.website_context = json!({
        "enforcementMode": "enforced",
        "craftPacks": ["accessibility-baseline", "responsive-layout"]
    });
    let profile = store.create_design_profile(profile).await.unwrap();
    store
        .bind_project_design_profile("project-dcp-enforced-worker-loss", &profile.id)
        .await
        .unwrap();

    let build_script = r#"const fs=require('fs'); fs.mkdirSync('dist',{recursive:true}); fs.copyFileSync('src/pages/index.astro','dist/index.html');"#;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(
            [
                "inputs/brief.md",
                "inputs/design-profile.json",
                "inputs/design-profile-usage.md",
                "inputs/component-recipes.json",
                "inputs/template-style-contract.json",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                ToolCall::new(
                    format!("worker-loss-bootstrap-read-{index}"),
                    "fs.read",
                    json!({ "path": path }),
                )
            })
            .collect(),
        ),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "worker-loss-init",
            "project.init",
            json!({ "template": "astro-website" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "worker-loss-read-style-contract",
            "fs.read",
            json!({ "path": "state/style-contract.json" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "worker-loss-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "worker-loss-build-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "worker-loss-page",
                "fs.write",
                json!({
                    "path": "project/src/pages/index.astro",
                    "text": "<!doctype html><main><h1>Worker loss evidence</h1></main>"
                }),
            ),
            ToolCall::new(
                "worker-loss-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-worker-loss-shot" }),
            ),
        ]),
        ModelResponse::TextOnly("The required verification worker became unavailable.".to_string()),
        ModelResponse::TextOnly(
            "Waiting for the Runtime verification worker to recover.".to_string(),
        ),
        ModelResponse::TextOnly(
            "The publish must remain partial until verification can run.".to_string(),
        ),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.enable_design_context_package = true;
    config.enable_design_context_enforcement = true;
    config.design_context_browser_executable = Some(browser);
    config.design_context_enforcement_allowlist_json = Some(
        json!([{
            "projectId": "project-dcp-enforced-worker-loss",
            "designProfileId": profile.id,
            "designProfileVersion": profile.version,
        }])
        .to_string(),
    );
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model),
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-dcp-enforced-worker-loss",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": { "briefId": brief_id }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let run_id = payload["runId"].as_str().unwrap().to_string();
    assert!(
        wait_for_terminal_with_timeout(&store, &run_id, 45).await,
        "worker-loss run did not reach terminal status: {:?}",
        store.events(&run_id).await
    );
    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(run.status, AgentRunStatus::Partial);
    assert!(run.output_version_id.is_none());
    let events = store.events(&run_id).await;
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolFailed { tool, recoverable: true, metadata: Some(metadata), .. }
            if tool == "preview.publish"
                && metadata["errorKind"] == "design_verification_runtime_lost"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded {
            name,
            metadata: Some(metadata),
            ..
        } if name == "design_context_verifier_unavailable_total"
            && metadata["reason"] == "runtime_lost"
            && metadata["mode"] == "enforced"
            && metadata["surface"] == "website"
            && metadata["phase"] == "build"
    )));
    let candidate_id = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::PreviewCandidate { version_id, .. } => Some(version_id.clone()),
            _ => None,
        })
        .expect("worker loss must retain a candidate identity");
    let candidate = store.get_project_version(&candidate_id).await.unwrap();
    assert_eq!(
        candidate.status,
        anydesign_runtime::types::ProjectVersionStatus::Candidate
    );
    assert!(store
        .current_project_version("project-dcp-enforced-worker-loss")
        .await
        .is_none());
    assert!(store
        .artifact_publish_for_version("project-dcp-enforced-worker-loss", &run_id, &candidate_id)
        .await
        .is_none());
    assert!(!workspace
        .join("project-dcp-enforced-worker-loss/state/design-profile-fidelity.json")
        .exists());
    assert!(store
        .open_blocking_findings("project-dcp-enforced-worker-loss", &candidate_id)
        .await
        .is_empty());

    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn public_runtime_enforced_dcp_repairs_required_a11y_failure_before_promotion() {
    let browser = PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    if !browser.is_file() {
        eprintln!("skipping enforced fidelity-repair lifecycle test: Chrome is unavailable");
        return;
    }
    let workspace = unique_temp_dir("http-lifecycle-dcp-enforced-repair");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-dcp-enforced-repair".to_string(),
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let mut profile = dcp_observe_profile("project-dcp-enforced-repair");
    profile.id = "http-lifecycle-dcp-enforced-repair-profile".to_string();
    profile.website_context = json!({
        "enforcementMode": "enforced",
        "craftPacks": ["accessibility-baseline", "responsive-layout"]
    });
    let profile = store.create_design_profile(profile).await.unwrap();
    store
        .bind_project_design_profile("project-dcp-enforced-repair", &profile.id)
        .await
        .unwrap();

    let build_script = r#"const fs=require('fs'); fs.mkdirSync('dist',{recursive:true}); fs.copyFileSync('src/pages/index.astro','dist/index.html');"#;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(
            [
                "inputs/brief.md",
                "inputs/design-profile.json",
                "inputs/design-profile-usage.md",
                "inputs/component-recipes.json",
                "inputs/template-style-contract.json",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                ToolCall::new(
                    format!("enforced-repair-bootstrap-read-{index}"),
                    "fs.read",
                    json!({ "path": path }),
                )
            })
            .collect(),
        ),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "enforced-repair-init",
            "project.init",
            json!({ "template": "astro-website" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "enforced-repair-read-style-contract",
            "fs.read",
            json!({ "path": "state/style-contract.json" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "enforced-repair-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "enforced-repair-build-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "enforced-repair-page-with-violation",
                "fs.write",
                json!({
                    "path": "project/src/pages/index.astro",
                    "text": "<!doctype html><html><head><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>html,body{margin:0;max-width:100%;overflow-x:hidden}main{padding:24px}</style></head><body><main><h1>Repair required fidelity</h1><img src=\"/hero.png\"></main></body></html>"
                }),
            ),
            ToolCall::new(
                "enforced-repair-first-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-enforced-repair-first" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "enforced-repair-read-report",
                "fs.read",
                json!({ "path": "state/design-profile-fidelity.json" }),
            ),
            ToolCall::new(
                "enforced-repair-read-page",
                "fs.read",
                json!({ "path": "project/src/pages/index.astro" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "enforced-repair-add-alt",
            "fs.patch",
            json!({
                "path": "project/src/pages/index.astro",
                "oldStr": "<img src=\"/hero.png\">",
                "newStr": "<img src=\"/hero.png\" alt=\"Product dashboard preview\">"
            }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "enforced-repair-second-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-enforced-repair-second" }),
            ),
            ToolCall::new(
                "enforced-repair-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Required a11y failure repaired before promotion." }),
            ),
        ]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.enable_design_context_package = true;
    config.enable_design_context_enforcement = true;
    config.design_context_browser_executable = Some(browser);
    config.design_context_enforcement_allowlist_json = Some(
        json!([{
            "projectId": "project-dcp-enforced-repair",
            "designProfileId": profile.id,
            "designProfileVersion": profile.version,
        }])
        .to_string(),
    );
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model),
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-dcp-enforced-repair",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": { "briefId": brief_id }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    let run_id = payload["runId"].as_str().unwrap().to_string();
    assert!(
        wait_for_terminal_with_timeout(&store, &run_id, 45).await,
        "enforced repair run did not reach terminal status: {:?}",
        store.events(&run_id).await
    );
    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(
        run.status,
        AgentRunStatus::Completed,
        "enforced repair run failed: {run:?} events={:?}",
        store.events(&run_id).await
    );
    let promoted_id = run.output_version_id.clone().unwrap();
    let events = store.events(&run_id).await;
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolFailed { tool, recoverable: true, metadata: Some(metadata), .. }
            if tool == "preview.publish"
                && metadata["errorKind"] == "design_context.required_verification_failed"
    )));
    let fidelity_metrics = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MetricRecorded {
                name,
                metadata: Some(metadata),
                ..
            } if name == "design_context_fidelity_pass_rate" => Some(metadata),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(fidelity_metrics.iter().any(|metadata| {
        metadata["status"] == "failed"
            && metadata["attempt"] == "initial"
            && metadata["mode"] == "enforced"
    }));
    assert!(fidelity_metrics.iter().any(|metadata| {
        metadata["status"] == "passed"
            && metadata["attempt"] == "repair"
            && metadata["mode"] == "enforced"
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded {
            name,
            metadata: Some(metadata),
            ..
        } if name == "design_context_recipe_rule_fail_total"
            && metadata["kind"] == "a11y"
            && metadata["priority"] == "required"
            && metadata.get("reason").is_none()
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded {
            name,
            metadata: Some(metadata),
            ..
        } if name == "design_context_a11y_required_fail_total"
            && metadata["severity"] == "blocking"
            && metadata["mode"] == "enforced"
    )));
    let candidate_ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::PreviewCandidate { version_id, .. } => Some(version_id.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(candidate_ids.len(), 2);
    assert_ne!(candidate_ids[0], promoted_id);
    assert_eq!(
        store
            .get_project_version(&candidate_ids[0])
            .await
            .unwrap()
            .status,
        anydesign_runtime::types::ProjectVersionStatus::Candidate
    );
    assert_eq!(
        store
            .open_blocking_findings("project-dcp-enforced-repair", &candidate_ids[0])
            .await
            .len(),
        1
    );
    assert_eq!(
        store
            .get_project_version(&promoted_id)
            .await
            .unwrap()
            .status,
        anydesign_runtime::types::ProjectVersionStatus::Promoted
    );
    let fidelity: Value = serde_json::from_str(
        &fs::read_to_string(
            workspace.join("project-dcp-enforced-repair/state/design-profile-fidelity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(fidelity["status"], json!("passed"));
    assert_eq!(fidelity["requiredFailedRuleIds"], json!([]));

    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn public_runtime_dcp_source_fallback_reads_verified_source_before_build_publish() {
    let workspace = unique_temp_dir("http-lifecycle-dcp-source-fallback");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-dcp-source".to_string(),
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let source =
        b"# Imported Design Evidence\nUse a focused primary action and accessible controls.\n";
    let artifact = store
        .create_design_source_artifact(
            json!({ "projectId": "project-dcp-source" }),
            "DESIGN.md".to_string(),
            "text/markdown".to_string(),
            source.to_vec(),
        )
        .await
        .unwrap();
    let mut profile = dcp_observe_profile("project-dcp-source");
    profile.id = "http-lifecycle-dcp-source-profile".to_string();
    profile.source = json!({
        "kind": "imported",
        "sourceArtifactIds": [artifact.id],
        "primarySourceArtifactId": artifact.id,
        "sourceHash": artifact.sha256,
        "converterVersion": "test@1",
        "integrity": "verified"
    });
    let profile = store.create_design_profile(profile).await.unwrap();
    store
        .bind_project_design_profile("project-dcp-source", &profile.id)
        .await
        .unwrap();

    let build_script = r#"const fs=require('fs'); fs.mkdirSync('dist',{recursive:true}); const html=fs.readFileSync('src/pages/index.astro','utf8'); fs.writeFileSync('dist/index.html', html);"#;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(
            [
                "inputs/brief.md",
                "inputs/design-profile.json",
                "inputs/design-profile-usage.md",
                "inputs/component-recipes.json",
                "inputs/template-style-contract.json",
                "inputs/design-source.md",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                ToolCall::new(
                    format!("source-fallback-read-{index}"),
                    "fs.read",
                    json!({ "path": path }),
                )
            })
            .collect(),
        ),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "source-fallback-init",
            "project.init",
            json!({ "template": "astro-website" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "source-fallback-read-style-contract",
            "fs.read",
            json!({ "path": "state/style-contract.json" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "source-fallback-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "source-fallback-build-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "source-fallback-page",
                "fs.write",
                json!({
                    "path": "project/src/pages/index.astro",
                    "text": "<main><h1>Imported source fallback</h1></main>"
                }),
            ),
            ToolCall::new(
                "source-fallback-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-source-fallback-shot" }),
            ),
            ToolCall::new(
                "source-fallback-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Source fallback website published." }),
            ),
        ]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.enable_design_context_package = true;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model),
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-dcp-source",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": { "briefId": brief_id }
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
    let run_id = payload["runId"].as_str().unwrap().to_string();
    wait_for_terminal(&store, &run_id).await;
    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(
        run.status,
        AgentRunStatus::Completed,
        "source fallback build failed: {run:?} events={:?}",
        store.events(&run_id).await
    );
    assert_eq!(run.design_fidelity_mode.as_deref(), Some("source_fallback"));
    assert_eq!(
        run.design_source_hash.as_deref(),
        Some(artifact.sha256.as_str())
    );
    assert!(run.design_context_materialization_hash.is_some());
    assert!(run
        .design_context_read_files
        .iter()
        .any(|path| path == "inputs/design-source.md"));
    assert!(workspace
        .join("project-dcp-source/project/dist/index.html")
        .exists());
    assert_eq!(
        store
            .read_design_source_artifact_content(&artifact.id)
            .await
            .unwrap(),
        source
    );

    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn public_runtime_dcp_large_source_fallback_reads_required_index_section_before_publish() {
    let workspace = unique_temp_dir("http-lifecycle-dcp-large-source-fallback");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-dcp-large-source".to_string(),
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let mut source = b"# Overview\nA concise imported visual reference.\n\n## Tokens\nUse the verified primary token for the main action.\n\n## Archive\n".to_vec();
    source.extend(std::iter::repeat_n(b'x', 40 * 1024));
    let artifact = store
        .create_design_source_artifact(
            json!({ "projectId": "project-dcp-large-source" }),
            "DESIGN.md".to_string(),
            "text/markdown".to_string(),
            source.clone(),
        )
        .await
        .unwrap();
    let mut profile = dcp_observe_profile("project-dcp-large-source");
    profile.id = "http-lifecycle-dcp-large-source-profile".to_string();
    profile.source = json!({
        "kind": "imported",
        "sourceArtifactIds": [artifact.id],
        "primarySourceArtifactId": artifact.id,
        "sourceHash": artifact.sha256,
        "converterVersion": "test@1",
        "integrity": "verified"
    });
    profile.signature_rules = vec![json!({
        "id": "required-token-source",
        "category": "color",
        "statement": "Read and preserve the imported token evidence.",
        "priority": "required",
        "appliesTo": ["website"],
        "sourceSectionIds": ["section-2-tokens"],
        "verification": {
            "kind": "token",
            "token": "color.primary",
            "expected": "#2563eb",
            "comparator": { "kind": "color-equivalent" }
        }
    })];
    let profile = store.create_design_profile(profile).await.unwrap();
    store
        .bind_project_design_profile("project-dcp-large-source", &profile.id)
        .await
        .unwrap();

    let build_script = r#"const fs=require('fs'); fs.mkdirSync('dist',{recursive:true}); const html=fs.readFileSync('src/pages/index.astro','utf8'); fs.writeFileSync('dist/index.html', html);"#;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(
            [
                "inputs/brief.md",
                "inputs/design-profile.json",
                "inputs/design-profile-usage.md",
                "inputs/component-recipes.json",
                "inputs/template-style-contract.json",
                "inputs/design-source-index.json",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                ToolCall::new(
                    format!("large-source-bootstrap-read-{index}"),
                    "fs.read",
                    json!({ "path": path }),
                )
            })
            .collect(),
        ),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "large-source-read-required-section",
            "design_source.read_sections",
            json!({
                "sourceArtifactId": artifact.id,
                "expectedSourceHash": artifact.sha256,
                "sectionIds": ["section-2-tokens"]
            }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "large-source-init",
            "project.init",
            json!({ "template": "astro-website" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "large-source-read-style-contract",
            "fs.read",
            json!({ "path": "state/style-contract.json" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "large-source-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "large-source-build-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "large-source-page",
                "fs.write",
                json!({
                    "path": "project/src/pages/index.astro",
                    "text": "<main><h1>Indexed imported source</h1></main>"
                }),
            ),
            ToolCall::new(
                "large-source-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-large-source-shot" }),
            ),
            ToolCall::new(
                "large-source-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Indexed source fallback published." }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "large-source-edit-profile",
                "fs.read",
                json!({ "path": "inputs/design-profile.json" }),
            ),
            ToolCall::new(
                "large-source-edit-usage",
                "fs.read",
                json!({ "path": "inputs/design-profile-usage.md" }),
            ),
            ToolCall::new(
                "large-source-edit-recipes",
                "fs.read",
                json!({ "path": "inputs/component-recipes.json" }),
            ),
            ToolCall::new(
                "large-source-edit-index",
                "fs.read",
                json!({ "path": "inputs/design-source-index.json" }),
            ),
            ToolCall::new(
                "large-source-edit-style-contract",
                "fs.read",
                json!({ "path": "state/style-contract.json" }),
            ),
            ToolCall::new(
                "large-source-edit-page",
                "fs.read",
                json!({ "path": "project/src/pages/index.astro" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "large-source-edit-required-section",
            "design_source.read_sections",
            json!({
                "sourceArtifactId": artifact.id,
                "expectedSourceHash": artifact.sha256,
                "sectionIds": ["section-2-tokens"]
            }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "large-source-edit-patch",
                "fs.patch",
                json!({
                    "path": "project/src/pages/index.astro",
                    "oldStr": "Indexed imported source",
                    "newStr": "Indexed imported edit"
                }),
            ),
            ToolCall::new(
                "large-source-edit-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-large-source-edit-shot" }),
            ),
            ToolCall::new(
                "large-source-edit-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Indexed source fallback edit published." }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "large-source-repair-profile",
                "fs.read",
                json!({ "path": "inputs/design-profile.json" }),
            ),
            ToolCall::new(
                "large-source-repair-usage",
                "fs.read",
                json!({ "path": "inputs/design-profile-usage.md" }),
            ),
            ToolCall::new(
                "large-source-repair-recipes",
                "fs.read",
                json!({ "path": "inputs/component-recipes.json" }),
            ),
            ToolCall::new(
                "large-source-repair-index",
                "fs.read",
                json!({ "path": "inputs/design-source-index.json" }),
            ),
            ToolCall::new(
                "large-source-repair-style-contract",
                "fs.read",
                json!({ "path": "state/style-contract.json" }),
            ),
            ToolCall::new(
                "large-source-repair-page",
                "fs.read",
                json!({ "path": "project/src/pages/index.astro" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "large-source-repair-required-section",
            "design_source.read_sections",
            json!({
                "sourceArtifactId": artifact.id,
                "expectedSourceHash": artifact.sha256,
                "sectionIds": ["section-2-tokens"]
            }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "large-source-repair-patch",
                "fs.patch",
                json!({
                    "path": "project/src/pages/index.astro",
                    "oldStr": "Indexed imported edit",
                    "newStr": "Indexed imported repair"
                }),
            ),
            ToolCall::new(
                "large-source-repair-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-large-source-repair-shot" }),
            ),
            ToolCall::new(
                "large-source-repair-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Indexed source fallback repair published." }),
            ),
        ]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.enable_design_context_package = true;
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
                        "projectId": "project-dcp-large-source",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": { "briefId": brief_id }
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
    let run_id = payload["runId"].as_str().unwrap().to_string();
    wait_for_terminal(&store, &run_id).await;
    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(
        run.status,
        AgentRunStatus::Completed,
        "large source fallback build failed: {run:?} events={:?}",
        store.events(&run_id).await
    );
    assert!(run.design_source_size_bytes.unwrap_or_default() > 32 * 1024);
    assert!(run
        .design_context_read_files
        .iter()
        .any(|path| path == "inputs/design-source-index.json"));
    assert!(!run
        .design_context_read_files
        .iter()
        .any(|path| path == "inputs/design-source.md"));
    let required_section = run
        .design_source_sections
        .iter()
        .find(|section| section.id == "section-2-tokens")
        .unwrap();
    assert!(run
        .design_source_read_section_hashes
        .iter()
        .any(|hash| hash == &required_section.sha256));
    assert!(run.design_source_bytes_read <= 16 * 1024);
    assert!(store.events(&run_id).await.iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded {
            name,
            value: 1,
            metadata: Some(metadata),
            ..
        } if name == "design_context_source_sections_read"
            && metadata["accessMode"] == "indexed"
            && metadata["bytesRead"].as_u64().is_some_and(|bytes| bytes > 0)
            && metadata["mode"] == "observe"
            && metadata.get("sourceArtifactId").is_none()
    )));
    assert!(workspace
        .join("project-dcp-large-source/project/dist/index.html")
        .exists());
    assert_eq!(
        store
            .read_design_source_artifact_content(&artifact.id)
            .await
            .unwrap(),
        source
    );

    let build_version_id = run.output_version_id.clone().unwrap();
    let runtime_state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-dcp-large-source/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(runtime_state.status(), StatusCode::OK);
    let runtime_state_body = to_bytes(runtime_state.into_body(), 32 * 1024)
        .await
        .unwrap();
    let runtime_state: Value = serde_json::from_slice(&runtime_state_body).unwrap();
    let edit_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-dcp-large-source",
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "briefId": brief_id,
                            "baseVersionId": build_version_id,
                            "sandboxBindingId": runtime_state["sandboxBindingId"]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit_response.status(), StatusCode::OK);
    let edit_body = to_bytes(edit_response.into_body(), 4096).await.unwrap();
    let edit_payload: Value = serde_json::from_slice(&edit_body).unwrap();
    let edit_run_id = edit_payload["runId"].as_str().unwrap().to_string();
    let continued = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{edit_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Edit the indexed source fallback title." }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(continued.status(), StatusCode::OK);
    wait_for_terminal(&store, &edit_run_id).await;
    let edit = store.get_run(&edit_run_id).await.unwrap();
    assert_eq!(
        edit.status,
        AgentRunStatus::Completed,
        "large source fallback edit failed: {edit:?} events={:?}",
        store.events(&edit_run_id).await
    );
    assert_eq!(
        edit.design_context_content_hash,
        run.design_context_content_hash
    );
    assert!(edit.design_context_materialization_hash.is_some());
    assert_eq!(edit.design_context_style_contract_verified, Some(true));
    assert!(edit
        .design_context_read_files
        .iter()
        .any(|path| path == "state/style-contract.json"));
    assert!(edit
        .design_source_read_section_hashes
        .iter()
        .any(|hash| hash == &required_section.sha256));

    let edit_version_id = edit.output_version_id.clone().unwrap();
    let review = store
        .create_child_run(
            &edit_run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{edit_version_id}")),
            vec![],
        )
        .await
        .unwrap();
    let finding = store
        .record_review_finding(
            "project-dcp-large-source",
            &review.id,
            &edit_version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "Repair the indexed source fallback title",
            None,
            true,
        )
        .await
        .unwrap();
    let repair_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-dcp-large-source",
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": review.id,
                            "findingIds": [finding.id]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repair_response.status(), StatusCode::OK);
    let repair_body = to_bytes(repair_response.into_body(), 4096).await.unwrap();
    let repair_payload: Value = serde_json::from_slice(&repair_body).unwrap();
    let repair_run_id = repair_payload["runId"].as_str().unwrap().to_string();
    wait_for_terminal(&store, &repair_run_id).await;
    let repair = store.get_run(&repair_run_id).await.unwrap();
    assert_eq!(
        repair.status,
        AgentRunStatus::Completed,
        "large source fallback repair failed: {repair:?} events={:?}",
        store.events(&repair_run_id).await
    );
    assert_eq!(
        repair.design_context_content_hash,
        edit.design_context_content_hash
    );
    assert!(repair.design_context_materialization_hash.is_some());
    assert_eq!(repair.design_context_style_contract_verified, Some(true));
    assert!(repair
        .design_context_read_files
        .iter()
        .any(|path| path == "state/style-contract.json"));
    assert!(repair
        .design_source_read_section_hashes
        .iter()
        .any(|hash| hash == &required_section.sha256));
    let project = workspace.join("project-dcp-large-source/project");
    let repaired = fs::read_to_string(project.join("dist/index.html")).unwrap();
    assert!(repaired.contains("Indexed imported repair"));

    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn public_runtime_dcp_build_reads_mutates_and_publishes_real_workspace_output() {
    let workspace = unique_temp_dir("http-lifecycle-dcp-build");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-dcp".to_string(),
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let profile = store
        .create_design_profile(dcp_observe_profile("project-dcp"))
        .await
        .unwrap();
    store
        .bind_project_design_profile("project-dcp", &profile.id)
        .await
        .unwrap();

    let build_script = r#"const fs=require('fs'); fs.mkdirSync('dist',{recursive:true}); const html=fs.readFileSync('src/pages/index.astro','utf8'); const tokens=fs.readFileSync('src/styles/tokens.css','utf8'); fs.writeFileSync('dist/index.html', `${html}\n<style>${tokens}</style>`);"#;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(
            [
                "inputs/brief.md",
                "inputs/design-profile.json",
                "inputs/design-profile-usage.md",
                "inputs/component-recipes.json",
                "inputs/template-style-contract.json",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                ToolCall::new(
                    format!("dcp-bootstrap-read-{index}"),
                    "fs.read",
                    json!({ "path": path }),
                )
            })
            .collect(),
        ),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "dcp-init",
            "project.init",
            json!({ "template": "astro-website" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "dcp-read-style-contract",
            "fs.read",
            json!({ "path": "state/style-contract.json" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "dcp-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "dcp-build-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "dcp-update-token",
                "style.update_tokens",
                json!({ "tokens": { "color.primary": "#663af3" } }),
            ),
            ToolCall::new(
                "dcp-write-page",
                "fs.write",
                json!({
                    "path": "project/src/pages/index.astro",
                    "text": "<main><h1>DCP lifecycle hero</h1></main>"
                }),
            ),
            ToolCall::new(
                "dcp-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-http-shot" }),
            ),
            ToolCall::new(
                "dcp-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "DCP website published." }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "dcp-edit-profile",
                "fs.read",
                json!({ "path": "inputs/design-profile.json" }),
            ),
            ToolCall::new(
                "dcp-edit-usage",
                "fs.read",
                json!({ "path": "inputs/design-profile-usage.md" }),
            ),
            ToolCall::new(
                "dcp-edit-recipes",
                "fs.read",
                json!({ "path": "inputs/component-recipes.json" }),
            ),
            ToolCall::new(
                "dcp-edit-style-contract",
                "fs.read",
                json!({ "path": "state/style-contract.json" }),
            ),
            ToolCall::new(
                "dcp-edit-source",
                "fs.read",
                json!({ "path": "project/src/pages/index.astro" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "dcp-edit-patch",
                "fs.patch",
                json!({
                    "path": "project/src/pages/index.astro",
                    "oldStr": "DCP lifecycle hero",
                    "newStr": "DCP inherited edit"
                }),
            ),
            ToolCall::new(
                "dcp-edit-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-http-edit-shot" }),
            ),
            ToolCall::new(
                "dcp-edit-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "DCP inherited edit published." }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "dcp-repair-usage",
                "fs.read",
                json!({ "path": "inputs/design-profile-usage.md" }),
            ),
            ToolCall::new(
                "dcp-repair-recipes",
                "fs.read",
                json!({ "path": "inputs/component-recipes.json" }),
            ),
            ToolCall::new(
                "dcp-repair-style-contract",
                "fs.read",
                json!({ "path": "state/style-contract.json" }),
            ),
            ToolCall::new(
                "dcp-repair-source",
                "fs.read",
                json!({ "path": "project/src/pages/index.astro" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "dcp-repair-patch",
                "fs.patch",
                json!({
                    "path": "project/src/pages/index.astro",
                    "oldStr": "DCP inherited edit",
                    "newStr": "DCP repaired hero"
                }),
            ),
            ToolCall::new(
                "dcp-repair-publish",
                "preview.publish",
                json!({ "screenshotId": "dcp-http-repair-shot" }),
            ),
            ToolCall::new(
                "dcp-repair-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "DCP repair published." }),
            ),
        ]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.enable_design_context_package = true;
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
                        "projectId": "project-dcp",
                        "phase": "build",
                        "agentProfile": "build",
                        "inputContext": { "briefId": brief_id }
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
    let run_id = payload["runId"].as_str().unwrap().to_string();
    wait_for_terminal(&store, &run_id).await;

    let run = store.get_run(&run_id).await.unwrap();
    assert_eq!(
        run.status,
        AgentRunStatus::Completed,
        "DCP build failed: {run:?} events={:?}",
        store.events(&run_id).await
    );
    assert!(run.design_context_content_hash.is_some());
    assert!(run.design_context_materialization_hash.is_some());
    assert_eq!(run.design_context_style_contract_verified, Some(true));
    assert_eq!(
        run.design_context_effective_compatibility_mode.as_deref(),
        Some("observe")
    );
    for path in [
        "inputs/brief.md",
        "inputs/design-profile.json",
        "inputs/design-profile-usage.md",
        "inputs/component-recipes.json",
        "inputs/template-style-contract.json",
        "state/style-contract.json",
    ] {
        assert!(
            run.design_context_read_files
                .iter()
                .any(|read| read == path),
            "missing DCP read {path}: {:?}",
            run.design_context_read_files
        );
    }
    assert!(run.output_version_id.is_some());
    let project = workspace.join("project-dcp/project");
    let source = fs::read_to_string(project.join("src/pages/index.astro")).unwrap();
    let output = fs::read_to_string(project.join("dist/index.html")).unwrap();
    let tokens = fs::read_to_string(project.join("src/styles/tokens.css")).unwrap();
    assert!(source.contains("DCP lifecycle hero"));
    assert!(output.contains("DCP lifecycle hero"));
    assert!(output.contains("--runtime-primary: #663af3;"));
    assert!(tokens.contains("--runtime-primary: #663af3;"));
    let fidelity: Value = serde_json::from_str(
        &fs::read_to_string(workspace.join("project-dcp/state/design-profile-fidelity.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(fidelity["requiredFailedRuleIds"], json!([]));

    let build_version_id = run.output_version_id.clone().unwrap();
    let runtime_state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-dcp/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(runtime_state.status(), StatusCode::OK);
    let runtime_state_body = to_bytes(runtime_state.into_body(), 32 * 1024)
        .await
        .unwrap();
    let runtime_state: Value = serde_json::from_slice(&runtime_state_body).unwrap();
    let edit_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-dcp",
                        "phase": "edit",
                        "agentProfile": "edit",
                        "inputContext": {
                            "briefId": brief_id,
                            "baseVersionId": build_version_id,
                            "sandboxBindingId": runtime_state["sandboxBindingId"]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit_response.status(), StatusCode::OK);
    let edit_body = to_bytes(edit_response.into_body(), 4096).await.unwrap();
    let edit_payload: Value = serde_json::from_slice(&edit_body).unwrap();
    let edit_run_id = edit_payload["runId"].as_str().unwrap().to_string();
    assert_eq!(
        store.get_run(&edit_run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );
    let continued = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{edit_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Change the DCP lifecycle title and publish." })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(continued.status(), StatusCode::OK);
    wait_for_terminal(&store, &edit_run_id).await;
    let edit = store.get_run(&edit_run_id).await.unwrap();
    assert_eq!(
        edit.status,
        AgentRunStatus::Completed,
        "DCP inherited edit failed: {edit:?} events={:?}",
        store.events(&edit_run_id).await
    );
    assert_eq!(
        edit.design_context_content_hash,
        run.design_context_content_hash
    );
    assert!(edit.design_context_materialization_hash.is_some());
    assert_eq!(edit.design_context_style_contract_verified, Some(true));
    for path in [
        "inputs/design-profile.json",
        "inputs/design-profile-usage.md",
        "inputs/component-recipes.json",
        "state/style-contract.json",
    ] {
        assert!(
            edit.design_context_read_files
                .iter()
                .any(|read| read == path),
            "missing inherited DCP read {path}: {:?}",
            edit.design_context_read_files
        );
    }
    assert_ne!(edit.output_version_id, Some(build_version_id));
    let edited_source = fs::read_to_string(project.join("src/pages/index.astro")).unwrap();
    let edited_output = fs::read_to_string(project.join("dist/index.html")).unwrap();
    assert!(edited_source.contains("DCP inherited edit"));
    assert!(edited_output.contains("DCP inherited edit"));

    let edit_version_id = edit.output_version_id.clone().unwrap();
    let review = store
        .create_child_run(
            &edit_run_id,
            AgentPhase::Review,
            "visual-review".to_string(),
            "internal-fast".to_string(),
            Some(format!("preview.candidate:{edit_version_id}")),
            vec![],
        )
        .await
        .unwrap();
    let finding = store
        .record_review_finding(
            "project-dcp",
            &review.id,
            &edit_version_id,
            ReviewFindingSeverity::Blocking,
            ReviewFindingCategory::Visual,
            "Repair the DCP lifecycle hero copy",
            None,
            true,
        )
        .await
        .unwrap();
    let repair_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "projectId": "project-dcp",
                        "phase": "repair",
                        "agentProfile": "repair",
                        "inputContext": {
                            "parentRunId": review.id,
                            "findingIds": [finding.id]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repair_response.status(), StatusCode::OK);
    let repair_body = to_bytes(repair_response.into_body(), 4096).await.unwrap();
    let repair_payload: Value = serde_json::from_slice(&repair_body).unwrap();
    let repair_run_id = repair_payload["runId"].as_str().unwrap().to_string();
    wait_for_terminal(&store, &repair_run_id).await;
    let repair = store.get_run(&repair_run_id).await.unwrap();
    assert_eq!(
        repair.status,
        AgentRunStatus::Completed,
        "DCP repair run failed: {repair:?} events={:?}",
        store.events(&repair_run_id).await
    );
    assert_eq!(
        repair.design_context_content_hash,
        edit.design_context_content_hash
    );
    assert!(repair.design_context_materialization_hash.is_some());
    assert_eq!(repair.design_context_style_contract_verified, Some(true));
    for path in [
        "inputs/design-profile-usage.md",
        "inputs/component-recipes.json",
        "state/style-contract.json",
    ] {
        assert!(
            repair
                .design_context_read_files
                .iter()
                .any(|read| read == path),
            "missing repair DCP read {path}: {:?}",
            repair.design_context_read_files
        );
    }
    let repaired_source = fs::read_to_string(project.join("src/pages/index.astro")).unwrap();
    let repaired_output = fs::read_to_string(project.join("dist/index.html")).unwrap();
    assert!(repaired_source.contains("DCP repaired hero"));
    assert!(repaired_output.contains("DCP repaired hero"));

    fs::remove_dir_all(workspace).unwrap();
}

#[tokio::test]
async fn public_runtime_lifecycle_build_runtime_state_edit_and_rebuilds() {
    let workspace = unique_temp_dir("http-lifecycle-edit");
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let (preview_url, _preview_server) = start_preview_server().await;
    let build_script = r#"const fs=require('fs'); fs.mkdirSync('dist',{recursive:true}); const html=fs.readFileSync('src/pages/index.astro','utf8'); const tokens=fs.existsSync('src/styles/tokens.css')?fs.readFileSync('src/styles/tokens.css','utf8'):''; fs.writeFileSync('dist/index.html', `${html}\n<style>${tokens}</style>`);"#;
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "build-init",
                "project.init",
                json!({ "template": "astro-website" }),
            ),
            ToolCall::new(
                "build-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "build-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "build-tokens",
                "fs.write",
                json!({
                    "path": "project/src/styles/tokens.css",
                    "text": ":root {\n  --runtime-primary: #2563eb;\n}\n"
                }),
            ),
            ToolCall::new(
                "build-page",
                "fs.write",
                json!({ "path": "project/src/pages/index.astro", "text": "<main><h1>Initial hero</h1></main>" }),
            ),
            ToolCall::new("build-run", "project.build", json!({ "cwd": "project" })),
            ToolCall::new(
                "build-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "build-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "build-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-build", "blank": false }),
            ),
            ToolCall::new(
                "build-candidate",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-build"
                }),
            ),
            ToolCall::new(
                "build-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Initial preview promoted" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "edit-read",
                "fs.read",
                json!({ "path": "project/src/pages/index.astro" }),
            ),
            ToolCall::new(
                "edit-patch",
                "fs.patch",
                json!({
                    "path": "project/src/pages/index.astro",
                    "oldStr": "Initial hero",
                    "newStr": "Edited hero"
                }),
            ),
            ToolCall::new("edit-build", "project.build", json!({ "cwd": "project" })),
            ToolCall::new(
                "edit-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "edit-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "edit-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-edit", "blank": false }),
            ),
            ToolCall::new(
                "edit-candidate",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-edit"
                }),
            ),
            ToolCall::new(
                "edit-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Edited preview promoted" }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "theme-style-update",
                "style.update_tokens",
                json!({
                    "tokens": {
                        "color.primary": "#f97316"
                    }
                }),
            ),
            ToolCall::new("theme-build", "project.build", json!({ "cwd": "project" })),
            ToolCall::new(
                "theme-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "theme-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "theme-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-theme-edit", "blank": false }),
            ),
            ToolCall::new(
                "theme-candidate",
                "preview.report_candidate",
                json!({
                    "url": preview_url,
                    "screenshotId": "shot-theme-edit"
                }),
            ),
            ToolCall::new(
                "theme-complete",
                "run.complete",
                json!({ "status": "completed", "summary": "Theme preview promoted" }),
            ),
        ]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model),
    });

    let build_response = app
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
                            "briefId": brief_id
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(build_response.status(), StatusCode::OK);
    let body = to_bytes(build_response.into_body(), 4096).await.unwrap();
    let build_payload: Value = serde_json::from_slice(&body).unwrap();
    let build_run_id = build_payload["runId"].as_str().unwrap().to_string();
    wait_for_terminal(&store, &build_run_id).await;
    let build_run = store.get_run(&build_run_id).await.unwrap();
    assert_eq!(
        build_run.status,
        AgentRunStatus::Completed,
        "build run failed: {build_run:?} events={:?}",
        store.events(&build_run_id).await
    );
    let initial_version_id = build_run.output_version_id.clone().unwrap();

    let runtime_state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(runtime_state.status(), StatusCode::OK);
    let body = to_bytes(runtime_state.into_body(), 32 * 1024)
        .await
        .unwrap();
    let runtime_state: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(runtime_state["currentVersionId"], initial_version_id);
    assert_eq!(runtime_state["templateKey"], "astro-website");
    fs::write(
        workspace.join("project-1/project/src/pages/index.astro"),
        "<main><h1>Corrupted workspace</h1></main>",
    )
    .unwrap();

    let edit_response = app
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
                            "briefId": brief_id,
                            "baseVersionId": runtime_state["currentVersionId"],
                            "sandboxBindingId": runtime_state["sandboxBindingId"]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(edit_response.status(), StatusCode::OK);
    let body = to_bytes(edit_response.into_body(), 4096).await.unwrap();
    let edit_payload: Value = serde_json::from_slice(&body).unwrap();
    let edit_run_id = edit_payload["runId"].as_str().unwrap().to_string();
    assert_eq!(
        store.get_run(&edit_run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );

    let continue_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{edit_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Change the hero title to Edited hero" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(continue_response.status(), StatusCode::OK);
    wait_for_terminal(&store, &edit_run_id).await;
    let edit_run = store.get_run(&edit_run_id).await.unwrap();
    assert_eq!(edit_run.status, AgentRunStatus::Completed);
    let edited_version_id = edit_run.output_version_id.clone().unwrap();
    assert_ne!(edited_version_id, initial_version_id);
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        edited_version_id
    );
    let html = fs::read_to_string(workspace.join("project-1/project/dist/index.html")).unwrap();
    assert!(html.contains("Edited hero"));
    assert!(!html.contains("Initial hero"));
    assert!(store
        .events(&edit_run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "preview.updated"));

    let hero_runtime_state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/runtime-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hero_runtime_state.status(), StatusCode::OK);
    let body = to_bytes(hero_runtime_state.into_body(), 32 * 1024)
        .await
        .unwrap();
    let hero_runtime_state: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(hero_runtime_state["currentVersionId"], edited_version_id);
    assert_eq!(
        hero_runtime_state["styleContract"]["tokens"]["color.primary"],
        "--runtime-primary"
    );

    let theme_response = app
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
                            "briefId": brief_id,
                            "baseVersionId": hero_runtime_state["currentVersionId"],
                            "sandboxBindingId": hero_runtime_state["sandboxBindingId"]
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(theme_response.status(), StatusCode::OK);
    let body = to_bytes(theme_response.into_body(), 4096).await.unwrap();
    let theme_payload: Value = serde_json::from_slice(&body).unwrap();
    let theme_run_id = theme_payload["runId"].as_str().unwrap().to_string();
    assert_eq!(
        store.get_run(&theme_run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );

    let continue_theme_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{theme_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Change the primary theme color token to #f97316" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(continue_theme_response.status(), StatusCode::OK);
    wait_for_terminal(&store, &theme_run_id).await;
    let theme_run = store.get_run(&theme_run_id).await.unwrap();
    assert_eq!(
        theme_run.status,
        AgentRunStatus::Completed,
        "theme run failed: {theme_run:?} events={:?}",
        store.events(&theme_run_id).await
    );
    let theme_version_id = theme_run.output_version_id.clone().unwrap();
    assert_ne!(theme_version_id, edited_version_id);
    assert_eq!(
        store.current_project_version("project-1").await.unwrap().id,
        theme_version_id
    );
    let tokens =
        fs::read_to_string(workspace.join("project-1/project/src/styles/tokens.css")).unwrap();
    assert!(tokens.contains("--runtime-primary: #f97316;"));
    assert!(!tokens.contains("--runtime-primary: #2563eb;"));
    let themed_html =
        fs::read_to_string(workspace.join("project-1/project/dist/index.html")).unwrap();
    assert!(themed_html.contains("Edited hero"));
    assert!(themed_html.contains("--runtime-primary: #f97316;"));
    assert!(store
        .events(&theme_run_id)
        .await
        .iter()
        .any(|event| serde_json::to_value(event).unwrap()["type"] == "preview.updated"));
}

#[tokio::test]
async fn phase_a_public_run_uses_project_scoped_workspace_root() {
    let workspace = unique_temp_dir("http-project-workspace-isolation");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-a".to_string(),
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
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let model = MockModelClient::new(
        (0..8)
            .map(|index| {
                ModelResponse::ToolCalls(vec![
                    ToolCall::new(
                        format!("init-website-{index}"),
                        "project.init",
                        json!({ "template": "astro-website" }),
                    ),
                    ToolCall::new(
                        format!("complete-website-{index}"),
                        "run.complete",
                        json!({ "status": "completed", "summary": "website initialized" }),
                    ),
                ])
            })
            .collect(),
    );
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model),
    });

    let website_run_id = start_public_run(
        app.clone(),
        "project-a",
        "build",
        json!({ "briefId": brief_id }),
    )
    .await;
    assert!(
        wait_for_terminal_with_timeout(&store, &website_run_id, 5).await,
        "website run should finish"
    );

    assert!(workspace
        .join("project-a/project/src/pages/index.astro")
        .exists());
    assert!(!workspace.join("project").exists());
    assert!(!workspace.join("project-a/project/app").exists());
}
