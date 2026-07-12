use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route("/runs", post(start_run))
}

async fn start_run(
    State(state): State<AppState>,
    Json(request): Json<StartRunRequest>,
) -> Result<Json<StartRunResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_start_run_request(&request)?;
    validate_project_access_before_initial_run(&state, &request).await?;
    validate_sandbox_context(&state.store, &request).await?;
    validate_project_lifecycle_context(&state.store, &request).await?;
    let design_profile = resolve_design_profile_context(&state.store, &request).await?;
    let design_profile_target = design_profile_execution_target(&state.store, &request).await?;
    let design_profile_conflict =
        preflight_design_profile_conflicts(&state.store, &request, design_profile.as_ref()).await?;
    let content_sources = merge_content_sources(
        inherited_build_content_sources(&state.store, &request).await,
        request.input_context.content_sources.clone(),
    );
    let run = if let Some(parent_run_id) = request.input_context.parent_run_id.as_deref() {
        if request.phase == AgentPhase::Repair {
            state
                .store
                .create_repair_run_for_findings(
                    parent_run_id,
                    &request.input_context.finding_ids,
                    None,
                    request.agent_profile,
                    state.config.agent_model.clone(),
                )
                .await
                .map_err(repair_run_error)?
        } else {
            state
                .store
                .create_child_run(
                    parent_run_id,
                    request.phase,
                    request.agent_profile,
                    state.config.agent_model.clone(),
                    None,
                    request.input_context.finding_ids,
                )
                .await
                .map_err(|_| not_found(format!("parent run not found: {parent_run_id}")))?
        }
    } else {
        state
            .store
            .create_run_with_context(
                request.project_id,
                request.phase,
                request.agent_profile,
                state.config.agent_model.clone(),
                content_sources,
                request.input_context.brief_id,
                request.input_context.base_version_id,
            )
            .await
    };
    let run = if let Some(profile) = design_profile.as_ref() {
        let effective_target = if design_profile_conflict.is_none() {
            design_profile_target.as_ref()
        } else {
            None
        };
        if let Some((surface, template)) = effective_target {
            state
                .store
                .attach_run_effective_design_profile(
                    &run.id,
                    profile,
                    Some(surface),
                    Some(template),
                )
                .await
                .map_err(design_profile_error)?
        } else {
            state
                .store
                .attach_run_design_profile(&run.id, profile)
                .await
                .map_err(design_profile_error)?
        }
    } else {
        run
    };
    let run = if let Some(profile) = design_profile.as_ref() {
        let configured = state
            .store
            .configure_run_design_fidelity(
                &run.id,
                profile,
                request.input_context.design_fidelity_mode.as_deref(),
            )
            .await
            .map_err(design_profile_error)?;
        if let Some(mode) = request.input_context.design_fidelity_mode.as_deref() {
            state
                .store
                .append_audit_record(
                    &run.project_id,
                    &run.id,
                    "design_profile.fidelity_mode",
                    format!("mode={mode}"),
                    "allow",
                    "explicit StartRun input",
                )
                .await;
        }
        configured
    } else {
        run
    };
    if let Some(profile) = design_profile.as_ref() {
        if let Some((blocked_state, message)) =
            design_profile_prebuild_failure(&state.store, &run, profile).await
        {
            state
                .store
                .append_conversation_item(
                    &run.project_id,
                    Some(&run.id),
                    "approval_request",
                    Some("assistant"),
                    &message,
                    Some(json!({
                        "state": blocked_state,
                        "designProfileId": profile.id,
                    })),
                )
                .await;
            state
                .store
                .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::StateChanged {
                    run_id: run.id.clone(),
                    state: blocked_state,
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            return Ok(Json(StartRunResponse {
                run_id: run.id,
                status: "needs_user_input",
            }));
        }
    }
    if !run.design_profile_unsupported_extended_tokens.is_empty() {
        state
            .store
            .append_audit_record(
                &run.project_id,
                &run.id,
                "design_profile.capability_gap",
                format!(
                    "unsupportedExtendedTokens={}",
                    run.design_profile_unsupported_extended_tokens.join(",")
                ),
                if run.design_profile_blocking_capability_rule_ids.is_empty() {
                    "allow"
                } else {
                    "ask"
                },
                "effective profile versus template style contract",
            )
            .await;
    }
    if !run.design_profile_blocking_capability_rule_ids.is_empty() {
        state
            .store
            .append_conversation_item(
                &run.project_id,
                Some(&run.id),
                "approval_request",
                Some("assistant"),
                "Required DesignProfile rules depend on template capabilities that are not supported.",
                Some(json!({
                    "state": "needs_user_input:design_profile_capability_gap",
                    "ruleIds": run.design_profile_blocking_capability_rule_ids,
                    "unsupportedExtendedTokens": run.design_profile_unsupported_extended_tokens,
                })),
            )
            .await;
        state
            .store
            .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
            .await
            .map_err(conflict_error)?;
        state
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: run.id.clone(),
                state: "needs_user_input:design_profile_capability_gap".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal_error)?;
        return Ok(Json(StartRunResponse {
            run_id: run.id,
            status: "needs_user_input",
        }));
    }
    if let Some(conflict_reason) = design_profile_conflict {
        state
            .store
            .append_conversation_item(
                &run.project_id,
                Some(&run.id),
                "approval_request",
                Some("assistant"),
                format!("DesignProfile conflict requires confirmation: {conflict_reason}"),
                Some(json!({
                    "reason": conflict_reason,
                    "designProfileId": run.design_profile_id.as_deref(),
                    "state": "needs_user_input:design_profile_conflict",
                })),
            )
            .await;
        state
            .store
            .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
            .await
            .map_err(conflict_error)?;
        state
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: run.id.clone(),
                state: "needs_user_input:design_profile_conflict".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal_error)?;
        return Ok(Json(StartRunResponse {
            run_id: run.id,
            status: "needs_user_input",
        }));
    }
    let run = if let Some(sandbox_binding_id) = request.input_context.sandbox_binding_id.as_deref()
    {
        state
            .store
            .bind_run_to_sandbox(&run.id, sandbox_binding_id)
            .await
            .map_err(sandbox_binding_error)?
    } else {
        run
    };
    let run = maybe_provision_build_sandbox(&state, run).await?;
    if sandbox_phase_requires_binding(run.phase) && run.sandbox_id.is_some() {
        let allowed_parent_run_id = request.input_context.parent_run_id.as_deref();
        if let Err(error) = state
            .store
            .acquire_sandbox_binding_for_run(&run.id, allowed_parent_run_id)
            .await
        {
            let _ = state
                .store
                .update_run_status(&run.id, AgentRunStatus::Cancelled)
                .await;
            return Err(sandbox_binding_error(error));
        }
    }
    if run.phase == AgentPhase::Edit {
        if let Err(error) = restore_edit_workspace_from_base_version(&state, &run).await {
            let _ = state
                .store
                .update_run_status(&run.id, AgentRunStatus::Cancelled)
                .await;
            return Err(conflict_error(error));
        }
    }
    let run_id = run.id.clone();
    if run.phase != AgentPhase::Edit {
        spawn_session(state, run_id.clone());
    }

    Ok(Json(StartRunResponse {
        run_id: run.id,
        status: "queued",
    }))
}

