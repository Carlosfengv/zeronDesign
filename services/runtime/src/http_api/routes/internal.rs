use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/template-build", post(internal_template_build))
        .route("/internal/previews/promote", post(internal_promote_preview))
        .route(
            "/internal/projects/{project_id}/access",
            put(internal_upsert_project_access),
        )
        .route(
            "/internal/projects/{project_id}/release-evidence",
            get(internal_project_release_evidence),
        )
        .route(
            "/internal/projects/{project_id}/release-sandbox",
            post(internal_release_project_sandbox),
        )
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

async fn internal_upsert_project_access(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpsertProjectAccessRequest>,
) -> Result<Json<ProjectAccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        state
            .store
            .append_audit_record(
                &project_id,
                "",
                "internal.project_access.upsert",
                "project access record".to_string(),
                "deny",
                "missing or invalid internal service authorization",
            )
            .await;
        return Err(unauthorized(
            "project access update requires service authorization".to_string(),
        ));
    }
    if project_id.trim().is_empty() || request.owner_principal_id.trim().is_empty() {
        return Err(bad_request(
            "projectId and ownerPrincipalId are required".to_string(),
        ));
    }
    let record = state
        .store
        .upsert_project_access(
            &project_id,
            request.owner_principal_id,
            request.workspace_id,
            request.organization_id,
        )
        .await
        .map_err(internal_error)?;
    state
        .store
        .append_audit_record(
            &project_id,
            "",
            "internal.project_access.upsert",
            "project access record".to_string(),
            "allow",
            "project access record persisted",
        )
        .await;
    Ok(Json(ProjectAccessResponse {
        project_access: record,
    }))
}

async fn internal_project_release_evidence(
    State(state): State<AppState>,
    Extension(evidence): Extension<Arc<dyn RuntimeEvidenceStore>>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        return Err(unauthorized(
            "release evidence requires service authorization".to_string(),
        ));
    }
    let current = state
        .store
        .current_project_version(&project_id)
        .await
        .ok_or_else(|| not_found(format!("current version not found: {project_id}")))?;
    let edit_run_id = current.created_by_run_id.clone();
    let publish = state
        .store
        .artifact_publish_for_version(&project_id, &edit_run_id, &current.id)
        .await
        .ok_or_else(|| not_found(format!("artifact publish not found: {}", current.id)))?;
    let base_version_id = publish.expected_current_version_id.clone().ok_or_else(|| {
        conflict_error(anyhow::anyhow!(
            "release evidence requires an Edit promotion with a base version"
        ))
    })?;
    let base_version = state
        .store
        .get_project_version(&base_version_id)
        .await
        .ok_or_else(|| not_found(format!("base version not found: {base_version_id}")))?;
    let lease = state
        .store
        .preview_lease_for_run(&edit_run_id)
        .await
        .ok_or_else(|| not_found(format!("preview lease not found: {edit_run_id}")))?;
    let binding = state
        .store
        .get_sandbox_binding(&lease.sandbox_binding_id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "sandbox binding not found: {}",
                lease.sandbox_binding_id
            ))
        })?;
    let events = state.store.events(&edit_run_id).await;
    let build_events = state.store.events(&base_version.created_by_run_id).await;
    let failure_counts = build_events.iter().chain(events.iter()).fold(
        (0_u64, 0_u64),
        |(recoverable, terminal), event| match event {
            AgentEvent::ToolFailed {
                recoverable: true, ..
            } => (recoverable + 1, terminal),
            AgentEvent::ToolFailed {
                recoverable: false, ..
            } => (recoverable, terminal + 1),
            _ => (recoverable, terminal),
        },
    );
    let preview_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::PreviewUpdated { .. }))
        .ok_or_else(|| conflict_error(anyhow::anyhow!("preview.updated event missing")))?;
    let completed_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::RunCompleted { .. }))
        .ok_or_else(|| conflict_error(anyhow::anyhow!("run.completed event missing")))?;
    let screenshot_id = current
        .screenshot_id
        .clone()
        .ok_or_else(|| conflict_error(anyhow::anyhow!("screenshot ID missing")))?;
    let screenshot = evidence
        .read_screenshot(&project_id, &edit_run_id, &screenshot_id)
        .map_err(internal_error)?;
    Ok(Json(json!({
        "projectId": project_id,
        "buildRunId": base_version.created_by_run_id,
        "editRunId": edit_run_id,
        "bindingId": binding.id,
        "podUid": binding.pod_uid,
        "buildId": publish.build_id,
        "candidateManifestHash": publish.candidate_manifest_hash,
        "sourceSnapshotUri": publish.source_snapshot_uri,
        "previewLeaseId": lease.id,
        "previewLeaseStatus": lease.status,
        "screenshotId": screenshot_id,
        "nonblankPixelRatio": screenshot["nonblankPixelRatio"],
        "screenshotPngSha256": screenshot["pngSha256"],
        "screenshotDocumentSha256": screenshot["documentSha256"],
        "versionBeforeCas": base_version_id,
        "versionAfterCas": current.id,
        "artifactManifestHash": publish.artifact_manifest_hash,
        "artifactUrl": format!("/artifacts/{project_id}/current/"),
        "events": {
            "previewUpdated": format!("{}/{}", current.created_by_run_id, preview_index),
            "runCompleted": format!("{}/{}", current.created_by_run_id, completed_index),
            "sequenceValid": preview_index < completed_index,
        },
        "recoverableToolFailureCount": failure_counts.0,
        "terminalToolFailureCount": failure_counts.1,
        "sandboxStatus": binding.status,
        "sandboxReleasedAt": binding.last_seen_at,
    })))
}

