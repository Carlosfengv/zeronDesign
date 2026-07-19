use super::super::*;

pub(in crate::http_api) async fn authorize_project_operation(
    state: &AppState,
    policy: &ApplicationAuthorizationPolicy,
    headers: &HeaderMap,
    project_id: &str,
    required_operation: &str,
) -> Result<Option<crate::authorization::AuthenticatedPrincipal>, (StatusCode, Json<ErrorResponse>)>
{
    if state.config.public_principal_auth_mode == PublicPrincipalAuthMode::Disabled {
        return Ok(None);
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
                    error_code: None,
                }),
            )
        })?;
    let principal: crate::authorization::AuthenticatedPrincipal = principal.into();
    policy
        .authorize_project_owner(&principal, project_id)
        .await
        .map_err(|error| match error {
            AuthorizationPolicyError::Forbidden => {
                forbidden("public_auth.project_forbidden".to_string())
            }
            AuthorizationPolicyError::Conflict(message) => conflict_error(anyhow::anyhow!(message)),
        })?;
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
            "project-scoped public principal authorized",
        )
        .await;
    Ok(Some(principal))
}

pub(in crate::http_api) async fn authorize_current_project_operation(
    state: &AppState,
    policy: &ApplicationAuthorizationPolicy,
    headers: &HeaderMap,
    required_operation: &str,
) -> Result<Option<crate::authorization::AuthenticatedPrincipal>, (StatusCode, Json<ErrorResponse>)>
{
    if state.config.public_principal_auth_mode == PublicPrincipalAuthMode::Disabled {
        return Ok(None);
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
                    error_code: None,
                }),
            )
        })?;
    let principal: crate::authorization::AuthenticatedPrincipal = principal.into();
    policy
        .authorize_project_owner(&principal, &principal.project_id)
        .await
        .map_err(|error| match error {
            AuthorizationPolicyError::Forbidden => {
                forbidden("public_auth.project_forbidden".to_string())
            }
            AuthorizationPolicyError::Conflict(message) => conflict_error(anyhow::anyhow!(message)),
        })?;
    Ok(Some(principal))
}
