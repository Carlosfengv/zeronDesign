use super::super::super::*;
use crate::{
    design_context::{
        compile_website_design_context, frozen_run_design_context_manifest,
        DesignContextCompileOptions, DesignContextManifest,
    },
    profile_token_sync::{ProfileTokenSyncOperation, ProfileTokenSyncService, TokenSyncResolution},
    project::resolve_built_in_template_for_init,
    tools::control_plane::control_plane_executor_for_config,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use std::path::{Component, Path as FsPath};

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/runs/{run_id}/design-context-manifest",
            get(design_context_manifest),
        )
        .route(
            "/runs/{run_id}/design-context-diagnostics",
            get(design_context_diagnostics),
        )
        .route(
            "/runs/{run_id}/design-profile-sync-plan",
            post(plan_design_profile_sync),
        )
        .route(
            "/runs/{run_id}/design-profile-sync-operations/{operation_id}/confirm",
            post(confirm_design_profile_sync),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesignProfileSyncPlanRequest {
    target_design_profile_id: String,
    target_design_profile_version: u32,
    target_effective_profile_hash: String,
    expected_source_content_hash: String,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesignProfileSyncConfirmRequest {
    plan_hash: String,
    #[serde(default)]
    conflict_decisions: std::collections::BTreeMap<String, TokenSyncResolution>,
    idempotency_key: String,
}

async fn design_context_manifest(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let run = authorized_run(&state, &policy, &headers, &run_id).await?;
    let manifest = frozen_manifest(&run)?;
    Ok(Json(json!({
        "runId": run.id,
        "package": package_summary(&run, &manifest),
        "artifacts": manifest.artifact_manifest.artifacts.iter().map(|artifact| json!({
            "path": artifact.path,
            "kind": artifact.kind,
            "bytes": artifact.bytes,
            "sha256": artifact.sha256,
            "requiredBeforeMutation": artifact.required_before_mutation,
        })).collect::<Vec<_>>(),
    })))
}

async fn design_context_diagnostics(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let run = authorized_run(&state, &policy, &headers, &run_id).await?;
    let manifest = frozen_manifest(&run)?;
    let required_reads = manifest
        .payload
        .required_reads
        .iter()
        .filter(|requirement| requirement.phases.contains(&run.phase))
        .map(|requirement| json!({ "path": requirement.path, "reason": requirement.reason }))
        .collect::<Vec<_>>();
    let missing_required_reads = manifest
        .payload
        .required_reads
        .iter()
        .filter(|requirement| requirement.phases.contains(&run.phase))
        .filter(|requirement| !run.design_context_read_files.contains(&requirement.path))
        .map(|requirement| requirement.path.clone())
        .collect::<Vec<_>>();
    let fidelity = latest_fidelity_summary(&state, &run).await;
    Ok(Json(json!({
        "runId": run.id,
        "package": package_summary(&run, &manifest),
        "requiredReads": required_reads,
        "readFiles": run.design_context_read_files,
        "missingRequiredReads": missing_required_reads,
        "gate": if missing_required_reads.is_empty() { "ready" } else { "read_required" },
        "materialization": {
            "hash": run.design_context_materialization_hash,
            "ready": run.design_context_materialization_hash.is_some(),
        },
        "styleContract": { "verified": run.design_context_style_contract_verified },
        "verification": verification_summary(&run),
        "fidelity": fidelity,
    })))
}

async fn plan_design_profile_sync(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DesignProfileSyncPlanRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("runId", &run_id)?;
    validate_required_string("targetDesignProfileId", &request.target_design_profile_id)?;
    validate_required_string(
        "targetEffectiveProfileHash",
        &request.target_effective_profile_hash,
    )?;
    validate_required_string(
        "expectedSourceContentHash",
        &request.expected_source_content_hash,
    )?;
    validate_required_string("idempotencyKey", &request.idempotency_key)?;
    if request.target_design_profile_version == 0 {
        return Err(bad_request(
            "targetDesignProfileVersion must be positive".to_string(),
        ));
    }

    let (source_run, authorized_principal_id) =
        authorized_write_run(&state, &policy, &headers, &run_id).await?;
    let result: Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> = async {
        let source_manifest = frozen_manifest(&source_run)?;
        if source_manifest.content_hash != request.expected_source_content_hash {
            return Err(profile_sync_precondition_failed(
                "expectedSourceContentHash does not match the frozen source DCP".to_string(),
            ));
        }
        if source_manifest.payload.surface != "website" {
            return Err(profile_sync_precondition_failed(
                "profile sync is supported only for Website runs".to_string(),
            ));
        }

        let target_profile = state
            .store
            .design_profile_versions(&request.target_design_profile_id)
            .await
            .map_err(|error| not_found(error.to_string()))?
            .into_iter()
            .find(|profile| profile.version == request.target_design_profile_version)
            .ok_or_else(|| {
                not_found(format!(
                    "design profile version not found: {}@{}",
                    request.target_design_profile_id, request.target_design_profile_version
                ))
            })?;
        if target_profile.status != "active" {
            return Err(profile_sync_precondition_failed(
                "target DesignProfile revision must be active".to_string(),
            ));
        }
        if target_profile
            .project_id()
            .is_some_and(|project_id| project_id != source_run.project_id)
        {
            return Err(forbidden(
                "target DesignProfile is not visible to the source Run project".to_string(),
            ));
        }

        let brief_id = source_run.brief_version.as_deref().ok_or_else(|| {
            profile_sync_precondition_failed("source run is missing a frozen Brief".to_string())
        })?;
        let brief = state.store.get_brief(brief_id).await.ok_or_else(|| {
            profile_sync_precondition_failed(format!("source Brief not found: {brief_id}"))
        })?;
        let template = resolve_built_in_template_for_init(&source_manifest.payload.template)
            .await
            .map_err(|error| profile_sync_precondition_failed(error.to_string()))?;
        let effective = target_profile
            .effective_for(
                &source_manifest.payload.surface,
                &source_manifest.payload.template,
            )
            .map_err(|error| profile_sync_precondition_failed(error))?;
        if effective.effective_profile_hash != request.target_effective_profile_hash {
            return Err(profile_sync_precondition_failed(
                "targetEffectiveProfileHash does not match the requested target revision"
                    .to_string(),
            ));
        }
        let enforcement_enabled = match state
            .store
            .get_design_context_enforcement_policy(
                &source_run.project_id,
                &target_profile.id,
                target_profile.version,
            )
            .await
        {
            Some(policy) => policy.enabled,
            None => state
                .config
                .design_context_enforcement_allowed_for(
                    &source_run.project_id,
                    &target_profile.id,
                    target_profile.version,
                )
                .map_err(profile_sync_precondition_failed)?,
        };
        let target = compile_website_design_context(
            &effective,
            &brief,
            &template,
            &DesignContextCompileOptions {
                expected_app_root: source_manifest.payload.expected_app_root.clone(),
                compiler_version: source_manifest.payload.compiler_version.clone(),
                enforcement_enabled,
                verification_policy: source_manifest.payload.verification_policy.clone(),
            },
        )
        .map_err(profile_sync_precondition_failed)?;

        let contract_text =
            read_runtime_workspace_text(&state, &source_run, "state/style-contract.json").await?;
        let contract: Value = serde_json::from_str(&contract_text).map_err(|error| {
            profile_sync_precondition_failed(format!("style contract is not valid JSON: {error}"))
        })?;
        let token_file = contract
            .get("tokenFile")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                profile_sync_precondition_failed("style contract is missing tokenFile".to_string())
            })?;
        if !is_safe_workspace_relative_path(token_file) {
            return Err(profile_sync_precondition_failed(
                "style contract tokenFile is not a safe workspace-relative path".to_string(),
            ));
        }
        let token_file_content =
            read_runtime_workspace_text(&state, &source_run, token_file).await?;
        // Reading the current style contract records verification on the Run.
        // Re-fetch so planning cannot proceed with a pre-read `verified=true`
        // snapshot after the actual contract has drifted.
        let source_run = state
            .store
            .get_run(&run_id)
            .await
            .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
        let now = Utc::now();
        let operation = ProfileTokenSyncService::plan_operation(
            state.store.next_id("profile-sync"),
            &source_run,
            &target,
            &contract,
            &token_file_content,
            authorized_principal_id,
            request.idempotency_key,
            now + Duration::minutes(10),
            now,
        )
        .map_err(profile_sync_precondition_failed)?;
        let operation = state
            .store
            .create_profile_token_sync_operation(operation)
            .await
            .map_err(|error| profile_sync_precondition_failed(error.to_string()))?;
        record_profile_sync_metric(&state, &source_run.id, "plan", "planned", None).await;
        Ok(Json(profile_sync_operation_response(&operation)))
    }
    .await;
    if let Err((_, Json(error))) = &result {
        record_profile_sync_metric(
            &state,
            &source_run.id,
            "plan",
            "rejected",
            Some(
                error
                    .error_code
                    .as_deref()
                    .unwrap_or("profile_sync_precondition_failed"),
            ),
        )
        .await;
    }
    result
}

