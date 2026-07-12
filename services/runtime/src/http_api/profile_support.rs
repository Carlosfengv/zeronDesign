use super::*;

pub(in crate::http_api) fn scope_with_project_id(scope: Value, project_id: Option<&str>) -> Value {
    let mut object = match scope {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    if let Some(project_id) = project_id {
        object
            .entry("projectId".to_string())
            .or_insert_with(|| Value::String(project_id.to_string()));
    }
    Value::Object(object)
}

pub(in crate::http_api) fn design_profile_payload_from_request(
    request: &CreateDesignProfileRequest,
) -> Result<Map<String, Value>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(profile) = request.profile.as_ref() {
        return profile
            .as_object()
            .cloned()
            .ok_or_else(|| bad_request("profile must be an object".to_string()));
    }
    Ok(request.legacy_profile.clone())
}

pub(in crate::http_api) fn payload_string(
    payload: &Map<String, Value>,
    field: &str,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let value = payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request(format!("profile.{field} must be a string")))?;
    validate_required_string(&format!("profile.{field}"), value)?;
    Ok(value.to_string())
}

pub(in crate::http_api) fn payload_required_value(
    payload: &Map<String, Value>,
    field: &str,
) -> Result<Value, (StatusCode, Json<ErrorResponse>)> {
    payload
        .get(field)
        .cloned()
        .ok_or_else(|| bad_request(format!("profile.{field} is required")))
}

pub(in crate::http_api) fn payload_value(
    payload: &Map<String, Value>,
    field: &str,
) -> Option<Value> {
    payload.get(field).cloned()
}

pub(in crate::http_api) fn normalize_design_profile_component_roles(
    components: &mut Value,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let Some(primitives) = components
        .get_mut("primitives")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    for (name, guideline) in primitives {
        let Some(guideline) = guideline.as_object_mut() else {
            continue;
        };
        let role = guideline.get("role").and_then(Value::as_str);
        let intent = guideline.get("intent").and_then(Value::as_str);
        if let (Some(role), Some(intent)) = (role, intent) {
            if role != intent {
                return Err(bad_request(format!(
                    "components.primitives.{name}.role conflicts with legacy intent"
                )));
            }
        }
        let canonical_role = role.or(intent).map(ToString::to_string);
        if let Some(canonical_role) = canonical_role {
            guideline.insert("role".to_string(), Value::String(canonical_role));
            guideline.remove("intent");
        }
    }
    Ok(())
}

pub(in crate::http_api) fn signature_rule_applies_to_surface(rule: &Value, surface: &str) -> bool {
    match rule.get("appliesTo") {
        Some(Value::String(value)) => value == "all",
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(surface)),
        _ => false,
    }
}

