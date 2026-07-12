use anydesign_runtime::{
    config::SandboxBackendMode,
    http_api::{self, AppState},
    model_gateway::{ModelClient, ModelRequest, ModelResponse, ToolCall},
    tools::control_plane::{sandbox_backend_for_config, SandboxBackend},
    types::{sha256_hex, AgentEvent, AgentPhase, AgentRunStatus, Brief},
    RuntimeConfig, RuntimeStore,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, time};

#[derive(Clone)]
struct RoutingFixtureModel {
    store: RuntimeStore,
    turns: Arc<Mutex<HashMap<String, u32>>>,
}

#[async_trait::async_trait]
impl ModelClient for RoutingFixtureModel {
    async fn next_response(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        let run = self
            .store
            .get_run(&request.run_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("fixture run not found"))?;
        let turn = {
            let mut turns = self.turns.lock().unwrap();
            let turn = turns.entry(run.id.clone()).or_default();
            *turn += 1;
            *turn
        };
        match (run.phase, run.project_id.as_str(), turn) {
            (AgentPhase::Build, "website-k3d", 1) => Ok(website_build_response()),
            (AgentPhase::Build, "docs-k3d", 1) => Ok(docs_init_response()),
            (AgentPhase::Build, "docs-k3d", 2) => Ok(docs_build_response()),
            (AgentPhase::Edit, _, 1) => Ok(deterministic_edit_response("edit")),
            _ => Err(anyhow::anyhow!(
                "unexpected fixture model turn: project={} phase={:?} turn={turn}",
                run.project_id,
                run.phase
            )),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn website_and_docs_public_runtime_lifecycle_on_k3d() {
    if std::env::var("RUN_PUBLIC_RUNTIME_K8S_E2E").ok().as_deref() != Some("1") {
        eprintln!("skipping Public Runtime k3d E2E; set RUN_PUBLIC_RUNTIME_K8S_E2E=1");
        return;
    }
    let signing_key = PathBuf::from(
        std::env::var("WORKSPACE_CHANNEL_SIGNING_KEY_FILE")
            .expect("runner must provide WORKSPACE_CHANNEL_SIGNING_KEY_FILE"),
    );
    let storage = unique_temp_dir("k8s-public-runtime-storage");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let capture_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let capture_address = capture_listener.local_addr().unwrap();
    let mut config = RuntimeConfig::from_env();
    config.sandbox_backend_mode = SandboxBackendMode::Kubernetes;
    config.k8s_namespace = "anydesign-sandboxes".to_string();
    config.workspace_channel_signing_key_file = Some(signing_key);
    config.runtime_storage_dir = storage.clone();
    config.workspace_root = PathBuf::from("/workspace");
    config.runtime_public_base_url = format!("http://{address}");
    config.runtime_browser_proxy_bind = capture_address;
    config.npm_registry =
        "http://anydesign-npm-proxy.anydesign-runtime.svc.cluster.local:4873/".to_string();

    let store = RuntimeStore::with_checkpoint_dir(&storage);
    let website_brief_id = confirmed_brief(&store, "website-k3d", website_brief()).await;
    let docs_brief_id = confirmed_brief(&store, "docs-k3d", docs_brief()).await;
    let model = RoutingFixtureModel {
        store: store.clone(),
        turns: Arc::new(Mutex::new(HashMap::new())),
    };
    let state = AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: config.clone(),
        store: store.clone(),
        model: Arc::new(model),
    };
    let capture_state = state.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, http_api::router_with_state(state))
            .await
            .unwrap();
    });
    let capture_server = tokio::spawn(async move {
        axum::serve(
            capture_listener,
            http_api::capture_router_with_state(capture_state),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();

    let (website_build, docs_build) = tokio::join!(
        run_build(
            &client,
            &config.runtime_public_base_url,
            &store,
            "website-k3d",
            &website_brief_id,
        ),
        run_build(
            &client,
            &config.runtime_public_base_url,
            &store,
            "docs-k3d",
            &docs_brief_id,
        )
    );
    let website_build_version = store
        .current_project_version("website-k3d")
        .await
        .expect("website build version");
    let docs_build_version = store
        .current_project_version("docs-k3d")
        .await
        .expect("docs build version");
    let (website, docs) = tokio::join!(
        run_edit(
            &client,
            &config.runtime_public_base_url,
            &store,
            "website-k3d",
            &website_build_version.id,
            website_build.sandbox_id.as_deref().unwrap(),
        ),
        run_edit(
            &client,
            &config.runtime_public_base_url,
            &store,
            "docs-k3d",
            &docs_build_version.id,
            docs_build.sandbox_id.as_deref().unwrap(),
        )
    );
    assert_ne!(
        store
            .current_project_version("website-k3d")
            .await
            .unwrap()
            .id,
        website_build_version.id
    );
    assert_ne!(
        store.current_project_version("docs-k3d").await.unwrap().id,
        docs_build_version.id
    );

    let backend: Arc<dyn SandboxBackend> = sandbox_backend_for_config(&config);
    let website_binding = website.sandbox_id.as_deref().expect("website binding");
    let docs_binding = docs.sandbox_id.as_deref().expect("docs binding");
    let website_identity = store
        .get_sandbox_binding(website_binding)
        .await
        .expect("website sandbox identity");
    let docs_identity = store
        .get_sandbox_binding(docs_binding)
        .await
        .expect("docs sandbox identity");
    assert_ne!(website_identity.pod_uid, docs_identity.pod_uid);
    backend
        .release(&store, website_binding)
        .await
        .expect("release website sandbox");
    backend
        .release(&store, docs_binding)
        .await
        .expect("release docs sandbox");

    let website_artifact = client
        .get(format!(
            "{}/artifacts/website-k3d/current/",
            config.runtime_public_base_url
        ))
        .send()
        .await
        .expect("website artifact after release");
    let docs_artifact = client
        .get(format!(
            "{}/artifacts/docs-k3d/current/",
            config.runtime_public_base_url
        ))
        .send()
        .await
        .expect("docs artifact after release");
    assert!(website_artifact.status().is_success());
    assert!(docs_artifact.status().is_success());
    let website_html = website_artifact.text().await.unwrap();
    let docs_html = docs_artifact.text().await.unwrap();
    assert!(
        website_html.contains("K3d Website Edited"),
        "website artifact was: {website_html}"
    );
    assert!(
        docs_html.contains("Docs Edited"),
        "docs artifact was: {docs_html}"
    );

    assert_event_order(&store, &website.id).await;
    assert_event_order(&store, &docs.id).await;
    write_evidence(
        &store,
        &storage,
        &website_build,
        &docs_build,
        &website_build_version,
        &docs_build_version,
        &website,
        &docs,
        &website_identity,
        &docs_identity,
    )
    .await;
    server.abort();
    capture_server.abort();
}

async fn confirmed_brief(store: &RuntimeStore, project_id: &str, brief: Brief) -> String {
    let run = store
        .create_run(
            project_id.to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "fixture".to_string(),
            vec![],
        )
        .await;
    let brief_id = store.write_brief(&run.id, brief).await.unwrap();
    store.confirm_brief(&run.id, &brief_id).await.unwrap();
    brief_id
}

async fn run_build(
    client: &reqwest::Client,
    base_url: &str,
    store: &RuntimeStore,
    project_id: &str,
    brief_id: &str,
) -> anydesign_runtime::types::AgentRun {
    let response = client
        .post(format!("{base_url}/runs"))
        .json(&json!({
            "projectId": project_id,
            "phase": "build",
            "agentProfile": "build",
            "inputContext": { "briefId": brief_id }
        }))
        .send()
        .await
        .expect("start Public Runtime build");
    let status = response.status();
    let payload: Value = response.json().await.expect("build response JSON");
    assert!(status.is_success(), "build start failed: {payload}");
    let run_id = payload["runId"].as_str().expect("runId");
    let deadline = time::Instant::now() + Duration::from_secs(180);
    loop {
        let run = store.get_run(run_id).await.expect("persisted run");
        if run.status.is_terminal() {
            assert_eq!(
                run.status,
                AgentRunStatus::Completed,
                "run failed: {run:?}; events={:?}",
                store.events(run_id).await
            );
            assert!(run.output_version_id.is_some());
            return run;
        }
        assert!(time::Instant::now() < deadline, "run timed out: {run_id}");
        time::sleep(Duration::from_millis(200)).await;
    }
}

async fn run_edit(
    client: &reqwest::Client,
    base_url: &str,
    store: &RuntimeStore,
    project_id: &str,
    base_version_id: &str,
    sandbox_binding_id: &str,
) -> anydesign_runtime::types::AgentRun {
    let response = client
        .post(format!("{base_url}/runs"))
        .json(&json!({
            "projectId": project_id,
            "phase": "edit",
            "agentProfile": "edit",
            "inputContext": {
                "baseVersionId": base_version_id,
                "sandboxBindingId": sandbox_binding_id
            }
        }))
        .send()
        .await
        .expect("start Public Runtime edit");
    let status = response.status();
    let payload: Value = response.json().await.expect("edit response JSON");
    assert!(status.is_success(), "edit start failed: {payload}");
    let run_id = payload["runId"].as_str().expect("runId");
    let continued = client
        .post(format!("{base_url}/runs/{run_id}/continue"))
        .json(&json!({ "userMessage": "Apply the deterministic RC edit." }))
        .send()
        .await
        .expect("continue Public Runtime edit");
    assert!(continued.status().is_success());
    let deadline = time::Instant::now() + Duration::from_secs(180);
    loop {
        let run = store.get_run(run_id).await.expect("persisted edit run");
        if run.status.is_terminal() {
            let events = store.events(run_id).await;
            assert_eq!(
                run.status,
                AgentRunStatus::Completed,
                "edit failed: {run:?}; events={:?}",
                events
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ToolFailed { .. })),
                "edit contained failed tools: {events:?}"
            );
            assert!(run.output_version_id.is_some());
            return run;
        }
        assert!(time::Instant::now() < deadline, "edit timed out: {run_id}");
        time::sleep(Duration::from_millis(200)).await;
    }
}

