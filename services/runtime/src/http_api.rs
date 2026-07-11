use crate::{
    artifact_publisher::{safe_segment, FileArtifactPublisher},
    channel_manager::ChannelManager,
    config::{RuntimeConfig, SandboxBackendMode},
    conversation::{RuntimeStore, SequencedAgentEvent},
    model_gateway::{model_client_from_config, ModelClient},
    preview::{promote_preview, PromotionGateReport},
    profiles::build::{run_template_build, TemplateBuildRequest},
    profiles::edit::{self, EditIntent},
    query_session::QuerySession,
    recovery::{recover_interrupted_runs, RecoveryOutcome},
    tools::{
        control_plane::{control_plane_executor_for_config, sandbox_backend_for_config},
        runtime::ToolContext,
        sandbox::{
            cleanup_staged_writes_for_run, cleanup_staged_writes_for_run_backend,
            LocalWorkspaceBackend, SandboxChannelWorkspaceBackend, WorkspaceBackend,
        },
    },
    types::{
        sha256_hex, AgentEvent, AgentPhase, AgentRun, AgentRunStatus, Brief, ContentSource,
        ConversationItem, DesignProfile, DesignProfileConversionReport, DesignProfileDraft,
        DesignProfileFidelityReport, DesignProfileUnmappedItem, DesignProfileValidationIssue,
        DesignSourceArtifact, PreviewLeaseStatus, DESIGN_PROFILE_SCHEMA_V2,
        MAX_DESIGN_SOURCE_BYTES,
    },
};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, Response,
    },
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::Utc;
use futures::stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeSet, VecDeque},
    convert::Infallible,
    fs,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};
use tokio::sync::broadcast;

const MAX_DESIGN_SOURCE_REQUEST_BYTES: usize = 384 * 1024;
const MAX_DESIGN_SOURCE_BASE64_BYTES: usize = (MAX_DESIGN_SOURCE_BYTES + 2) / 3 * 4;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResponse {
    pub service: &'static str,
    pub repository_commit: String,
    pub repository_dirty: bool,
    pub image_ref: Option<String>,
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
    ChannelManager::shared().reconcile(&state.store).await?;
    state.store.reconcile_artifact_promotions().await?;
    garbage_collect_artifacts(&state).await?;
    let outcomes = recover_interrupted_runs(&state.store).await?;
    for outcome in outcomes {
        if let RecoveryOutcome::Resumed { run_id, .. } = outcome {
            spawn_session(state.clone(), run_id);
        }
    }
    Ok(state)
}

async fn garbage_collect_artifacts(state: &AppState) -> anyhow::Result<()> {
    let publisher = FileArtifactPublisher::new(&state.config.runtime_storage_dir);
    for publish in state
        .store
        .garbage_collectable_artifact_publishes(Utc::now())
        .await?
    {
        let is_current = state
            .store
            .current_project_version(&publish.project_id)
            .await
            .is_some_and(|version| version.id == publish.version_id);
        if is_current {
            continue;
        }
        publisher.garbage_collect(&publish)?;
        state
            .store
            .transition_artifact_publish(
                &publish.id,
                crate::types::ArtifactPublishStatus::GarbageCollected,
                None,
                None,
                None,
                None,
            )
            .await?;
    }
    Ok(())
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
        .route("/", get(root))
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/runs", post(start_run))
        .route("/runs/{run_id}/continue", post(continue_run))
        .route("/runs/{run_id}/cancel", post(cancel_run))
        .route("/runs/{run_id}/events", get(stream_run_events))
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
        .route(
            "/design-profiles",
            post(create_design_profile).get(list_design_profiles),
        )
        .route("/design-profiles/import", post(import_design_profile))
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
        .route(
            "/projects/{project_id}/conversation",
            get(project_conversation),
        )
        .route(
            "/projects/{project_id}/runtime-state",
            get(project_runtime_state),
        )
        .route("/preview/{project_id}/current", get(preview_current))
        .route("/preview/{project_id}/{version_id}", get(preview_version))
        .route("/previews/{lease_id}", get(candidate_preview_root))
        .route("/previews/{lease_id}/", get(candidate_preview_root))
        .route(
            "/previews/{lease_id}/{*preview_path}",
            get(candidate_preview_file),
        )
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
        .route("/_next/{*artifact_path}", get(next_artifact_asset_file))
        .route("/internal/template-build", post(internal_template_build))
        .route("/internal/previews/promote", post(internal_promote_preview))
        .route(
            "/permissions/{permission_id}/decision",
            post(resolve_permission),
        )
        .with_state(state)
}

async fn candidate_preview_root(
    State(state): State<AppState>,
    Path(lease_id): Path<String>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    proxy_candidate_preview(state, lease_id, String::new()).await
}

async fn candidate_preview_file(
    State(state): State<AppState>,
    Path((lease_id, preview_path)): Path<(String, String)>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    proxy_candidate_preview(state, lease_id, preview_path).await
}

