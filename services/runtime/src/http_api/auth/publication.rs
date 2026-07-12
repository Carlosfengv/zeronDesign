use super::super::*;

pub(in crate::http_api) async fn authorize_publication(
    state: &AppState,
    headers: &HeaderMap,
    project_id: &str,
    required_operation: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if state.config.public_principal_auth_mode == PublicPrincipalAuthMode::Disabled {
        return Ok(());
    }
    let verifier = PublicPrincipalVerifier::from_public_key_files(
        &state.config.public_principal_public_key_files,
        state.config.public_principal_issuer.clone(),
        state.config.public_principal_audience.clone(),
        state.config.public_principal_max_ttl_seconds,
    )
    .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let principal = verifier
        .verify_bearer(authorization, required_operation)
        .map_err(|error| {
            let status = if error == PublicPrincipalError::InvalidScope {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            };
            (
                status,
                Json(ErrorResponse {
                    error: error.message().into(),
                }),
            )
        })?;
    if principal.project_id != project_id {
        return Err(forbidden("public_auth.project_forbidden".to_string()));
    }
    let access = state
        .store
        .get_project_access(project_id)
        .await
        .ok_or_else(|| forbidden("public_auth.project_forbidden".to_string()))?;
    if access.owner_principal_id != principal.principal_id {
        return Err(forbidden("public_auth.project_forbidden".to_string()));
    }
    state
        .store
        .append_audit_record(
            project_id,
            "",
            required_operation,
            format!(
                "principalHash={}",
                sha256_hex(principal.principal_id.as_bytes())
            ),
            "allow",
            "project-scoped publication principal authorized",
        )
        .await;
    Ok(())
}
