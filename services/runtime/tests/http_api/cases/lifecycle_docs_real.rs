use super::*;
use anydesign_runtime::design_profile_service::{CreateProfileCommand, DesignProfileService};
use anydesign_runtime::types::AgentRun;

#[tokio::test]
async fn public_runtime_docs_lifecycle_build_runtime_state_edit_and_rebuilds() {
    let workspace = unique_temp_dir("http-docs-lifecycle-edit");
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "docs-project".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief(&brief_run.id, docs_brief())
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let (preview_url, _preview_server) = start_preview_server().await;
    let build_script = "const fs=require('fs'); fs.mkdirSync('out/docs',{recursive:true}); const mdx=fs.readFileSync('content/docs/index.mdx','utf8'); const head='<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Runtime docs lifecycle</title><style>body{font-family:sans-serif;max-width:100%;overflow-x:hidden}</style>'; const navigation='<nav><a href=\"/\">Home</a><a href=\"/docs/\">Docs</a></nav><label>Search <input type=\"search\" aria-label=\"Search docs\"></label>'; fs.writeFileSync('out/docs/index.html', `<!doctype html><html lang=\"en\"><head>${head}</head><body>${navigation}<main><h1 id=\"overview\">Overview</h1><p>${mdx}</p></main></body></html>`); fs.writeFileSync('out/index.html', `<!doctype html><html lang=\"en\"><head>${head}</head><body>${navigation}<main><h1>Docs lifecycle</h1></main></body></html>`);";
    let model = MockModelClient::new(vec![
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-init",
            "project.init",
            json!({ "template": "fumadocs-docs" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "docs-package",
                "fs.write",
                json!({
                    "path": "project/package.json",
                    "text": serde_json::to_string_pretty(&json!({
                        "scripts": { "build": "node build.cjs" }
                    })).unwrap()
                }),
            ),
            ToolCall::new(
                "docs-script",
                "fs.write",
                json!({ "path": "project/build.cjs", "text": build_script }),
            ),
            ToolCall::new(
                "docs-mdx",
                "fs.write",
                json!({
                    "path": "project/content/docs/index.mdx",
                    "text": "---\ntitle: Overview\n---\n\n# Initial docs title\n\nInitial lifecycle section"
                }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-build",
            "project.build",
            json!({ "cwd": "project" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "docs-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "docs-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "docs-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-docs-build", "blank": false }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-candidate",
            "preview.publish",
            json!({
                "url": preview_url,
                "screenshotId": "shot-docs-build"
            }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Docs preview promoted" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-edit-read",
            "fs.read",
            json!({ "path": "project/content/docs/index.mdx" }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-edit-patch",
            "fs.patch",
            json!({
                "path": "project/content/docs/index.mdx",
                "oldStr": "Initial docs title",
                "newStr": "Edited docs title"
            }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-edit-build",
            "project.build",
            json!({ "cwd": "project" }),
        )]),
        ModelResponse::ToolCalls(vec![
            ToolCall::new(
                "docs-edit-preview",
                "preview.start",
                json!({ "url": preview_url, "port": 4321 }),
            ),
            ToolCall::new(
                "docs-edit-browser",
                "browser.open",
                json!({ "url": preview_url }),
            ),
            ToolCall::new(
                "docs-edit-shot",
                "browser.screenshot",
                json!({ "screenshotId": "shot-docs-edit", "blank": false }),
            ),
        ]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-edit-candidate",
            "preview.publish",
            json!({
                "url": preview_url,
                "screenshotId": "shot-docs-edit"
            }),
        )]),
        ModelResponse::ToolCalls(vec![ToolCall::new(
            "docs-edit-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Edited docs preview promoted" }),
        )]),
    ]);
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    // The real-provider Website leg is also the DCP model-adherence gate.
    // Docs remains on its existing contract so the matrix preserves both
    // surfaces without silently broadening Docs scope.
    config.enable_design_context_package = true;
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
                        "projectId": "docs-project",
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
        "docs build failed: {build_run:?} events={:?}",
        store.events(&build_run_id).await
    );
    let initial_version_id = build_run.output_version_id.clone().unwrap();

    let runtime_state = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/docs-project/runtime-state")
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
    assert_eq!(runtime_state["templateKey"], "fumadocs-docs");
    fs::write(
        workspace.join("docs-project/project/content/docs/index.mdx"),
        "# Corrupted docs title\n\nCorrupted content",
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
                        "projectId": "docs-project",
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
    let continue_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/runs/{edit_run_id}/continue"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "userMessage": "Rename the overview page to Edited docs title" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(continue_response.status(), StatusCode::OK);
    wait_for_terminal(&store, &edit_run_id).await;
    let edit_run = store.get_run(&edit_run_id).await.unwrap();
    assert_eq!(
        edit_run.status,
        AgentRunStatus::Completed,
        "docs edit failed: {edit_run:?} events={:?}",
        store.events(&edit_run_id).await
    );
    let edited_version_id = edit_run.output_version_id.clone().unwrap();
    assert_ne!(edited_version_id, initial_version_id);
    assert_eq!(
        store
            .current_project_version("docs-project")
            .await
            .unwrap()
            .id,
        edited_version_id
    );
    let mdx =
        fs::read_to_string(workspace.join("docs-project/project/content/docs/index.mdx")).unwrap();
    assert!(mdx.contains("Edited docs title"));
    assert!(!mdx.contains("Initial docs title"));
    let html =
        fs::read_to_string(workspace.join("docs-project/project/out/docs/index.html")).unwrap();
    assert!(html.contains("Edited docs title"));
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY, network access, and npm registry access"]
async fn real_provider_public_runtime_website_and_docs_lifecycle_matrix() {
    let api_key =
        std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY must be set for this test");
    let _approval_reference = std::env::var("RUNTIME_PROVIDER_APPROVAL_ID")
        .expect("RUNTIME_PROVIDER_APPROVAL_ID must be set for this test");
    let base_url =
        std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".into());
    let model_name = std::env::var("DEEPSEEK_E2E_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
    let workspace = unique_temp_dir("real-provider-http-lifecycle");
    let store = RuntimeStore::with_checkpoint_dir(workspace.join("state/checkpoints"));
    let project_filter = std::env::var("REAL_PROVIDER_PROJECT_FILTER").ok();
    let website_profile = if project_filter.as_deref() != Some("docs") {
        let mut profile_payload =
            design_profile_request("real-http-website", vec!["astro-website"])["profile"]
                .as_object()
                .unwrap()
                .clone();
        profile_payload.insert(
            "websiteContext".to_string(),
            json!({
                "enforcementMode": "enforced",
                "craftPacks": ["accessibility-baseline", "responsive-layout"]
            }),
        );
        let profiles = DesignProfileService::new(store.clone());
        let profile = profiles
            .create(CreateProfileCommand {
                project_id: Some("real-http-website".to_string()),
                name: "Real provider enforced DCP Website".to_string(),
                payload: profile_payload,
            })
            .await
            .unwrap();
        profiles
            .bind_project("real-http-website", &profile.id)
            .await
            .unwrap();
        Some(profile)
    } else {
        None
    };
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.agent_model = model_name;
    config.policy_profile = RuntimePolicyProfile::LocalE2e;
    config.npm_registry = std::env::var("RUNTIME_E2E_NPM_REGISTRY")
        .unwrap_or_else(|_| "https://registry.npmjs.org/".to_string());
    if let Some(profile) = website_profile.as_ref() {
        config.enable_design_context_package = true;
        config.enable_design_context_enforcement = true;
        config.design_context_enforcement_allowlist_json = Some(
            json!([{
                "projectId": "real-http-website",
                "designProfileId": profile.id,
                "designProfileVersion": profile.version,
            }])
            .to_string(),
        );
    }
    let model = OpenAiCompatibleModelClient::new(base_url, api_key, Some("deepseek"))
        .with_streaming(env_flag("MODEL_STREAMING"))
        .with_strict_tools(env_flag("MODEL_STRICT_TOOLS"));
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model),
    });
    if project_filter.as_deref() != Some("docs") {
        run_real_provider_lifecycle_project(
            app.clone(),
            &store,
            &workspace,
            "real-http-website",
            website_brief(),
            vec![
                ContentSource::readable(
                    "website-design",
                    "design_md",
                    "# Website design\n- Build a compact operational SaaS website for runtime harness engineers.\n- Use Tailwind/token styling and local UI primitives from the runtime template.\n- Hero title should initially include Runtime Harness.\n- Include sections for lifecycle, typed recovery, preview promotion, and evidence.\n",
                ),
                ContentSource::readable(
                    "website-instructions",
                    "prompt",
                    "Use project.init with astro-website if needed, inspect the project, and use preview.publish for build/screenshot/candidate/promotion. Prefer style.update_tokens for color edits. Do not use raw npm/pnpm install commands through shell.run.",
                ),
            ],
            "Acceptance criteria: the promoted website artifact must contain the literal text TESTXXX in the hero title. Change the hero title to TESTXXX 标题内容, set the primary theme color token to #f97316 using style.update_tokens when possible, then rebuild and promote with preview.publish exactly once. Do not call run.complete until the served artifact contains TESTXXX.",
            "/artifacts/real-http-website/current/",
            "project/dist/index.html",
            "TESTXXX",
            true,
        )
        .await;
    }

    if project_filter.as_deref() != Some("website") {
        run_real_provider_lifecycle_project(
            app,
            &store,
            &workspace,
            "real-http-docs",
            docs_brief(),
            vec![
                ContentSource::readable(
                    "docs-design",
                    "design_md",
                    "# Docs design\n- Build a Fumadocs documentation portal for runtime lifecycle operations.\n- The overview page should explain create, generate, edit, build, screenshot, and promote.\n- Include a section on typed recoverable errors and preview evidence.\n",
                ),
                ContentSource::readable(
                    "docs-instructions",
                    "prompt",
                    "Use project.init with fumadocs-docs if needed, inspect the project, and use preview.publish for build/screenshot/candidate/promotion. Keep Docs source editable and tokenized.",
                ),
            ],
            "Rename the overview page to Edited docs title and add one short section about browser computed-style verification. Rebuild and promote with preview.publish.",
            "/artifacts/real-http-docs/current/docs",
            "project/out/docs.html",
            "Edited docs title",
            false,
        )
        .await;
    }
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY, network access, and npm registry access"]
async fn real_provider_attachment_docs_lifecycle() {
    let api_key =
        std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY must be set for this test");
    let _approval_reference = std::env::var("RUNTIME_PROVIDER_APPROVAL_ID")
        .expect("RUNTIME_PROVIDER_APPROVAL_ID must be set for this test");
    let base_url =
        std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".into());
    let model_name = std::env::var("DEEPSEEK_E2E_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
    let workspace = unique_temp_dir("real-provider-attachment-docs");
    let store = RuntimeStore::with_checkpoint_dir(workspace.join("state/checkpoints"));
    let mut config = phase_a_contract_config();
    config.workspace_root = workspace.clone();
    config.agent_model = model_name;
    config.policy_profile = RuntimePolicyProfile::LocalE2e;
    config.npm_registry = std::env::var("RUNTIME_E2E_NPM_REGISTRY")
        .unwrap_or_else(|_| "https://registry.npmjs.org/".to_string());
    let model = OpenAiCompatibleModelClient::new(base_url, api_key, Some("deepseek"))
        .with_streaming(env_flag("MODEL_STREAMING"))
        .with_strict_tools(env_flag("MODEL_STRICT_TOOLS"));
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config,
        store: store.clone(),
        model: Arc::new(model),
    });

    run_real_provider_lifecycle_project(
        app,
        &store,
        &workspace,
        "real-attachment-docs",
        docs_brief(),
        vec![ContentSource::readable(
            "attachment-aurora-workspace-guide",
            "attachment_text",
            include_str!("../../fixtures/attachment-docs-product-guide.md"),
        )],
        "Update the existing quickstart section so the served documentation contains the exact heading Attachment Docs: Edited Quickstart. Keep the rest of the attached guide intact, then rebuild and promote with preview.publish.",
        "/artifacts/real-attachment-docs/current/docs",
        "project/out/docs.html",
        "Attachment Docs: Edited Quickstart",
        false,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn run_real_provider_lifecycle_project(
    app: axum::Router,
    store: &RuntimeStore,
    workspace_root: &Path,
    project_id: &str,
    brief: Brief,
    content_sources: Vec<ContentSource>,
    edit_prompt: &str,
    artifact_path: &str,
    local_artifact_relative: &str,
    expected_artifact_text: &str,
    require_dcp: bool,
) {
    let brief_run = store
        .create_run(
            project_id.to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store.write_brief(&brief_run.id, brief).await.unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();

    let build_run_id = start_public_run(
        app.clone(),
        project_id,
        "build",
        json!({
            "briefId": brief_id,
            "contentSources": content_sources
        }),
    )
    .await;
    if !wait_for_terminal_with_timeout(store, &build_run_id, REAL_PROVIDER_STAGE_TIMEOUT_SECS).await
    {
        emit_real_provider_run_stream(store, &build_run_id, project_id, "build").await;
        panic!(
            "run {build_run_id} did not reach terminal status within {REAL_PROVIDER_STAGE_TIMEOUT_SECS}s"
        );
    }
    emit_real_provider_run_stream(store, &build_run_id, project_id, "build").await;
    let build_run = store.get_run(&build_run_id).await.unwrap();
    assert_eq!(
        build_run.status,
        AgentRunStatus::Completed,
        "real provider build run {build_run_id} should complete; events={}",
        serde_json::to_string(&store.events(&build_run_id).await).unwrap()
    );
    assert_preview_updated_before_completed(store, &build_run_id).await;
    let build_dcp_evidence = if require_dcp {
        real_provider_dcp_evidence(&build_run, AgentPhase::Build)
    } else {
        Value::Null
    };
    let initial_version_id = build_run.output_version_id.clone().unwrap();

    let runtime_state = get_json(
        app.clone(),
        &format!("/projects/{project_id}/runtime-state"),
        8192,
    )
    .await;
    assert_eq!(runtime_state["currentVersionId"], initial_version_id);
    let initial_snapshot = runtime_state["sourceSnapshotUri"]
        .as_str()
        .expect("runtime state should include source snapshot")
        .to_string();
    assert_eq!(
        runtime_state["latestBuild"]["sourceSnapshotUri"], initial_snapshot,
        "build runtime-state latestBuild sourceSnapshotUri should match promoted sourceSnapshotUri"
    );
    let build_current_preview =
        get_json(app.clone(), &format!("/preview/{project_id}/current"), 8192).await;
    assert_eq!(build_current_preview["versionId"], initial_version_id);
    assert_eq!(build_current_preview["status"], "promoted");
    let build_artifact = get_text(app.clone(), artifact_path, 256_000).await;
    let build_artifact_byte_length = build_artifact.len();
    assert!(
        build_artifact_byte_length > 0,
        "build artifact {artifact_path} should be non-empty"
    );
    let build_local_artifact_url =
        local_artifact_url(workspace_root, project_id, local_artifact_relative);
    emit_real_provider_evidence(
        project_id,
        "build",
        &build_run_id,
        json!({
            "runtimeState": runtime_state.clone(),
            "currentPreview": build_current_preview,
            "sourceSnapshotUri": initial_snapshot,
            "artifactPath": artifact_path,
            "localArtifactUrl": build_local_artifact_url,
            "artifactServed": true,
            "artifactByteLength": build_artifact_byte_length,
            "previewUpdatedBeforeCompleted": true,
            "designContext": build_dcp_evidence,
        }),
    );

    let edit_run_id = start_public_run(
        app.clone(),
        project_id,
        "edit",
        json!({
            "briefId": brief_id,
            "baseVersionId": runtime_state["currentVersionId"],
            "sandboxBindingId": runtime_state["sandboxBindingId"]
        }),
    )
    .await;
    post_continue(app.clone(), &edit_run_id, edit_prompt).await;
    if !wait_for_terminal_with_timeout(store, &edit_run_id, REAL_PROVIDER_STAGE_TIMEOUT_SECS).await
    {
        emit_real_provider_run_stream(store, &edit_run_id, project_id, "edit").await;
        panic!(
            "run {edit_run_id} did not reach terminal status within {REAL_PROVIDER_STAGE_TIMEOUT_SECS}s"
        );
    }
    emit_real_provider_run_stream(store, &edit_run_id, project_id, "edit").await;
    let edit_run = store.get_run(&edit_run_id).await.unwrap();
    assert_eq!(
        edit_run.status,
        AgentRunStatus::Completed,
        "real provider edit run {edit_run_id} should complete; events={}",
        serde_json::to_string(&store.events(&edit_run_id).await).unwrap()
    );
    assert_preview_updated_before_completed(store, &edit_run_id).await;
    let edit_dcp_evidence = if require_dcp {
        assert_eq!(
            edit_run.design_context_content_hash,
            build_run.design_context_content_hash,
            "real provider Edit must inherit the frozen Build DCP rather than recompile the current profile"
        );
        real_provider_dcp_evidence(&edit_run, AgentPhase::Edit)
    } else {
        Value::Null
    };
    let edited_version_id = edit_run.output_version_id.clone().unwrap();
    assert_ne!(edited_version_id, initial_version_id);

    let edited_state = get_json(
        app.clone(),
        &format!("/projects/{project_id}/runtime-state"),
        8192,
    )
    .await;
    assert_eq!(edited_state["currentVersionId"], edited_version_id);
    let edited_snapshot = edited_state["sourceSnapshotUri"]
        .as_str()
        .expect("edited runtime state should include source snapshot")
        .to_string();
    assert_eq!(
        edited_state["latestBuild"]["sourceSnapshotUri"], edited_snapshot,
        "edit runtime-state latestBuild sourceSnapshotUri should match promoted sourceSnapshotUri"
    );
    let source_snapshot_changed = edited_snapshot != initial_snapshot;
    assert!(source_snapshot_changed);

    let current_preview =
        get_json(app.clone(), &format!("/preview/{project_id}/current"), 8192).await;
    assert_eq!(current_preview["versionId"], edited_version_id);
    assert_eq!(current_preview["status"], "promoted");

    let artifact = get_text(app.clone(), artifact_path, 256_000).await;
    let artifact_byte_length = artifact.len();
    let artifact_contains_expected = artifact.contains(expected_artifact_text);
    let edited_local_artifact_url =
        local_artifact_url(workspace_root, project_id, local_artifact_relative);
    emit_real_provider_evidence(
        project_id,
        "edit",
        &edit_run_id,
        json!({
            "runtimeState": edited_state,
            "currentPreview": current_preview,
            "initialVersionId": initial_version_id,
            "editedVersionId": edited_version_id,
            "sourceSnapshotUri": edited_snapshot,
            "initialSourceSnapshotUri": initial_snapshot,
            "editedSourceSnapshotUri": edited_snapshot,
            "sourceSnapshotChanged": source_snapshot_changed,
            "artifactPath": artifact_path,
            "localArtifactUrl": edited_local_artifact_url,
            "artifactServed": true,
            "artifactByteLength": artifact_byte_length,
            "artifactContainsExpectedText": artifact_contains_expected,
            "artifactContainsEditMarker": artifact_contains_expected,
            "expectedArtifactText": expected_artifact_text,
            "previewUpdatedBeforeCompleted": true,
            "designContext": edit_dcp_evidence,
        }),
    );
    assert!(
        artifact_contains_expected,
        "artifact {artifact_path} should include edited text {expected_artifact_text:?}; body preview={}",
        artifact.chars().take(1000).collect::<String>()
    );

    if require_dcp {
        let review = store
            .create_child_run(
                &edit_run_id,
                AgentPhase::Review,
                "real-provider-auditor-review".to_string(),
                "internal-fast".to_string(),
                Some(format!("preview.candidate:{edited_version_id}")),
                vec![],
            )
            .await
            .unwrap();
        let repair_expected_text = "TESTXXX REPAIRED PROVIDER TITLE";
        let finding = store
            .record_review_finding(
                project_id,
                &review.id,
                &edited_version_id,
                ReviewFindingSeverity::Blocking,
                ReviewFindingCategory::Visual,
                &format!(
                    "Replace the hero title with the exact text {repair_expected_text}; preserve the primary token #f97316, rebuild, verify the served artifact, and publish the repaired candidate"
                ),
                None,
                true,
            )
            .await
            .unwrap();
        let repair_run_id = start_public_run(
            app.clone(),
            project_id,
            "repair",
            json!({
                "parentRunId": review.id,
                "findingIds": [finding.id]
            }),
        )
        .await;
        if !wait_for_terminal_with_timeout(store, &repair_run_id, REAL_PROVIDER_STAGE_TIMEOUT_SECS)
            .await
        {
            emit_real_provider_run_stream(store, &repair_run_id, project_id, "repair").await;
            panic!(
                "run {repair_run_id} did not reach terminal status within {REAL_PROVIDER_STAGE_TIMEOUT_SECS}s"
            );
        }
        emit_real_provider_run_stream(store, &repair_run_id, project_id, "repair").await;
        let repair_run = store.get_run(&repair_run_id).await.unwrap();
        assert_eq!(
            repair_run.status,
            AgentRunStatus::Completed,
            "real provider repair run {repair_run_id} should complete; events={}",
            serde_json::to_string(&store.events(&repair_run_id).await).unwrap()
        );
        assert_preview_updated_before_completed(store, &repair_run_id).await;
        assert_eq!(
            repair_run.design_context_content_hash, edit_run.design_context_content_hash,
            "real provider Repair must inherit the frozen Edit DCP"
        );
        let repair_dcp_evidence = real_provider_dcp_evidence(&repair_run, AgentPhase::Repair);
        let repaired_version_id = repair_run.output_version_id.clone().unwrap();
        assert_ne!(repaired_version_id, edited_version_id);
        let repaired_finding = store.get_review_finding(&finding.id).await.unwrap();
        assert_eq!(repaired_finding.status, ReviewFindingStatus::Fixed);

        let repaired_state = get_json(
            app.clone(),
            &format!("/projects/{project_id}/runtime-state"),
            8192,
        )
        .await;
        assert_eq!(repaired_state["currentVersionId"], repaired_version_id);
        let repaired_snapshot = repaired_state["sourceSnapshotUri"]
            .as_str()
            .expect("repaired runtime state should include source snapshot")
            .to_string();
        assert_ne!(repaired_snapshot, edited_snapshot);
        let repaired_preview =
            get_json(app.clone(), &format!("/preview/{project_id}/current"), 8192).await;
        assert_eq!(repaired_preview["versionId"], repaired_version_id);
        assert_eq!(repaired_preview["status"], "promoted");
        let repaired_artifact = get_text(app, artifact_path, 256_000).await;
        let repaired_artifact_contains_expected = repaired_artifact.contains(repair_expected_text);
        assert!(
            repaired_artifact_contains_expected,
            "artifact {artifact_path} should include repaired text {repair_expected_text:?}; body preview={}",
            repaired_artifact.chars().take(1000).collect::<String>()
        );
        emit_real_provider_evidence(
            project_id,
            "repair",
            &repair_run_id,
            json!({
                "runtimeState": repaired_state,
                "currentPreview": repaired_preview,
                "baseVersionId": edited_version_id,
                "repairedVersionId": repaired_version_id,
                "sourceSnapshotUri": repaired_snapshot,
                "baseSourceSnapshotUri": edited_snapshot,
                "repairedSourceSnapshotUri": repaired_snapshot,
                "sourceSnapshotChanged": true,
                "artifactPath": artifact_path,
                "localArtifactUrl": local_artifact_url(
                    workspace_root,
                    project_id,
                    local_artifact_relative,
                ),
                "artifactServed": true,
                "artifactByteLength": repaired_artifact.len(),
                "artifactContainsExpectedText": repaired_artifact_contains_expected,
                "expectedArtifactText": repair_expected_text,
                "previewUpdatedBeforeCompleted": true,
                "reviewRunId": review.id,
                "findingId": finding.id,
                "findingStatus": repaired_finding.status,
                "candidateVersionId": edited_version_id,
                "findingSource": "harness-seeded-review",
                "designContext": repair_dcp_evidence,
            }),
        );
    }
}

fn real_provider_dcp_evidence(run: &AgentRun, phase: AgentPhase) -> Value {
    let manifest: anydesign_runtime::design_context::DesignContextManifest =
        serde_json::from_value(
            run.design_context_manifest
                .clone()
                .expect("real provider Website run must inherit a DCP"),
        )
        .unwrap();
    assert_eq!(manifest.payload.surface, "website");
    assert_eq!(
        manifest.payload.effective_compatibility_mode,
        anydesign_runtime::design_context::ProfileCompatibilityMode::Enforced,
        "real provider Website DCP gate must run in enforced mode"
    );
    let required_read_paths = manifest
        .payload
        .required_reads
        .iter()
        .filter(|requirement| requirement.phases.contains(&phase))
        .map(|requirement| requirement.path.clone())
        .collect::<Vec<_>>();
    assert!(
        !required_read_paths.is_empty(),
        "real provider {phase:?} must have required DCP reads"
    );
    assert!(
        required_read_paths
            .iter()
            .all(|path| run.design_context_read_files.contains(path)),
        "real provider may publish only after every required DCP {phase:?} read; required={required_read_paths:?} actual={:?}",
        run.design_context_read_files
    );
    json!({
        "contentHash": manifest.content_hash,
        "artifactManifestHash": manifest.payload.artifact_manifest_hash,
        "briefHash": manifest.payload.brief_hash,
        "verificationPolicyId": manifest.payload.verification_policy.policy_id,
        "effectiveCompatibilityMode": manifest.payload.effective_compatibility_mode,
        "requiredReadPaths": required_read_paths,
        "readFiles": run.design_context_read_files.clone(),
    })
}

fn local_artifact_url(workspace_root: &Path, project_id: &str, relative: &str) -> String {
    let path = workspace_root.join(project_id).join(relative);
    assert!(
        path.exists(),
        "local artifact file should exist for provider evidence: {}",
        path.display()
    );
    format!("file://{}", path.display())
}

fn emit_real_provider_evidence(project_id: &str, stage: &str, run_id: &str, evidence: Value) {
    let model = std::env::var("DEEPSEEK_E2E_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
    let approval_reference = std::env::var("RUNTIME_PROVIDER_APPROVAL_ID")
        .expect("RUNTIME_PROVIDER_APPROVAL_ID must be set for real-provider evidence");
    eprintln!(
        "REAL_PROVIDER_EVIDENCE {}",
        serde_json::to_string(&json!({
            "project": project_id,
            "stage": stage,
            "runId": run_id,
            "provider": {
                "name": "deepseek",
                "model": model,
                "approvalReference": approval_reference,
            },
            "evidence": evidence
        }))
        .unwrap()
    );
}

async fn emit_real_provider_run_stream(
    store: &RuntimeStore,
    run_id: &str,
    project_id: &str,
    stage: &str,
) {
    let run = store.get_run(run_id).await;
    eprintln!(
        "REAL_PROVIDER_STREAM_BEGIN project={} stage={} run={} status={:?} outputVersion={:?}",
        project_id,
        stage,
        run_id,
        run.as_ref().map(|run| &run.status),
        run.as_ref()
            .and_then(|run| run.output_version_id.as_deref())
    );
    for event in store.events(run_id).await {
        eprintln!(
            "REAL_PROVIDER_EVENT {}",
            serde_json::to_string(&event).unwrap()
        );
    }
    eprintln!(
        "REAL_PROVIDER_STREAM_END project={} stage={} run={}",
        project_id, stage, run_id
    );
}
