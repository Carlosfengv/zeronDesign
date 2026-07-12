use super::super::*;

pub(in crate::http_api) async fn authenticate_candidate_preview(
    state: &AppState,
    headers: &HeaderMap,
    lease_id: &str,
) -> Result<Option<crate::public_principal::PublicPrincipal>, (StatusCode, Json<ErrorResponse>)> {
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
    Ok(Some(principal))
}

pub(in crate::http_api) async fn authorize_candidate_preview(
    state: &AppState,
    principal: Option<&crate::public_principal::PublicPrincipal>,
    lease_id: &str,
    run_id: &str,
    lease_project_id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let Some(principal) = principal else {
        return Ok(());
    };
    let run = state.store.get_run(run_id).await.ok_or_else(|| {
        conflict_error(anyhow::anyhow!(
            "candidate preview lease references a missing run"
        ))
    })?;
    if lease_project_id != run.project_id {
        return Err(conflict_error(anyhow::anyhow!(
            "candidate preview lease project identity drift detected"
        )));
    }
    if principal.project_id != run.project_id {
        state
            .store
            .append_audit_record(
                &run.project_id,
                run_id,
                "public.preview.read",
                preview_audit_summary(lease_id, &principal.principal_id),
                "deny",
                "principal project scope does not match preview run",
            )
            .await;
        return Err(forbidden("public_auth.project_forbidden".to_string()));
    }
    let access = state
        .store
        .get_project_access(&run.project_id)
        .await
        .ok_or_else(|| forbidden("public_auth.project_forbidden".to_string()))?;
    if access.project_id != run.project_id {
        return Err(conflict_error(anyhow::anyhow!(
            "project access identity drift detected"
        )));
    }
    if access.owner_principal_id != principal.principal_id {
        state
            .store
            .append_audit_record(
                &run.project_id,
                run_id,
                "public.preview.read",
                preview_audit_summary(lease_id, &principal.principal_id),
                "deny",
                "principal does not own the project",
            )
            .await;
        return Err(forbidden("public_auth.project_forbidden".to_string()));
    }
    state
        .store
        .append_audit_record(
            &run.project_id,
            run_id,
            "public.preview.read",
            preview_audit_summary(lease_id, &principal.principal_id),
            "allow",
            "project-scoped public principal authorized",
        )
        .await;
    Ok(())
}

pub(in crate::http_api) fn preview_audit_summary(lease_id: &str, principal_id: &str) -> String {
    format!(
        "leaseId={lease_id},principalHash={}",
        sha256_hex(principal_id.as_bytes())
    )
}

pub(in crate::http_api) fn validated_preview_prefix(
    config: &RuntimeConfig,
    headers: &HeaderMap,
    project_id: &str,
    lease_id: &str,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let fallback = format!("/previews/{lease_id}");
    let Some(value) = headers
        .get("x-anydesign-preview-prefix")
        .and_then(|value| value.to_str().ok())
    else {
        return if config.public_principal_auth_mode == PublicPrincipalAuthMode::Disabled {
            Ok(fallback)
        } else {
            Err(bad_request(
                "x-anydesign-preview-prefix is required for public preview proxying".to_string(),
            ))
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
