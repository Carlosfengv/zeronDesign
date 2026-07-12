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
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_response(&state, &project_id, "").await
}

async fn artifact_current_file(
    State(state): State<AppState>,
    Path((project_id, artifact_path)): Path<(String, String)>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_response(&state, &project_id, &artifact_path).await
}

async fn next_artifact_asset_file(
    State(state): State<AppState>,
    Path(artifact_path): Path<String>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    let project_id = artifact_project_id_from_referer(&headers)
        .ok_or_else(|| not_found("Next artifact asset requires an artifact referer".to_string()))?;
    artifact_response(&state, &project_id, &format!("_next/{artifact_path}")).await
}

// remote-fs-boundary: allow-begin runtime-storage-artifact-serving
async fn artifact_response(
    state: &AppState,
    project_id: &str,
    artifact_path: &str,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    let current = state
        .store
        .current_project_version(project_id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "current artifact not found for project: {project_id}"
            ))
        })?;
    let output_root = FileArtifactPublisher::version_root(
        &state.config.runtime_storage_dir,
        project_id,
        &current.id,
    );
    if !output_root.is_dir() {
        return Err(not_found(format!(
            "immutable artifact output not found for version: {}",
            current.id
        )));
    }
    let publish = state
        .store
        .artifact_publish_for_version(project_id, &current.created_by_run_id, &current.id)
        .await;
    if let Some(expected_hash) = publish
        .as_ref()
        .and_then(|publish| publish.artifact_manifest_hash.as_deref())
    {
        let resolver = ArtifactResolver::load_for_version(
            &output_root,
            expected_hash,
            project_id,
            &current.id,
        )
        .map_err(conflict_error)?
        .ok_or_else(|| {
            conflict_error(anyhow::anyhow!(
                "promoted artifact manifest is missing for version {}",
                current.id
            ))
        })?;
        let resolved = resolver
            .resolve(artifact_path)
            .map_err(conflict_error)?
            .ok_or_else(|| not_found(format!("artifact not found: {artifact_path}")))?;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&resolved.content_type)
                .map_err(|error| conflict_error(anyhow::Error::new(error)))?,
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        return Ok((headers, resolved.bytes));
    }

    let (content_type, bytes) = read_legacy_artifact(&output_root, artifact_path, project_id)?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    Ok((headers, bytes))
}
// remote-fs-boundary: allow-end runtime-storage-artifact-serving

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