fn website_build_response() -> ModelResponse {
    let build_script = "const fs=require('fs');fs.mkdirSync('dist',{recursive:true});fs.writeFileSync('dist/index.html','<!doctype html><style>body{font:48px sans-serif;background:#fff;color:#111}</style><h1>K3d Website</h1>');";
    ModelResponse::ToolCalls(vec![
        ToolCall::new(
            "web-init",
            "project.init",
            json!({ "template": "astro-website" }),
        ),
        ToolCall::new(
            "web-package",
            "fs.write",
            json!({ "path": "project/package.json", "text": "{\"scripts\":{\"build\":\"node build.cjs\"}}" }),
        ),
        ToolCall::new(
            "web-script",
            "fs.write",
            json!({ "path": "project/build.cjs", "text": build_script }),
        ),
        ToolCall::new("web-build", "project.build", json!({ "cwd": "project" })),
        ToolCall::new("web-preview", "preview.start", json!({})),
        ToolCall::new(
            "web-open",
            "browser.open",
            json!({ "url": "http://127.0.0.1:4321" }),
        ),
        ToolCall::new(
            "web-shot",
            "browser.screenshot",
            json!({ "screenshotId": "website-k3d-shot" }),
        ),
        ToolCall::new(
            "web-promote",
            "preview.report_candidate",
            json!({ "url": "http://127.0.0.1:4321", "screenshotId": "website-k3d-shot" }),
        ),
        ToolCall::new(
            "web-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Website k3d gate complete" }),
        ),
    ])
}

