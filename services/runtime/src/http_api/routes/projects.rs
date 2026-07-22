use super::super::*;

mod draft_previews;
mod project_assets;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project_id}/conversation",
            get(project_conversation),
        )
        .route(
            "/projects/{project_id}/runtime-state",
            get(project_runtime_state),
        )
        .route("/projects/{project_id}/history", get(project_history))
        .route(
            "/projects/{project_id}/draft-preview",
            get(draft_previews::current_draft_preview),
        )
        .route(
            "/draft-preview-sessions/{session_id}",
            get(draft_previews::get_draft_preview),
        )
        .route(
            "/draft-preview-sessions/{session_id}/heartbeat",
            post(draft_previews::heartbeat_draft_preview),
        )
        .route(
            "/draft-preview-sessions/{session_id}/takeover",
            post(draft_previews::takeover_draft_preview),
        )
        .route(
            "/projects/{project_id}/element-observations",
            post(create_element_observation),
        )
        .route(
            "/projects/{project_id}/element-observations/{observation_id}",
            get(get_element_observation),
        )
        .route(
            "/projects/{project_id}/edit-impact-plans",
            post(create_edit_impact_plan),
        )
        .route(
            "/projects/{project_id}/edit-impact-plans/{plan_hash}",
            get(get_edit_impact_plan),
        )
        .route(
            "/projects/{project_id}/edit-impact-plans/{plan_hash}/confirm",
            post(confirm_edit_impact_plan),
        )
        .route(
            "/projects/{project_id}/assets",
            get(project_assets::list_project_assets),
        )
        .route(
            "/projects/{project_id}/assets/{asset_id}",
            get(project_assets::get_project_asset),
        )
        .route(
            "/projects/{project_id}/visual-artifacts",
            post(create_visual_artifact)
                .layer(DefaultBodyLimit::max(MAX_VISUAL_ARTIFACT_REQUEST_BYTES)),
        )
        .route(
            "/projects/{project_id}/visual-artifacts/{artifact_id}",
            get(get_visual_artifact).delete(delete_visual_artifact),
        )
        .route(
            "/projects/{project_id}/visual-artifacts/{artifact_id}/content",
            get(get_visual_artifact_content),
        )
        .route(
            "/projects/{project_id}/runs/{run_id}/visual-bindings",
            get(list_run_visual_bindings).post(create_run_visual_binding),
        )
        .route(
            "/projects/{project_id}/visual-reviews",
            post(schedule_visual_review),
        )
}

async fn create_element_observation(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateElementObservationRequest>,
) -> Result<Json<crate::visual_contracts::ElementObservation>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    load_project_visual_artifact(&state, &project_id, &request.screenshot_crop_artifact_id).await?;
    state
        .store
        .edit_guard_store()
        .create_observation(
            &state.store.draft_preview_store(),
            crate::edit_guard::CreateElementObservation {
                project_id,
                session_id: request.session_id,
                session_epoch: request.session_epoch,
                workspace_revision: request.workspace_revision,
                route: request.route,
                viewport: request.viewport,
                dom_path: request.dom_path,
                data_slot: request.data_slot,
                accessible_name: request.accessible_name,
                visible_text_hash: request.visible_text_hash,
                bounding_box: request.bounding_box,
                source_candidates: request.source_candidates,
                screenshot_crop_artifact_id: request.screenshot_crop_artifact_id,
            },
        )
        .map(Json)
        .map_err(edit_guard_error)
}

async fn get_element_observation(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, observation_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<crate::visual_contracts::ElementObservation>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let observation = state
        .store
        .edit_guard_store()
        .get_observation(&state.store.draft_preview_store(), &observation_id)
        .map_err(edit_guard_error)?;
    if observation.project_id != project_id {
        return Err(not_found(format!(
            "ElementObservation not found: {observation_id}"
        )));
    }
    Ok(Json(observation))
}

async fn create_edit_impact_plan(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(run_lifecycle): Extension<RunLifecycleService>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateEditImpactPlanRequest>,
) -> Result<Json<crate::visual_contracts::EditImpactPlan>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    let predecessor_run_id = request.predecessor_run_id.clone();
    if let Some(predecessor_run_id) = predecessor_run_id.as_deref() {
        authorize_project_run(&state, &project_id, predecessor_run_id).await?;
    }
    let plan = state
        .store
        .edit_guard_store()
        .create_plan(
            &state.store.draft_preview_store(),
            crate::edit_guard::CreateEditImpactPlan {
                observation_id: request.observation_id,
                scope: request.scope,
                targets: request.targets,
                operations: request.operations,
                risk: request.risk,
                edit_base: request.edit_base,
            },
        )
        .map_err(edit_guard_error)?;
    let (plan_project_id, _) = state
        .store
        .edit_guard_store()
        .get_plan(&plan.plan_hash)
        .ok_or_else(|| internal_error(anyhow::anyhow!("created EditImpactPlan is missing")))?;
    if plan_project_id != project_id {
        return Err(not_found("EditImpactPlan not found".to_string()));
    }
    if let Some(predecessor_run_id) = predecessor_run_id.as_deref() {
        state
            .store
            .edit_guard_store()
            .bind_replan_predecessor(&plan.plan_hash, &project_id, predecessor_run_id)
            .map_err(edit_guard_error)?;
        if !plan.requires_confirmation {
            run_lifecycle
                .dispatch_replan_successor(predecessor_run_id, &plan.plan_hash)
                .await
                .map_err(run_lifecycle_error)?;
        }
    }
    Ok(Json(plan))
}

