use super::*;

pub(in crate::http_api) fn validate_start_run_request(
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("projectId", &request.project_id)?;
    validate_required_string("agentProfile", &request.agent_profile)?;
    for source in &request.input_context.content_sources {
        validate_required_string("contentSources[].id", &source.id)?;
        validate_required_string("contentSources[].kind", &source.kind)?;
    }
    validate_optional_string("briefId", request.input_context.brief_id.as_deref())?;
    validate_optional_string(
        "baseVersionId",
        request.input_context.base_version_id.as_deref(),
    )?;
    validate_optional_string(
        "sandboxBindingId",
        request.input_context.sandbox_binding_id.as_deref(),
    )?;
    validate_optional_string(
        "parentRunId",
        request.input_context.parent_run_id.as_deref(),
    )?;
    validate_optional_string(
        "designProfileId",
        request.input_context.design_profile_id.as_deref(),
    )?;
    if let Some(mode) = request.input_context.design_fidelity_mode.as_deref() {
        if !matches!(mode, "profile_only" | "source_fallback") {
            return Err(bad_request(
                "designFidelityMode must be profile_only or source_fallback".to_string(),
            ));
        }
    }
    validate_optional_string("workspaceId", request.input_context.workspace_id.as_deref())?;
    validate_optional_string(
        "organizationId",
        request.input_context.organization_id.as_deref(),
    )?;
    validate_string_list("findingIds", &request.input_context.finding_ids)?;
    Ok(())
}

pub(in crate::http_api) fn validate_create_design_profile_request(
    request: &CreateDesignProfileRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("name", &request.name)?;
    validate_optional_string("projectId", request.project_id.as_deref())?;
    if request.profile.is_none() && request.legacy_profile.is_empty() {
        return Err(bad_request("profile is required".to_string()));
    }
    Ok(())
}

pub(in crate::http_api) fn validate_internal_template_build_request(
    request: &InternalTemplateBuildRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("projectId", &request.project_id)?;
    validate_required_string("template", &request.template)?;
    validate_required_string("audience", &request.audience)?;
    validate_required_string("visualDirection", &request.visual_direction)?;
    validate_string_list("contentHierarchy", &request.content_hierarchy)?;
    validate_string_list("assumptions", &request.assumptions)?;
    validate_string_list("missingInformation", &request.missing_information)?;
    validate_optional_string("model", request.model.as_deref())?;
    validate_optional_string("publicBaseUrl", request.public_base_url.as_deref())?;
    Ok(())
}

pub(in crate::http_api) fn validate_promote_preview_request(
    request: &PromotePreviewRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("projectId", &request.project_id)?;
    validate_required_string("runId", &request.run_id)?;
    validate_required_string("candidateVersionId", &request.candidate_version_id)?;
    Ok(())
}

pub(in crate::http_api) fn validate_optional_string(
    field: &str,
    value: Option<&str>,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if let Some(value) = value {
        validate_required_string(field, value)?;
    }
    Ok(())
}

pub(in crate::http_api) fn validate_string_list(
    field: &str,
    values: &[String],
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    for value in values {
        if value.trim().is_empty() {
            return Err(bad_request(format!(
                "{field} must not contain empty strings"
            )));
        }
    }
    Ok(())
}

pub(in crate::http_api) fn validate_required_string(
    field: &str,
    value: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if value.trim().is_empty() {
        return Err(bad_request(format!("{field} must not be empty")));
    }
    Ok(())
}

pub(in crate::http_api) fn last_event_sequence(last_event_id: Option<&str>, run_id: &str) -> usize {
    let Some(last_event_id) = last_event_id else {
        return 0;
    };
    let Some((id_run_id, sequence)) = last_event_id.rsplit_once('/') else {
        return 0;
    };
    if id_run_id != run_id {
        return 0;
    }
    sequence.parse::<usize>().unwrap_or(0)
}

pub(crate) fn spawn_session(state: AppState, run_id: String) {
    let task_name = format!("session/{run_id}");
    let supervisor = state.supervisor.clone();
    let config = state.config;
    let store = state.store;
    let model = state.model;
    let _ = supervisor.spawn(task_name, false, async move {
        let tool_executor = if let Some(run) = store.get_run(&run_id).await {
            let workspace_root = effective_workspace_root(&config, &run.project_id);
            if config.sandbox_backend_mode == SandboxBackendMode::PhaseAContract {
                // remote-fs-boundary: allow-begin phase-a-workspace-bootstrap
                let _ = fs::create_dir_all(&workspace_root);
                // remote-fs-boundary: allow-end phase-a-workspace-bootstrap
            }
            control_plane_executor_for_config(&config).with_workspace_root(workspace_root)
        } else {
            control_plane_executor_for_config(&config)
        };
        let session = QuerySession::with_tool_executor(store, model, tool_executor);
        let _ = session.submit_run(&run_id).await;
        Ok(())
    });
}

pub(in crate::http_api) fn require_design_source_authorization(
    config: &RuntimeConfig,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if internal_admin_authorized(config, headers) {
        return Ok(());
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: "design source artifacts require service authorization".to_string(),
        }),
    ))
}

pub(in crate::http_api) async fn validate_design_profile_source_reference(
    store: &RuntimeStore,
    profile: &DesignProfile,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let Some(artifact_id) = profile
        .source
        .get("primarySourceArtifactId")
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    validate_required_string("profile.source.primarySourceArtifactId", artifact_id)?;
    let artifact = store
        .get_design_source_artifact(artifact_id)
        .await
        .ok_or_else(|| not_found(format!("design source artifact not found: {artifact_id}")))?;
    if artifact.scope != profile.scope {
        return Err(bad_request(
            "profile source artifact scope must exactly match profile scope".to_string(),
        ));
    }
    let source_hash = profile
        .source
        .get("sourceHash")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            bad_request(
                "profile.source.sourceHash is required with primarySourceArtifactId".to_string(),
            )
        })?;
    if !artifact.sha256.eq_ignore_ascii_case(source_hash) {
        return Err(bad_request(
            "profile.source.sourceHash does not match the referenced artifact".to_string(),
        ));
    }
    store
        .read_design_source_artifact_content(artifact_id)
        .await
        .map_err(design_source_error)?;
    Ok(())
}
