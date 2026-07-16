use super::super::super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/internal/projects/{project_id}/design-context-enforcement",
        put(internal_upsert_design_context_enforcement),
    )
}

async fn internal_upsert_design_context_enforcement(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpsertDesignContextEnforcementPolicyRequest>,
) -> Result<Json<DesignContextEnforcementPolicyResponse>, (StatusCode, Json<ErrorResponse>)> {
    let summary = format!(
        "designProfileId={} designProfileVersion={} enabled={}",
        request.design_profile_id, request.design_profile_version, request.enabled
    );
    if !internal_admin_authorized(&state.config, &headers) {
        state
            .store
            .append_audit_record(
                &project_id,
                "",
                "internal.design_context_enforcement.upsert",
                summary,
                "deny",
                "missing or invalid internal service authorization",
            )
            .await;
        return Err(unauthorized(
            "design context enforcement update requires service authorization".to_string(),
        ));
    }
    let profile = state
        .store
        .get_design_profile(&request.design_profile_id)
        .await
        .ok_or_else(|| not_found("design profile not found".to_string()))?;
    if profile.project_id() != Some(project_id.as_str())
        || profile.version != request.design_profile_version
    {
        return Err(conflict_error(anyhow::anyhow!(
            "design profile does not match the requested project/revision"
        )));
    }
    let policy = state
        .store
        .upsert_design_context_enforcement_policy(
            &project_id,
            &request.design_profile_id,
            request.design_profile_version,
            request.enabled,
            request.expected_revision,
            request.updated_by.clone(),
        )
        .await
        .map_err(conflict_error)?;
    state
        .store
        .append_audit_record(
            &project_id,
            "",
            "internal.design_context_enforcement.upsert",
            format!("{summary} policyRevision={}", policy.revision),
            "allow",
            format!("persisted by {}", request.updated_by),
        )
        .await;
    Ok(Json(DesignContextEnforcementPolicyResponse { policy }))
}