async fn validate_project_access_before_initial_run(
    state: &AppState,
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let production_auth_active = state.config.policy_profile
        == crate::config::RuntimePolicyProfile::Production
        && state.config.public_principal_auth_mode == PublicPrincipalAuthMode::Required
        && state.config.validate_startup().is_ok();
    if !production_auth_active
        || request.input_context.parent_run_id.is_some()
        || !matches!(request.phase, AgentPhase::Brief | AgentPhase::Build)
    {
        return Ok(());
    }
    let access = state
        .store
        .get_project_access(&request.project_id)
        .await
        .ok_or_else(|| {
            conflict_error(anyhow::anyhow!(
                "project access ownership must be registered before the initial run"
            ))
        })?;
    if request
        .input_context
        .workspace_id
        .as_deref()
        .is_some_and(|workspace_id| access.workspace_id.as_deref() != Some(workspace_id))
        || request
            .input_context
            .organization_id
            .as_deref()
            .is_some_and(|organization_id| {
                access.organization_id.as_deref() != Some(organization_id)
            })
    {
        return Err(conflict_error(anyhow::anyhow!(
            "project access scope identity drift detected"
        )));
    }
    Ok(())
}

async fn restore_edit_workspace_from_base_version(
    state: &AppState,
    run: &AgentRun,
) -> anyhow::Result<()> {
    let base_version_id = run
        .base_version_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("edit run missing baseVersionId"))?;
    let version = state
        .store
        .get_project_version(base_version_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("base version not found: {base_version_id}"))?;
    let source_snapshot_uri = version.source_snapshot_uri.as_deref().ok_or_else(|| {
        anyhow::anyhow!("base version {base_version_id} is missing sourceSnapshotUri")
    })?;
    restore_project_source_snapshot(&state.store, &state.config, run, source_snapshot_uri).await
}

