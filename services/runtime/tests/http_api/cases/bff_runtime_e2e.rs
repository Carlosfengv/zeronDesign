use super::*;
use anydesign_runtime::{
    design_context::{
        compile_website_design_context, DesignContextCompileOptions, VerifierRegistry,
    },
    design_profile_service::{CreateProfileCommand, DesignProfileService, UpdateProfileCommand},
    profile_token_sync::{ProfileTokenSyncOperationStatus, ProfileTokenSyncService},
    run_lifecycle::{
        BuildSandboxProvisioner, EditWorkspaceRestorer, RunLifecycleService, RunSessionLauncher,
    },
    tools::control_plane::control_plane_executor_for_config,
};
use axum::{extract::State, routing::post, Json, Router};
use chrono::{Duration as ChronoDuration, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
use tokio::process::Command;

struct RecoverySessionLauncher;

impl RunSessionLauncher for RecoverySessionLauncher {
    fn launch(&self, _run_id: String) -> anyhow::Result<()> {
        Ok(())
    }
}

struct RecoverySandboxProvisioner;

#[async_trait::async_trait]
impl BuildSandboxProvisioner for RecoverySandboxProvisioner {
    async fn provision_ready(
        &self,
        _store: &RuntimeStore,
        _project_id: &str,
        _template_key: &str,
    ) -> anyhow::Result<anydesign_runtime::types::SandboxBinding> {
        unreachable!("profile sync recovery must reuse the persisted child Run")
    }
}

struct RecoveryWorkspaceRestorer;

#[async_trait::async_trait]
impl EditWorkspaceRestorer for RecoveryWorkspaceRestorer {
    async fn restore(
        &self,
        _store: &RuntimeStore,
        _config: &anydesign_runtime::RuntimeConfig,
        _run: &anydesign_runtime::types::AgentRun,
        _source_snapshot_uri: &str,
    ) -> anyhow::Result<()> {
        unreachable!("profile sync recovery must reuse the persisted child Run")
    }
}

#[derive(Clone)]
struct ProfileSyncFixtureState {
    runtime: AppState,
    seeded: Arc<Mutex<Vec<SeededProfileSync>>>,
}

#[derive(Clone)]
struct SeededProfileSync {
    run_id: String,
    token_file: PathBuf,
    expected_primary: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeedProfileSyncRequest {
    project_id: String,
    scenario: String,
}

fn resolve_web_app_root() -> PathBuf {
    if let Some(repo_root) = std::env::var_os("ZERONDESIGN_REPO_ROOT") {
        let candidate = PathBuf::from(repo_root).join("apps/web");
        assert!(
            candidate.join("package.json").is_file(),
            "ZERONDESIGN_REPO_ROOT does not contain apps/web/package.json"
        );
        return candidate;
    }

    let current_dir =
        std::env::current_dir().expect("could not resolve the current test directory");
    if let Some(candidate) = current_dir
        .ancestors()
        .map(|ancestor| ancestor.join("apps/web"))
        .find(|candidate| candidate.join("package.json").is_file())
    {
        return candidate;
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Runtime manifest directory must be nested under the repository root")
        .join("apps/web")
}

/// This is deliberately ignored in the ordinary Rust suite: it launches the
/// real Next development server and a headless browser. It remains a
/// single-command L4 gate, with a test-only seed handler mounted *outside* the
/// production Runtime router.
#[tokio::test]
#[ignore = "requires a local Next dev server; run with --ignored bff_profile_sync_bff_to_real_runtime"]
async fn bff_profile_sync_bff_to_real_runtime() {
    let mut config = phase_a_contract_config();
    config.workspace_root = unique_temp_dir("bff-profile-sync-runtime-workspaces");
    let store = RuntimeStore::with_checkpoint_dir(config.runtime_storage_dir.clone());
    let runtime_state = AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: config.clone(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    };
    let seeded = Arc::new(Mutex::new(Vec::new()));
    let fixture_state = ProfileSyncFixtureState {
        runtime: runtime_state.clone(),
        seeded: seeded.clone(),
    };
    let app = Router::new()
        .route("/__test/profile-sync-seed", post(seed_profile_sync_fixture))
        .with_state(fixture_state)
        .merge(http_api::router_with_state(runtime_state));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime_base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let app_root = resolve_web_app_root();
    let output = timeout(
        Duration::from_secs(210),
        Command::new("node")
            .arg("scripts/run-design-context-bff-smoke.mjs")
            .current_dir(&app_root)
            .env("RUNTIME_REAL_FIXTURE_URL", &runtime_base_url)
            .env("BFF_SMOKE_BROWSER", "1")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("BFF real Runtime fixture timed out")
    .expect("failed to start BFF real Runtime fixture");
    server.abort();
    assert!(
        output.status.success(),
        "BFF real Runtime fixture failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let browser_evidence: Value = serde_json::from_slice(&output.stdout)
        .expect("BFF real Runtime fixture must emit machine-readable evidence");
    assert!(
        browser_evidence["checks"]
            .as_array()
            .is_some_and(|checks| checks
                .iter()
                .any(|check| { check.as_str() == Some("browser-real-runtime-profile-sync") })),
        "L4 evidence must identify the joint browser + real Runtime path"
    );
    assert!(
        browser_evidence["browserEvidence"]["screenshotSha256"]
            .as_str()
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .chars()
                        .all(|character| character.is_ascii_hexdigit())
            }),
        "L4 evidence must include the final browser screenshot digest"
    );

    let seeded = seeded.lock().unwrap().clone();
    assert_eq!(
        seeded.len(),
        3,
        "clean, HTTP conflict, and browser conflict fixtures must all run"
    );
    for fixture in seeded {
        let token_file = std::fs::read_to_string(&fixture.token_file).unwrap();
        assert!(
            token_file.contains(&fixture.expected_primary),
            "BFF confirm did not write the expected target token for {}: {token_file}",
            fixture.run_id
        );
        assert_eq!(
            store.child_runs(&fixture.run_id).await.len(),
            1,
            "confirm replay must not create another child Run for {}",
            fixture.run_id
        );
    }
}

#[tokio::test]
async fn profile_sync_recovers_after_token_write_before_operation_completion() {
    let mut config = phase_a_contract_config();
    config.workspace_root = unique_temp_dir("profile-sync-recovery-workspaces");
    let store = RuntimeStore::with_checkpoint_dir(config.runtime_storage_dir.clone());
    let fixture = ProfileSyncFixtureState {
        runtime: AppState {
            supervisor: http_api::RuntimeSupervisor::new(),
            config: config.clone(),
            store: store.clone(),
            model: Arc::new(MockModelClient::new(vec![])),
        },
        seeded: Arc::new(Mutex::new(Vec::new())),
    };
    let seed = seed_profile_sync_fixture(
        State(fixture),
        Json(SeedProfileSyncRequest {
            project_id: "profile-sync-recovery".to_string(),
            scenario: "clean".to_string(),
        }),
    )
    .await
    .unwrap()
    .0;
    let source_run_id = seed["runId"].as_str().unwrap();
    let source_run = store.get_run(source_run_id).await.unwrap();
    let manifest: anydesign_runtime::design_context::DesignContextManifest =
        serde_json::from_value(source_run.design_context_manifest.clone().unwrap()).unwrap();
    let target_profile = store
        .design_profile_versions(source_run.design_profile_id.as_deref().unwrap())
        .await
        .unwrap()
        .into_iter()
        .find(|profile| profile.version == 2)
        .unwrap();
    let brief = store
        .get_brief(source_run.brief_version.as_deref().unwrap())
        .await
        .unwrap();
    let template = anydesign_runtime::project::resolve_built_in_template_for_init("astro-website")
        .await
        .unwrap();
    let target_dcp = compile_website_design_context(
        &target_profile
            .effective_for("website", "astro-website")
            .unwrap(),
        &brief,
        &template,
        &DesignContextCompileOptions {
            expected_app_root: manifest.payload.expected_app_root.clone(),
            compiler_version: manifest.payload.compiler_version.clone(),
            enforcement_enabled: false,
            verification_policy: manifest.payload.verification_policy.clone(),
        },
    )
    .unwrap();
    let workspace = config.workspace_root.join(&source_run.project_id);
    let contract: Value = serde_json::from_str(
        &std::fs::read_to_string(workspace.join("state/style-contract.json")).unwrap(),
    )
    .unwrap();
    let token_file = workspace.join("project/tokens.css");
    let operation = ProfileTokenSyncService::plan_operation(
        store.next_id("profile-sync"),
        &source_run,
        &target_dcp,
        &contract,
        &std::fs::read_to_string(&token_file).unwrap(),
        "recovery-fixture".to_string(),
        "recovery-plan-key".to_string(),
        Utc::now() + ChronoDuration::minutes(10),
        Utc::now(),
    )
    .unwrap();
    let operation = store
        .create_profile_token_sync_operation(operation)
        .await
        .unwrap();
    let operation = store
        .confirm_profile_token_sync_operation(
            &operation.id,
            &operation.plan.plan_hash,
            BTreeMap::new(),
            "recovery-confirm-key".to_string(),
        )
        .await
        .unwrap();
    let child = store
        .create_child_run(
            &source_run.id,
            AgentPhase::Edit,
            "edit".to_string(),
            "fixture".to_string(),
            None,
            Vec::new(),
        )
        .await
        .unwrap();
    store
        .begin_profile_token_sync_apply(&operation.id, &child.id)
        .await
        .unwrap();
    let after_write = std::fs::read_to_string(&token_file)
        .unwrap()
        .replace("#2563eb", "#b91c1c");
    std::fs::write(&token_file, after_write).unwrap();
    store
        .mark_profile_token_sync_recovery_required(&operation.id, "injected crash after write")
        .await
        .unwrap();

    let restarted_store = RuntimeStore::with_checkpoint_dir(config.runtime_storage_dir.clone());
    let service = RunLifecycleService::new(
        config,
        restarted_store.clone(),
        Arc::new(RecoverySessionLauncher),
        Arc::new(RecoverySandboxProvisioner),
        Arc::new(RecoveryWorkspaceRestorer),
        DesignProfileService::new(restarted_store.clone()),
    );
    let outcome = service
        .apply_confirmed_profile_token_sync(&operation.id)
        .await
        .unwrap();
    assert_eq!(outcome.run_id, child.id);
    let recovered = restarted_store
        .profile_token_sync_operation(&operation.id)
        .await
        .unwrap();
    assert_eq!(recovered.status, ProfileTokenSyncOperationStatus::Applied);
    assert_eq!(recovered.child_run_id.as_deref(), Some(child.id.as_str()));
    assert!(std::fs::read_to_string(token_file)
        .unwrap()
        .contains("#b91c1c"));
}

async fn seed_profile_sync_fixture(
    State(fixture): State<ProfileSyncFixtureState>,
    Json(request): Json<SeedProfileSyncRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if request.project_id.trim().is_empty()
        || !matches!(request.scenario.as_str(), "clean" | "conflict")
    {
        return Err(fixture_bad_request(
            "projectId and a supported scenario are required",
        ));
    }
    let state = &fixture.runtime;
    let service = DesignProfileService::new(state.store.clone());
    let existing_profile = service
        .project_profile(&request.project_id)
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    let (source_profile, target_primary) =
        if request.scenario == "clean" || existing_profile.is_none() {
            let source_payload = design_profile_request(&request.project_id, vec!["astro-website"])
                .get("profile")
                .and_then(Value::as_object)
                .cloned()
                .ok_or_else(|| fixture_bad_request("fixture profile payload is invalid"))?;
            let source_profile = service
                .create(CreateProfileCommand {
                    project_id: Some(request.project_id.clone()),
                    name: format!("BFF fixture {} source", request.scenario),
                    payload: source_payload.clone(),
                })
                .await
                .map_err(|error| fixture_bad_request(error.to_string()))?;
            let mut target_payload = Value::Object(source_payload);
            let target_primary = if request.scenario == "clean" {
                "#b91c1c"
            } else {
                "#7c3aed"
            };
            target_payload["runtimeTokenMapping"]["color.primary"] = json!(target_primary);
            service
                .update(UpdateProfileCommand {
                    design_profile_id: source_profile.id.clone(),
                    expected_version: Some(source_profile.version),
                    name: format!("BFF fixture {} target", request.scenario),
                    profile: target_payload,
                })
                .await
                .map_err(|error| fixture_bad_request(error.to_string()))?;
            service
                .bind_project(&request.project_id, &source_profile.id)
                .await
                .map_err(|error| fixture_bad_request(error.to_string()))?;
            (source_profile, target_primary)
        } else {
            let source_profile = existing_profile.expect("profile checked above");
            let mut target_payload = serde_json::to_value(&source_profile)
                .map_err(|error| fixture_bad_request(error.to_string()))?;
            target_payload["runtimeTokenMapping"]["color.primary"] = json!("#7c3aed");
            service
                .update(UpdateProfileCommand {
                    design_profile_id: source_profile.id.clone(),
                    expected_version: Some(source_profile.version),
                    name: "BFF fixture conflict target".to_string(),
                    profile: target_payload,
                })
                .await
                .map_err(|error| fixture_bad_request(error.to_string()))?;
            (source_profile, "#7c3aed")
        };

    let source_run = state
        .store
        .create_run(
            request.project_id.clone(),
            AgentPhase::Build,
            "build".to_string(),
            "fixture".to_string(),
            Vec::new(),
        )
        .await;
    let brief_id = state
        .store
        .write_brief(&source_run.id, website_brief())
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    let source_run = state
        .store
        .attach_run_effective_design_profile(
            &source_run.id,
            &source_profile,
            Some("website"),
            Some("astro-website"),
        )
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    let template = anydesign_runtime::project::resolve_built_in_template_for_init("astro-website")
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    let brief = state.store.get_brief(&brief_id).await.unwrap();
    let effective = source_profile
        .effective_for("website", "astro-website")
        .map_err(fixture_bad_request)?;
    let source_dcp = compile_website_design_context(
        &effective,
        &brief,
        &template,
        &DesignContextCompileOptions::default(),
    )
    .map_err(fixture_bad_request)?;
    state
        .store
        .attach_run_design_context(&source_run.id, &source_dcp, &VerifierRegistry::discover())
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    state
        .store
        .record_run_design_context_materialization(
            &source_run.id,
            &source_dcp.manifest.payload.artifact_manifest_hash,
        )
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    for requirement in source_dcp
        .manifest
        .payload
        .required_reads
        .iter()
        .filter(|requirement| requirement.phases.contains(&AgentPhase::Build))
    {
        state
            .store
            .record_design_context_file_read(&source_run.id, &requirement.path)
            .await
            .map_err(|error| fixture_bad_request(error.to_string()))?;
    }
    let binding = state
        .store
        .create_sandbox_binding(
            &request.project_id,
            format!("bff-fixture-sandbox-{}", source_run.id),
            format!("bff-fixture-claim-{}", source_run.id),
            format!("bff-fixture-workspace-{}", source_run.id),
            "bff-fixture-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    state
        .store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    state
        .store
        .bind_run_to_sandbox(&source_run.id, &binding.id)
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    let workspace = state.config.workspace_root.join(&request.project_id);
    std::fs::create_dir_all(workspace.join("state"))
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    std::fs::create_dir_all(workspace.join("project"))
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    let mut style_contract: Value = serde_json::from_str(
        source_dcp
            .files
            .get("inputs/template-style-contract.json")
            .ok_or_else(|| fixture_bad_request("compiled DCP has no template style contract"))?,
    )
    .map_err(|error| fixture_bad_request(error.to_string()))?;
    let token_mappings = style_contract
        .get("tokens")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| fixture_bad_request("compiled style contract has no token map"))?;
    // The frozen contract renders workspace-absolute paths for agents, while
    // this HTTP boundary accepts only safe workspace-relative paths. Path
    // fields are runtime-only and intentionally excluded from DCP identity.
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
            let current = if request.scenario == "conflict" && token == "color.primary" {
                "#16a34a"
            } else {
                value
            };
            let css_variable = css_variable.as_str().ok_or_else(|| {
                fixture_bad_request(format!(
                    "compiled style contract has no CSS variable for {token}"
                ))
            })?;
            Ok(format!("{css_variable}: {current};"))
        })
        .collect::<Result<Vec<_>, (StatusCode, Json<Value>)>>()?
        .join("\n");
    let token_file = workspace.join("project/tokens.css");
    std::fs::write(&token_file, format!(":root {{\n{token_css}\n}}"))
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    std::fs::write(
        workspace.join("state/style-contract.json"),
        serde_json::to_vec(&style_contract).unwrap(),
    )
    .map_err(|error| fixture_bad_request(error.to_string()))?;
    let verification = control_plane_executor_for_config(&state.config)
        .with_workspace_root(&workspace)
        .execute(
            state.store.clone(),
            &source_run.id,
            "bootstrap:bff-fixture-style-contract-verification",
            "fs.read",
            json!({ "path": "state/style-contract.json" }),
        )
        .await;
    if verification.result.is_error {
        return Err(fixture_bad_request(format!(
            "fixture style contract verification failed: {}",
            verification.result.content
        )));
    }
    let materialized_source_run = state
        .store
        .get_run(&source_run.id)
        .await
        .ok_or_else(|| fixture_bad_request("fixture source Run disappeared after seeding"))?;
    if materialized_source_run
        .design_context_materialization_hash
        .as_deref()
        != Some(source_dcp.manifest.payload.artifact_manifest_hash.as_str())
        || materialized_source_run.design_context_style_contract_verified != Some(true)
    {
        return Err(fixture_bad_request(
            "fixture source Run did not retain verified DCP materialization state",
        ));
    }
    if request.scenario == "conflict" {
        state
            .store
            .append_conversation_item(
                &request.project_id,
                Some(&source_run.id),
                "design_profile_fidelity_checked",
                Some("assistant"),
                "Fixture fidelity failed with required responsive and accessibility findings.",
                Some(json!({
                    "version": "design-profile-fidelity@2",
                    "status": "failed",
                    "runId": source_run.id,
                    "designProfileId": source_profile.id,
                    "designProfileVersion": source_profile.version,
                    "outputVersionId": "fixture-source-version",
                    "checkedAt": Utc::now(),
                    "requiredFailedRuleIds": [
                        "craft:accessibility-baseline:image-alt",
                        "craft:responsive-layout:no-horizontal-overflow:375"
                    ],
                    "assertions": [{
                        "ruleId": "craft:accessibility-baseline:image-alt",
                        "recipeId": "accessibility-baseline",
                        "priority": "required",
                        "kind": "a11y",
                        "route": "/",
                        "viewport": null,
                        "selector": "main img",
                        "property": null,
                        "rawActual": ["<img src=fixture.png>"],
                        "normalizedActual": ["missing alt"],
                        "expected": [],
                        "comparator": "equals",
                        "passed": false,
                        "reason": "Image alternative text is required."
                    }, {
                        "ruleId": "craft:responsive-layout:no-horizontal-overflow:375",
                        "recipeId": "responsive-layout",
                        "priority": "required",
                        "kind": "viewport",
                        "route": "/",
                        "viewport": 375,
                        "selector": "html",
                        "property": "scrollWidth",
                        "rawActual": "420px",
                        "normalizedActual": "420px",
                        "expected": "375px",
                        "comparator": "less-than-or-equal",
                        "passed": false,
                        "reason": "Page width exceeds the required mobile viewport."
                    }],
                    "repairContext": {
                        "globalCssFile": "/workspace/project/src/styles/global.css",
                        "componentRoot": "/workspace/project/src/components",
                        "instructions": [
                            "Repair the imported page source.",
                            "Verify the fixed 375px viewport before publishing again."
                        ]
                    }
                })),
            )
            .await;
    }
    state
        .store
        .update_run_status(&source_run.id, AgentRunStatus::Completed)
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    state
        .store
        .append_event(AgentEvent::RunCompleted {
            run_id: source_run.id.clone(),
            status: "completed".to_string(),
            summary: "BFF Profile Sync source fixture completed.".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .map_err(|error| fixture_bad_request(error.to_string()))?;
    fixture.seeded.lock().unwrap().push(SeededProfileSync {
        run_id: source_run.id.clone(),
        token_file,
        expected_primary: target_primary.to_string(),
    });
    Ok(Json(json!({
        "runId": source_run.id,
        "sourceContentHash": source_dcp.manifest.content_hash,
        "targetDesignProfileId": source_profile.id,
        "targetDesignProfileVersion": source_profile.version + 1,
    })))
}

fn fixture_bad_request(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": message.into() })),
    )
}
