use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/design-profiles",
            post(create_design_profile).get(list_design_profiles),
        )
        .route(
            "/design-profiles/{design_profile_id}",
            get(get_design_profile).put(update_design_profile),
        )
        .route(
            "/design-profiles/{design_profile_id}/versions",
            get(design_profile_versions),
        )
        .route(
            "/design-profiles/{design_profile_id}/diff",
            get(design_profile_diff),
        )
        .route(
            "/design-profiles/{design_profile_id}/archive",
            post(archive_design_profile),
        )
        .route(
            "/design-profiles/{design_profile_id}/activate",
            post(activate_design_profile),
        )
        .route(
            "/design-profiles/{design_profile_id}/conversion-report",
            get(current_design_profile_conversion_report),
        )
        .route(
            "/design-profiles/{design_profile_id}/versions/{version}/conversion-report",
            get(versioned_design_profile_conversion_report),
        )
        .route(
            "/design-profiles/{design_profile_id}/versions/{version}/fidelity-report",
            get(design_profile_fidelity_report),
        )
        .route(
            "/projects/{project_id}/design-profile",
            post(bind_project_design_profile).get(project_design_profile),
        )
}

async fn create_design_profile(
    State(state): State<AppState>,
    Json(request): Json<CreateDesignProfileRequest>,
) -> Result<Json<DesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_create_design_profile_request(&request)?;
    let now = Utc::now();
    let id = state.store.next_id("design-profile");
    let payload = design_profile_payload_from_request(&request)?;
    let mut profile = DesignProfile {
        id,
        schema_version: payload
            .get("schemaVersion")
            .and_then(Value::as_str)
            .unwrap_or(crate::types::DESIGN_PROFILE_SCHEMA_V1)
            .to_string(),
        name: request.name.clone(),
        status: payload_string(&payload, "status")?,
        version: 1,
        scope: scope_with_project_id(
            payload_value(&payload, "scope").unwrap_or(Value::Null),
            request.project_id.as_deref(),
        ),
        source: payload_value(&payload, "source").unwrap_or_else(|| json!({ "kind": "manual" })),
        product: payload_required_value(&payload, "product")?,
        brand: payload_required_value(&payload, "brand")?,
        visual: payload_required_value(&payload, "visual")?,
        tokens: payload_required_value(&payload, "tokens")?,
        runtime_token_mapping: payload_required_value(&payload, "runtimeTokenMapping")?,
        extended_token_mapping: payload_value(&payload, "extendedTokenMapping")
            .unwrap_or_else(|| json!({})),
        components: payload_required_value(&payload, "components")?,
        content: payload_required_value(&payload, "content")?,
        accessibility: payload_required_value(&payload, "accessibility")?,
        technical: payload_required_value(&payload, "technical")?,
        governance: payload_required_value(&payload, "governance")?,
        signature_rules: payload
            .get("signatureRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        overrides: payload_value(&payload, "overrides").unwrap_or_else(|| json!({})),
        created_at: now,
        updated_at: now,
    };
    normalize_design_profile_component_roles(&mut profile.components)?;
    validate_design_profile_template_availability(&profile)
        .await
        .map_err(|error| conflict_error(anyhow::anyhow!(error)))?;
    validate_design_profile_source_reference(&state.store, &profile).await?;
    let profile = state
        .store
        .create_design_profile(profile)
        .await
        .map_err(design_profile_error)?;
    Ok(Json(DesignProfileResponse {
        design_profile: profile.clone(),
        profile: Some(profile),
    }))
}