pub(in crate::http_api) fn unsupported_extended_tokens_for_template(
    mapping: &Value,
    template: &str,
) -> Vec<String> {
    let supported = registered_template_spec(template)
        .map(|spec| {
            spec.style
                .tokens
                .iter()
                .map(|token| token.name)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    let mut unsupported = mapping
        .as_object()
        .map(|tokens| {
            tokens
                .keys()
                .filter(|token| !supported.contains(token.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    unsupported.sort();
    unsupported
}

pub(in crate::http_api) fn registered_template_spec(
    template: &str,
) -> Option<Arc<crate::templates::TemplateSpec>> {
    let id = TemplateId::parse(template).ok()?;
    BuiltInTemplateRegistry::built_in().current(&id).ok()
}

pub(in crate::http_api) struct ParsedDesignProfileSource {
    pub(in crate::http_api) headings: Vec<String>,
    pub(in crate::http_api) tokens: Map<String, Value>,
    pub(in crate::http_api) extracted_token_count: usize,
    pub(in crate::http_api) extracted_component_count: usize,
    pub(in crate::http_api) unmapped_items: Vec<DesignProfileUnmappedItem>,
    pub(in crate::http_api) warnings: Vec<String>,
}

pub(in crate::http_api) fn parse_design_profile_source(source: &str) -> ParsedDesignProfileSource {
    let mut headings = Vec::new();
    let mut tokens = Map::new();
    let mut extracted_component_count = 0usize;
    let mut unmapped_items = Vec::new();
    let mut offset = 0usize;
    let mut operational_instruction_detected = false;

    for raw_line in source.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();
        let start_byte = offset;
        let end_byte = offset + raw_line.len();
        offset = end_byte;
        if trimmed.is_empty() || trimmed.starts_with("```") {
            continue;
        }
        let normalized = trimmed.to_ascii_lowercase();
        if [
            "ignore system",
            "ignore previous",
            "call the tool",
            "call tool",
            "change permission",
            "read /",
            "upload data",
            "exfiltrate",
        ]
        .iter()
        .any(|pattern| normalized.contains(pattern))
        {
            operational_instruction_detected = true;
        }

        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            if !heading.is_empty() {
                if heading.to_ascii_lowercase().contains("component")
                    || ["button", "input", "card", "badge"]
                        .iter()
                        .any(|name| heading.eq_ignore_ascii_case(name))
                {
                    extracted_component_count += 1;
                }
                headings.push(heading.to_string());
                continue;
            }
        }

        if let Some((name, value)) =
            parse_css_custom_property(trimmed).or_else(|| parse_markdown_token_row(trimmed))
        {
            if let Some(existing) = tokens.get(&name) {
                if existing.as_str() != Some(value.as_str()) {
                    unmapped_items.push(unmapped_source_item(
                        "token-conflict",
                        start_byte,
                        end_byte,
                        line,
                        "duplicate",
                    ));
                }
            } else {
                tokens.insert(name, Value::String(value));
            }
            continue;
        }

        unmapped_items.push(unmapped_source_item(
            headings.last().map(String::as_str).unwrap_or("document"),
            start_byte,
            end_byte,
            line,
            "unsupported-field",
        ));
    }

    let mut warnings = Vec::new();
    if headings.is_empty() {
        warnings.push("No Markdown headings were extracted".to_string());
    }
    if tokens.is_empty() {
        warnings.push("No CSS custom properties or token table rows were extracted".to_string());
    }
    if !unmapped_items.is_empty() {
        warnings.push(format!(
            "{} source items require review",
            unmapped_items.len()
        ));
    }
    if operational_instruction_detected {
        warnings.push(
            "Operational instruction detected and excluded from design semantics".to_string(),
        );
    }
    let extracted_token_count = tokens.len();
    ParsedDesignProfileSource {
        headings,
        tokens,
        extracted_token_count,
        extracted_component_count,
        unmapped_items,
        warnings,
    }
}

pub(in crate::http_api) fn parse_css_custom_property(line: &str) -> Option<(String, String)> {
    let line = line.trim().trim_end_matches(';');
    let (name, value) = line.split_once(':')?;
    let name = name.trim();
    let value = value.trim();
    if !name.starts_with("--") || name.len() < 3 || value.is_empty() {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

pub(in crate::http_api) fn parse_markdown_token_row(line: &str) -> Option<(String, String)> {
    if !line.starts_with('|') || !line.ends_with('|') {
        return None;
    }
    let cells = line
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect::<Vec<_>>();
    if cells.len() < 2
        || cells.iter().all(|cell| {
            cell.chars()
                .all(|character| matches!(character, '-' | ':' | ' '))
        })
    {
        return None;
    }
    let name = cells[0].trim_matches('`');
    let value = cells[1].trim_matches('`');
    let token_like_name = name.starts_with("--")
        || name.contains('.')
        || name.contains('-')
        || name.to_ascii_lowercase().contains("color");
    if !token_like_name || value.is_empty() || value.eq_ignore_ascii_case("value") {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

pub(in crate::http_api) fn unmapped_source_item(
    source_section: &str,
    start_byte: usize,
    end_byte: usize,
    line: &str,
    reason: &str,
) -> DesignProfileUnmappedItem {
    let excerpt = line.chars().take(500).collect::<String>();
    DesignProfileUnmappedItem {
        source_section: source_section.to_string(),
        start_byte,
        end_byte,
        excerpt_hash: sha256_hex(excerpt.as_bytes()),
        excerpt,
        reason: reason.to_string(),
    }
}

pub(in crate::http_api) fn design_profile_candidate_issues(
    candidate: &Value,
    imported: bool,
) -> Vec<DesignProfileValidationIssue> {
    let required_fields = [
        "product",
        "brand",
        "visual",
        "tokens",
        "runtimeTokenMapping",
        "components",
        "content",
        "accessibility",
        "technical",
        "governance",
    ];
    let mut issues = Vec::new();
    let object = match candidate.as_object() {
        Some(object) => object,
        None => {
            issues.push(DesignProfileValidationIssue {
                path: "candidate".to_string(),
                code: "invalid_type".to_string(),
                message: "candidate must be an object".to_string(),
                blocking: true,
            });
            return issues;
        }
    };
    for field in required_fields {
        if !object.contains_key(field) {
            issues.push(DesignProfileValidationIssue {
                path: field.to_string(),
                code: "required".to_string(),
                message: format!("{field} is required before activation"),
                blocking: true,
            });
        }
    }
    if imported
        && object
            .get("signatureRules")
            .and_then(Value::as_array)
            .is_none_or(|rules| {
                !rules
                    .iter()
                    .any(|rule| rule.get("priority").and_then(Value::as_str) == Some("required"))
            })
    {
        issues.push(DesignProfileValidationIssue {
            path: "signatureRules".to_string(),
            code: "required_signature_rule".to_string(),
            message: "imported profile requires at least one required signature rule".to_string(),
            blocking: true,
        });
    }
    issues
}

pub(in crate::http_api) async fn resolve_design_profile_context(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Result<Option<DesignProfile>, (StatusCode, Json<ErrorResponse>)> {
    store
        .resolve_design_profile(
            &request.project_id,
            request.input_context.workspace_id.as_deref(),
            request.input_context.organization_id.as_deref(),
            request.input_context.design_profile_id.as_deref(),
        )
        .await
        .map_err(design_profile_error)
}

pub(in crate::http_api) async fn design_profile_execution_target(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Result<Option<(String, String)>, (StatusCode, Json<ErrorResponse>)> {
    if request.phase != AgentPhase::Build {
        return Ok(None);
    }
    let Some(brief_id) = request.input_context.brief_id.as_deref() else {
        return Ok(None);
    };
    let brief = store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    let template = resolve_built_in_template_for_init(&brief.recommended_template)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Some((
        template.surface.to_string(),
        brief.recommended_template,
    )))
}

pub(in crate::http_api) async fn design_profile_prebuild_failure(
    store: &RuntimeStore,
    run: &AgentRun,
    profile: &DesignProfile,
) -> Option<(String, String)> {
    if run.phase != AgentPhase::Build {
        return None;
    }
    if profile.status != "active" {
        return Some((
            "needs_user_input:design_profile_integrity_failed".to_string(),
            "DesignProfile must be active before Build.".to_string(),
        ));
    }
    if run.design_profile_hash.as_deref() != Some(profile.stable_hash().as_str()) {
        return Some((
            "needs_user_input:design_profile_integrity_failed".to_string(),
            "DesignProfile hash no longer matches the run snapshot.".to_string(),
        ));
    }
    if let (Some(surface), Some(template), Some(expected_hash)) = (
        run.design_profile_surface.as_deref(),
        run.design_profile_template.as_deref(),
        run.design_profile_effective_hash.as_deref(),
    ) {
        match profile.effective_for(surface, template) {
            Ok(effective) if effective.effective_profile_hash == expected_hash => {}
            _ => {
                return Some((
                    "needs_user_input:design_profile_integrity_failed".to_string(),
                    "Effective DesignProfile hash or template resolution changed.".to_string(),
                ))
            }
        }
    }
    if profile.schema_version == crate::types::DESIGN_PROFILE_SCHEMA_V1 {
        store
            .append_audit_record(
                &run.project_id,
                &run.id,
                "design_profile.legacy_source",
                "schemaVersion=design-profile@1",
                "allow",
                "legacy-warning: source artifact verification unavailable",
            )
            .await;
        return None;
    }
    if profile.source.get("kind").and_then(Value::as_str) != Some("imported") {
        return None;
    }
    if profile.source.get("integrity").and_then(Value::as_str) != Some("verified") {
        return Some((
            "needs_user_input:design_profile_integrity_failed".to_string(),
            "Imported DesignProfile source integrity is not verified.".to_string(),
        ));
    }
    let Some(artifact_id) = run.design_source_artifact_id.as_deref() else {
        return Some((
            "needs_user_input:design_profile_source_missing".to_string(),
            "Imported DesignProfile source artifact is missing from the run snapshot.".to_string(),
        ));
    };
    let Some(artifact) = store.get_design_source_artifact(artifact_id).await else {
        return Some((
            "needs_user_input:design_profile_source_missing".to_string(),
            "Imported DesignProfile source artifact metadata is missing.".to_string(),
        ));
    };
    if run.design_source_hash.as_deref() != Some(artifact.sha256.as_str())
        || profile.source.get("sourceHash").and_then(Value::as_str)
            != Some(artifact.sha256.as_str())
    {
        return Some((
            "needs_user_input:design_profile_integrity_failed".to_string(),
            "Imported DesignProfile source hash does not match the immutable artifact.".to_string(),
        ));
    }
    if store
        .read_design_source_artifact_content(artifact_id)
        .await
        .is_err()
    {
        return Some((
            "needs_user_input:design_profile_integrity_failed".to_string(),
            "Imported DesignProfile source bytes failed integrity verification.".to_string(),
        ));
    }
    None
}

pub(in crate::http_api) async fn preflight_design_profile_conflicts(
    store: &RuntimeStore,
    request: &StartRunRequest,
    design_profile: Option<&DesignProfile>,
) -> Result<Option<String>, (StatusCode, Json<ErrorResponse>)> {
    let Some(design_profile) = design_profile else {
        return Ok(None);
    };
    if request.phase != AgentPhase::Build {
        return Ok(None);
    }
    let Some(brief_id) = request.input_context.brief_id.as_deref() else {
        return Ok(None);
    };
    let brief = store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    let allowed = design_profile
        .technical
        .get("allowedTemplates")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    if !allowed.is_empty() && !allowed.contains(&brief.recommended_template.as_str()) {
        return Ok(Some(format!(
            "Brief recommendedTemplate={} is not allowed by DesignProfile {}",
            brief.recommended_template, design_profile.id
        )));
    }
    Ok(None)
}
