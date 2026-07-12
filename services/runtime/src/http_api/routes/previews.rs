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
    State(state): State<AppState>,
    Path(lease_id): Path<String>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    proxy_candidate_preview(state, lease_id, String::new(), HeaderMap::new(), false).await
}

pub(in crate::http_api) async fn candidate_capture_file(
    State(state): State<AppState>,
    Path((lease_id, preview_path)): Path<(String, String)>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    proxy_candidate_preview(state, lease_id, preview_path, HeaderMap::new(), false).await
}

async fn candidate_preview_root(
    State(state): State<AppState>,
    Path(lease_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    proxy_candidate_preview(state, lease_id, String::new(), headers, true).await
}

async fn candidate_preview_file(
    State(state): State<AppState>,
    Path((lease_id, preview_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    proxy_candidate_preview(state, lease_id, preview_path, headers, true).await
}

async fn proxy_candidate_preview(
    state: AppState,
    lease_id: String,
    preview_path: String,
    headers: HeaderMap,
    public_access: bool,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let principal = if public_access {
        authenticate_candidate_preview(&state, &headers, &lease_id).await?
    } else {
        None
    };
    if preview_path
        .split('/')
        .any(|component| component == ".." || component.contains('\\'))
    {
        return Err(not_found("candidate preview path is invalid".to_string()));
    }
    let lease = state
        .store
        .get_preview_lease(&lease_id)
        .await
        .filter(|lease| lease.status == PreviewLeaseStatus::Active)
        .ok_or_else(|| not_found("candidate preview lease is unavailable".to_string()))?;
    authorize_candidate_preview(
        &state,
        principal.as_ref(),
        &lease_id,
        &lease.run_id,
        &lease.project_id,
    )
    .await?;
    let preview_prefix = if public_access {
        validated_preview_prefix(&state.config, &headers, &lease.project_id, &lease_id)?
    } else {
        format!("/preview-captures/{lease_id}")
    };
    let binding = state
        .store
        .get_sandbox_binding(&lease.sandbox_binding_id)
        .await
        .ok_or_else(|| not_found("candidate preview sandbox is unavailable".to_string()))?;
    if binding.sandbox_name != lease.sandbox_name
        || binding.pod_uid.as_deref() != Some(lease.pod_uid.as_str())
    {
        return Err(conflict_error(anyhow::anyhow!(
            "candidate preview sandbox identity changed"
        )));
    }

    let endpoint = ChannelManager::shared()
        .endpoint(&state.store, &binding, &lease.run_id, 4321, "http", "")
        .await
        .map_err(internal_error)?;
    let mut upstream =
        reqwest::Url::parse(&endpoint).map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    upstream.set_path(&format!("/candidates/{}/{}", lease.build_id, preview_path));
    let upstream_response = reqwest::Client::new()
        .get(upstream)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| not_found(format!("candidate preview upstream unavailable: {error}")))?;
    let status = upstream_response.status();
    if !status.is_success() {
        return Err(not_found(format!(
            "candidate preview file not found: {preview_path}"
        )));
    }
    let manifest_hash = upstream_response
        .headers()
        .get("x-anydesign-candidate-manifest-hash")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| not_found("candidate preview manifest evidence missing".to_string()))?;
    if manifest_hash != lease.candidate_manifest_hash {
        return Err(conflict_error(anyhow::anyhow!(
            "candidate preview manifest hash mismatch"
        )));
    }
    let content_type = upstream_response
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    let mut bytes = upstream_response
        .bytes()
        .await
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?
        .to_vec();
    if content_type
        .to_str()
        .ok()
        .is_some_and(|value| value.starts_with("text/html"))
    {
        if let Ok(html) = String::from_utf8(bytes.clone()) {
            bytes = html
                .replace("href=\"/", &format!("href=\"{preview_prefix}/"))
                .replace("src=\"/", &format!("src=\"{preview_prefix}/"))
                .into_bytes();
        }
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, no-store")
        .header("x-anydesign-preview-lease", lease_id)
        .body(Body::from(bytes))
        .map_err(|error| internal_error(anyhow::anyhow!(error)))
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