async fn proxy_candidate_preview(
    state: AppState,
    lease_id: String,
    preview_path: String,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    if preview_path
        .split('/')
        .any(|component| component == ".." || component.contains('\\'))
    {
        return Err(not_found("candidate preview path is invalid".to_string()));
    }
    let lease = state
        .store
        .get_preview_lease(&lease_id)
        .await
        .filter(|lease| lease.status == PreviewLeaseStatus::Active)
        .ok_or_else(|| not_found("candidate preview lease is unavailable".to_string()))?;
    let binding = state
        .store
        .get_sandbox_binding(&lease.sandbox_binding_id)
        .await
        .filter(|binding| {
            binding.sandbox_name == lease.sandbox_name
                && binding.pod_uid.as_deref() == Some(lease.pod_uid.as_str())
        })
        .ok_or_else(|| not_found("candidate preview sandbox identity changed".to_string()))?;

    let endpoint = ChannelManager::shared()
        .endpoint(&state.store, &binding, &lease.run_id, 4321, "http", "")
        .await
        .map_err(internal_error)?;
    let mut upstream =
        reqwest::Url::parse(&endpoint).map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    upstream.set_path(&format!("/candidates/{}/{}", lease.build_id, preview_path));
    let upstream_response = reqwest::Client::new()
        .get(upstream)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| not_found(format!("candidate preview upstream unavailable: {error}")))?;
    let status = upstream_response.status();
    if !status.is_success() {
        return Err(not_found(format!(
            "candidate preview file not found: {preview_path}"
        )));
    }
    let manifest_hash = upstream_response
        .headers()
        .get("x-anydesign-candidate-manifest-hash")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| not_found("candidate preview manifest evidence missing".to_string()))?;
    if manifest_hash != lease.candidate_manifest_hash {
        return Err(conflict_error(anyhow::anyhow!(
            "candidate preview manifest hash mismatch"
        )));
    }
    let content_type = upstream_response
        .headers()
        .get(header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/octet-stream"));
    let cache_control = upstream_response
        .headers()
        .get(header::CACHE_CONTROL)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("no-store"));
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
            let prefix = format!("/previews/{lease_id}");
            bytes = html
                .replace("href=\"/", &format!("href=\"{prefix}/"))
                .replace("src=\"/", &format!("src=\"{prefix}/"))
                .into_bytes();
        }
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .header("x-anydesign-preview-lease", lease_id)
        .body(Body::from(bytes))
        .map_err(|error| internal_error(anyhow::anyhow!(error)))
}