async fn restore_project_source_snapshot(
    store: &RuntimeStore,
    config: &RuntimeConfig,
    run: &AgentRun,
    source_snapshot_uri: &str,
) -> anyhow::Result<()> {
    let workspace_root = effective_workspace_root(config, &run.project_id);
    let project_root = workspace_root.join("project");
    let mut ctx = ToolContext::new(store.clone(), run.clone(), workspace_root.clone());
    ctx.remote_workspace = config.sandbox_backend_mode == SandboxBackendMode::Kubernetes;
    ctx.runtime_storage_dir = config.runtime_storage_dir.clone();
    let backend: Box<dyn WorkspaceBackend> = match config.sandbox_backend_mode {
        SandboxBackendMode::Kubernetes => Box::new(
            SandboxChannelWorkspaceBackend::from_runtime_config(config)
                .map_err(|error| anyhow::anyhow!(error))?,
        ),
        SandboxBackendMode::PhaseAContract => Box::new(LocalWorkspaceBackend),
    };
    if let Err(error) = backend.remove_dir_all(&ctx, &project_root).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(anyhow::anyhow!(error));
        }
    }
    if let Some(runtime_path) = source_snapshot_uri.strip_prefix("runtime://source-snapshots/") {
        let segments = runtime_path.split('/').collect::<Vec<_>>();
        if segments.len() != 2 || segments.iter().any(|segment| segment.is_empty()) {
            return Err(anyhow::anyhow!("invalid Runtime source snapshot URI"));
        }
        let snapshot_project_id = segments[0];
        let snapshot_id = segments[1];
        if snapshot_project_id != safe_segment(&run.project_id) {
            return Err(anyhow::anyhow!("source snapshot project mismatch"));
        }
        for file in FileArtifactPublisher::read_source_snapshot(
            &config.runtime_storage_dir,
            &run.project_id,
            snapshot_id,
        )? {
            let target = project_root.join(&file.path);
            backend
                .write_bytes(&ctx, &target, &file.bytes)
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
            let restored = backend
                .read_bytes(&ctx, &target)
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
            if restored != file.bytes {
                return Err(anyhow::anyhow!(
                    "source snapshot integrity check failed after restore: {}",
                    file.path.display()
                ));
            }
        }
    } else {
        let snapshot_root =
            workspace_file_uri_to_workspace_path(&workspace_root, source_snapshot_uri)?;
        backend
            .copy_dir_all(&ctx, &snapshot_root, &project_root, &[])
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
    }
    let dependency_state = serde_json::to_string_pretty(&json!({
        "needsRestore": true,
        "reason": "source_snapshot_restored_without_node_modules",
        "sourceSnapshotUri": source_snapshot_uri,
        "markedAt": Utc::now().to_rfc3339(),
    }))?;
    backend
        .write_string(
            &ctx,
            &workspace_root.join("state/dependency-state.json"),
            &dependency_state,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

fn workspace_file_uri_to_workspace_path(
    workspace_root: &FsPath,
    uri: &str,
) -> anyhow::Result<PathBuf> {
    let path = uri
        .strip_prefix("file:///workspace/")
        .ok_or_else(|| anyhow::anyhow!("unsupported source snapshot URI: {uri}"))?;
    let relative = FsPath::new(path);
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow::anyhow!("source snapshot escapes workspace: {uri}"));
    }
    Ok(workspace_root.join(relative))
}

