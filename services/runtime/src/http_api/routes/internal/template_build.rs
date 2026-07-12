use super::super::super::*;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/internal/template-build", post(internal_template_build))
}

async fn internal_template_build(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<InternalTemplateBuildRequest>,
) -> Result<Json<InternalTemplateBuildResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !state.config.enable_internal_template_build_api {
        return Err(not_found(
            "internal template build endpoint is disabled".to_string(),
        ));
    }
    if !internal_admin_authorized(&state.config, &headers) {
        state
            .store
            .append_audit_record(
                &request.project_id,
                "",
                "internal.template_build",
                format!("template={}", request.template),
                "deny",
                "missing or invalid internal template build authorization",
            )
            .await;
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "internal template build requires service authorization".to_string(),
            }),
        ));
    }
    validate_internal_template_build_request(&request)?;
    let project_id = request.project_id.clone();
    let workspace_root = project_workspace_root(&state.config, &project_id);
    let template = request.template.clone();
    let template_spec = resolve_built_in_template_for_init(&template)
        .await
        .map_err(|error| bad_request(error.to_string()))?;
    let content_hierarchy = if request.content_hierarchy.is_empty() {
        vec![template_spec.default_title.to_string()]
    } else {
        request.content_hierarchy
    };
    let brief = Brief {
        project_type: template_spec.surface.to_string(),
        audience: request.audience,
        content_hierarchy,
        page_structure: if request.page_structure.is_null() {
            serde_json::json!([])
        } else {
            request.page_structure
        },
        visual_direction: request.visual_direction,
        recommended_template: template,
        assumptions: request.assumptions,
        missing_information: request.missing_information,
    };
    let brief_run = state
        .store
        .create_run(
            project_id.clone(),
            AgentPhase::Brief,
            "brief".to_string(),
            request
                .model
                .clone()
                .unwrap_or_else(|| "internal-template-build".to_string()),
            vec![],
        )
        .await;
    let brief_id = state
        .store
        .write_brief(&brief_run.id, brief)
        .await
        .map_err(internal_error)?;
    let build_run = state
        .store
        .create_run_with_context(
            project_id.clone(),
            AgentPhase::Build,
            "build".to_string(),
            request
                .model
                .unwrap_or_else(|| "internal-template-build".to_string()),
            vec![],
            Some(brief_id.clone()),
            None,
        )
        .await;
    let public_base_url = request
        .public_base_url
        .unwrap_or_else(|| format!("http://{}:{}", state.config.host, state.config.port));
    let output = run_template_build(
        &state.store,
        TemplateBuildRequest {
            project_id: project_id.clone(),
            run_id: build_run.id.clone(),
            brief_id: brief_id.clone(),
            workspace_root,
            preview_base_url: public_base_url.clone(),
        },
    )
    .await
    .map_err(internal_error)?;

    Ok(Json(InternalTemplateBuildResponse {
        project_id: project_id.clone(),
        brief_id,
        run_id: build_run.id.clone(),
        version_id: output.promoted_version.id,
        checkpoint_id: output.checkpoint_id,
        stream_url: format!("{public_base_url}/runs/{}/events", build_run.id),
        preview_url: output.promoted_version.preview_url,
        artifact_url: format!(
            "{}/artifacts/{}/current",
            public_base_url.trim_end_matches('/'),
            project_id
        ),
    }))
}