async fn get_edit_impact_plan(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, plan_hash)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<crate::visual_contracts::EditImpactPlan>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let (stored_project_id, plan) = state
        .store
        .edit_guard_store()
        .get_plan(&plan_hash)
        .ok_or_else(|| not_found(format!("EditImpactPlan not found: {plan_hash}")))?;
    if stored_project_id != project_id {
        return Err(not_found(format!("EditImpactPlan not found: {plan_hash}")));
    }
    Ok(Json(plan))
}

async fn confirm_edit_impact_plan(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(run_lifecycle): Extension<RunLifecycleService>,
    Path((project_id, plan_hash)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<crate::visual_contracts::EditImpactPlan>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    let (stored_project_id, _) = state
        .store
        .edit_guard_store()
        .get_plan(&plan_hash)
        .ok_or_else(|| not_found(format!("EditImpactPlan not found: {plan_hash}")))?;
    if stored_project_id != project_id {
        return Err(not_found(format!("EditImpactPlan not found: {plan_hash}")));
    }
    let plan = state
        .store
        .edit_guard_store()
        .confirm(&state.store.draft_preview_store(), &plan_hash)
        .map_err(edit_guard_error)?;
    if let Some(predecessor_run_id) = state
        .store
        .edit_guard_store()
        .replan_predecessor(&plan_hash)
    {
        run_lifecycle
            .dispatch_replan_successor(&predecessor_run_id, &plan_hash)
            .await
            .map_err(run_lifecycle_error)?;
    }
    Ok(Json(plan))
}

fn edit_guard_error(error: crate::edit_guard::EditGuardError) -> (StatusCode, Json<ErrorResponse>) {
    use crate::edit_guard::EditGuardError;
    match error {
        EditGuardError::InvalidInput(message) => bad_request(message),
        EditGuardError::NotFound(message) => not_found(message),
        EditGuardError::Conflict { kind, message } => (
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: message,
                error_code: Some(kind.to_string()),
            }),
        ),
        EditGuardError::Storage(message) => internal_error(anyhow::anyhow!(message)),
    }
}

async fn schedule_visual_review(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Extension(service): Extension<VisualReviewService>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ScheduleVisualReviewHttpRequest>,
) -> Result<Json<VisualReviewResult>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    let model = request
        .model
        .unwrap_or_else(|| state.config.agent_model.clone());
    let result = service
        .schedule(ScheduleVisualReviewRequest {
            project_id,
            mode: request.mode,
            target: request.target,
            model,
            bindings: request.bindings,
        })
        .await
        .map_err(visual_artifact_error)?;
    Ok(Json(result))
}

async fn authorize_project_run(
    state: &AppState,
    project_id: &str,
    run_id: &str,
) -> Result<crate::types::AgentRun, (StatusCode, Json<ErrorResponse>)> {
    let run = state
        .store
        .get_run(run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    if run.project_id != project_id {
        return Err(not_found(format!("run not found: {run_id}")));
    }
    Ok(run)
}

async fn create_run_visual_binding(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, run_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<CreateRunVisualBindingRequest>,
) -> Result<Json<RunVisualBindingResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    let run = authorize_project_run(&state, &project_id, &run_id).await?;
    if run.generation_context_status.is_some() {
        return Err(conflict_error(anyhow::anyhow!(
            "Run visual bindings are frozen after Generation Context compilation; start a new Run with inputContext.visualBindings"
        )));
    }
    let artifact = load_project_visual_artifact(&state, &project_id, &request.artifact_id).await?;
    if artifact.retention_state
        == crate::visual_contracts::DraftSnapshotRetentionState::DeletionPending
    {
        return Err(conflict_error(anyhow::anyhow!(
            "VisualArtifact is pending deletion and cannot be bound"
        )));
    }
    match &request.target {
        RunVisualTarget::StaticSnapshot { snapshot_id, .. } => {
            let snapshot = state
                .store
                .get_draft_snapshot(snapshot_id)
                .await
                .ok_or_else(|| not_found(format!("DraftSnapshot not found: {snapshot_id}")))?;
            if snapshot.project_id != project_id {
                return Err(not_found(format!("DraftSnapshot not found: {snapshot_id}")));
            }
        }
        RunVisualTarget::Version { version_id, .. } => {
            let version = state
                .store
                .get_project_version(version_id)
                .await
                .ok_or_else(|| not_found(format!("project version not found: {version_id}")))?;
            if version.project_id != project_id {
                return Err(not_found(format!(
                    "project version not found: {version_id}"
                )));
            }
        }
        RunVisualTarget::Draft { .. } => {}
    }
    let binding = state
        .store
        .upsert_run_visual_binding(RunVisualBinding {
            run_id,
            artifact_id: request.artifact_id,
            role: request.role,
            route: request.route,
            viewport: request.viewport,
            target: request.target,
            order: request.order,
        })
        .await
        .map_err(visual_artifact_error)?;
    Ok(Json(RunVisualBindingResponse { binding }))
}

