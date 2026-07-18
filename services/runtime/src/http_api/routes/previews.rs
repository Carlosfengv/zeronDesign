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

pub(in crate::http_api) async fn candidate_capture_host_root(
    Extension(preview_access): Extension<PreviewAccessService>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    candidate_capture_host_response(preview_access, capture_host(&headers)?, String::new()).await
}

pub(in crate::http_api) async fn candidate_capture_host_file(
    Extension(preview_access): Extension<PreviewAccessService>,
    Path(preview_path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    candidate_capture_host_response(preview_access, capture_host(&headers)?, preview_path).await
}

async fn candidate_capture_host_response(
    preview_access: PreviewAccessService,
    host: &str,
    preview_path: String,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let lease_id = capture_lease_id_from_host(host)
        .ok_or_else(|| not_found("candidate capture host is invalid".to_string()))?;
    proxy_candidate_preview(
        preview_access,
        lease_id,
        preview_path,
        None,
        false,
        PreviewAccessContext::InternalCaptureHost,
    )
    .await
}

fn capture_host(headers: &HeaderMap) -> Result<&str, (StatusCode, Json<ErrorResponse>)> {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| not_found("candidate capture host is missing".to_string()))
}

fn capture_lease_id_from_host(host: &str) -> Option<String> {
    let hostname = host.split(':').next()?;
    let lease_id = hostname.strip_suffix(".preview.local")?;
    (!lease_id.is_empty()
        && lease_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
    .then(|| lease_id.to_string())
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
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<PreviewCurrentResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PREVIEW_READ_OPERATION,
    )
    .await?;
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
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, version_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<PreviewVersionResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PREVIEW_READ_OPERATION,
    )
    .await?;
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

#[cfg(test)]
mod tests {
    use super::capture_lease_id_from_host;

    #[test]
    fn capture_host_accepts_only_a_single_safe_lease_label() {
        assert_eq!(
            capture_lease_id_from_host("lease-123.preview.local:8081").as_deref(),
            Some("lease-123")
        );
        assert!(capture_lease_id_from_host("preview.local:8081").is_none());
        assert!(capture_lease_id_from_host("bad.name.preview.local:8081").is_none());
    }
}
