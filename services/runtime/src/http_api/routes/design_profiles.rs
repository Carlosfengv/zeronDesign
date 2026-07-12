use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/design-profiles",
            post(create_design_profile).get(list_design_profiles),
        )
        .route(
            "/design-profiles/{design_profile_id}",
            get(get_design_profile).put(update_design_profile),
        )
        .route(
            "/design-profiles/{design_profile_id}/versions",
            get(design_profile_versions),
        )
        .route(
            "/design-profiles/{design_profile_id}/diff",
            get(design_profile_diff),
        )
        .route(
            "/design-profiles/{design_profile_id}/archive",
            post(archive_design_profile),
        )
        .route(
            "/design-profiles/{design_profile_id}/activate",
            post(activate_design_profile),
        )
        .route(
            "/design-profiles/{design_profile_id}/conversion-report",
            get(current_design_profile_conversion_report),
        )
        .route(
            "/design-profiles/{design_profile_id}/versions/{version}/conversion-report",
            get(versioned_design_profile_conversion_report),
        )
        .route(
            "/design-profiles/{design_profile_id}/versions/{version}/fidelity-report",
            get(design_profile_fidelity_report),
        )
        .route(
            "/projects/{project_id}/design-profile",
            post(bind_project_design_profile).get(project_design_profile),
        )
}

async fn create_design_profile(
    Extension(service): Extension<DesignProfileService>,
    Json(request): Json<CreateDesignProfileRequest>,
) -> Result<Json<DesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_create_design_profile_request(&request)?;
    let payload = design_profile_payload_from_request(&request)?;
    let profile = service
        .create(crate::design_profile_service::CreateProfileCommand {
            project_id: request.project_id,
            name: request.name,
            payload,
        })
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(DesignProfileResponse {
        design_profile: profile.clone(),
        profile: Some(profile),
    }))
}

async fn list_design_profiles(
    Extension(service): Extension<DesignProfileService>,
    Query(query): Query<ListDesignProfilesQuery>,
) -> Result<Json<ListDesignProfilesResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_optional_string("projectId", query.project_id.as_deref())?;
    validate_optional_string("workspaceId", query.workspace_id.as_deref())?;
    validate_optional_string("organizationId", query.organization_id.as_deref())?;
    let design_profiles = service
        .list(crate::design_profile_service::ListProfilesQuery {
            project_id: query.project_id,
            workspace_id: query.workspace_id,
            organization_id: query.organization_id,
            include_archived: query.include_archived,
        })
        .await;
    Ok(Json(ListDesignProfilesResponse { design_profiles }))
}

async fn get_design_profile(
    Extension(service): Extension<DesignProfileService>,
    Path(design_profile_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let record = service
        .get(&design_profile_id)
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(json!({
        "designProfile": record,
        "profile": record,
    })))
}

async fn design_profile_versions(
    Extension(service): Extension<DesignProfileService>,
    Path(design_profile_id): Path<String>,
) -> Result<Json<DesignProfileVersionsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let versions = service
        .versions(&design_profile_id)
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(DesignProfileVersionsResponse {
        design_profile_id,
        versions,
    }))
}

async fn design_profile_diff(
    Extension(service): Extension<DesignProfileService>,
    Path(design_profile_id): Path<String>,
    Query(query): Query<DesignProfileDiffQuery>,
) -> Result<Json<DesignProfileDiffResponse>, (StatusCode, Json<ErrorResponse>)> {
    let changes = service
        .diff(&design_profile_id, query.from_version, query.to_version)
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(DesignProfileDiffResponse {
        design_profile_id,
        from_version: query.from_version,
        to_version: query.to_version,
        changes,
    }))
}

async fn archive_design_profile(
    Extension(service): Extension<DesignProfileService>,
    Path(design_profile_id): Path<String>,
) -> Result<Json<DesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    let profile = service
        .archive(&design_profile_id)
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(DesignProfileResponse {
        design_profile: profile.clone(),
        profile: Some(profile),
    }))
}

async fn activate_design_profile(
    State(state): State<AppState>,
    Extension(service): Extension<DesignProfileService>,
    Path(design_profile_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ActivateDesignProfileRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_design_source_authorization(&state.config, &headers)
        .map_err(error_response_as_value)?;
    let profile = service
        .activate(&design_profile_id, request.expected_version)
        .await
        .map_err(design_profile_activation_error)?;
    Ok(Json(json!({
        "designProfile": profile,
        "profile": profile,
    })))
}

async fn current_design_profile_conversion_report(
    State(state): State<AppState>,
    Extension(service): Extension<DesignProfileService>,
    Path(design_profile_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<DesignProfileConversionReport>, (StatusCode, Json<ErrorResponse>)> {
    require_design_source_authorization(&state.config, &headers)?;
    let report = service
        .conversion_report(&design_profile_id, None)
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(report))
}

async fn versioned_design_profile_conversion_report(
    State(state): State<AppState>,
    Extension(service): Extension<DesignProfileService>,
    Path((design_profile_id, version)): Path<(String, u32)>,
    headers: HeaderMap,
) -> Result<Json<DesignProfileConversionReport>, (StatusCode, Json<ErrorResponse>)> {
    require_design_source_authorization(&state.config, &headers)?;
    let report = service
        .conversion_report(&design_profile_id, Some(version))
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(report))
}

async fn design_profile_fidelity_report(
    Extension(service): Extension<DesignProfileService>,
    Path((design_profile_id, version)): Path<(String, u32)>,
    Query(query): Query<DesignProfileFidelityQuery>,
) -> Result<Json<DesignProfileFidelityReport>, (StatusCode, Json<ErrorResponse>)> {
    let surface = query
        .surface
        .as_deref()
        .ok_or_else(|| bad_request("surface is required".to_string()))?;
    let template = query
        .template
        .as_deref()
        .ok_or_else(|| bad_request("template is required".to_string()))?;
    let report = service
        .fidelity_report(&design_profile_id, version, surface, template)
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(report))
}

async fn update_design_profile(
    Extension(service): Extension<DesignProfileService>,
    Path(design_profile_id): Path<String>,
    Json(request): Json<UpdateDesignProfileRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    let profile = service
        .update(crate::design_profile_service::UpdateProfileCommand {
            design_profile_id,
            expected_version: request.expected_version,
            name: request.name,
            profile: request.profile,
        })
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(json!({
        "designProfile": profile,
        "profile": profile,
    })))
}

async fn bind_project_design_profile(
    Extension(service): Extension<DesignProfileService>,
    Path(project_id): Path<String>,
    Json(request): Json<BindProjectDesignProfileRequest>,
) -> Result<Json<ProjectDesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    let profile = service
        .bind_project(&project_id, &request.design_profile_id)
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(ProjectDesignProfileResponse {
        project_id,
        design_profile: Some(profile.clone()),
        profile: Some(profile),
    }))
}

async fn project_design_profile(
    Extension(service): Extension<DesignProfileService>,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectDesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    let profile = service
        .project_profile(&project_id)
        .await
        .map_err(design_profile_service_error)?;
    Ok(Json(ProjectDesignProfileResponse {
        project_id,
        design_profile: profile.clone(),
        profile,
    }))
}
