use super::super::super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/internal/previews/promote", post(internal_promote_preview))
}

async fn internal_promote_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PromotePreviewRequest>,
) -> Result<Json<PreviewCurrentResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_promote_preview_request(&request)?;
    if !state.config.enable_internal_promote_api {
        return Err(not_found(
            "internal preview promotion endpoint is disabled".to_string(),
        ));
    }
    if !internal_admin_authorized(&state.config, &headers) {
        state
            .store
            .append_audit_record(
                &request.project_id,
                &request.run_id,
                "internal.previews.promote",
                format!("candidateVersionId={}", request.candidate_version_id),
                "deny",
                "missing or invalid internal promote authorization",
            )
            .await;
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "internal preview promotion requires service authorization".to_string(),
            }),
        ));
    }
    let version = promote_preview(
        &state.store,
        &request.project_id,
        &request.run_id,
        &request.candidate_version_id,
        request.gate_report.into(),
    )
    .await
    .map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    state
        .store
        .append_audit_record(
            &request.project_id,
            &request.run_id,
            "internal.previews.promote",
            format!("candidateVersionId={}", version.id),
            "allow",
            "internal preview promotion API",
        )
        .await;
    Ok(Json(PreviewCurrentResponse {
        project_id: request.project_id,
        version_id: version.id,
        preview_url: version.preview_url,
        status: "promoted",
    }))
}
