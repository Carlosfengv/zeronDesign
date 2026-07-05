use crate::{
    config::RuntimeConfig,
    conversation::RuntimeStore,
    model_gateway::{model_client_from_config, ModelClient},
    preview::{promote_preview, PromotionGateReport},
    profiles::build::{run_template_build, TemplateBuildRequest},
    profiles::edit::{self, EditIntent},
    query_session::QuerySession,
    recovery::{recover_interrupted_runs, RecoveryOutcome},
    tools::control_plane::{control_plane_executor_for_config, sandbox_backend_for_config},
    types::{
        AgentEvent, AgentPhase, AgentRun, AgentRunStatus, Brief, ContentSource, ConversationItem,
    },
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use futures::stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    convert::Infallible,
    fs,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Clone)]
pub struct AppState {
    pub config: RuntimeConfig,
    pub store: RuntimeStore,
    pub model: Arc<dyn ModelClient>,
}

pub fn app_state(config: RuntimeConfig) -> AppState {
    AppState {
        model: Arc::new(
            model_client_from_config(&config)
                .expect("runtime model provider configuration should be valid"),
        ),
        store: RuntimeStore::with_checkpoint_dir(config.runtime_storage_dir.clone()),
        config,
    }
}

pub async fn recover_startup_runs(state: AppState) -> anyhow::Result<AppState> {
    let outcomes = recover_interrupted_runs(&state.store).await?;
    for outcome in outcomes {
        if let RecoveryOutcome::Resumed { run_id, .. } = outcome {
            spawn_session(state.clone(), run_id);
        }
    }
    Ok(state)
}

pub async fn recovered_router(config: RuntimeConfig) -> anyhow::Result<Router> {
    Ok(router_with_state(
        recover_startup_runs(app_state(config)).await?,
    ))
}

pub fn router(config: RuntimeConfig) -> Router {
    router_with_state(app_state(config))
}

pub fn router_with_state(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/runs", post(start_run))
        .route("/runs/{run_id}/continue", post(continue_run))
        .route("/runs/{run_id}/cancel", post(cancel_run))
        .route("/runs/{run_id}/events", get(stream_run_events))
        .route(
            "/projects/{project_id}/conversation",
            get(project_conversation),
        )
        .route("/preview/{project_id}/current", get(preview_current))
        .route("/preview/{project_id}/{version_id}", get(preview_version))
        .route(
            "/artifacts/{project_id}/current",
            get(artifact_current_index),
        )
        .route(
            "/artifacts/{project_id}/current/",
            get(artifact_current_index),
        )
        .route(
            "/artifacts/{project_id}/current/{*artifact_path}",
            get(artifact_current_file),
        )
        .route("/internal/template-build", post(internal_template_build))
        .route("/internal/previews/promote", post(internal_promote_preview))
        .route(
            "/permissions/{permission_id}/decision",
            post(resolve_permission),
        )
        .with_state(state)
}

