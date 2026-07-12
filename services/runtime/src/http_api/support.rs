use super::*;

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

pub(in crate::http_api) fn run_lifecycle_service(
    state: &AppState,
    design_profiles: DesignProfileService,
) -> RunLifecycleService {
    RunLifecycleService::new(
        state.config.clone(),
        state.store.clone(),
        Arc::new(RuntimeSessionLauncher::new(
            state.config.clone(),
            state.store.clone(),
            state.model.clone(),
            state.supervisor.clone(),
        )),
        Arc::new(RuntimeBuildSandboxProvisioner::new(
            sandbox_backend_for_config(&state.config),
        )),
        Arc::new(RuntimeEditWorkspaceRestorer),
        design_profiles,
    )
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
