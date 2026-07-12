use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/design-source-artifacts",
            post(create_design_source_artifact)
                .layer(DefaultBodyLimit::max(MAX_DESIGN_SOURCE_REQUEST_BYTES)),
        )
        .route(
            "/design-source-artifacts/{artifact_id}",
            get(get_design_source_artifact),
        )
        .route(
            "/design-source-artifacts/{artifact_id}/content",
            get(get_design_source_artifact_content),
        )
        .route("/design-profiles/import", post(import_design_profile))
}

async fn create_design_source_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateDesignSourceArtifactRequest>,
) -> Result<Json<DesignSourceArtifactResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_design_source_authorization(&state.config, &headers)?;
    if request.content_base64.len() > MAX_DESIGN_SOURCE_BASE64_BYTES {
        return Err(bad_request(format!(
            "contentBase64 exceeds the {MAX_DESIGN_SOURCE_BYTES}-byte decoded source limit"
        )));
    }
    let content = BASE64_STANDARD
        .decode(request.content_base64.as_bytes())
        .map_err(|_| bad_request("contentBase64 must be valid base64".to_string()))?;
    if content.len() > MAX_DESIGN_SOURCE_BYTES {
        return Err(bad_request(format!(
            "decoded design source exceeds {MAX_DESIGN_SOURCE_BYTES} bytes"
        )));
    }
    let digest = sha256_hex(&content);
    if let Some(client_sha256) = request.client_sha256.as_deref() {
        if client_sha256.len() != 64 || !client_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(bad_request(
                "clientSha256 must be a 64-character hexadecimal digest".to_string(),
            ));
        }
        if !digest.eq_ignore_ascii_case(client_sha256) {
            return Err(bad_request(
                "clientSha256 does not match decoded design source bytes".to_string(),
            ));
        }
    }
    let artifact = state
        .store
        .create_design_source_artifact(
            request.scope,
            request.file_name,
            request.media_type,
            content,
        )
        .await
        .map_err(design_source_error)?;
    Ok(Json(DesignSourceArtifactResponse { artifact }))
}

async fn get_design_source_artifact(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<DesignSourceArtifactResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_design_source_authorization(&state.config, &headers)?;
    validate_required_string("artifactId", &artifact_id)?;
    let artifact = state
        .store
        .get_design_source_artifact(&artifact_id)
        .await
        .ok_or_else(|| not_found(format!("design source artifact not found: {artifact_id}")))?;
    Ok(Json(DesignSourceArtifactResponse { artifact }))
}

async fn get_design_source_artifact_content(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    require_design_source_authorization(&state.config, &headers)?;
    validate_required_string("artifactId", &artifact_id)?;
    let artifact = state
        .store
        .get_design_source_artifact(&artifact_id)
        .await
        .ok_or_else(|| not_found(format!("design source artifact not found: {artifact_id}")))?;
    let content = state
        .store
        .read_design_source_artifact_content(&artifact_id)
        .await
        .map_err(design_source_error)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&format!("{}; charset=utf-8", artifact.media_type)).map_err(
            |_| internal_error(anyhow::anyhow!("invalid stored design source media type")),
        )?,
    );
    response_headers.insert(
        "x-design-source-sha256",
        HeaderValue::from_str(&artifact.sha256)
            .map_err(|_| internal_error(anyhow::anyhow!("invalid stored design source hash")))?,
    );
    response_headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok((response_headers, content))
}

async fn import_design_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ImportDesignProfileRequest>,
) -> Result<Json<ImportDesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    require_design_source_authorization(&state.config, &headers)?;
    validate_required_string("name", &request.name)?;
    crate::types::validate_design_source_scope(&request.scope).map_err(bad_request)?;
    let artifact = state
        .store
        .get_design_source_artifact(&request.source_artifact_id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "design source artifact not found: {}",
                request.source_artifact_id
            ))
        })?;
    if artifact.scope != request.scope {
        return Err(bad_request(
            "design source artifact scope must exactly match import scope".to_string(),
        ));
    }
    let content = state
        .store
        .read_design_source_artifact_content(&artifact.id)
        .await
        .map_err(design_source_error)?;
    let source = std::str::from_utf8(&content)
        .map_err(|_| bad_request("design source artifact content must be UTF-8".to_string()))?;
    let now = Utc::now();
    let profile_id = state.store.next_id("design-profile");
    let report_id = state.store.next_id("design-profile-conversion-report");
    let parsed = parse_design_profile_source(source);
    let converter_version = "design-profile-import@1";
    let candidate = json!({
        "visual": {
            "direction": parsed.headings.first().cloned().unwrap_or_else(|| request.name.clone()),
            "principles": [],
            "moodKeywords": [],
            "avoidKeywords": [],
            "composition": {},
            "imagery": {},
            "motion": {}
        },
        "tokens": {
            "color": parsed.tokens,
            "typography": {},
            "radius": {},
            "shadow": {},
            "spacing": {}
        },
        "signatureRules": []
    });
    let validation_issues = design_profile_candidate_issues(&candidate, true);
    let source_metadata = json!({
        "kind": "imported",
        "sourceArtifactIds": [artifact.id.clone()],
        "primarySourceArtifactId": artifact.id.clone(),
        "sourceHash": artifact.sha256.clone(),
        "converterVersion": converter_version,
        "importedAt": now,
        "integrity": "verified"
    });
    let draft = DesignProfileDraft {
        id: profile_id.clone(),
        schema_version: DESIGN_PROFILE_SCHEMA_V2.to_string(),
        version: 1,
        name: request.name,
        status: "draft".to_string(),
        scope: request.scope,
        source: source_metadata,
        candidate,
        conversion_report_id: report_id.clone(),
        validation_issues,
        created_at: now,
        updated_at: now,
    };
    let report = DesignProfileConversionReport {
        id: report_id,
        design_profile_id: profile_id,
        profile_version: 1,
        converter_version: converter_version.to_string(),
        deterministic_parser_version: "markdown-css-parser@1".to_string(),
        source_artifact_id: artifact.id,
        source_hash: artifact.sha256,
        extracted_sections: parsed.headings,
        extracted_token_count: parsed.extracted_token_count,
        extracted_component_count: parsed.extracted_component_count,
        required_signature_rule_count: 0,
        unmapped_items: parsed.unmapped_items,
        warnings: parsed.warnings,
        created_at: now,
    };
    let (draft, report) = state
        .store
        .create_design_profile_draft(draft, report)
        .await
        .map_err(design_profile_error)?;
    Ok(Json(ImportDesignProfileResponse {
        design_profile_draft: draft,
        conversion_report: report,
        requires_review: true,
    }))
}