async fn list_design_profiles(
    State(state): State<AppState>,
    Query(query): Query<ListDesignProfilesQuery>,
) -> Result<Json<ListDesignProfilesResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_optional_string("projectId", query.project_id.as_deref())?;
    validate_optional_string("workspaceId", query.workspace_id.as_deref())?;
    validate_optional_string("organizationId", query.organization_id.as_deref())?;
    let active_profiles = state
        .store
        .list_design_profiles(
            query.project_id.as_deref(),
            query.workspace_id.as_deref(),
            query.organization_id.as_deref(),
            query.include_archived,
        )
        .await;
    let drafts = state
        .store
        .list_design_profile_drafts(
            query.project_id.as_deref(),
            query.workspace_id.as_deref(),
            query.organization_id.as_deref(),
        )
        .await;
    let active_ids = active_profiles
        .iter()
        .map(|profile| profile.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut design_profiles = active_profiles
        .into_iter()
        .map(|profile| serde_json::to_value(profile).unwrap_or(Value::Null))
        .collect::<Vec<_>>();
    design_profiles.extend(
        drafts
            .into_iter()
            .filter(|draft| !active_ids.contains(&draft.id))
            .map(|draft| serde_json::to_value(draft).unwrap_or(Value::Null)),
    );
    Ok(Json(ListDesignProfilesResponse { design_profiles }))
}

async fn get_design_profile(
    State(state): State<AppState>,
    Path(design_profile_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("designProfileId", &design_profile_id)?;
    if let Some(profile) = state.store.get_design_profile(&design_profile_id).await {
        return Ok(Json(json!({
            "designProfile": profile,
            "profile": profile,
        })));
    }
    let draft = state
        .store
        .get_design_profile_draft(&design_profile_id)
        .await
        .ok_or_else(|| not_found(format!("design profile not found: {design_profile_id}")))?;
    Ok(Json(json!({
        "designProfile": draft,
        "profile": draft,
    })))
}

async fn design_profile_versions(
    State(state): State<AppState>,
    Path(design_profile_id): Path<String>,
) -> Result<Json<DesignProfileVersionsResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("designProfileId", &design_profile_id)?;
    let active_versions = state
        .store
        .design_profile_versions(&design_profile_id)
        .await
        .map_err(design_profile_error)?;
    let draft_versions = state
        .store
        .design_profile_draft_versions(&design_profile_id)
        .await
        .map_err(design_profile_error)?;
    let mut versions = active_versions
        .into_iter()
        .map(|profile| serde_json::to_value(profile).unwrap_or(Value::Null))
        .chain(
            draft_versions
                .into_iter()
                .map(|draft| serde_json::to_value(draft).unwrap_or(Value::Null)),
        )
        .collect::<Vec<_>>();
    versions.sort_by_key(|record| record.get("version").and_then(Value::as_u64).unwrap_or(0));
    if versions.is_empty() {
        return Err(not_found(format!(
            "design profile not found: {design_profile_id}"
        )));
    }
    Ok(Json(DesignProfileVersionsResponse {
        design_profile_id,
        versions,
    }))
}

async fn design_profile_diff(
    State(state): State<AppState>,
    Path(design_profile_id): Path<String>,
    Query(query): Query<DesignProfileDiffQuery>,
) -> Result<Json<DesignProfileDiffResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("designProfileId", &design_profile_id)?;
    if query.from_version == 0 || query.to_version == 0 {
        return Err(bad_request(
            "fromVersion and toVersion must be positive".to_string(),
        ));
    }
    let versions = state
        .store
        .design_profile_versions(&design_profile_id)
        .await
        .map_err(design_profile_error)?;
    if versions.is_empty() {
        return Err(not_found(format!(
            "design profile not found: {design_profile_id}"
        )));
    }
    let from_profile = versions
        .iter()
        .find(|profile| profile.version == query.from_version)
        .ok_or_else(|| {
            not_found(format!(
                "design profile version not found: {design_profile_id}@{}",
                query.from_version
            ))
        })?;
    let to_profile = versions
        .iter()
        .find(|profile| profile.version == query.to_version)
        .ok_or_else(|| {
            not_found(format!(
                "design profile version not found: {design_profile_id}@{}",
                query.to_version
            ))
        })?;
    let changes = diff_design_profiles(from_profile, to_profile);
    Ok(Json(DesignProfileDiffResponse {
        design_profile_id,
        from_version: query.from_version,
        to_version: query.to_version,
        changes,
    }))
}

