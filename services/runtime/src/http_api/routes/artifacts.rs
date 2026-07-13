use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/artifacts/{project_id}/current",
            get(artifact_current_index),
        )
        .route(
            "/artifacts/{project_id}/current/",
            get(artifact_current_index),
        )
        .route(
            "/artifacts/{project_id}/current/{*artifact_path}",
            get(artifact_current_file),
        )
        .route(
            "/artifacts/{project_id}/versions/{version_id}",
            get(artifact_version_index),
        )
        .route(
            "/artifacts/{project_id}/versions/{version_id}/",
            get(artifact_version_index),
        )
        .route(
            "/artifacts/{project_id}/versions/{version_id}/{*artifact_path}",
            get(artifact_version_file),
        )
        .route("/_next/{*artifact_path}", get(next_artifact_asset_file))
}

async fn artifact_current_index(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(artifacts): Extension<ArtifactAccessService>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PREVIEW_READ_OPERATION,
    )
    .await?;
    artifact_response(&artifacts, &project_id, "").await
}

async fn artifact_current_file(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(artifacts): Extension<ArtifactAccessService>,
    Path((project_id, artifact_path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PREVIEW_READ_OPERATION,
    )
    .await?;
    artifact_response(&artifacts, &project_id, &artifact_path).await
}

async fn next_artifact_asset_file(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(artifacts): Extension<ArtifactAccessService>,
    Path(artifact_path): Path<String>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    let project_id = artifact_project_id_from_referer(&headers)
        .ok_or_else(|| not_found("Next artifact asset requires an artifact referer".to_string()))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PREVIEW_READ_OPERATION,
    )
    .await?;
    artifact_response(&artifacts, &project_id, &format!("_next/{artifact_path}")).await
}

async fn artifact_response(
    artifacts: &ArtifactAccessService,
    project_id: &str,
    artifact_path: &str,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    let content = artifacts
        .read_current(project_id, artifact_path)
        .await
        .map_err(artifact_read_error)?;
    present_artifact(content, &format!("/artifacts/{project_id}/current"))
}

async fn artifact_version_index(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(artifacts): Extension<ArtifactAccessService>,
    Path((project_id, version_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_version_response(
        &state,
        &policy,
        &artifacts,
        &headers,
        &project_id,
        &version_id,
        "",
    )
    .await
}

async fn artifact_version_file(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(artifacts): Extension<ArtifactAccessService>,
    Path((project_id, version_id, artifact_path)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_version_response(
        &state,
        &policy,
        &artifacts,
        &headers,
        &project_id,
        &version_id,
        &artifact_path,
    )
    .await
}

async fn artifact_version_response(
    state: &AppState,
    policy: &ApplicationAuthorizationPolicy,
    artifacts: &ArtifactAccessService,
    headers: &HeaderMap,
    project_id: &str,
    version_id: &str,
    artifact_path: &str,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(state, policy, headers, project_id, PREVIEW_READ_OPERATION).await?;
    let content = artifacts
        .read_version(project_id, version_id, artifact_path)
        .await
        .map_err(artifact_read_error)?;
    present_artifact(
        content,
        &format!("/artifacts/{project_id}/versions/{version_id}"),
    )
}

pub(in crate::http_api) fn artifact_project_id_from_referer(headers: &HeaderMap) -> Option<String> {
    let referer = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())?;
    let marker = "/artifacts/";
    let start = referer.find(marker)? + marker.len();
    let rest = &referer[start..];
    let end = rest.find("/current")?;
    let project_id = &rest[..end];
    (!project_id.trim().is_empty()).then(|| project_id.to_string())
}