fn docs_init_response() -> ModelResponse {
    ModelResponse::ToolCalls(vec![ToolCall::new(
        "docs-init",
        "project.init",
        json!({ "template": "fumadocs-docs" }),
    )])
}

fn docs_build_response() -> ModelResponse {
    let build_script = "const fs=require('fs');fs.mkdirSync('out',{recursive:true});fs.writeFileSync('out/index.html','<!doctype html><style>body{font:40px sans-serif;background:#fff;color:#111}</style><h1>Docs</h1><a href=\"/docs\">Overview</a>');fs.writeFileSync('out/docs.html','<h1>Docs Overview</h1>');";
    ModelResponse::ToolCalls(vec![
        ToolCall::new(
            "docs-package",
            "fs.write",
            json!({ "path": "project/package.json", "text": "{\"scripts\":{\"build\":\"node build.cjs\"}}" }),
        ),
        ToolCall::new(
            "docs-script",
            "fs.write",
            json!({ "path": "project/build.cjs", "text": build_script }),
        ),
        ToolCall::new(
            "docs-source",
            "fs.write",
            json!({ "path": "project/content/docs/index.mdx", "text": "---\ntitle: Overview\n---\n\n# Docs Overview" }),
        ),
        ToolCall::new("docs-build", "project.build", json!({ "cwd": "project" })),
        ToolCall::new("docs-preview", "preview.start", json!({})),
        ToolCall::new(
            "docs-open",
            "browser.open",
            json!({ "url": "http://127.0.0.1:4321" }),
        ),
        ToolCall::new(
            "docs-shot",
            "browser.screenshot",
            json!({ "screenshotId": "docs-k3d-shot" }),
        ),
        ToolCall::new(
            "docs-promote",
            "preview.report_candidate",
            json!({ "url": "http://127.0.0.1:4321", "screenshotId": "docs-k3d-shot" }),
        ),
        ToolCall::new(
            "docs-complete",
            "run.complete",
            json!({ "status": "completed", "summary": "Docs k3d gate complete" }),
        ),
    ])
}