async fn archive_design_profile(
    State(state): State<AppState>,
    Path(design_profile_id): Path<String>,
) -> Result<Json<DesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("designProfileId", &design_profile_id)?;
    let profile = state
        .store
        .archive_design_profile(&design_profile_id)
        .await
        .map_err(design_profile_error)?;
    Ok(Json(DesignProfileResponse {
        design_profile: profile.clone(),
        profile: Some(profile),
    }))
}

async fn activate_design_profile(
    State(state): State<AppState>,
    Path(design_profile_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ActivateDesignProfileRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_design_source_authorization(&state.config, &headers)
        .map_err(error_response_as_value)?;
    let draft = state
        .store
        .get_design_profile_draft(&design_profile_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("design profile draft not found: {design_profile_id}") })),
            )
        })?;
    if draft.version != request.expected_version {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": "design profile version conflict",
                "currentVersion": draft.version,
                "validationIssues": draft.validation_issues,
            })),
        ));
    }

    let now = Utc::now();
    let mut value = draft.candidate.clone();
    let object = value.as_object_mut().ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "draft candidate must be an object",
                "currentVersion": draft.version,
                "validationIssues": [{
                    "path": "candidate",
                    "code": "invalid_type",
                    "message": "candidate must be an object",
                    "blocking": true
                }]
            })),
        )
    })?;
    object.insert("id".to_string(), json!(draft.id));
    object.insert("schemaVersion".to_string(), json!(DESIGN_PROFILE_SCHEMA_V2));
    object.insert("name".to_string(), json!(draft.name));
    object.insert("status".to_string(), json!("active"));
    object.insert("version".to_string(), json!(draft.version + 1));
    object.insert("scope".to_string(), draft.scope.clone());
    object.insert("source".to_string(), draft.source.clone());
    object.insert("createdAt".to_string(), json!(draft.created_at));
    object.insert("updatedAt".to_string(), json!(now));
    let mut profile: DesignProfile = serde_json::from_value(value).map_err(|error| {
        let issues = design_profile_candidate_issues(&draft.candidate, true);
        (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("draft activation validation failed: {error}"),
                "currentVersion": draft.version,
                "validationIssues": issues,
            })),
        )
    })?;
    normalize_design_profile_component_roles(&mut profile.components)
        .map_err(error_response_as_value)?;
    if let Err(error) = profile.validate_for_runtime() {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("draft activation validation failed: {error}"),
                "currentVersion": draft.version,
                "validationIssues": [{
                    "path": "candidate",
                    "code": "runtime_validation",
                    "message": error,
                    "blocking": true
                }]
            })),
        ));
    }
    validate_design_profile_template_availability(&profile)
        .await
        .map_err(|error| error_response_as_value(conflict_error(anyhow::anyhow!(error))))?;
    validate_design_profile_source_reference(&state.store, &profile)
        .await
        .map_err(error_response_as_value)?;
    let profile = state
        .store
        .create_design_profile(profile)
        .await
        .map_err(|error| error_response_as_value(design_profile_error(error)))?;
    Ok(Json(json!({
        "designProfile": profile,
        "profile": profile,
    })))
}

