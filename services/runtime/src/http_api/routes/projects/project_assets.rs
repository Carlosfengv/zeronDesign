use super::super::super::*;

pub(super) async fn list_project_assets(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::visual_contracts::ProjectAsset>>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let store =
        ProjectAssetStore::open(&state.config.runtime_storage_dir).map_err(project_asset_error)?;
    Ok(Json(store.list_project(&project_id)))
}

pub(super) async fn get_project_asset(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<crate::visual_contracts::ProjectAsset>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let store =
        ProjectAssetStore::open(&state.config.runtime_storage_dir).map_err(project_asset_error)?;
    store
        .get(&asset_id)
        .filter(|asset| asset.project_id == project_id)
        .map(Json)
        .ok_or_else(|| not_found(format!("ProjectAsset not found: {asset_id}")))
}

fn project_asset_error(error: ProjectAssetError) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        ProjectAssetError::Invalid(message) => bad_request(message),
        ProjectAssetError::NotFound(message) => not_found(message),
        ProjectAssetError::Conflict(message) => conflict_error(anyhow::anyhow!(message)),
        ProjectAssetError::Storage(message) => internal_error(anyhow::anyhow!(message)),
    }
}