fn deterministic_edit_response(prefix: &str) -> ModelResponse {
    let build_script = "const fs=require('fs');const docs=fs.existsSync('content/docs/index.mdx');const dir=docs?'out':'dist';fs.mkdirSync(dir,{recursive:true});const title=docs?'Docs Edited':'K3d Website Edited';fs.writeFileSync(dir+'/index.html','<!doctype html><style>body{font:44px sans-serif;background:#fff;color:#111}</style><h1>'+title+'</h1>');if(docs)fs.writeFileSync(dir+'/docs.html','<h1>Docs Overview Edited</h1>');";
    let screenshot_id = "k3d-edit-shot";
    ModelResponse::ToolCalls(vec![
        ToolCall::new(
            format!("{prefix}-write"),
            "fs.write",
            json!({ "path": "project/build.cjs", "text": build_script }),
        ),
        ToolCall::new(
            format!("{prefix}-build"),
            "project.build",
            json!({ "cwd": "project" }),
        ),
        ToolCall::new(format!("{prefix}-preview"), "preview.start", json!({})),
        ToolCall::new(
            format!("{prefix}-open"),
            "browser.open",
            json!({ "url": "http://127.0.0.1:4321" }),
        ),
        ToolCall::new(
            format!("{prefix}-shot"),
            "browser.screenshot",
            json!({ "screenshotId": screenshot_id }),
        ),
        ToolCall::new(
            format!("{prefix}-promote"),
            "preview.report_candidate",
            json!({ "url": "http://127.0.0.1:4321", "screenshotId": screenshot_id }),
        ),
        ToolCall::new(
            format!("{prefix}-complete"),
            "run.complete",
            json!({ "status": "completed", "summary": "K3d edit gate complete" }),
        ),
    ])
}