async fn current_design_profile_conversion_report(
    State(state): State<AppState>,
    Path(design_profile_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<DesignProfileConversionReport>, (StatusCode, Json<ErrorResponse>)> {
    require_design_source_authorization(&state.config, &headers)?;
    let report = state
        .store
        .design_profile_conversion_report(&design_profile_id, None)
        .await
        .map_err(design_profile_error)?
        .ok_or_else(|| {
            not_found(format!(
                "design profile conversion report not found: {design_profile_id}"
            ))
        })?;
    Ok(Json(report))
}

async fn versioned_design_profile_conversion_report(
    State(state): State<AppState>,
    Path((design_profile_id, version)): Path<(String, u32)>,
    headers: HeaderMap,
) -> Result<Json<DesignProfileConversionReport>, (StatusCode, Json<ErrorResponse>)> {
    require_design_source_authorization(&state.config, &headers)?;
    if version == 0 {
        return Err(bad_request("version must be positive".to_string()));
    }
    let report = state
        .store
        .design_profile_conversion_report(&design_profile_id, Some(version))
        .await
        .map_err(design_profile_error)?
        .ok_or_else(|| {
            not_found(format!(
                "design profile conversion report not found: {design_profile_id}@{version}"
            ))
        })?;
    Ok(Json(report))
}

async fn design_profile_fidelity_report(
    State(state): State<AppState>,
    Path((design_profile_id, version)): Path<(String, u32)>,
    Query(query): Query<DesignProfileFidelityQuery>,
) -> Result<Json<DesignProfileFidelityReport>, (StatusCode, Json<ErrorResponse>)> {
    let surface = query
        .surface
        .as_deref()
        .ok_or_else(|| bad_request("surface is required".to_string()))?;
    let template = query
        .template
        .as_deref()
        .ok_or_else(|| bad_request("template is required".to_string()))?;
    if version == 0 {
        return Err(bad_request("version must be positive".to_string()));
    }
    let versions = state
        .store
        .design_profile_versions(&design_profile_id)
        .await
        .map_err(design_profile_error)?;
    let profile = versions
        .into_iter()
        .find(|profile| profile.version == version)
        .ok_or_else(|| {
            not_found(format!(
                "design profile version not found: {design_profile_id}@{version}"
            ))
        })?;
    let effective = profile
        .effective_for(surface, template)
        .map_err(bad_request)?;
    let materialized: DesignProfile = serde_json::from_value(effective.profile.clone())
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    let capsule =
        crate::agent_loop::render_design_profile_markdown(&materialized).map_err(internal_error)?;
    let mut required_signature_rule_ids = materialized
        .signature_rules
        .iter()
        .filter(|rule| rule.get("priority").and_then(Value::as_str) == Some("required"))
        .filter(|rule| signature_rule_applies_to_surface(rule, surface))
        .filter_map(|rule| {
            rule.get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    required_signature_rule_ids.sort();
    let capsule_included_rule_ids = required_signature_rule_ids
        .iter()
        .filter(|id| capsule.contains(&format!("[{id}]")))
        .cloned()
        .collect::<Vec<_>>();
    let capsule_missing_rule_ids = required_signature_rule_ids
        .iter()
        .filter(|id| !capsule_included_rule_ids.contains(id))
        .cloned()
        .collect::<Vec<_>>();
    let unsupported_extended_tokens =
        unsupported_extended_tokens_for_template(&materialized.extended_token_mapping, template);

    let source_integrity = profile
        .source
        .get("integrity")
        .and_then(Value::as_str)
        .unwrap_or(
            if profile.schema_version == crate::types::DESIGN_PROFILE_SCHEMA_V1 {
                "unverified"
            } else {
                "missing"
            },
        )
        .to_string();
    let source_hash_matches = if let Some(artifact_id) = profile
        .source
        .get("primarySourceArtifactId")
        .and_then(Value::as_str)
    {
        match state.store.get_design_source_artifact(artifact_id).await {
            Some(artifact) => Some(
                profile.source.get("sourceHash").and_then(Value::as_str)
                    == Some(artifact.sha256.as_str())
                    && state
                        .store
                        .read_design_source_artifact_content(artifact_id)
                        .await
                        .is_ok(),
            ),
            None => Some(false),
        }
    } else {
        None
    };
    let mut warnings = Vec::new();
    if source_hash_matches == Some(false) {
        warnings.push("source artifact integrity verification failed".to_string());
    }
    if !unsupported_extended_tokens.is_empty() {
        warnings.push(format!(
            "template does not support extended tokens: {}",
            unsupported_extended_tokens.join(", ")
        ));
    }
    if !capsule_missing_rule_ids.is_empty() {
        warnings.push("Design Capsule is missing required signature rules".to_string());
    }
    Ok(Json(DesignProfileFidelityReport {
        design_profile_id,
        version,
        schema_version: profile.schema_version,
        surface: surface.to_string(),
        template: template.to_string(),
        style_contract_version: registered_template_spec(template)
            .map(|spec| spec.style.version.to_string())
            .unwrap_or_else(|| "runtime-style-contract@p2".to_string()),
        effective_profile_hash: effective.effective_profile_hash,
        source_integrity,
        source_hash_matches,
        required_signature_rule_ids,
        capsule_included_rule_ids,
        capsule_missing_rule_ids,
        unsupported_extended_tokens,
        warnings,
    }))
}

async fn update_design_profile(
    State(state): State<AppState>,
    Path(design_profile_id): Path<String>,
    Json(request): Json<UpdateDesignProfileRequest>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("designProfileId", &design_profile_id)?;
    validate_required_string("name", &request.name)?;
    let existing = state.store.get_design_profile(&design_profile_id).await;
    if existing.is_none() {
        let _draft = state
            .store
            .get_design_profile_draft(&design_profile_id)
            .await
            .ok_or_else(|| not_found(format!("design profile not found: {design_profile_id}")))?;
        let expected_version = request.expected_version.ok_or_else(|| {
            bad_request("expectedVersion is required when updating a draft".to_string())
        })?;
        let issues = design_profile_candidate_issues(&request.profile, true);
        let updated = state
            .store
            .update_design_profile_draft(
                &design_profile_id,
                expected_version,
                request.name,
                request.profile,
                issues,
            )
            .await
            .map_err(design_profile_error)?;
        return Ok(Json(json!({
            "designProfile": updated,
            "profile": updated,
        })));
    }
    let existing = existing.expect("existing design profile checked above");
    if existing.schema_version == DESIGN_PROFILE_SCHEMA_V2 {
        let expected_version = request.expected_version.ok_or_else(|| {
            bad_request("expectedVersion is required when updating a V2 profile".to_string())
        })?;
        if expected_version != existing.version {
            return Err((
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error: format!(
                        "design profile version conflict: expected {expected_version}, current {}",
                        existing.version
                    ),
                }),
            ));
        }
    }
    let payload = request
        .profile
        .as_object()
        .cloned()
        .ok_or_else(|| bad_request("profile must be an object".to_string()))?;
    let now = Utc::now();
    let mut profile = DesignProfile {
        id: existing.id,
        schema_version: payload
            .get("schemaVersion")
            .and_then(Value::as_str)
            .unwrap_or(&existing.schema_version)
            .to_string(),
        name: request.name,
        status: payload_string(&payload, "status")?,
        version: existing.version + 1,
        scope: payload_required_value(&payload, "scope")?,
        source: payload_value(&payload, "source").unwrap_or(existing.source),
        product: payload_required_value(&payload, "product")?,
        brand: payload_required_value(&payload, "brand")?,
        visual: payload_required_value(&payload, "visual")?,
        tokens: payload_required_value(&payload, "tokens")?,
        runtime_token_mapping: payload_required_value(&payload, "runtimeTokenMapping")?,
        extended_token_mapping: payload_value(&payload, "extendedTokenMapping")
            .unwrap_or(existing.extended_token_mapping),
        components: payload_required_value(&payload, "components")?,
        content: payload_required_value(&payload, "content")?,
        accessibility: payload_required_value(&payload, "accessibility")?,
        technical: payload_required_value(&payload, "technical")?,
        governance: payload_required_value(&payload, "governance")?,
        signature_rules: payload
            .get("signatureRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or(existing.signature_rules),
        overrides: payload_value(&payload, "overrides").unwrap_or(existing.overrides),
        created_at: existing.created_at,
        updated_at: now,
    };
    normalize_design_profile_component_roles(&mut profile.components)?;
    validate_design_profile_template_availability(&profile)
        .await
        .map_err(|error| conflict_error(anyhow::anyhow!(error)))?;
    validate_design_profile_source_reference(&state.store, &profile).await?;
    let profile = state
        .store
        .create_design_profile(profile)
        .await
        .map_err(design_profile_error)?;
    Ok(Json(json!({
        "designProfile": profile,
        "profile": profile,
    })))
}

fn diff_design_profiles(
    from_profile: &DesignProfile,
    to_profile: &DesignProfile,
) -> Vec<DesignProfileDiffChange> {
    let mut from_value = serde_json::to_value(from_profile).unwrap_or(Value::Null);
    let mut to_value = serde_json::to_value(to_profile).unwrap_or(Value::Null);
    remove_design_profile_diff_metadata(&mut from_value);
    remove_design_profile_diff_metadata(&mut to_value);
    let mut changes = Vec::new();
    collect_value_diff("", &from_value, &to_value, &mut changes);
    changes
}

fn remove_design_profile_diff_metadata(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for key in ["id", "version", "createdAt", "updatedAt"] {
        object.remove(key);
    }
}

fn collect_value_diff(
    path: &str,
    before: &Value,
    after: &Value,
    changes: &mut Vec<DesignProfileDiffChange>,
) {
    if before == after {
        return;
    }
    match (before, after) {
        (Value::Object(before_object), Value::Object(after_object)) => {
            let keys = before_object
                .keys()
                .chain(after_object.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match (before_object.get(&key), after_object.get(&key)) {
                    (Some(before_child), Some(after_child)) => {
                        collect_value_diff(&child_path, before_child, after_child, changes);
                    }
                    (Some(before_child), None) => changes.push(DesignProfileDiffChange {
                        path: child_path,
                        before: Some(before_child.clone()),
                        after: None,
                    }),
                    (None, Some(after_child)) => changes.push(DesignProfileDiffChange {
                        path: child_path,
                        before: None,
                        after: Some(after_child.clone()),
                    }),
                    (None, None) => {}
                }
            }
        }
        _ => changes.push(DesignProfileDiffChange {
            path: path.to_string(),
            before: Some(before.clone()),
            after: Some(after.clone()),
        }),
    }
}

async fn bind_project_design_profile(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(request): Json<BindProjectDesignProfileRequest>,
) -> Result<Json<ProjectDesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("projectId", &project_id)?;
    validate_required_string("designProfileId", &request.design_profile_id)?;
    if state
        .store
        .get_design_profile(&request.design_profile_id)
        .await
        .is_none()
        && state
            .store
            .get_design_profile_draft(&request.design_profile_id)
            .await
            .is_some()
    {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "draft design profile cannot be bound to a project".to_string(),
            }),
        ));
    }
    if let Some(profile) = state
        .store
        .get_design_profile(&request.design_profile_id)
        .await
    {
        validate_design_profile_template_availability(&profile)
            .await
            .map_err(|error| conflict_error(anyhow::anyhow!(error)))?;
    }
    let profile = state
        .store
        .bind_project_design_profile(&project_id, &request.design_profile_id)
        .await
        .map_err(design_profile_error)?;
    Ok(Json(ProjectDesignProfileResponse {
        project_id,
        design_profile: Some(profile.clone()),
        profile: Some(profile),
    }))
}

async fn project_design_profile(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectDesignProfileResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("projectId", &project_id)?;
    let profile = state.store.project_design_profile(&project_id).await;
    Ok(Json(ProjectDesignProfileResponse {
        project_id,
        design_profile: profile.clone(),
        profile,
    }))
}