async fn health(State(_state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse { status: "ready" })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunRequest {
    pub project_id: String,
    pub phase: AgentPhase,
    pub agent_profile: String,
    #[serde(default)]
    pub input_context: StartRunInputContext,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunInputContext {
    #[serde(default)]
    pub content_sources: Vec<ContentSource>,
    pub brief_id: Option<String>,
    pub base_version_id: Option<String>,
    pub sandbox_binding_id: Option<String>,
    pub parent_run_id: Option<String>,
    #[serde(default)]
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StartRunResponse {
    #[serde(rename = "runId")]
    pub run_id: String,
    pub status: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinueRunRequest {
    pub user_message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvePermissionRequest {
    pub decision: PermissionDecision,
    pub updated_input: Option<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

impl PermissionDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunStatusResponse {
    #[serde(rename = "runId")]
    pub run_id: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCurrentResponse {
    pub project_id: String,
    pub version_id: String,
    pub preview_url: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewVersionResponse {
    pub project_id: String,
    pub version_id: String,
    pub preview_url: String,
    pub status: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationQuery {
    #[serde(default)]
    pub include_debug: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationListResponse {
    pub project_id: String,
    pub items: Vec<ConversationItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotePreviewRequest {
    pub project_id: String,
    pub run_id: String,
    pub candidate_version_id: String,
    #[serde(default)]
    pub gate_report: PromotePreviewGateReport,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalTemplateBuildRequest {
    pub project_id: String,
    pub template: String,
    pub audience: String,
    #[serde(default)]
    pub content_hierarchy: Vec<String>,
    pub visual_direction: String,
    #[serde(default)]
    pub page_structure: Value,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub missing_information: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub public_base_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalTemplateBuildResponse {
    pub project_id: String,
    pub brief_id: String,
    pub run_id: String,
    pub version_id: String,
    pub checkpoint_id: String,
    pub stream_url: String,
    pub preview_url: String,
    pub artifact_url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotePreviewGateReport {
    #[serde(default)]
    pub build_log_has_terminal_error: bool,
    #[serde(default = "default_true")]
    pub preview_accessible: bool,
    #[serde(default)]
    pub screenshot_blank: bool,
    #[serde(default = "default_true")]
    pub screenshot_available: bool,
    #[serde(default)]
    pub blocking_findings: u32,
}

impl From<PromotePreviewGateReport> for PromotionGateReport {
    fn from(value: PromotePreviewGateReport) -> Self {
        Self {
            build_log_has_terminal_error: value.build_log_has_terminal_error,
            preview_accessible: value.preview_accessible,
            screenshot_blank: value.screenshot_blank,
            screenshot_available: value.screenshot_available,
            blocking_findings: value.blocking_findings,
        }
    }
}

fn default_true() -> bool {
    true
}

async fn start_run(
    State(state): State<AppState>,
    Json(request): Json<StartRunRequest>,
) -> Result<Json<StartRunResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_sandbox_context(&state.store, &request).await?;
    let run = if let Some(parent_run_id) = request.input_context.parent_run_id.as_deref() {
        if request.phase == AgentPhase::Repair {
            state
                .store
                .create_repair_run_for_findings(
                    parent_run_id,
                    &request.input_context.finding_ids,
                    None,
                    request.agent_profile,
                    "internal-balanced".to_string(),
                )
                .await
                .map_err(repair_run_error)?
        } else {
            state
                .store
                .create_child_run(
                    parent_run_id,
                    request.phase,
                    request.agent_profile,
                    "internal-balanced".to_string(),
                    None,
                    request.input_context.finding_ids,
                )
                .await
                .map_err(|_| not_found(format!("parent run not found: {parent_run_id}")))?
        }
    } else {
        state
            .store
            .create_run_with_context(
                request.project_id,
                request.phase,
                request.agent_profile,
                "internal-balanced".to_string(),
                request.input_context.content_sources,
                request.input_context.brief_id,
                request.input_context.base_version_id,
            )
            .await
    };
    let run = if let Some(sandbox_binding_id) = request.input_context.sandbox_binding_id.as_deref()
    {
        state
            .store
            .bind_run_to_sandbox(&run.id, sandbox_binding_id)
            .await
            .map_err(sandbox_binding_error)?
    } else {
        run
    };
    let run = maybe_provision_build_sandbox(&state, run).await?;
    if sandbox_phase_requires_binding(run.phase) {
        if run.sandbox_id.is_some() {
            let allowed_parent_run_id = request.input_context.parent_run_id.as_deref();
            if let Err(error) = state
                .store
                .acquire_sandbox_binding_for_run(&run.id, allowed_parent_run_id)
                .await
            {
                let _ = state
                    .store
                    .update_run_status(&run.id, AgentRunStatus::Cancelled)
                    .await;
                return Err(sandbox_binding_error(error));
            }
        }
    }
    let run_id = run.id.clone();
    spawn_session(state, run_id.clone());

    Ok(Json(StartRunResponse {
        run_id: run.id,
        status: "queued",
    }))
}

async fn validate_sandbox_context(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let requested_binding = request.input_context.sandbox_binding_id.as_deref();

    if let Some(parent_run_id) = request.input_context.parent_run_id.as_deref() {
        let parent = store
            .get_run(parent_run_id)
            .await
            .ok_or_else(|| not_found(format!("parent run not found: {parent_run_id}")))?;
        if let (Some(parent_binding), Some(requested_binding)) =
            (parent.sandbox_id.as_deref(), requested_binding)
        {
            if parent_binding != requested_binding {
                return Err(conflict_error(anyhow::anyhow!(
                    "child run must use parent sandbox binding {parent_binding}, got {requested_binding}"
                )));
            }
        }
        if sandbox_phase_requires_binding(request.phase)
            && parent.sandbox_id.is_none()
            && requested_binding.is_none()
        {
            return Err(conflict_error(anyhow::anyhow!(
                "{:?} run requires sandboxBindingId or a parent run with an existing sandbox binding",
                request.phase
            )));
        }
        let binding_to_validate = requested_binding.or(parent.sandbox_id.as_deref());
        if let Some(binding_id) = binding_to_validate {
            validate_openable_sandbox_binding(store, binding_id, Some(parent_run_id)).await?;
        }
        return Ok(());
    }

    if let Some(binding_id) = requested_binding {
        validate_openable_sandbox_binding(store, binding_id, None).await?;
    }

    if request.phase == AgentPhase::Build {
        validate_build_confirmed_brief(store, request).await?;
    }

    if sandbox_phase_requires_binding(request.phase) && requested_binding.is_none() {
        if request.phase == AgentPhase::Build {
            return Ok(());
        }
        return Err(conflict_error(anyhow::anyhow!(
            "{:?} run requires sandboxBindingId",
            request.phase
        )));
    }

    Ok(())
}

async fn validate_build_confirmed_brief(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let brief_id = request.input_context.brief_id.as_deref().ok_or_else(|| {
        conflict_error(anyhow::anyhow!(
            "Build run requires a confirmed briefId before generation"
        ))
    })?;
    store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    if !store.is_brief_confirmed(brief_id).await {
        return Err(conflict_error(anyhow::anyhow!(
            "Build run requires a confirmed brief: {brief_id}"
        )));
    }
    Ok(())
}

fn is_brief_confirmation_message(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "confirm"
            | "confirmed"
            | "approve"
            | "approved"
            | "yes"
            | "ok"
            | "确认"
            | "确认 brief"
            | "确认brief"
            | "同意"
            | "可以"
            | "开始生成"
    )
}

async fn maybe_provision_build_sandbox(
    state: &AppState,
    run: AgentRun,
) -> Result<AgentRun, (StatusCode, Json<ErrorResponse>)> {
    if run.phase != AgentPhase::Build || run.sandbox_id.is_some() {
        return Ok(run);
    }
    let Some(brief_id) = run.brief_version.as_deref() else {
        return Ok(run);
    };
    let brief = state
        .store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| not_found(format!("brief not found: {brief_id}")))?;
    let backend = sandbox_backend_for_config(&state.config);
    let binding = match backend
        .claim(&state.store, &run.project_id, &brief.recommended_template)
        .await
    {
        Ok(binding) => binding,
        Err(error) => {
            let _ = state
                .store
                .update_run_status(&run.id, AgentRunStatus::Cancelled)
                .await;
            return Err(sandbox_binding_error(error));
        }
    };
    let binding = match backend
        .wait_ready(&state.store, &binding.id, Some(120_000))
        .await
    {
        Ok(binding) => binding,
        Err(error) => {
            let _ = backend.release(&state.store, &binding.id).await;
            let _ = state
                .store
                .update_run_status(&run.id, AgentRunStatus::Cancelled)
                .await;
            return Err(sandbox_binding_error(error));
        }
    };
    state
        .store
        .bind_run_to_sandbox(&run.id, &binding.id)
        .await
        .map_err(sandbox_binding_error)
}

fn sandbox_phase_requires_binding(phase: AgentPhase) -> bool {
    matches!(
        phase,
        AgentPhase::Build | AgentPhase::Repair | AgentPhase::Review | AgentPhase::Edit
    )
}

async fn validate_openable_sandbox_binding(
    store: &RuntimeStore,
    binding_id: &str,
    allowed_parent_run_id: Option<&str>,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    store
        .ensure_sandbox_binding_available(binding_id, allowed_parent_run_id)
        .await
        .map(|_| ())
        .map_err(sandbox_binding_error)
}

async fn continue_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    Json(request): Json<ContinueRunRequest>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    if run.status.is_terminal() {
        return Err(conflict_error(anyhow::anyhow!(
            "run {run_id} is already terminal with status {:?}",
            run.status
        )));
    }
    state
        .store
        .append_conversation_item(
            &run.project_id,
            Some(&run_id),
            "user_message",
            Some("user"),
            request.user_message.clone(),
            None,
        )
        .await;
    if run.phase == AgentPhase::Brief
        && run.status == AgentRunStatus::NeedsUserInput
        && run.brief_version.is_some()
        && is_brief_confirmation_message(&request.user_message)
    {
        let brief_id = run.brief_version.clone().unwrap();
        state
            .store
            .confirm_brief(&run_id, &brief_id)
            .await
            .map_err(internal_error)?;
        state
            .store
            .update_run_status(&run_id, AgentRunStatus::Completed)
            .await
            .map_err(conflict_error)?;
        state
            .store
            .append_event(AgentEvent::RunCompleted {
                run_id: run_id.clone(),
                status: "completed".to_string(),
                summary: "Brief confirmed.".to_string(),
                timestamp: Utc::now(),
            })
            .await;
        state
            .store
            .append_conversation_item(
                &run.project_id,
                Some(&run_id),
                "run_completed",
                Some("system"),
                "Brief confirmed.",
                Some(serde_json::json!({ "briefId": brief_id })),
            )
            .await;
        return Ok(Json(RunStatusResponse {
            run_id,
            status: "completed".to_string(),
        }));
    }
    if run.status == AgentRunStatus::Running {
        state.store.request_continue_interrupt(&run_id).await;
        state
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: run_id.clone(),
                state: "running:continue_queued".to_string(),
                timestamp: Utc::now(),
            })
            .await;
        return Ok(Json(RunStatusResponse {
            run_id,
            status: "running".to_string(),
        }));
    }
    if run.phase == AgentPhase::Edit {
        match edit::classify_edit_intent(&state.store, &run, &request.user_message)
            .await
            .map_err(internal_error)?
        {
            EditIntent::Compatible => {}
            EditIntent::BriefConflict { reason } => {
                state
                    .store
                    .append_conversation_item(
                        &run.project_id,
                        Some(&run_id),
                        "approval_request",
                        Some("assistant"),
                        format!("This edit may change the confirmed Brief: {reason}"),
                        Some(serde_json::json!({ "reason": reason })),
                    )
                    .await;
                state
                    .store
                    .update_run_status(&run_id, AgentRunStatus::NeedsUserInput)
                    .await
                    .map_err(conflict_error)?;
                state
                    .store
                    .append_event(AgentEvent::StateChanged {
                        run_id: run_id.clone(),
                        state: "needs_user_input:brief_conflict".to_string(),
                        timestamp: Utc::now(),
                    })
                    .await;
                return Ok(Json(RunStatusResponse {
                    run_id,
                    status: "needs_user_input".to_string(),
                }));
            }
        }
    }
    state
        .store
        .update_run_status(&run_id, AgentRunStatus::Running)
        .await
        .map_err(conflict_error)?;
    state
        .store
        .append_event(AgentEvent::StateChanged {
            run_id: run_id.clone(),
            state: "running".to_string(),
            timestamp: Utc::now(),
        })
        .await;
    spawn_session(state, run_id.clone());
    Ok(Json(RunStatusResponse {
        run_id,
        status: "running".to_string(),
    }))
}

async fn cancel_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    state
        .store
        .update_run_status(&run_id, AgentRunStatus::Cancelled)
        .await
        .map_err(run_update_error)?;
    state
        .store
        .append_event(AgentEvent::RunCompleted {
            run_id: run_id.clone(),
            status: "cancelled".to_string(),
            summary: "Run cancelled.".to_string(),
            timestamp: Utc::now(),
        })
        .await;
    Ok(Json(RunStatusResponse {
        run_id,
        status: "cancelled".to_string(),
    }))
}

async fn resolve_permission(
    State(state): State<AppState>,
    Path(permission_id): Path<String>,
    Json(request): Json<ResolvePermissionRequest>,
) -> Result<Json<RunStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let pending_permission = state
        .store
        .pending_permission(&permission_id)
        .await
        .ok_or_else(|| not_found(format!("permission request not found: {permission_id}")))?;
    if pending_permission.status != "pending" {
        return Err(conflict_error(anyhow::anyhow!(
            "permission request {permission_id} is already {}",
            pending_permission.status
        )));
    }
    let permission_run = state
        .store
        .get_run(&pending_permission.run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {}", pending_permission.run_id)))?;
    if permission_run.status.is_terminal() {
        return Err(conflict_error(anyhow::anyhow!(
            "run {} is already terminal with status {:?}",
            permission_run.id,
            permission_run.status
        )));
    }
    let permission = state
        .store
        .resolve_permission(&permission_id, request.decision.as_str())
        .await
        .map_err(internal_error)?;
    let status = match request.decision {
        PermissionDecision::Allow => {
            state
                .store
                .update_run_status(&permission.run_id, AgentRunStatus::Running)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::StateChanged {
                    run_id: permission.run_id.clone(),
                    state: "running".to_string(),
                    timestamp: Utc::now(),
                })
                .await;
            state
                .store
                .append_audit_record(
                    &permission.project_id,
                    &permission.run_id,
                    &permission.tool,
                    request
                        .updated_input
                        .as_ref()
                        .map(|_| "updatedInput provided")
                        .unwrap_or("no updatedInput"),
                    "allow",
                    "permission resolved by API",
                )
                .await;
            spawn_session(state, permission.run_id.clone());
            "running"
        }
        PermissionDecision::Ask => {
            state
                .store
                .update_run_status(&permission.run_id, AgentRunStatus::NeedsUserInput)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::StateChanged {
                    run_id: permission.run_id.clone(),
                    state: "needs_user_input:permission_ask".to_string(),
                    timestamp: Utc::now(),
                })
                .await;
            state
                .store
                .append_audit_record(
                    &permission.project_id,
                    &permission.run_id,
                    &permission.tool,
                    request
                        .updated_input
                        .as_ref()
                        .map(|_| "updatedInput provided")
                        .unwrap_or("permission decision"),
                    "ask",
                    "permission requires additional user input",
                )
                .await;
            "needs_user_input"
        }
        PermissionDecision::Deny => {
            state
                .store
                .update_run_status(&permission.run_id, AgentRunStatus::Blocked)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::PermissionDenied {
                    run_id: permission.run_id.clone(),
                    tool: permission.tool.clone(),
                    reason: "permission denied by API".to_string(),
                    timestamp: Utc::now(),
                })
                .await;
            state
                .store
                .append_conversation_item(
                    &permission.project_id,
                    Some(&permission.run_id),
                    "permission_denied",
                    Some("system"),
                    format!("Permission denied for {}", permission.tool),
                    Some(serde_json::json!({
                        "tool": permission.tool.clone(),
                        "reason": "permission denied by API",
                    })),
                )
                .await;
            state
                .store
                .append_audit_record(
                    &permission.project_id,
                    &permission.run_id,
                    &permission.tool,
                    "permission decision",
                    "deny",
                    "permission denied by API",
                )
                .await;
            "blocked"
        }
    };
    Ok(Json(RunStatusResponse {
        run_id: permission.run_id,
        status: status.to_string(),
    }))
}

async fn stream_run_events(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<
    Sse<impl futures::Stream<Item = Result<Event, Infallible>>>,
    (StatusCode, Json<ErrorResponse>),
> {
    state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    let events = state.store.events(&run_id).await;
    let start_after = last_event_sequence(
        headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok()),
        &run_id,
    );
    let stream = stream::iter(
        events
            .into_iter()
            .enumerate()
            .filter_map(move |(index, event)| {
                let sequence = index + 1;
                if sequence <= start_after {
                    return None;
                }
                let json = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
                Some(Ok(Event::default()
                    .id(format!("{run_id}/{sequence}"))
                    .data(json)))
            }),
    );
    Ok(Sse::new(stream))
}

async fn project_conversation(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ConversationQuery>,
) -> Json<ConversationListResponse> {
    let mut items = state.store.conversation_items(&project_id).await;
    if !query.include_debug {
        items.retain(|item| item.visibility == "user");
    }
    Json(ConversationListResponse { project_id, items })
}

async fn preview_current(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<PreviewCurrentResponse>, (StatusCode, Json<ErrorResponse>)> {
    let version = state
        .store
        .current_project_version(&project_id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "current preview not found for project: {project_id}"
            ))
        })?;
    Ok(Json(PreviewCurrentResponse {
        project_id,
        version_id: version.id,
        preview_url: version.preview_url,
        status: "promoted",
    }))
}

async fn preview_version(
    State(state): State<AppState>,
    Path((project_id, version_id)): Path<(String, String)>,
) -> Result<Json<PreviewVersionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let version = state
        .store
        .get_project_version(&version_id)
        .await
        .ok_or_else(|| not_found(format!("project version not found: {version_id}")))?;
    if version.project_id != project_id {
        return Err(not_found(format!(
            "project version {version_id} not found for project: {project_id}"
        )));
    }
    Ok(Json(PreviewVersionResponse {
        project_id,
        version_id: version.id,
        preview_url: version.preview_url,
        status: serde_json::to_value(version.status)
            .ok()
            .and_then(|status| status.as_str().map(ToString::to_string))
            .unwrap_or_else(|| "unknown".to_string()),
    }))
}

async fn artifact_current_index(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_response(&state.config, &project_id, "")
}

async fn artifact_current_file(
    State(state): State<AppState>,
    Path((project_id, artifact_path)): Path<(String, String)>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_response(&state.config, &project_id, &artifact_path)
}

fn artifact_response(
    config: &RuntimeConfig,
    project_id: &str,
    artifact_path: &str,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    let root = project_workspace_root(config, project_id);
    let output_root = if root.join("project/dist").exists() {
        root.join("project/dist")
    } else if root.join("project/out").exists() {
        root.join("project/out")
    } else {
        return Err(not_found(format!(
            "artifact output not found for project: {project_id}"
        )));
    };
    let path = resolve_artifact_file(&output_root, artifact_path)?;
    let content_type = content_type_for_path(&path);
    let bytes =
        fs::read(&path).map_err(|_| not_found(format!("artifact not found: {artifact_path}")))?;
    let bytes = if content_type.starts_with("text/html") {
        String::from_utf8(bytes)
            .map(|html| rewrite_artifact_html(&html, project_id).into_bytes())
            .unwrap_or_else(|error| error.into_bytes())
    } else {
        bytes
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    Ok((headers, bytes))
}

fn resolve_artifact_file(
    output_root: &FsPath,
    artifact_path: &str,
) -> Result<PathBuf, (StatusCode, Json<ErrorResponse>)> {
    let relative = artifact_path.trim().trim_start_matches('/');
    if relative.is_empty() {
        return static_artifact_path(output_root, &output_root.join("index.html"));
    }

    let requested = static_artifact_path(output_root, &output_root.join(relative))?;
    if requested.is_file() {
        return Ok(requested);
    }
    if requested.is_dir() {
        let index = requested.join("index.html");
        if index.is_file() {
            return Ok(index);
        }
    }
    if FsPath::new(relative).extension().is_none() {
        let html =
            static_artifact_path(output_root, &output_root.join(format!("{relative}.html")))?;
        if html.is_file() {
            return Ok(html);
        }
    }

    Err(not_found(format!("artifact not found: {artifact_path}")))
}

fn static_artifact_path(
    output_root: &FsPath,
    requested: &FsPath,
) -> Result<PathBuf, (StatusCode, Json<ErrorResponse>)> {
    let root = fs::canonicalize(output_root)
        .map_err(|_| not_found("artifact output root is not readable".to_string()))?;
    let path = if requested.exists() {
        fs::canonicalize(requested)
            .map_err(|_| not_found("artifact path is not readable".to_string()))?
    } else {
        let parent = requested
            .parent()
            .ok_or_else(|| not_found("artifact path is invalid".to_string()))?;
        let parent = fs::canonicalize(parent)
            .map_err(|_| not_found("artifact parent path is not readable".to_string()))?;
        parent.join(
            requested
                .file_name()
                .ok_or_else(|| not_found("artifact path is invalid".to_string()))?,
        )
    };
    if !path.starts_with(&root) {
        return Err(conflict_error(anyhow::anyhow!(
            "artifact path escapes project output"
        )));
    }
    Ok(path)
}

fn rewrite_artifact_html(html: &str, project_id: &str) -> String {
    let prefix = format!("/artifacts/{project_id}/current");
    html.replace("href=\"/_next/", &format!("href=\"{prefix}/_next/"))
        .replace("src=\"/_next/", &format!("src=\"{prefix}/_next/"))
        .replace("href=\"/docs", &format!("href=\"{prefix}/docs"))
        .replace("href=\"/\"", &format!("href=\"{prefix}/\""))
        .replace("\\\"/_next/", &format!("\\\"{prefix}/_next/"))
        .replace("\\\"/docs", &format!("\\\"{prefix}/docs"))
        .replace("\\\"/\\\"", &format!("\\\"{prefix}/\\\""))
}

fn content_type_for_path(path: &FsPath) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "text/html; charset=utf-8",
    }
}