async fn assert_event_order(store: &RuntimeStore, run_id: &str) {
    let events = store.events(run_id).await;
    let preview = events
        .iter()
        .position(|event| matches!(event, AgentEvent::PreviewUpdated { .. }))
        .expect("preview.updated event");
    let completed = events
        .iter()
        .position(|event| matches!(event, AgentEvent::RunCompleted { .. }))
        .expect("run.completed event");
    assert!(preview < completed);
}

#[allow(clippy::too_many_arguments)]
async fn write_evidence(
    store: &RuntimeStore,
    storage: &std::path::Path,
    website_build: &anydesign_runtime::types::AgentRun,
    docs_build: &anydesign_runtime::types::AgentRun,
    website_build_version: &anydesign_runtime::types::ProjectVersion,
    docs_build_version: &anydesign_runtime::types::ProjectVersion,
    website: &anydesign_runtime::types::AgentRun,
    docs: &anydesign_runtime::types::AgentRun,
    website_binding: &anydesign_runtime::types::SandboxBinding,
    docs_binding: &anydesign_runtime::types::SandboxBinding,
) {
    let Ok(path) = std::env::var("PUBLIC_RUNTIME_EVIDENCE_PATH") else {
        return;
    };
    let website_version = store
        .current_project_version(&website.project_id)
        .await
        .unwrap();
    let docs_version = store
        .current_project_version(&docs.project_id)
        .await
        .unwrap();
    let website_publish = store
        .artifact_publish_for_version(&website.project_id, &website.id, &website_version.id)
        .await
        .unwrap();
    let docs_publish = store
        .artifact_publish_for_version(&docs.project_id, &docs.id, &docs_version.id)
        .await
        .unwrap();
    let website_preview = store.preview_lease_for_run(&website.id).await.unwrap();
    let docs_preview = store.preview_lease_for_run(&docs.id).await.unwrap();
    let website_events = store.events(&website.id).await;
    let docs_events = store.events(&docs.id).await;
    let event_ids = |run_id: &str, events: &[AgentEvent]| {
        let preview = events
            .iter()
            .position(|event| matches!(event, AgentEvent::PreviewUpdated { .. }))
            .unwrap();
        let completed = events
            .iter()
            .position(|event| matches!(event, AgentEvent::RunCompleted { .. }))
            .unwrap();
        json!({
            "previewUpdated": format!("{run_id}/{preview}"),
            "runCompleted": format!("{run_id}/{completed}"),
            "sequenceValid": preview < completed,
        })
    };
    let website_released = store
        .get_sandbox_binding(&website_binding.id)
        .await
        .unwrap();
    let docs_released = store.get_sandbox_binding(&docs_binding.id).await.unwrap();
    let website_screenshot =
        screenshot_evidence(storage, &website.project_id, &website.id, "k3d-edit-shot");
    let docs_screenshot = screenshot_evidence(storage, &docs.project_id, &docs.id, "k3d-edit-shot");
    assert_ne!(
        website_screenshot["documentSha256"], docs_screenshot["documentSha256"],
        "Website and Docs browser evidence must come from different documents"
    );
    let evidence = json!({
        "schemaVersion": "anydesign-public-runtime-k3d-evidence@1",
        "provider": { "mode": "fixture", "model": "deterministic-tool-sequence" },
        "repository": {
            "commit": std::env::var("E2E_REPOSITORY_COMMIT").unwrap_or_default(),
            "dirtyFiles": std::env::var("E2E_REPOSITORY_DIRTY_FILES").ok().and_then(|value| value.parse::<u64>().ok()),
        },
        "cluster": {
            "name": std::env::var("E2E_K3D_CLUSTER").unwrap_or_default(),
            "kubeContext": format!("k3d-{}", std::env::var("E2E_K3D_CLUSTER").unwrap_or_default()),
        },
        "sandbox": {
            "imageRef": std::env::var("E2E_SANDBOX_IMAGE").unwrap_or_default(),
            "imageId": std::env::var("E2E_SANDBOX_IMAGE_ID").unwrap_or_default(),
        },
        "projects": [
            {
                "kind": "website", "projectId": website.project_id,
                "buildRunId": website_build.id, "editRunId": website.id,
                "sandboxBindingId": website_binding.id, "podUid": website_binding.pod_uid,
                "versionBeforeCas": website_build_version.id,
                "versionAfterCas": website_version.id,
                "buildId": website_publish.build_id,
                "candidateManifestHash": website_publish.candidate_manifest_hash,
                "sourceSnapshotUri": website_publish.source_snapshot_uri,
                "previewLeaseId": website_preview.id,
                "previewLeaseStatusAfterRelease": website_preview.status,
                "screenshot": website_screenshot,
                "artifactManifestHash": website_publish.artifact_manifest_hash,
                "artifactUri": website_publish.immutable_artifact_uri,
                "artifactUrl": format!("/artifacts/{}/current/", website.project_id),
                "artifactHttpStatusAfterRelease": 200,
                "currentVersionBeforeCas": website_publish.expected_current_version_id,
                "currentVersionAfterCas": website_version.id,
                "events": event_ids(&website.id, &website_events),
                "sandboxReleasedAt": website_released.last_seen_at,
            },
            {
                "kind": "docs", "projectId": docs.project_id,
                "buildRunId": docs_build.id, "editRunId": docs.id,
                "sandboxBindingId": docs_binding.id, "podUid": docs_binding.pod_uid,
                "versionBeforeCas": docs_build_version.id,
                "versionAfterCas": docs_version.id,
                "buildId": docs_publish.build_id,
                "candidateManifestHash": docs_publish.candidate_manifest_hash,
                "sourceSnapshotUri": docs_publish.source_snapshot_uri,
                "previewLeaseId": docs_preview.id,
                "previewLeaseStatusAfterRelease": docs_preview.status,
                "screenshot": docs_screenshot,
                "artifactManifestHash": docs_publish.artifact_manifest_hash,
                "artifactUri": docs_publish.immutable_artifact_uri,
                "artifactUrl": format!("/artifacts/{}/current/", docs.project_id),
                "artifactHttpStatusAfterRelease": 200,
                "currentVersionBeforeCas": docs_publish.expected_current_version_id,
                "currentVersionAfterCas": docs_version.id,
                "events": event_ids(&docs.id, &docs_events),
                "sandboxReleasedAt": docs_released.last_seen_at,
            }
        ]
    });
    fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
}

