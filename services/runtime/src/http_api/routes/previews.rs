use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route("/preview/{project_id}/current", get(preview_current))
        .route("/preview/{project_id}/{version_id}", get(preview_version))
        .route("/previews/{lease_id}", get(candidate_preview_root))
        .route("/previews/{lease_id}/", get(candidate_preview_root))
        .route(
            "/previews/{lease_id}/{*preview_path}",
            get(candidate_preview_file),
        )
}

pub(in crate::http_api) async fn candidate_capture_root(
    Extension(preview_access): Extension<PreviewAccessService>,
    Path(lease_id): Path<String>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    proxy_candidate_preview(
        preview_access,
        lease_id,
        String::new(),
        None,
        false,
        PreviewAccessContext::InternalCapture,
    )
    .await
}

pub(in crate::http_api) async fn candidate_capture_file(
    Extension(preview_access): Extension<PreviewAccessService>,
    Path((lease_id, preview_path)): Path<(String, String)>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    proxy_candidate_preview(
        preview_access,
        lease_id,
        preview_path,
        None,
        false,
        PreviewAccessContext::InternalCapture,
    )
    .await
}

async fn candidate_preview_root(
    State(state): State<AppState>,
    Extension(preview_access): Extension<PreviewAccessService>,
    Path(lease_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let principal = authenticate_candidate_preview(&state, &headers, &lease_id).await?;
    let requested_prefix = preview_prefix_header(&headers)?;
    let prefix_required =
        state.config.public_principal_auth_mode != PublicPrincipalAuthMode::Disabled;
    proxy_candidate_preview(
        preview_access,
        lease_id,
        String::new(),
        requested_prefix,
        prefix_required,
        PreviewAccessContext::Public(principal.as_ref()),
    )
    .await
}

async fn candidate_preview_file(
    State(state): State<AppState>,
    Extension(preview_access): Extension<PreviewAccessService>,
    Path((lease_id, preview_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let principal = authenticate_candidate_preview(&state, &headers, &lease_id).await?;
    let requested_prefix = preview_prefix_header(&headers)?;
    let prefix_required =
        state.config.public_principal_auth_mode != PublicPrincipalAuthMode::Disabled;
    proxy_candidate_preview(
        preview_access,
        lease_id,
        preview_path,
        requested_prefix,
        prefix_required,
        PreviewAccessContext::Public(principal.as_ref()),
    )
    .await
}

fn preview_prefix_header(
    headers: &HeaderMap,
) -> Result<Option<String>, (StatusCode, Json<ErrorResponse>)> {
    headers
        .get("x-anydesign-preview-prefix")
        .map(|value| {
            value
                .to_str()
                .map(str::to_string)
                .map_err(|_| bad_request("x-anydesign-preview-prefix is invalid".to_string()))
        })
        .transpose()
}

async fn preview_current(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<PreviewCurrentResponse>, (StatusCode, Json<ErrorResponse>)> {
    let version = state
        .store
        .current_project_version(&project_id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "current preview not found for project: {project_id}"
            ))
        })?;
    Ok(Json(PreviewCurrentResponse {
        project_id,
        version_id: version.id,
        preview_url: version.preview_url,
        status: "promoted",
    }))
}

async fn preview_version(
    State(state): State<AppState>,
    Path((project_id, version_id)): Path<(String, String)>,
) -> Result<Json<PreviewVersionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let version = state
        .store
        .get_project_version(&version_id)
        .await
        .ok_or_else(|| not_found(format!("project version not found: {version_id}")))?;
    if version.project_id != project_id {
        return Err(not_found(format!(
            "project version {version_id} not found for project: {project_id}"
        )));
    }
    Ok(Json(PreviewVersionResponse {
        project_id,
        version_id: version.id,
        preview_url: version.preview_url,
        status: serde_json::to_value(version.status)
            .ok()
            .and_then(|status| status.as_str().map(ToString::to_string))
            .unwrap_or_else(|| "unknown".to_string()),
    }))
}
