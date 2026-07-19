use super::super::super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/internal/projects/{project_id}/access",
        put(internal_upsert_project_access),
    )
}

async fn internal_upsert_project_access(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpsertProjectAccessRequest>,
) -> Result<Json<ProjectAccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        state
            .store
            .append_audit_record(
                &project_id,
                "",
                "internal.project_access.upsert",
                "project access record".to_string(),
                "deny",
                "missing or invalid internal service authorization",
            )
            .await;
        return Err(unauthorized(
            "project access update requires service authorization".to_string(),
        ));
    }
    if project_id.trim().is_empty() || request.owner_principal_id.trim().is_empty() {
        return Err(bad_request(
            "projectId and ownerPrincipalId are required".to_string(),
        ));
    }
    crate::types::validate_workspace_namespace(&request.workspace_namespace)
        .map_err(bad_request)?;
    let record = state
        .store
        .upsert_project_access(
            &project_id,
            request.owner_principal_id,
            request.workspace_namespace,
        )
        .await
        .map_err(|error| {
            if error.to_string().contains(" is immutable;") {
                conflict_error(error)
            } else {
                internal_error(error)
            }
        })?;
    state
        .store
        .append_audit_record(
            &project_id,
            "",
            "internal.project_access.upsert",
            "project access record".to_string(),
            "allow",
            "project access record persisted",
        )
        .await;
    Ok(Json(ProjectAccessResponse {
        project_access: record,
    }))
}