fn screenshot_evidence(
    storage: &std::path::Path,
    project_id: &str,
    run_id: &str,
    screenshot_id: &str,
) -> Value {
    let directory = storage.join("screenshots").join(project_id).join(run_id);
    let png_bytes =
        fs::read(directory.join(format!("{screenshot_id}.png"))).expect("Runtime screenshot PNG");
    let mut metadata: Value = serde_json::from_slice(
        &fs::read(directory.join(format!("{screenshot_id}.json")))
            .expect("Runtime screenshot metadata"),
    )
    .expect("valid Runtime screenshot metadata");
    assert_eq!(metadata["pngSha256"], sha256_hex(&png_bytes));
    metadata["screenshotId"] = json!(screenshot_id);
    metadata
}

fn website_brief() -> Brief {
    Brief {
        project_type: "website".to_string(),
        audience: "release gate".to_string(),
        content_hierarchy: vec!["hero".to_string()],
        page_structure: json!([{ "title": "Home", "level": 1 }]),
        visual_direction: "high contrast".to_string(),
        recommended_template: "astro-website".to_string(),
        assumptions: vec![],
        missing_information: vec![],
    }
}

fn docs_brief() -> Brief {
    Brief {
        project_type: "docs".to_string(),
        audience: "operators".to_string(),
        content_hierarchy: vec!["overview".to_string()],
        page_structure: json!([{ "title": "Overview", "level": 1 }]),
        visual_direction: "technical documentation".to_string(),
        recommended_template: "fumadocs-docs".to_string(),
        assumptions: vec![],
        missing_information: vec![],
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
