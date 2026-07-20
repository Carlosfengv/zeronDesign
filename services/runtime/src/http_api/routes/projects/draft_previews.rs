use super::super::super::*;

pub(super) async fn current_draft_preview(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::visual_contracts::DraftPreviewSession>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    state
        .store
        .draft_preview_store()
        .active_for_project(&project_id)
        .map(Json)
        .ok_or_else(|| {
            not_found(format!(
                "active DraftPreviewSession not found: {project_id}"
            ))
        })
}

pub(super) async fn get_draft_preview(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<crate::visual_contracts::DraftPreviewSession>, (StatusCode, Json<ErrorResponse>)> {
    let session = state
        .store
        .draft_preview_store()
        .get(&session_id)
        .ok_or_else(|| not_found(format!("DraftPreviewSession not found: {session_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &session.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    Ok(Json(session))
}

pub(super) async fn heartbeat_draft_preview(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DraftPreviewHeartbeatRequest>,
) -> Result<Json<crate::visual_contracts::DraftPreviewSession>, (StatusCode, Json<ErrorResponse>)> {
    let session = state
        .store
        .draft_preview_store()
        .get(&session_id)
        .ok_or_else(|| not_found(format!("DraftPreviewSession not found: {session_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &session.project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    state
        .store
        .draft_preview_store()
        .heartbeat(
            &session_id,
            &request.writer_lease_id,
            request.session_epoch,
            request.ttl_seconds,
        )
        .map(Json)
        .map_err(draft_preview_error)
}

pub(super) async fn takeover_draft_preview(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DraftPreviewTakeoverRequest>,
) -> Result<Json<crate::visual_contracts::DraftPreviewSession>, (StatusCode, Json<ErrorResponse>)> {
    let session = state
        .store
        .draft_preview_store()
        .get(&session_id)
        .ok_or_else(|| not_found(format!("DraftPreviewSession not found: {session_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &session.project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    state
        .store
        .draft_preview_store()
        .takeover(
            &session_id,
            request.expected_session_epoch,
            request.ttl_seconds,
        )
        .map(Json)
        .map_err(draft_preview_error)
}

fn draft_preview_error(
    error: crate::draft_preview::DraftPreviewStoreError,
) -> (StatusCode, Json<ErrorResponse>) {
    use crate::draft_preview::DraftPreviewStoreError;
    match error {
        DraftPreviewStoreError::InvalidInput(message) => bad_request(message),
        DraftPreviewStoreError::NotFound(message) => not_found(message),
        DraftPreviewStoreError::Conflict(message)
        | DraftPreviewStoreError::InvalidTransition(message) => {
            conflict_error(anyhow::anyhow!(message))
        }
        DraftPreviewStoreError::Storage(message) => internal_error(anyhow::anyhow!(message)),
    }
}
