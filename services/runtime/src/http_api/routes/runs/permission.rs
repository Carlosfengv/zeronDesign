use super::super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route(
        "/permissions/{permission_id}/decision",
        post(resolve_permission),
    )
}

async fn resolve_permission(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(service): Extension<RunLifecycleService>,
    Path(permission_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ResolvePermissionRequest>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("permissionId", &permission_id)?;
    let pending = state
        .store
        .pending_permission(&permission_id)
        .await
        .ok_or_else(|| not_found(format!("permission request not found: {permission_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &pending.project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    let decision = match request.decision {
        PermissionDecision::Allow => crate::run_lifecycle::PermissionDecision::Allow,
        PermissionDecision::Ask => crate::run_lifecycle::PermissionDecision::Ask,
        PermissionDecision::Deny => crate::run_lifecycle::PermissionDecision::Deny,
    };
    let outcome = service
        .resolve_permission(&permission_id, decision, request.updated_input)
        .await
        .map_err(run_lifecycle_error)?;
    Ok(Json(RunStatusResponse {
        run_id: outcome.run_id,
        status: outcome.status,
    }))
}