async fn inherited_build_content_sources(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Vec<ContentSource> {
    if request.phase != AgentPhase::Build {
        return Vec::new();
    }
    let Some(brief_id) = request.input_context.brief_id.as_deref() else {
        return Vec::new();
    };
    store
        .content_sources_for_brief(brief_id)
        .await
        .into_iter()
        .filter(|source| source.readable)
        .collect()
}

fn merge_content_sources(
    inherited: Vec<ContentSource>,
    explicit: Vec<ContentSource>,
) -> Vec<ContentSource> {
    let mut merged: Vec<ContentSource> = Vec::new();
    for source in inherited.into_iter().chain(explicit) {
        if let Some(index) = merged
            .iter()
            .position(|existing| existing.id == source.id || existing.kind == source.kind)
        {
            merged[index] = source;
        } else {
            merged.push(source);
        }
    }
    merged
}

async fn validate_sandbox_context(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let requested_binding = request.input_context.sandbox_binding_id.as_deref();

    if let Some(parent_run_id) = request.input_context.parent_run_id.as_deref() {
        let parent = store
            .get_run(parent_run_id)
            .await
            .ok_or_else(|| not_found(format!("parent run not found: {parent_run_id}")))?;
        if let (Some(parent_binding), Some(requested_binding)) =
            (parent.sandbox_id.as_deref(), requested_binding)
        {
            if parent_binding != requested_binding {
                return Err(conflict_error(anyhow::anyhow!(
                    "child run must use parent sandbox binding {parent_binding}, got {requested_binding}"
                )));
            }
        }
        if sandbox_phase_requires_binding(request.phase)
            && parent.sandbox_id.is_none()
            && requested_binding.is_none()
        {
            return Err(conflict_error(anyhow::anyhow!(
                "{:?} run requires sandboxBindingId or a parent run with an existing sandbox binding",
                request.phase
            )));
        }
        let binding_to_validate = requested_binding.or(parent.sandbox_id.as_deref());
        if let Some(binding_id) = binding_to_validate {
            validate_openable_sandbox_binding(store, binding_id, Some(parent_run_id)).await?;
        }
        return Ok(());
    }

    if let Some(binding_id) = requested_binding {
        validate_openable_sandbox_binding(store, binding_id, None).await?;
    }

    if request.phase == AgentPhase::Build {
        validate_build_confirmed_brief(store, request).await?;
    }

    if sandbox_phase_requires_binding(request.phase) && requested_binding.is_none() {
        if request.phase == AgentPhase::Build {
            return Ok(());
        }
        return Err(conflict_error(anyhow::anyhow!(
            "{:?} run requires sandboxBindingId",
            request.phase
        )));
    }

    Ok(())
}

async fn validate_build_confirmed_brief(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let brief_id = request.input_context.brief_id.as_deref().ok_or_else(|| {
        conflict_error(anyhow::anyhow!(
            "Build run requires a confirmed briefId before generation"
        ))
    })?;
    let brief = store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    if !store.is_brief_confirmed(brief_id).await {
        return Err(conflict_error(anyhow::anyhow!(
            "Build run requires a confirmed brief: {brief_id}"
        )));
    }
    resolve_built_in_template_for_init(&brief.recommended_template)
        .await
        .map_err(|error| conflict_error(anyhow::anyhow!(error.to_string())))?;
    Ok(())
}

pub(in crate::http_api) async fn validate_design_profile_template_availability(
    profile: &DesignProfile,
) -> Result<(), String> {
    let allowed_templates = profile
        .technical
        .get("allowedTemplates")
        .and_then(Value::as_array)
        .ok_or_else(|| "technical.allowedTemplates is required".to_string())?;
    for template in allowed_templates {
        let template = template
            .as_str()
            .ok_or_else(|| "technical.allowedTemplates must contain strings".to_string())?;
        resolve_built_in_template_for_init(template)
            .await
            .map_err(|error| format!("{}: {error}", error.error_kind()))?;
    }
    Ok(())
}

async fn validate_project_lifecycle_context(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if is_mutable_phase(request.phase) && request.input_context.parent_run_id.is_none() {
        if let Some(active) = store
            .active_mutable_run_for_project(&request.project_id)
            .await
        {
            return Err(conflict_error(anyhow::anyhow!(
                "project {} already has active mutable run {}",
                request.project_id,
                active.id
            )));
        }
    }

    if request.phase == AgentPhase::Edit {
        let base_version_id = request
            .input_context
            .base_version_id
            .as_deref()
            .ok_or_else(|| {
                conflict_error(anyhow::anyhow!(
                    "Edit run requires baseVersionId for lifecycle snapshot verification"
                ))
            })?;
        let current = store
            .current_project_version(&request.project_id)
            .await
            .ok_or_else(|| {
                conflict_error(anyhow::anyhow!(
                    "Edit run requires a promoted current version for project {}",
                    request.project_id
                ))
            })?;
        if current.id != base_version_id {
            return Err(conflict_error(anyhow::anyhow!(
                "Edit run baseVersionId {base_version_id} is stale; currentVersionId is {}",
                current.id
            )));
        }
        if current.source_snapshot_uri.is_none() {
            return Err(conflict_error(anyhow::anyhow!(
                "Edit run requires sourceSnapshotUri for baseVersionId {base_version_id}"
            )));
        }
    }

    Ok(())
}

fn is_mutable_phase(phase: AgentPhase) -> bool {
    matches!(
        phase,
        AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair | AgentPhase::Export
    )
}

async fn maybe_provision_build_sandbox(
    state: &AppState,
    run: AgentRun,
) -> Result<AgentRun, (StatusCode, Json<ErrorResponse>)> {
    if run.phase != AgentPhase::Build || run.sandbox_id.is_some() {
        return Ok(run);
    }
    let Some(brief_id) = run.brief_version.as_deref() else {
        return Ok(run);
    };
    let brief = state
        .store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    let backend = sandbox_backend_for_config(&state.config);
    let binding = match backend
        .claim(&state.store, &run.project_id, &brief.recommended_template)
        .await
    {
        Ok(binding) => binding,
        Err(error) => {
            let _ = state
                .store
                .update_run_status(&run.id, AgentRunStatus::Cancelled)
                .await;
            return Err(sandbox_binding_error(error));
        }
    };
    let binding = match backend
        .wait_ready(&state.store, &binding.id, Some(120_000))
        .await
    {
        Ok(binding) => binding,
        Err(error) => {
            let _ = backend.release(&state.store, &binding.id).await;
            let _ = state
                .store
                .update_run_status(&run.id, AgentRunStatus::Cancelled)
                .await;
            return Err(sandbox_binding_error(error));
        }
    };
    state
        .store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .map_err(sandbox_binding_error)
}

fn sandbox_phase_requires_binding(phase: AgentPhase) -> bool {
    matches!(
        phase,
        AgentPhase::Build | AgentPhase::Repair | AgentPhase::Review | AgentPhase::Edit
    )
}

async fn validate_openable_sandbox_binding(
    store: &RuntimeStore,
    binding_id: &str,
    allowed_parent_run_id: Option<&str>,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    store
        .ensure_sandbox_binding_available(binding_id, allowed_parent_run_id)
        .await
        .map(|_| ())
        .map_err(sandbox_binding_error)
}