async fn list_run_visual_bindings(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, run_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<RunVisualBindingListResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    authorize_project_run(&state, &project_id, &run_id).await?;
    let bindings = state
        .store
        .run_visual_bindings(&run_id)
        .await
        .map_err(visual_artifact_error)?;
    Ok(Json(RunVisualBindingListResponse { bindings }))
}

async fn create_visual_artifact(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateVisualArtifactRequest>,
) -> Result<Json<VisualArtifactResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    if request.content_base64.len() > MAX_VISUAL_ARTIFACT_BASE64_BYTES {
        return Err(bad_request(format!(
            "contentBase64 exceeds the {MAX_VISUAL_ARTIFACT_INPUT_BYTES}-byte decoded image limit"
        )));
    }
    let content = BASE64_STANDARD
        .decode(request.content_base64.as_bytes())
        .map_err(|_| bad_request("contentBase64 must be valid base64".to_string()))?;
    if content.is_empty() || content.len() > MAX_VISUAL_ARTIFACT_INPUT_BYTES {
        return Err(bad_request(format!(
            "decoded visual artifact must contain 1..={MAX_VISUAL_ARTIFACT_INPUT_BYTES} bytes"
        )));
    }
    let input_digest = sha256_hex(&content);
    if let Some(client_sha256) = request.client_sha256.as_deref() {
        if client_sha256.len() != 64
            || !client_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !input_digest.eq_ignore_ascii_case(client_sha256)
        {
            return Err(bad_request(
                "clientSha256 must match the decoded input image bytes".to_string(),
            ));
        }
    }
    let root = state.config.runtime_storage_dir.join("visual-artifacts");
    let artifact = tokio::task::spawn_blocking(move || {
        VisualArtifactStore::open(root)?.create_upload(
            &project_id,
            &content,
            request.origin_metadata,
        )
    })
    .await
    .map_err(|error| internal_error(anyhow::anyhow!("visual artifact task failed: {error}")))?
    .map_err(visual_artifact_error)?;
    Ok(Json(VisualArtifactResponse { artifact }))
}

pub(in crate::http_api) async fn load_project_visual_artifact(
    state: &AppState,
    project_id: &str,
    artifact_id: &str,
) -> Result<crate::visual_contracts::VisualArtifact, (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("artifactId", artifact_id)?;
    let store =
        VisualArtifactStore::open(state.config.runtime_storage_dir.join("visual-artifacts"))
            .map_err(visual_artifact_error)?;
    let artifact = store
        .get(artifact_id)
        .map_err(visual_artifact_error)?
        .ok_or_else(|| not_found(format!("visual artifact not found: {artifact_id}")))?;
    if artifact.project_id != project_id {
        return Err(not_found(format!(
            "visual artifact not found: {artifact_id}"
        )));
    }
    Ok(artifact)
}

async fn get_visual_artifact(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, artifact_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<VisualArtifactResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let artifact = load_project_visual_artifact(&state, &project_id, &artifact_id).await?;
    Ok(Json(VisualArtifactResponse { artifact }))
}

async fn delete_visual_artifact(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, artifact_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<VisualArtifactResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_WRITE_OPERATION,
    )
    .await?;
    load_project_visual_artifact(&state, &project_id, &artifact_id).await?;
    let mut referenced = false;
    for run in state
        .store
        .project_runs(&project_id)
        .await
        .map_err(visual_artifact_error)?
    {
        if state
            .store
            .run_visual_bindings(&run.id)
            .await
            .map_err(visual_artifact_error)?
            .iter()
            .any(|binding| binding.artifact_id == artifact_id)
        {
            referenced = true;
            break;
        }
    }
    referenced |=
        crate::visual_review::FileVisualReviewStore::new(&state.config.runtime_storage_dir)
            .artifact_is_referenced(&artifact_id)
            .map_err(visual_artifact_error)?;
    let artifact =
        VisualArtifactStore::open(state.config.runtime_storage_dir.join("visual-artifacts"))
            .and_then(|store| store.request_delete(&artifact_id, referenced))
            .map_err(visual_artifact_error)?;
    Ok(Json(VisualArtifactResponse { artifact }))
}

