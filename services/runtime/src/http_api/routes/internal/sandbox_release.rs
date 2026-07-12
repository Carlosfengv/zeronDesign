use super::super::super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/internal/projects/{project_id}/release-sandbox",
        post(internal_release_project_sandbox),
    )
}

async fn internal_release_project_sandbox(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        return Err(unauthorized(
            "sandbox release requires service authorization".to_string(),
        ));
    }
    let binding = state
        .store
        .current_project_sandbox_binding(&project_id)
        .await
        .ok_or_else(|| not_found(format!("sandbox binding not found: {project_id}")))?;
    let backend = sandbox_backend_for_config(&state.config);
    backend
        .release(&state.store, &binding.id)
        .await
        .map_err(internal_error)?;
    let released = state
        .store
        .get_sandbox_binding(&binding.id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "released sandbox binding not found: {}",
                binding.id
            ))
        })?;
    Ok(Json(json!({
        "projectId": project_id,
        "bindingId": released.id,
        "status": released.status,
        "releasedAt": released.last_seen_at,
    })))
}