async fn confirm_design_profile_sync(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(service): Extension<RunLifecycleService>,
    Path((run_id, operation_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<DesignProfileSyncConfirmRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("runId", &run_id)?;
    validate_required_string("operationId", &operation_id)?;
    validate_required_string("planHash", &request.plan_hash)?;
    validate_required_string("idempotencyKey", &request.idempotency_key)?;
    let (run, _principal) = authorized_write_run(&state, &policy, &headers, &run_id).await?;
    let operation = state
        .store
        .profile_token_sync_operation(&operation_id)
        .await
        .ok_or_else(|| not_found(format!("profile sync operation not found: {operation_id}")))?;
    if operation.project_id != run.project_id || operation.source_run_id != run.id {
        return Err(not_found(format!(
            "profile sync operation not found for run: {operation_id}"
        )));
    }
    // The operation's plan key is the durable idempotency identity. Confirm is
    // naturally idempotent when the immutable plan hash and decisions match;
    // the client key is validated above and retained by the HTTP audit trail.
    let operation = match state
        .store
        .confirm_profile_token_sync_operation(
            &operation_id,
            &request.plan_hash,
            request.conflict_decisions,
            request.idempotency_key,
        )
        .await
    {
        Ok(operation) => operation,
        Err(error) => {
            let message = error.to_string();
            let reason = profile_sync_operation_error_code(&message);
            record_profile_sync_metric(&state, &run_id, "confirm", "rejected", Some(reason)).await;
            return Err(profile_sync_operation_error(message));
        }
    };
    record_profile_sync_metric(&state, &run_id, "confirm", "confirmed", None).await;
    if let Err(error) = service
        .apply_confirmed_profile_token_sync(&operation.id)
        .await
    {
        let reason = if error.to_string().contains("recovery_required") {
            "profile_sync_recovery_required"
        } else {
            "profile_sync_precondition_failed"
        };
        record_profile_sync_metric(&state, &run_id, "apply", "rejected", Some(reason)).await;
        return Err(run_lifecycle_error(error));
    }
    let operation = state
        .store
        .profile_token_sync_operation(&operation.id)
        .await
        .ok_or_else(|| {
            conflict_error(anyhow::anyhow!(
                "profile sync operation disappeared after lifecycle apply"
            ))
        })?;
    record_profile_sync_metric(&state, &run_id, "apply", "applied", None).await;
    Ok(Json(profile_sync_operation_response(&operation)))
}

async fn record_profile_sync_metric(
    state: &AppState,
    run_id: &str,
    stage: &str,
    status: &str,
    reason: Option<&str>,
) {
    let Some(run) = state.store.get_run(run_id).await else {
        return;
    };
    crate::tools::runtime::record_design_context_metric(
        &state.store,
        &run,
        "design_context_profile_sync_total",
        1,
        json!({
            "stage": stage,
            "status": status,
            "reason": reason,
        }),
    )
    .await;
}

async fn authorized_run(
    state: &AppState,
    policy: &ApplicationAuthorizationPolicy,
    headers: &HeaderMap,
    run_id: &str,
) -> Result<AgentRun, (StatusCode, Json<ErrorResponse>)> {
    let run = state
        .store
        .get_run(run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    authorize_project_operation(
        state,
        policy,
        headers,
        &run.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    Ok(run)
}

async fn authorized_write_run(
    state: &AppState,
    policy: &ApplicationAuthorizationPolicy,
    headers: &HeaderMap,
    run_id: &str,
) -> Result<(AgentRun, String), (StatusCode, Json<ErrorResponse>)> {
    let run = state
        .store
        .get_run(run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    let principal = authorize_project_operation(
        state,
        policy,
        headers,
        &run.project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    let principal_id = principal
        .map(|principal| principal.principal_id)
        .unwrap_or_else(|| "runtime-auth-disabled".to_string());
    Ok((run, principal_id))
}

async fn read_runtime_workspace_text(
    state: &AppState,
    run: &AgentRun,
    path: &str,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let result = control_plane_executor_for_config(&state.config)
        .with_workspace_root(crate::http_api::resolved_workspace_root(
            &state.config,
            &run.project_id,
        ))
        .execute(
            state.store.clone(),
            &run.id,
            &format!("bootstrap:profile-sync-read:{path}"),
            "fs.read",
            json!({ "path": path }),
        )
        .await
        .result;
    if result.is_error {
        return Err(profile_sync_precondition_failed(format!(
            "workspace read failed for profile sync: {}",
            result.content["error"].as_str().unwrap_or("unknown error")
        )));
    }
    result
        .content
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            profile_sync_precondition_failed(format!(
                "workspace read returned no text for profile sync path: {path}"
            ))
        })
}

fn is_safe_workspace_relative_path(path: &str) -> bool {
    !path.trim().is_empty()
        && FsPath::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn profile_sync_precondition_failed(
    message: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    profile_sync_error("profile_sync_precondition_failed", message)
}

fn profile_sync_operation_error(message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    let message = message.into();
    let code = profile_sync_operation_error_code(&message);
    profile_sync_error(code, message)
}

fn profile_sync_operation_error_code(message: &str) -> &'static str {
    if message.contains("expired") {
        "profile_sync_operation_expired"
    } else if message.contains("plan hash") {
        "profile_sync_plan_mismatch"
    } else if message.contains("conflict decision") {
        "profile_sync_conflict_decision_required"
    } else if message.contains("idempotency key") {
        "idempotency_key_reused"
    } else if message.contains("recovery_required") {
        "profile_sync_recovery_required"
    } else {
        "profile_sync_precondition_failed"
    }
}

fn profile_sync_error(code: &str, message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    let message = message.into();
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse {
            error: format!("{code}: {message}"),
            error_code: Some(code.to_string()),
        }),
    )
}

fn profile_sync_operation_response(operation: &ProfileTokenSyncOperation) -> Value {
    json!({
        "operationId": operation.id,
        "status": operation.status,
        "expiresAt": operation.expires_at,
        "planHash": operation.plan.plan_hash,
        "sourceContentHash": operation.source_design_context_content_hash,
        "targetDesignProfileId": operation.target_design_profile_id,
        "targetDesignProfileVersion": operation.target_design_profile_version,
        "targetEffectiveProfileHash": operation.target_effective_profile_hash,
        "styleContractIdentity": operation.style_contract_identity,
        "snapshots": {
            "baseHash": operation.plan.base.hash,
            "currentHash": operation.plan.current.hash,
            "targetHash": operation.plan.target.hash,
        },
        "items": operation.plan.items,
        "conflictDecisions": operation.conflict_decisions,
        "childRunId": operation.child_run_id,
    })
}

async fn latest_fidelity_summary(state: &AppState, run: &AgentRun) -> Option<Value> {
    let report = state
        .store
        .conversation_items(&run.project_id)
        .await
        .into_iter()
        .filter(|item| {
            item.run_id.as_deref() == Some(run.id.as_str())
                && item.kind == "design_profile_fidelity_checked"
        })
        .filter_map(|item| item.metadata)
        .last()?;
    let status = match report.get("status").and_then(Value::as_str) {
        Some("passed") => "passed",
        Some("failed") => "failed",
        _ => return None,
    };
    let assertions = report
        .get("assertions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(fidelity_assertion_summary)
        .take(64)
        .collect::<Vec<_>>();
    let required_failed_rule_ids =
        bounded_string_array(report.get("requiredFailedRuleIds"), 64, 200);
    let repair_context = report.get("repairContext").and_then(Value::as_object);
    let repair_targets = ["globalCssFile", "componentRoot"]
        .into_iter()
        .filter_map(|field| {
            repair_context
                .and_then(|context| context.get(field))
                .and_then(safe_workspace_display_path)
        })
        .collect::<Vec<_>>();
    let repair_instructions = bounded_string_array(
        repair_context.and_then(|context| context.get("instructions")),
        4,
        240,
    );
    Some(json!({
        "status": status,
        "checkedAt": bounded_string(report.get("checkedAt"), 80),
        "outputVersionId": bounded_string(report.get("outputVersionId"), 200),
        "requiredFailedRuleIds": required_failed_rule_ids,
        "assertions": assertions,
        "repairContext": {
            "targets": repair_targets,
            "instructions": repair_instructions,
        },
    }))
}

fn fidelity_assertion_summary(assertion: &Value) -> Option<Value> {
    let rule_id = bounded_string(assertion.get("ruleId"), 200)?;
    let kind = bounded_string(assertion.get("kind"), 80)?;
    let passed = assertion.get("passed").and_then(Value::as_bool)?;
    let actual = assertion
        .get("normalizedActual")
        .or_else(|| assertion.get("rawActual"));
    Some(json!({
        "ruleId": rule_id,
        "recipeId": bounded_string(assertion.get("recipeId"), 200),
        "priority": bounded_string(assertion.get("priority"), 40).unwrap_or_else(|| "preferred".to_string()),
        "kind": kind,
        "route": bounded_string(assertion.get("route"), 200).unwrap_or_else(|| "/".to_string()),
        "viewport": assertion.get("viewport").and_then(Value::as_u64),
        "selector": bounded_string(assertion.get("selector"), 240),
        "property": bounded_string(assertion.get("property"), 120),
        "actualSummary": fidelity_value_summary(&kind, actual),
        "expectedSummary": fidelity_value_summary(&kind, assertion.get("expected")),
        "comparator": bounded_string(assertion.get("comparator"), 80),
        "passed": passed,
        "reason": bounded_string(assertion.get("reason"), 320),
    }))
}

fn fidelity_value_summary(kind: &str, value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::Null => None,
        Value::Array(values) => Some(format!("{} item(s)", values.len())),
        Value::Object(values) => Some(format!("{} field(s)", values.len())),
        Value::String(value) if matches!(kind, "computed-style" | "viewport") => {
            Some(value.chars().take(160).collect())
        }
        Value::Number(value) if matches!(kind, "computed-style" | "viewport") => {
            Some(value.to_string())
        }
        Value::Bool(value) if matches!(kind, "computed-style" | "viewport") => {
            Some(value.to_string())
        }
        _ => Some("captured".to_string()),
    }
}

fn safe_workspace_display_path(value: &Value) -> Option<String> {
    let path = value.as_str()?.trim();
    let relative = path.strip_prefix("/workspace/").unwrap_or(path);
    is_safe_workspace_relative_path(relative).then(|| relative.to_string())
}

fn bounded_string(value: Option<&Value>, max_chars: usize) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty()).then(|| value.chars().take(max_chars).collect())
}