async fn internal_template_build(
    State(state): State<AppState>,
    Json(request): Json<InternalTemplateBuildRequest>,
) -> Result<Json<InternalTemplateBuildResponse>, (StatusCode, Json<ErrorResponse>)> {
    let project_id = request.project_id.clone();
    let workspace_root = project_workspace_root(&state.config, &project_id);
    let template = request.template.clone();
    let content_hierarchy = if request.content_hierarchy.is_empty() {
        vec![match template.as_str() {
            "fumadocs-docs" => "AnyDesign Runtime Docs".to_string(),
            _ => "AnyDesign Runtime Website".to_string(),
        }]
    } else {
        request.content_hierarchy
    };
    let brief = Brief {
        project_type: if template == "fumadocs-docs" {
            "docs".to_string()
        } else {
            "website".to_string()
        },
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

fn project_workspace_root(config: &RuntimeConfig, project_id: &str) -> PathBuf {
    let safe = project_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    config.workspace_root.join(safe)
}

async fn internal_promote_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PromotePreviewRequest>,
) -> Result<Json<PreviewCurrentResponse>, (StatusCode, Json<ErrorResponse>)> {
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

fn internal_admin_authorized(config: &RuntimeConfig, headers: &HeaderMap) -> bool {
    let Some(expected_token) = config.internal_admin_token.as_deref() else {
        return false;
    };
    let internal = headers
        .get("x-anydesign-internal")
        .and_then(|value| value.to_str().ok())
        == Some("true");
    let token = headers
        .get("x-runtime-admin-token")
        .and_then(|value| value.to_str().ok());
    internal && token == Some(expected_token)
}

fn last_event_sequence(last_event_id: Option<&str>, run_id: &str) -> usize {
    let Some(last_event_id) = last_event_id else {
        return 0;
    };
    let Some((id_run_id, sequence)) = last_event_id.rsplit_once('/') else {
        return 0;
    };
    if id_run_id != run_id {
        return 0;
    }
    sequence.parse::<usize>().unwrap_or(0)
}

fn spawn_session(state: AppState, run_id: String) {
    let session = QuerySession::with_tool_executor(
        state.store.clone(),
        state.model.clone(),
        control_plane_executor_for_config(&state.config),
    );
    tokio::spawn(async move {
        let _ = session.submit_run(&run_id).await;
    });
}

fn not_found(error: String) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { error }))
}

fn sandbox_binding_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("sandbox binding not found") {
        not_found(message)
    } else {
        conflict_error(anyhow::anyhow!(message))
    }
}

fn repair_run_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("parent run not found") || message.contains("review finding not found") {
        not_found(message)
    } else {
        conflict_error(anyhow::anyhow!(message))
    }
}

fn conflict_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}

fn run_update_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("run not found") {
        not_found(message)
    } else {
        conflict_error(anyhow::anyhow!(message))
    }
}

fn internal_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: error.to_string(),
        }),
    )
}
