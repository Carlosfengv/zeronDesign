use super::super::super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/internal/projects/{project_id}/release-evidence",
        get(internal_project_release_evidence),
    )
}

async fn internal_project_release_evidence(
    State(state): State<AppState>,
    Extension(service): Extension<ReleaseEvidenceService>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        return Err(unauthorized(
            "release evidence requires service authorization".to_string(),
        ));
    }
    let evidence = service
        .project_release_evidence(&project_id)
        .await
        .map_err(release_evidence_error)?;
    Ok(Json(evidence))
}

fn release_evidence_error(error: ReleaseEvidenceError) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        ReleaseEvidenceError::NotFound(message) => not_found(message),
        ReleaseEvidenceError::Conflict(message) => conflict_error(anyhow::anyhow!(message)),
        ReleaseEvidenceError::Internal(message) => internal_error(anyhow::anyhow!(message)),
    }
}
