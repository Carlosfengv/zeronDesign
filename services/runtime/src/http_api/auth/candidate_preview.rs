use super::super::*;

pub(in crate::http_api) async fn authenticate_candidate_preview(
    state: &AppState,
    headers: &HeaderMap,
    lease_id: &str,
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
    let principal = match verifier.verify_bearer(authorization, PREVIEW_READ_OPERATION) {
        Ok(principal) => principal,
        Err(error) => {
            let status = if error == PublicPrincipalError::InvalidScope {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            };
            state
                .store
                .append_audit_record(
                    "",
                    "",
                    "public.preview.read",
                    format!("leaseId={lease_id}"),
                    "deny",
                    error.message(),
                )
                .await;
            return Err((
                status,
                Json(ErrorResponse {
                    error: error.message().to_string(),
                }),
            ));
        }
    };
    Ok(Some(principal.into()))
}

pub(in crate::http_api) fn validated_preview_prefix(
    prefix_required: bool,
    requested_prefix: Option<&str>,
    project_id: &str,
    lease_id: &str,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let fallback = format!("/previews/{lease_id}");
    let Some(value) = requested_prefix else {
        return if prefix_required {
            Err(bad_request(
                "x-anydesign-preview-prefix is required for public preview proxying".to_string(),
            ))
        } else {
            Ok(fallback)
        };
    };
    let expected = format!("/projects/{project_id}/previews/{lease_id}");
    let normalized = value.to_ascii_lowercase();
    if value != expected
        || normalized.contains("%2e")
        || normalized.contains("%5c")
        || value.contains("..")
        || value.contains('\\')
        || value.contains("://")
        || value.contains('?')
        || value.contains('#')
    {
        return Err(bad_request(
            "x-anydesign-preview-prefix is invalid".to_string(),
        ));
    }
    Ok(value.to_string())
}
