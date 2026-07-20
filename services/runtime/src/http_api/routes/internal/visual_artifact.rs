use super::super::super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/internal/runs/{run_id}/visual-artifacts/{artifact_id}/content",
            get(internal_bound_visual_artifact_content),
        )
        .route(
            "/internal/visual-artifacts/gc",
            post(internal_visual_artifact_gc),
        )
}

async fn internal_visual_artifact_gc(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        return Err(unauthorized(
            "visual artifact GC requires service authorization".to_string(),
        ));
    }
    let purged =
        VisualArtifactStore::open(state.config.runtime_storage_dir.join("visual-artifacts"))
            .and_then(|store| store.purge_deletion_pending(Utc::now()))
            .map_err(visual_artifact_error)?;
    Ok(Json(json!({ "purgedArtifactIds": purged })))
}

async fn internal_bound_visual_artifact_content(
    State(state): State<AppState>,
    Path((run_id, artifact_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        return Err(unauthorized(
            "visual artifact content requires service authorization".to_string(),
        ));
    }
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    let bound = state
        .store
        .run_visual_bindings(&run_id)
        .await
        .map_err(visual_artifact_error)?
        .into_iter()
        .any(|binding| binding.artifact_id == artifact_id);
    if !bound {
        return Err(not_found(format!(
            "visual artifact is not bound to run: {artifact_id}"
        )));
    }
    let artifact = crate::http_api::routes::projects::load_project_visual_artifact(
        &state,
        &run.project_id,
        &artifact_id,
    )
    .await?;
    let root = state.config.runtime_storage_dir.join("visual-artifacts");
    let content_id = artifact_id.clone();
    let content = tokio::task::spawn_blocking(move || {
        VisualArtifactStore::open(root)?.read_content(&content_id)
    })
    .await
    .map_err(|error| internal_error(anyhow::anyhow!("visual artifact task failed: {error}")))?
    .map_err(visual_artifact_error)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response_headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response_headers.insert(
        "x-visual-artifact-sha256",
        HeaderValue::from_str(&artifact.sha256)
            .map_err(|_| internal_error(anyhow::anyhow!("invalid stored visual artifact hash")))?,
    );
    response_headers.insert(
        "x-visual-artifact-width",
        HeaderValue::from_str(&artifact.width.to_string())
            .map_err(|_| internal_error(anyhow::anyhow!("invalid visual artifact width")))?,
    );
    response_headers.insert(
        "x-visual-artifact-height",
        HeaderValue::from_str(&artifact.height.to_string())
            .map_err(|_| internal_error(anyhow::anyhow!("invalid visual artifact height")))?,
    );
    Ok((response_headers, content))
}
