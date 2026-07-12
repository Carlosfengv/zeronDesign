use super::*;

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
