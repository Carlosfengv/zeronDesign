use super::*;

pub(in crate::http_api) async fn proxy_candidate_preview(
    preview_access: PreviewAccessService,
    lease_id: String,
    preview_path: String,
    requested_prefix: Option<String>,
    prefix_required: bool,
    context: PreviewAccessContext<'_>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let access = preview_access
        .resolve_candidate(&lease_id, &preview_path, context)
        .await
        .map_err(preview_access_error)?;
    let preview_prefix = match context {
        PreviewAccessContext::Public(_) => validated_preview_prefix(
            prefix_required,
            requested_prefix.as_deref(),
            &access.project_id,
            &access.lease_id,
        )?,
        PreviewAccessContext::InternalCapture => format!("/preview-captures/{lease_id}"),
    };
    let mut upstream = reqwest::Url::parse(&access.upstream_endpoint)
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    upstream.set_path(&format!("/candidates/{}/{}", access.build_id, preview_path));
    let upstream_response = reqwest::Client::new()
        .get(upstream)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| not_found(format!("candidate preview upstream unavailable: {error}")))?;
    if !upstream_response.status().is_success() {
        return Err(not_found(format!(
            "candidate preview file not found: {preview_path}"
        )));
    }
    let manifest_hash = upstream_response
        .headers()
        .get("x-anydesign-candidate-manifest-hash")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| not_found("candidate preview manifest evidence missing".to_string()))?;
    if manifest_hash != access.candidate_manifest_hash {
        return Err(conflict_error(anyhow::anyhow!(
            "candidate preview manifest hash mismatch"
        )));
    }
    let content_type = upstream_response
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    let mut bytes = upstream_response
        .bytes()
        .await
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?
        .to_vec();
    if content_type
        .to_str()
        .ok()
        .is_some_and(|value| value.starts_with("text/html"))
    {
        if let Ok(html) = String::from_utf8(bytes.clone()) {
            bytes = html
                .replace("href=\"/", &format!("href=\"{preview_prefix}/"))
                .replace("src=\"/", &format!("src=\"{preview_prefix}/"))
                .into_bytes();
        }
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, no-store")
        .header("x-anydesign-preview-lease", access.lease_id)
        .body(Body::from(bytes))
        .map_err(|error| internal_error(anyhow::anyhow!(error)))
}

fn preview_access_error(error: PreviewAccessError) -> (StatusCode, Json<ErrorResponse>) {
    match error {
        PreviewAccessError::NotFound(message) => not_found(message),
        PreviewAccessError::Forbidden(message) => forbidden(message),
        PreviewAccessError::Conflict(message) => conflict_error(anyhow::anyhow!(message)),
        PreviewAccessError::Internal(message) => internal_error(anyhow::anyhow!(message)),
    }
}
