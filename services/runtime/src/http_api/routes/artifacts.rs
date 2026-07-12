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
        .route("/_next/{*artifact_path}", get(next_artifact_asset_file))
}

async fn artifact_current_index(
    Extension(artifacts): Extension<ArtifactAccessService>,
    Path(project_id): Path<String>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_response(&artifacts, &project_id, "").await
}

async fn artifact_current_file(
    Extension(artifacts): Extension<ArtifactAccessService>,
    Path((project_id, artifact_path)): Path<(String, String)>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_response(&artifacts, &project_id, &artifact_path).await
}

async fn next_artifact_asset_file(
    Extension(artifacts): Extension<ArtifactAccessService>,
    Path(artifact_path): Path<String>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    let project_id = artifact_project_id_from_referer(&headers)
        .ok_or_else(|| not_found("Next artifact asset requires an artifact referer".to_string()))?;
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
    present_artifact(content, project_id)
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