async fn root(State(state): State<AppState>) -> Html<String> {
    let base = format!("http://{}:{}", state.config.host, state.config.port);
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>AnyDesign Runtime</title>
  <style>
    :root {{ color-scheme: light dark; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    body {{ margin: 0; padding: 40px; background: #0f172a; color: #e5e7eb; }}
    main {{ max-width: 880px; margin: 0 auto; }}
    h1 {{ margin: 0 0 8px; font-size: 32px; }}
    p {{ color: #a5b4fc; line-height: 1.6; }}
    a {{ color: #67e8f9; }}
    .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; margin-top: 24px; }}
    .card {{ border: 1px solid #334155; border-radius: 8px; padding: 16px; background: #111827; }}
    code {{ background: #1f2937; border-radius: 4px; padding: 2px 5px; color: #f8fafc; }}
  </style>
</head>
<body>
  <main>
    <h1>AnyDesign Runtime</h1>
    <p>Status: <code>ready</code>. This root page is a local runtime index for browser checks.</p>
    <div class="grid">
      <div class="card"><strong>Health</strong><p><a href="{base}/health">{base}/health</a></p></div>
      <div class="card"><strong>Website artifact</strong><p><a href="{base}/artifacts/zeron-real-website-1783303319260/current">{base}/artifacts/zeron-real-website-1783303319260/current</a></p></div>
      <div class="card"><strong>Docs artifact</strong><p><a href="{base}/artifacts/zeron-real-docs-1783303417188/current/docs">{base}/artifacts/zeron-real-docs-1783303417188/current/docs</a></p></div>
      <div class="card"><strong>Run stream example</strong><p><code>{base}/runs/&lt;runId&gt;/events</code></p></div>
    </div>
  </main>
</body>
</html>"#
    ))
}

async fn health(State(_state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse { status: "ready" })
}

async fn version(State(state): State<AppState>) -> Json<VersionResponse> {
    Json(VersionResponse {
        service: "anydesign-runtime",
        repository_commit: state.config.repository_commit.clone(),
        repository_dirty: state.config.repository_dirty,
        image_ref: state.config.runtime_image_ref.clone(),
    })
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
    pub design_profile_id: Option<String>,
    pub design_fidelity_mode: Option<String>,
    pub workspace_id: Option<String>,
    pub organization_id: Option<String>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDesignSourceArtifactRequest {
    pub scope: Value,
    pub file_name: String,
    pub media_type: String,
    pub content_base64: String,
    pub client_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DesignSourceArtifactResponse {
    pub artifact: DesignSourceArtifact,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDesignProfileRequest {
    pub name: String,
    pub scope: Value,
    pub source_artifact_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDesignProfileResponse {
    pub design_profile_draft: DesignProfileDraft,
    pub conversion_report: DesignProfileConversionReport,
    pub requires_review: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivateDesignProfileRequest {
    pub expected_version: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationConflictResponse {
    pub error: String,
    pub current_version: u32,
    pub validation_issues: Vec<DesignProfileValidationIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDesignProfileRequest {
    pub project_id: Option<String>,
    pub name: String,
    pub profile: Option<Value>,
    #[serde(flatten)]
    pub legacy_profile: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDesignProfileRequest {
    pub expected_version: Option<u32>,
    pub name: String,
    pub profile: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileResponse {
    pub design_profile: DesignProfile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<DesignProfile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindProjectDesignProfileRequest {
    pub design_profile_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDesignProfileResponse {
    pub project_id: String,
    pub design_profile: Option<DesignProfile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<DesignProfile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDesignProfilesQuery {
    pub project_id: Option<String>,
    pub workspace_id: Option<String>,
    pub organization_id: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDesignProfilesResponse {
    pub design_profiles: Vec<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileDiffQuery {
    pub from_version: u32,
    pub to_version: u32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileFidelityQuery {
    pub surface: Option<String>,
    pub template: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileVersionsResponse {
    pub design_profile_id: String,
    pub versions: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileDiffResponse {
    pub design_profile_id: String,
    pub from_version: u32,
    pub to_version: u32,
    pub changes: Vec<DesignProfileDiffChange>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileDiffChange {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuntimeStateResponse {
    pub project_id: String,
    pub current_version_id: String,
    pub sandbox_binding_id: String,
    pub source_snapshot_uri: String,
    pub app_root: String,
    pub template_key: String,
    pub style_contract_path: Option<String>,
    pub style_contract: Option<Value>,
    pub latest_build: Option<Value>,
    pub dependency_state: Option<Value>,
    pub preview: Option<Value>,
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
        style_contract_version: if matches!(template, "astro-website" | "fumadocs-docs") {
            "runtime-style-contract@p3".to_string()
        } else {
            "runtime-style-contract@p2".to_string()
        },
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

async fn start_run(
    State(state): State<AppState>,
    Json(request): Json<StartRunRequest>,
) -> Result<Json<StartRunResponse>, (StatusCode, Json<ErrorResponse>)> {
    validate_start_run_request(&request)?;
    validate_sandbox_context(&state.store, &request).await?;
    validate_project_lifecycle_context(&state.store, &request).await?;
    let design_profile = resolve_design_profile_context(&state.store, &request).await?;
    let design_profile_target = design_profile_execution_target(&state.store, &request).await?;
    let design_profile_conflict =
        preflight_design_profile_conflicts(&state.store, &request, design_profile.as_ref()).await?;
    let content_sources = merge_content_sources(
        inherited_build_content_sources(&state.store, &request).await,
        request.input_context.content_sources.clone(),
    );
    let run = if let Some(parent_run_id) = request.input_context.parent_run_id.as_deref() {
        if request.phase == AgentPhase::Repair {
            state
                .store
                .create_repair_run_for_findings(
                    parent_run_id,
                    &request.input_context.finding_ids,
                    None,
                    request.agent_profile,
                    state.config.agent_model.clone(),
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
                    state.config.agent_model.clone(),
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
                state.config.agent_model.clone(),
                content_sources,
                request.input_context.brief_id,
                request.input_context.base_version_id,
            )
            .await
    };
    let run = if let Some(profile) = design_profile.as_ref() {
        let effective_target = if design_profile_conflict.is_none() {
            design_profile_target.as_ref()
        } else {
            None
        };
        if let Some((surface, template)) = effective_target {
            state
                .store
                .attach_run_effective_design_profile(
                    &run.id,
                    profile,
                    Some(surface),
                    Some(template),
                )
                .await
                .map_err(design_profile_error)?
        } else {
            state
                .store
                .attach_run_design_profile(&run.id, profile)
                .await
                .map_err(design_profile_error)?
        }
    } else {
        run
    };
    let run = if let Some(profile) = design_profile.as_ref() {
        let configured = state
            .store
            .configure_run_design_fidelity(
                &run.id,
                profile,
                request.input_context.design_fidelity_mode.as_deref(),
            )
            .await
            .map_err(design_profile_error)?;
        if let Some(mode) = request.input_context.design_fidelity_mode.as_deref() {
            state
                .store
                .append_audit_record(
                    &run.project_id,
                    &run.id,
                    "design_profile.fidelity_mode",
                    format!("mode={mode}"),
                    "allow",
                    "explicit StartRun input",
                )
                .await;
        }
        configured
    } else {
        run
    };
    if let Some(profile) = design_profile.as_ref() {
        if let Some((blocked_state, message)) =
            design_profile_prebuild_failure(&state.store, &run, profile).await
        {
            state
                .store
                .append_conversation_item(
                    &run.project_id,
                    Some(&run.id),
                    "approval_request",
                    Some("assistant"),
                    &message,
                    Some(json!({
                        "state": blocked_state,
                        "designProfileId": profile.id,
                    })),
                )
                .await;
            state
                .store
                .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
                .await
                .map_err(conflict_error)?;
            state
                .store
                .append_event(AgentEvent::StateChanged {
                    run_id: run.id.clone(),
                    state: blocked_state,
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            return Ok(Json(StartRunResponse {
                run_id: run.id,
                status: "needs_user_input",
            }));
        }
    }
    if !run.design_profile_unsupported_extended_tokens.is_empty() {
        state
            .store
            .append_audit_record(
                &run.project_id,
                &run.id,
                "design_profile.capability_gap",
                format!(
                    "unsupportedExtendedTokens={}",
                    run.design_profile_unsupported_extended_tokens.join(",")
                ),
                if run.design_profile_blocking_capability_rule_ids.is_empty() {
                    "allow"
                } else {
                    "ask"
                },
                "effective profile versus template style contract",
            )
            .await;
    }
    if !run.design_profile_blocking_capability_rule_ids.is_empty() {
        state
            .store
            .append_conversation_item(
                &run.project_id,
                Some(&run.id),
                "approval_request",
                Some("assistant"),
                "Required DesignProfile rules depend on template capabilities that are not supported.",
                Some(json!({
                    "state": "needs_user_input:design_profile_capability_gap",
                    "ruleIds": run.design_profile_blocking_capability_rule_ids,
                    "unsupportedExtendedTokens": run.design_profile_unsupported_extended_tokens,
                })),
            )
            .await;
        state
            .store
            .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
            .await
            .map_err(conflict_error)?;
        state
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: run.id.clone(),
                state: "needs_user_input:design_profile_capability_gap".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal_error)?;
        return Ok(Json(StartRunResponse {
            run_id: run.id,
            status: "needs_user_input",
        }));
    }
    if let Some(conflict_reason) = design_profile_conflict {
        state
            .store
            .append_conversation_item(
                &run.project_id,
                Some(&run.id),
                "approval_request",
                Some("assistant"),
                format!("DesignProfile conflict requires confirmation: {conflict_reason}"),
                Some(json!({
                    "reason": conflict_reason,
                    "designProfileId": run.design_profile_id.as_deref(),
                    "state": "needs_user_input:design_profile_conflict",
                })),
            )
            .await;
        state
            .store
            .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
            .await
            .map_err(conflict_error)?;
        state
            .store
            .append_event(AgentEvent::StateChanged {
                run_id: run.id.clone(),
                state: "needs_user_input:design_profile_conflict".to_string(),
                timestamp: Utc::now(),
            })
            .await
            .map_err(internal_error)?;
        return Ok(Json(StartRunResponse {
            run_id: run.id,
            status: "needs_user_input",
        }));
    }
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
    if run.phase == AgentPhase::Edit {
        if let Err(error) = restore_edit_workspace_from_base_version(&state, &run).await {
            let _ = state
                .store
                .update_run_status(&run.id, AgentRunStatus::Cancelled)
                .await;
            return Err(conflict_error(error));
        }
    }
    let run_id = run.id.clone();
    if run.phase != AgentPhase::Edit {
        spawn_session(state, run_id.clone());
    }

    Ok(Json(StartRunResponse {
        run_id: run.id,
        status: "queued",
    }))
}

async fn restore_edit_workspace_from_base_version(
    state: &AppState,
    run: &AgentRun,
) -> anyhow::Result<()> {
    let base_version_id = run
        .base_version_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("edit run missing baseVersionId"))?;
    let version = state
        .store
        .get_project_version(base_version_id)
        .await
        .ok_or_else(|| anyhow::anyhow!("base version not found: {base_version_id}"))?;
    let source_snapshot_uri = version.source_snapshot_uri.as_deref().ok_or_else(|| {
        anyhow::anyhow!("base version {base_version_id} is missing sourceSnapshotUri")
    })?;
    restore_project_source_snapshot(&state.store, &state.config, run, source_snapshot_uri).await
}

async fn restore_project_source_snapshot(
    store: &RuntimeStore,
    config: &RuntimeConfig,
    run: &AgentRun,
    source_snapshot_uri: &str,
) -> anyhow::Result<()> {
    let workspace_root = effective_workspace_root(config, &run.project_id);
    let project_root = workspace_root.join("project");
    let mut ctx = ToolContext::new(store.clone(), run.clone(), workspace_root.clone());
    ctx.remote_workspace = config.sandbox_backend_mode == SandboxBackendMode::Kubernetes;
    ctx.runtime_storage_dir = config.runtime_storage_dir.clone();
    let backend: Box<dyn WorkspaceBackend> = match config.sandbox_backend_mode {
        SandboxBackendMode::Kubernetes => Box::new(
            SandboxChannelWorkspaceBackend::from_runtime_config(config)
                .map_err(|error| anyhow::anyhow!(error))?,
        ),
        SandboxBackendMode::PhaseAContract => Box::new(LocalWorkspaceBackend),
    };
    if let Err(error) = backend.remove_dir_all(&ctx, &project_root).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(anyhow::anyhow!(error));
        }
    }
    if let Some(runtime_path) = source_snapshot_uri.strip_prefix("runtime://source-snapshots/") {
        let segments = runtime_path.split('/').collect::<Vec<_>>();
        if segments.len() != 2 || segments.iter().any(|segment| segment.is_empty()) {
            return Err(anyhow::anyhow!("invalid Runtime source snapshot URI"));
        }
        let snapshot_project_id = segments[0];
        let snapshot_id = segments[1];
        if snapshot_project_id != safe_segment(&run.project_id) {
            return Err(anyhow::anyhow!("source snapshot project mismatch"));
        }
        for file in FileArtifactPublisher::read_source_snapshot(
            &config.runtime_storage_dir,
            &run.project_id,
            snapshot_id,
        )? {
            let target = project_root.join(&file.path);
            backend
                .write_bytes(&ctx, &target, &file.bytes)
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
            let restored = backend
                .read_bytes(&ctx, &target)
                .await
                .map_err(|error| anyhow::anyhow!(error))?;
            if restored != file.bytes {
                return Err(anyhow::anyhow!(
                    "source snapshot integrity check failed after restore: {}",
                    file.path.display()
                ));
            }
        }
    } else {
        let snapshot_root =
            workspace_file_uri_to_workspace_path(&workspace_root, source_snapshot_uri)?;
        backend
            .copy_dir_all(&ctx, &snapshot_root, &project_root, &[])
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
    }
    let dependency_state = serde_json::to_string_pretty(&json!({
        "needsRestore": true,
        "reason": "source_snapshot_restored_without_node_modules",
        "sourceSnapshotUri": source_snapshot_uri,
        "markedAt": Utc::now().to_rfc3339(),
    }))?;
    backend
        .write_string(
            &ctx,
            &workspace_root.join("state/dependency-state.json"),
            &dependency_state,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

fn workspace_file_uri_to_workspace_path(
    workspace_root: &FsPath,
    uri: &str,
) -> anyhow::Result<PathBuf> {
    let path = uri
        .strip_prefix("file:///workspace/")
        .ok_or_else(|| anyhow::anyhow!("unsupported source snapshot URI: {uri}"))?;
    let relative = FsPath::new(path);
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow::anyhow!("source snapshot escapes workspace: {uri}"));
    }
    Ok(workspace_root.join(relative))
}

async fn inherited_build_content_sources(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Vec<ContentSource> {
    if request.phase != AgentPhase::Build {
        return Vec::new();
    }
    let Some(brief_id) = request.input_context.brief_id.as_deref() else {
        return Vec::new();
    };
    store
        .content_sources_for_brief(brief_id)
        .await
        .into_iter()
        .filter(|source| source.readable)
        .collect()
}

fn merge_content_sources(
    inherited: Vec<ContentSource>,
    explicit: Vec<ContentSource>,
) -> Vec<ContentSource> {
    let mut merged: Vec<ContentSource> = Vec::new();
    for source in inherited.into_iter().chain(explicit) {
        if let Some(index) = merged
            .iter()
            .position(|existing| existing.id == source.id || existing.kind == source.kind)
        {
            merged[index] = source;
        } else {
            merged.push(source);
        }
    }
    merged
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

async fn validate_project_lifecycle_context(
    store: &RuntimeStore,
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if is_mutable_phase(request.phase) && request.input_context.parent_run_id.is_none() {
        if let Some(active) = store
            .active_mutable_run_for_project(&request.project_id)
            .await
        {
            return Err(conflict_error(anyhow::anyhow!(
                "project {} already has active mutable run {}",
                request.project_id,
                active.id
            )));
        }
    }

    if request.phase == AgentPhase::Edit {
        let base_version_id = request
            .input_context
            .base_version_id
            .as_deref()
            .ok_or_else(|| {
                conflict_error(anyhow::anyhow!(
                    "Edit run requires baseVersionId for lifecycle snapshot verification"
                ))
            })?;
        let current = store
            .current_project_version(&request.project_id)
            .await
            .ok_or_else(|| {
                conflict_error(anyhow::anyhow!(
                    "Edit run requires a promoted current version for project {}",
                    request.project_id
                ))
            })?;
        if current.id != base_version_id {
            return Err(conflict_error(anyhow::anyhow!(
                "Edit run baseVersionId {base_version_id} is stale; currentVersionId is {}",
                current.id
            )));
        }
        if current.source_snapshot_uri.is_none() {
            return Err(conflict_error(anyhow::anyhow!(
                "Edit run requires sourceSnapshotUri for baseVersionId {base_version_id}"
            )));
        }
    }

    Ok(())
}

fn is_mutable_phase(phase: AgentPhase) -> bool {
    matches!(
        phase,
        AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair | AgentPhase::Export
    )
}

fn is_brief_confirmation_message(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    if matches!(
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
    ) {
        return true;
    }

    let confirmation_prefixes = ["确认", "同意", "可以", "批准", "开始"];
    confirmation_prefixes
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
        || normalized.contains("开始生成")
        || normalized.contains("开始构建")
        || normalized.contains("开始创建")
        || (normalized.contains("confirm") && normalized.contains("brief"))
        || (normalized.contains("approve") && normalized.contains("brief"))
        || (normalized.contains("start") && normalized.contains("build"))
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
    validate_required_string("runId", &run_id)?;
    validate_required_string("userMessage", &request.user_message)?;
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
            .await
            .map_err(internal_error)?;
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
            .await
            .map_err(internal_error)?;
        return Ok(Json(RunStatusResponse {
            run_id,
            status: "running".to_string(),
        }));
    }
    if run.phase == AgentPhase::Edit {
        let design_profile_override_accepted = run.status == AgentRunStatus::NeedsUserInput
            && run.design_profile_id.is_some()
            && is_design_profile_override_message(&request.user_message)
            && has_design_profile_conflict_state(&state.store, &run_id).await;
        if design_profile_override_accepted {
            state
                .store
                .append_conversation_item(
                    &run.project_id,
                    Some(&run_id),
                    "design_profile_override",
                    Some("user"),
                    "DesignProfile override accepted for this run.",
                    Some(json!({
                        "designProfileId": run.design_profile_id.as_deref(),
                        "designProfileVersion": run.design_profile_version,
                        "designProfileHash": run.design_profile_hash.as_deref(),
                        "decision": "override",
                        "state": "accepted",
                        "userMessage": request.user_message.clone(),
                    })),
                )
                .await;
        }
        if let Some(conflict_reason) =
            classify_design_profile_edit_conflict(&state.store, &run, &request.user_message).await?
        {
            state
                .store
                .append_conversation_item(
                    &run.project_id,
                    Some(&run_id),
                    "approval_request",
                    Some("assistant"),
                    format!("DesignProfile conflict requires confirmation: {conflict_reason}"),
                    Some(json!({
                        "reason": conflict_reason,
                        "designProfileId": run.design_profile_id.as_deref(),
                        "state": "needs_user_input:design_profile_conflict",
                    })),
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
                    state: "needs_user_input:design_profile_conflict".to_string(),
                    timestamp: Utc::now(),
                })
                .await
                .map_err(internal_error)?;
            return Ok(Json(RunStatusResponse {
                run_id,
                status: "needs_user_input".to_string(),
            }));
        }
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
                    .await
                    .map_err(internal_error)?;
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
        .await
        .map_err(internal_error)?;
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
    validate_required_string("runId", &run_id)?;
    state
        .store
        .update_run_status(&run_id, AgentRunStatus::Cancelled)
        .await
        .map_err(run_update_error)?;
    if let Some(run) = state.store.get_run(&run_id).await {
        let workspace_root = effective_workspace_root(&state.config, &run.project_id);
        if state.config.sandbox_backend_mode == SandboxBackendMode::Kubernetes
            && run.sandbox_id.is_some()
        {
            let mut ctx = ToolContext::new(state.store.clone(), run, workspace_root);
            ctx.remote_workspace = true;
            ctx.runtime_storage_dir = state.config.runtime_storage_dir.clone();
            let backend = SandboxChannelWorkspaceBackend::from_runtime_config(&state.config)
                .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
            cleanup_staged_writes_for_run_backend(&backend, &ctx, &run_id)
                .await
                .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
        } else {
            cleanup_staged_writes_for_run(&workspace_root, &run_id);
        }
    }
    state
        .store
        .append_event(AgentEvent::RunCompleted {
            run_id: run_id.clone(),
            status: "cancelled".to_string(),
            summary: "Run cancelled.".to_string(),
            timestamp: Utc::now(),
        })
        .await
        .map_err(internal_error)?;
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
    validate_required_string("permissionId", &permission_id)?;
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
                .await
                .map_err(internal_error)?;
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
                .await
                .map_err(internal_error)?;
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
                .await
                .map_err(internal_error)?;
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
    let start_after = last_event_sequence(
        headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok()),
        &run_id,
    );
    let live_events = state.store.subscribe_events(&run_id).await;
    let events = state.store.events(&run_id).await;
    let history_len = events.len();
    let replay_events = events
        .into_iter()
        .enumerate()
        .filter_map(move |(index, event)| {
            let sequence = index + 1;
            (sequence > start_after).then_some(SequencedAgentEvent { sequence, event })
        })
        .collect::<VecDeque<_>>();
    let stream = stream::unfold(
        RunEventsSseState {
            run_id,
            replay_events,
            live_events,
            min_live_sequence: history_len.max(start_after),
            finished: false,
        },
        next_run_event_sse,
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default().text("heartbeat")))
}

struct RunEventsSseState {
    run_id: String,
    replay_events: VecDeque<SequencedAgentEvent>,
    live_events: Option<broadcast::Receiver<SequencedAgentEvent>>,
    min_live_sequence: usize,
    finished: bool,
}

async fn next_run_event_sse(
    mut state: RunEventsSseState,
) -> Option<(Result<Event, Infallible>, RunEventsSseState)> {
    loop {
        if state.finished {
            return None;
        }
        if let Some(sequenced) = state.replay_events.pop_front() {
            let is_terminal = sequenced.event.is_run_completed();
            let event = encode_run_event_sse(&state.run_id, sequenced.sequence, &sequenced.event);
            if is_terminal {
                state.finished = true;
                state.live_events = None;
            }
            return Some((Ok(event), state));
        }
        let receiver = state.live_events.as_mut()?;
        let sequenced = match receiver.recv().await {
            Ok(sequenced) => sequenced,
            Err(broadcast::error::RecvError::Lagged(_))
            | Err(broadcast::error::RecvError::Closed) => return None,
        };
        if sequenced.sequence <= state.min_live_sequence {
            continue;
        }
        state.min_live_sequence = sequenced.sequence;
        let is_terminal = sequenced.event.is_run_completed();
        let event = encode_run_event_sse(&state.run_id, sequenced.sequence, &sequenced.event);
        if is_terminal {
            state.finished = true;
            state.live_events = None;
        }
        return Some((Ok(event), state));
    }
}

fn encode_run_event_sse(run_id: &str, sequence: usize, event: &AgentEvent) -> Event {
    Event::default()
        .id(format!("{run_id}/{sequence}"))
        .data(serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()))
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

async fn project_runtime_state(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectRuntimeStateResponse>, (StatusCode, Json<ErrorResponse>)> {
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
    artifact_response(&state, &project_id, "").await
}

async fn artifact_current_file(
    State(state): State<AppState>,
    Path((project_id, artifact_path)): Path<(String, String)>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    artifact_response(&state, &project_id, &artifact_path).await
}

async fn next_artifact_asset_file(
    State(state): State<AppState>,
    Path(artifact_path): Path<String>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    let project_id = artifact_project_id_from_referer(&headers)
        .ok_or_else(|| not_found("Next artifact asset requires an artifact referer".to_string()))?;
    artifact_response(&state, &project_id, &format!("_next/{artifact_path}")).await
}

// remote-fs-boundary: allow-begin runtime-storage-artifact-serving
async fn artifact_response(
    state: &AppState,
    project_id: &str,
    artifact_path: &str,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<ErrorResponse>)> {
    let current = state
        .store
        .current_project_version(project_id)
        .await
        .ok_or_else(|| {
            not_found(format!(
                "current artifact not found for project: {project_id}"
            ))
        })?;
    let output_root = FileArtifactPublisher::version_root(
        &state.config.runtime_storage_dir,
        project_id,
        &current.id,
    );
    if !output_root.is_dir() {
        return Err(not_found(format!(
            "immutable artifact output not found for version: {}",
            current.id
        )));
    }
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
// remote-fs-boundary: allow-end runtime-storage-artifact-serving

fn rewrite_artifact_html(html: &str, project_id: &str) -> String {
    let prefix = format!("/artifacts/{project_id}/current");
    html.replace("href=\"/_next/", &format!("href=\"{prefix}/_next/"))
        .replace("src=\"/_next/", &format!("src=\"{prefix}/_next/"))
        .replace("href=\"/_astro/", &format!("href=\"{prefix}/_astro/"))
        .replace("src=\"/_astro/", &format!("src=\"{prefix}/_astro/"))
        .replace(
            "href=\"/favicon.svg\"",
            &format!("href=\"{prefix}/favicon.svg\""),
        )
        .replace("href=\"/docs", &format!("href=\"{prefix}/docs"))
        .replace("href=\"/\"", &format!("href=\"{prefix}/\""))
        .replace("\\\"/_next/", &format!("\\\"{prefix}/_next/"))
        .replace("\\\"/_astro/", &format!("\\\"{prefix}/_astro/"))
        .replace("\\\"/docs", &format!("\\\"{prefix}/docs"))
        .replace("\\\"/\\\"", &format!("\\\"{prefix}/\\\""))
}

fn artifact_project_id_from_referer(headers: &HeaderMap) -> Option<String> {
    let referer = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())?;
    let marker = "/artifacts/";
    let start = referer.find(marker)? + marker.len();
    let rest = &referer[start..];
    let end = rest.find("/current")?;
    let project_id = &rest[..end];
    (!project_id.trim().is_empty()).then(|| project_id.to_string())
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

fn effective_workspace_root(config: &RuntimeConfig, project_id: &str) -> PathBuf {
    match config.sandbox_backend_mode {
        SandboxBackendMode::PhaseAContract => project_workspace_root(config, project_id),
        SandboxBackendMode::Kubernetes => config.workspace_root.clone(),
    }
}

fn project_state_roots(config: &RuntimeConfig, project_id: &str) -> Vec<PathBuf> {
    vec![
        project_workspace_root(config, project_id),
        config.workspace_root.clone(),
    ]
}

fn read_first_json_file(roots: &[PathBuf], relative: &str) -> Option<Value> {
    roots
        .iter()
        .find_map(|root| read_json_file(&root.join(relative)))
}

// remote-fs-boundary: allow-begin phase-a-runtime-state-fallback
fn read_json_file(path: &FsPath) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}
// remote-fs-boundary: allow-end phase-a-runtime-state-fallback

async fn read_runtime_state_json(
    state: &AppState,
    project_id: &str,
    run: Option<&AgentRun>,
    sandbox_binding_id: Option<&str>,
    relative: &str,
) -> Option<Value> {
    if state.config.sandbox_backend_mode == SandboxBackendMode::Kubernetes {
        if let (Some(run), Some(sandbox_binding_id)) = (run, sandbox_binding_id) {
            let mut run = run.clone();
            run.sandbox_id = Some(sandbox_binding_id.to_string());
            let ctx = ToolContext::new(
                state.store.clone(),
                run,
                state.config.workspace_root.clone(),
            );
            let backend = SandboxChannelWorkspaceBackend::new();
            if let Ok(text) = backend
                .read_to_string(&ctx, &state.config.workspace_root.join(relative))
                .await
            {
                if let Ok(value) = serde_json::from_str(&text) {
                    return Some(value);
                }
            }
        }
    }

    let state_roots = project_state_roots(&state.config, project_id);
    read_first_json_file(&state_roots, relative)
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

fn scope_with_project_id(scope: Value, project_id: Option<&str>) -> Value {
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

fn design_profile_payload_from_request(
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

fn payload_string(
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

fn payload_required_value(
    payload: &Map<String, Value>,
    field: &str,
) -> Result<Value, (StatusCode, Json<ErrorResponse>)> {
    payload
        .get(field)
        .cloned()
        .ok_or_else(|| bad_request(format!("profile.{field} is required")))
}

fn payload_value(payload: &Map<String, Value>, field: &str) -> Option<Value> {
    payload.get(field).cloned()
}

fn normalize_design_profile_component_roles(
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

fn signature_rule_applies_to_surface(rule: &Value, surface: &str) -> bool {
    match rule.get("appliesTo") {
        Some(Value::String(value)) => value == "all",
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(surface)),
        _ => false,
    }
}

fn unsupported_extended_tokens_for_template(mapping: &Value, template: &str) -> Vec<String> {
    let supported: &[&str] = match template {
        "astro-website" => &[
            "font.display",
            "font.mono",
            "type.display.size",
            "type.display.lineHeight",
            "type.display.letterSpacing",
            "type.body.letterSpacing",
            "spacing.pageGutter",
            "spacing.section",
            "spacing.cardPadding",
            "radius.input",
            "radius.badge",
            "radius.largeCard",
            "gradient.display",
            "gradient.ambient",
            "shadow.cardStrong",
        ],
        "fumadocs-docs" => &[
            "font.display",
            "font.mono",
            "type.display.letterSpacing",
            "type.body.letterSpacing",
            "spacing.pageGutter",
            "spacing.section",
            "radius.input",
            "radius.badge",
            "gradient.display",
        ],
        _ => &[],
    };
    let mut unsupported = mapping
        .as_object()
        .map(|tokens| {
            tokens
                .keys()
                .filter(|token| !supported.contains(&token.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    unsupported.sort();
    unsupported
}

struct ParsedDesignProfileSource {
    headings: Vec<String>,
    tokens: Map<String, Value>,
    extracted_token_count: usize,
    extracted_component_count: usize,
    unmapped_items: Vec<DesignProfileUnmappedItem>,
    warnings: Vec<String>,
}

fn parse_design_profile_source(source: &str) -> ParsedDesignProfileSource {
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

fn parse_css_custom_property(line: &str) -> Option<(String, String)> {
    let line = line.trim().trim_end_matches(';');
    let (name, value) = line.split_once(':')?;
    let name = name.trim();
    let value = value.trim();
    if !name.starts_with("--") || name.len() < 3 || value.is_empty() {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

fn parse_markdown_token_row(line: &str) -> Option<(String, String)> {
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

fn unmapped_source_item(
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

fn design_profile_candidate_issues(
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

async fn resolve_design_profile_context(
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

async fn design_profile_execution_target(
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
    let surface = match brief.recommended_template.as_str() {
        "astro-website" | "nextjs-website" => "website",
        "fumadocs-docs" | "docusaurus-docs" => "docs",
        template => {
            return Err(bad_request(format!(
                "unsupported brief template for DesignProfile: {template}"
            )))
        }
    };
    Ok(Some((surface.to_string(), brief.recommended_template)))
}

async fn design_profile_prebuild_failure(
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

async fn preflight_design_profile_conflicts(
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

async fn classify_design_profile_edit_conflict(
    store: &RuntimeStore,
    run: &AgentRun,
    user_message: &str,
) -> Result<Option<String>, (StatusCode, Json<ErrorResponse>)> {
    if run.status == AgentRunStatus::NeedsUserInput
        && is_design_profile_override_message(user_message)
    {
        return Ok(None);
    }
    let Some(design_profile_id) = run.design_profile_id.as_deref() else {
        return Ok(None);
    };
    let profile = store
        .get_design_profile(design_profile_id)
        .await
        .ok_or_else(|| not_found(format!("design profile not found: {design_profile_id}")))?;
    let normalized = user_message.to_lowercase();
    if let Some(keyword) = profile
        .visual
        .get("avoidKeywords")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|keyword| normalized.contains(&keyword.to_lowercase()))
    {
        return Ok(Some(format!(
            "User edit requests visual keyword \"{keyword}\" forbidden by DesignProfile {}",
            profile.id
        )));
    }
    if let Some(claim) = profile
        .brand
        .get("messaging")
        .and_then(|messaging| messaging.get("forbiddenClaims"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|claim| normalized.contains(&claim.to_lowercase()))
    {
        return Ok(Some(format!(
            "User edit requests forbidden claim \"{claim}\" from DesignProfile {}",
            profile.id
        )));
    }
    Ok(None)
}

async fn has_design_profile_conflict_state(store: &RuntimeStore, run_id: &str) -> bool {
    store.events(run_id).await.iter().any(|event| {
        matches!(
            event,
            AgentEvent::StateChanged { state, .. }
                if state == "needs_user_input:design_profile_conflict"
        )
    })
}

fn is_design_profile_override_message(message: &str) -> bool {
    let normalized = message.trim().to_lowercase();
    normalized.contains("override")
        || normalized.contains("temporary")
        || normalized.contains("continue anyway")
        || normalized.contains("临时覆盖")
        || normalized.contains("继续执行")
        || normalized.contains("仍然执行")
        || normalized.contains("忽略 profile")
        || normalized.contains("忽略profile")
}

fn validate_start_run_request(
    request: &StartRunRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("projectId", &request.project_id)?;
    validate_required_string("agentProfile", &request.agent_profile)?;
    for source in &request.input_context.content_sources {
        validate_required_string("contentSources[].id", &source.id)?;
        validate_required_string("contentSources[].kind", &source.kind)?;
    }
    validate_optional_string("briefId", request.input_context.brief_id.as_deref())?;
    validate_optional_string(
        "baseVersionId",
        request.input_context.base_version_id.as_deref(),
    )?;
    validate_optional_string(
        "sandboxBindingId",
        request.input_context.sandbox_binding_id.as_deref(),
    )?;
    validate_optional_string(
        "parentRunId",
        request.input_context.parent_run_id.as_deref(),
    )?;
    validate_optional_string(
        "designProfileId",
        request.input_context.design_profile_id.as_deref(),
    )?;
    if let Some(mode) = request.input_context.design_fidelity_mode.as_deref() {
        if !matches!(mode, "profile_only" | "source_fallback") {
            return Err(bad_request(
                "designFidelityMode must be profile_only or source_fallback".to_string(),
            ));
        }
    }
    validate_optional_string("workspaceId", request.input_context.workspace_id.as_deref())?;
    validate_optional_string(
        "organizationId",
        request.input_context.organization_id.as_deref(),
    )?;
    validate_string_list("findingIds", &request.input_context.finding_ids)?;
    Ok(())
}

fn validate_create_design_profile_request(
    request: &CreateDesignProfileRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("name", &request.name)?;
    validate_optional_string("projectId", request.project_id.as_deref())?;
    if request.profile.is_none() && request.legacy_profile.is_empty() {
        return Err(bad_request("profile is required".to_string()));
    }
    Ok(())
}

fn validate_internal_template_build_request(
    request: &InternalTemplateBuildRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("projectId", &request.project_id)?;
    validate_required_string("template", &request.template)?;
    validate_required_string("audience", &request.audience)?;
    validate_required_string("visualDirection", &request.visual_direction)?;
    validate_string_list("contentHierarchy", &request.content_hierarchy)?;
    validate_string_list("assumptions", &request.assumptions)?;
    validate_string_list("missingInformation", &request.missing_information)?;
    validate_optional_string("model", request.model.as_deref())?;
    validate_optional_string("publicBaseUrl", request.public_base_url.as_deref())?;
    Ok(())
}

fn validate_promote_preview_request(
    request: &PromotePreviewRequest,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    validate_required_string("projectId", &request.project_id)?;
    validate_required_string("runId", &request.run_id)?;
    validate_required_string("candidateVersionId", &request.candidate_version_id)?;
    Ok(())
}

fn validate_optional_string(
    field: &str,
    value: Option<&str>,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if let Some(value) = value {
        validate_required_string(field, value)?;
    }
    Ok(())
}

fn validate_string_list(
    field: &str,
    values: &[String],
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    for value in values {
        if value.trim().is_empty() {
            return Err(bad_request(format!(
                "{field} must not contain empty strings"
            )));
        }
    }
    Ok(())
}

fn validate_required_string(
    field: &str,
    value: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if value.trim().is_empty() {
        return Err(bad_request(format!("{field} must not be empty")));
    }
    Ok(())
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
    tokio::spawn(async move {
        let tool_executor = if let Some(run) = state.store.get_run(&run_id).await {
            let workspace_root = effective_workspace_root(&state.config, &run.project_id);
            if state.config.sandbox_backend_mode == SandboxBackendMode::PhaseAContract {
                // remote-fs-boundary: allow-begin phase-a-workspace-bootstrap
                let _ = fs::create_dir_all(&workspace_root);
                // remote-fs-boundary: allow-end phase-a-workspace-bootstrap
            }
            control_plane_executor_for_config(&state.config).with_workspace_root(workspace_root)
        } else {
            control_plane_executor_for_config(&state.config)
        };
        let session = QuerySession::with_tool_executor(
            state.store.clone(),
            state.model.clone(),
            tool_executor,
        );
        let _ = session.submit_run(&run_id).await;
    });
}

fn not_found(error: String) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { error }))
}

fn bad_request(error: String) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { error }))
}

fn error_response_as_value(error: (StatusCode, Json<ErrorResponse>)) -> (StatusCode, Json<Value>) {
    (error.0, Json(json!({ "error": error.1.error })))
}

fn sandbox_binding_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("sandbox binding not found") {
        not_found(message)
    } else {
        conflict_error(anyhow::anyhow!(message))
    }
}

fn design_profile_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("design profile not found") {
        not_found(message)
    } else if message.contains("invalid design profile") {
        bad_request(message)
    } else {
        conflict_error(anyhow::anyhow!(message))
    }
}

fn design_source_error(error: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    let message = error.to_string();
    if message.contains("design source artifact not found") {
        not_found(message)
    } else if message.contains("invalid design source artifact") {
        bad_request(message)
    } else {
        internal_error(anyhow::anyhow!(message))
    }
}

fn require_design_source_authorization(
    config: &RuntimeConfig,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if internal_admin_authorized(config, headers) {
        return Ok(());
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: "design source artifacts require service authorization".to_string(),
        }),
    ))
}

async fn validate_design_profile_source_reference(
    store: &RuntimeStore,
    profile: &DesignProfile,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let Some(artifact_id) = profile
        .source
        .get("primarySourceArtifactId")
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    validate_required_string("profile.source.primarySourceArtifactId", artifact_id)?;
    let artifact = store
        .get_design_source_artifact(artifact_id)
        .await
        .ok_or_else(|| not_found(format!("design source artifact not found: {artifact_id}")))?;
    if artifact.scope != profile.scope {
        return Err(bad_request(
            "profile source artifact scope must exactly match profile scope".to_string(),
        ));
    }
    let source_hash = profile
        .source
        .get("sourceHash")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            bad_request(
                "profile.source.sourceHash is required with primarySourceArtifactId".to_string(),
            )
        })?;
    if !artifact.sha256.eq_ignore_ascii_case(source_hash) {
        return Err(bad_request(
            "profile.source.sourceHash does not match the referenced artifact".to_string(),
        ));
    }
    store
        .read_design_source_artifact_content(artifact_id)
        .await
        .map_err(design_source_error)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_project_id_from_referer_extracts_current_artifact_project() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("http://127.0.0.1:8080/artifacts/project-docs-1/current/docs"),
        );

        assert_eq!(
            artifact_project_id_from_referer(&headers).as_deref(),
            Some("project-docs-1")
        );
    }

    #[test]
    fn artifact_project_id_from_referer_rejects_non_artifact_referer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("http://127.0.0.1:8080/docs"),
        );

        assert_eq!(artifact_project_id_from_referer(&headers), None);
    }
}
