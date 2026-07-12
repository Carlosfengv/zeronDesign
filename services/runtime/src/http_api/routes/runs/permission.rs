use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route(
        "/permissions/{permission_id}/decision",
        post(resolve_permission),
    )
}

async fn resolve_permission(
    Extension(service): Extension<RunLifecycleService>,
    Path(permission_id): Path<String>,
    Json(request): Json<ResolvePermissionRequest>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("permissionId", &permission_id)?;
    let decision = match request.decision {
        PermissionDecision::Allow => crate::run_lifecycle::PermissionDecision::Allow,
        PermissionDecision::Ask => crate::run_lifecycle::PermissionDecision::Ask,
        PermissionDecision::Deny => crate::run_lifecycle::PermissionDecision::Deny,
    };
    let outcome = service
        .resolve_permission(&permission_id, decision, request.updated_input.is_some())
        .await
        .map_err(run_lifecycle_error)?;
    Ok(Json(RunStatusResponse {
        run_id: outcome.run_id,
        status: outcome.status,
    }))
}