fn bounded_string_array(value: Option<&Value>, max_items: usize, max_chars: usize) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| bounded_string(Some(value), max_chars))
        .take(max_items)
        .collect()
}

fn frozen_manifest(
    run: &AgentRun,
) -> Result<DesignContextManifest, (StatusCode, Json<ErrorResponse>)> {
    frozen_run_design_context_manifest(run)
        .map_err(|error| {
            conflict_error(anyhow::anyhow!(
                "frozen Design Context identity is invalid: {error}"
            ))
        })?
        .ok_or_else(|| {
            not_found("frozen Design Context Package is not attached to this run".to_string())
        })
}

fn package_summary(run: &AgentRun, manifest: &DesignContextManifest) -> Value {
    json!({
        "version": run.design_context_package_version,
        "contentHash": run.design_context_content_hash,
        "artifactManifestHash": run.design_context_artifact_manifest_hash,
        "compilerVersion": run.design_context_compiler_version,
        "briefHash": run.design_context_brief_hash,
        "expectedAppRoot": run.design_context_expected_app_root,
        "declaredEnforcementMode": run.design_context_declared_enforcement_mode,
        "effectiveCompatibilityMode": run.design_context_effective_compatibility_mode,
        "verificationPolicyId": manifest.payload.verification_policy.policy_id,
        "warnings": run.design_context_warnings,
        "surface": manifest.payload.surface,
        "template": manifest.payload.template,
        "designProfileId": manifest.payload.design_profile_id,
        "designProfileVersion": manifest.payload.design_profile_version,
        "effectiveProfileHash": manifest.payload.effective_profile_hash,
    })
}

fn verification_summary(run: &AgentRun) -> Value {
    let environment = run
        .design_context_verification_environment
        .as_ref()
        .cloned()
        .unwrap_or(Value::Null);
    let capabilities = environment
        .get("capabilities")
        .and_then(Value::as_object)
        .map(|capabilities| {
            capabilities
                .iter()
                .map(|(kind, capability)| {
                    (
                        kind.clone(),
                        json!({
                            "available": capability
                                .get("available")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        }),
                    )
                })
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();
    json!({
        "policyId": run.design_context_verification_policy_id,
        "registryVersion": environment.get("registryVersion"),
        "capabilitySnapshotHash": environment.get("capabilitySnapshotHash"),
        "capabilities": capabilities,
    })
}