async fn get_visual_artifact_content(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path((project_id, artifact_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let artifact = load_project_visual_artifact(&state, &project_id, &artifact_id).await?;
    let root = state.config.runtime_storage_dir.join("visual-artifacts");
    let content_id = artifact_id.clone();
    let content = tokio::task::spawn_blocking(move || {
        VisualArtifactStore::open(root)?.read_content(&content_id)
    })
    .await
    .map_err(|error| internal_error(anyhow::anyhow!("visual artifact task failed: {error}")))?
    .map_err(visual_artifact_error)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response_headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response_headers.insert(
        "x-visual-artifact-sha256",
        HeaderValue::from_str(&artifact.sha256)
            .map_err(|_| internal_error(anyhow::anyhow!("invalid stored visual artifact hash")))?,
    );
    Ok((response_headers, content))
}

async fn project_conversation(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ConversationQuery>,
) -> Result<Json<ConversationListResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let mut items = state.store.conversation_items(&project_id).await;
    if !query.include_debug {
        items.retain(|item| item.visibility == "user");
    }
    Ok(Json(ConversationListResponse { project_id, items }))
}

async fn project_runtime_state(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ProjectRuntimeStateResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let current = state
        .store
        .current_project_version(&project_id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "current version not found for project: {project_id}"
            ))
        })?;
    let binding = state
        .store
        .current_project_sandbox_binding(&project_id)
        .await
        .ok_or_else(|| {
            conflict_error(anyhow::anyhow!(
                "editable sandbox binding not found for project: {project_id}"
            ))
        })?;
    let source_snapshot_uri = current.source_snapshot_uri.clone().ok_or_else(|| {
        conflict_error(anyhow::anyhow!(
            "source snapshot not found for current version: {}",
            current.id
        ))
    })?;
    let current_run = state.store.get_run(&current.created_by_run_id).await;
    let template_key = if let Some(run) = current_run.as_ref() {
        if let Some(brief_id) = &run.brief_version {
            state
                .store
                .get_brief(brief_id)
                .await
                .map(|brief| brief.recommended_template)
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            "unknown".to_string()
        }
    } else {
        "unknown".to_string()
    };
    let style_contract = read_runtime_state_json(
        &state,
        &project_id,
        current_run.as_ref(),
        Some(&binding.id),
        "state/style-contract.json",
    )
    .await;
    let latest_build = read_runtime_state_json(
        &state,
        &project_id,
        current_run.as_ref(),
        Some(&binding.id),
        "outputs/build/latest.json",
    )
    .await;
    let dependency_state = read_runtime_state_json(
        &state,
        &project_id,
        current_run.as_ref(),
        Some(&binding.id),
        "state/dependency-state.json",
    )
    .await;
    let preview = read_runtime_state_json(
        &state,
        &project_id,
        current_run.as_ref(),
        Some(&binding.id),
        "state/preview.json",
    )
    .await;

    Ok(Json(ProjectRuntimeStateResponse {
        project_id,
        current_version_id: current.id,
        sandbox_binding_id: binding.id,
        source_snapshot_uri,
        app_root: "project".to_string(),
        template_key,
        style_contract_path: style_contract
            .as_ref()
            .map(|_| "/workspace/state/style-contract.json".to_string()),
        style_contract,
        latest_build,
        dependency_state,
        preview,
    }))
}

async fn project_history(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ProjectHistoryListResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;

    let mut items = state
        .store
        .list_project_draft_snapshots(&project_id)
        .await
        .into_iter()
        .map(|snapshot| HistoryItem::DraftSnapshot {
            snapshot,
            recoverable: true,
            publishable: false,
        })
        .chain(
            state
                .store
                .list_project_versions(&project_id)
                .await
                .into_iter()
                .map(|version| {
                    let recoverable = version.source_snapshot_uri.is_some();
                    let publishable =
                        version.status == crate::types::ProjectVersionStatus::Promoted;
                    HistoryItem::WorkVersion {
                        version,
                        recoverable,
                        publishable,
                    }
                }),
        )
        .collect::<Vec<_>>();
    items.sort_by_key(|item| match item {
        HistoryItem::DraftSnapshot { snapshot, .. } => snapshot.created_at,
        HistoryItem::WorkVersion { version, .. } => version.created_at,
    });
    items.reverse();

    Ok(Json(ProjectHistoryListResponse { project_id, items }))
}