async fn internal_release_project_sandbox(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    if !internal_admin_authorized(&state.config, &headers) {
        return Err(unauthorized(
            "sandbox release requires service authorization".to_string(),
        ));
    }
    let binding = state
        .store
        .current_project_sandbox_binding(&project_id)
        .await
        .ok_or_else(|| not_found(format!("sandbox binding not found: {project_id}")))?;
    let backend = sandbox_backend_for_config(&state.config);
    backend
        .release(&state.store, &binding.id)
        .await
        .map_err(internal_error)?;
    let released = state
        .store
        .get_sandbox_binding(&binding.id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "released sandbox binding not found: {}",
                binding.id
            ))
        })?;
    Ok(Json(json!({
        "projectId": project_id,
        "bindingId": released.id,
        "status": released.status,
        "releasedAt": released.last_seen_at,
    })))
}

async fn internal_promote_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PromotePreviewRequest>,
) -> Result<Json<PreviewCurrentResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_promote_preview_request(&request)?;
    if !state.config.enable_internal_promote_api {
        return Err(not_found(
            "internal preview promotion endpoint is disabled".to_string(),
        ));
    }
    if !internal_admin_authorized(&state.config, &headers) {
        state
            .store
            .append_audit_record(
                &request.project_id,
                &request.run_id,
                "internal.previews.promote",
                format!("candidateVersionId={}", request.candidate_version_id),
                "deny",
                "missing or invalid internal promote authorization",
            )
            .await;
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "internal preview promotion requires service authorization".to_string(),
            }),
        ));
    }
    let version = promote_preview(
        &state.store,
        &request.project_id,
        &request.run_id,
        &request.candidate_version_id,
        request.gate_report.into(),
    )
    .await
    .map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: error.to_string(),
            }),
        )
    })?;
    state
        .store
        .append_audit_record(
            &request.project_id,
            &request.run_id,
            "internal.previews.promote",
            format!("candidateVersionId={}", version.id),
            "allow",
            "internal preview promotion API",
        )
        .await;
    Ok(Json(PreviewCurrentResponse {
        project_id: request.project_id,
        version_id: version.id,
        preview_url: version.preview_url,
        status: "promoted",
    }))
}
