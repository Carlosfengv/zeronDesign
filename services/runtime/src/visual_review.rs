use crate::{
    config::{ModelProvider, RuntimeConfig},
    conversation::RuntimeStore,
    model_gateway::{
        ModelClient, ModelGatewayScope, ModelRequest, ModelResponse, ModelToolDefinition, ToolCall,
    },
    tools::registry::ToolLoadingPolicy,
    types::{sha256_hex, AgentPhase, AgentRunStatus},
    visual_artifact_store::VisualArtifactStore,
    visual_contracts::{
        ElementBoundingBox, RunVisualBinding, RunVisualBindingRole, RunVisualTarget, VisualFinding,
        VisualFindingModelResourceSnapshot, VisualFindingStatus, VisualReviewMode,
        VisualReviewState, VisualReviewStatus, VisualViewport, VISUAL_FINDING_SCHEMA,
        VISUAL_REVIEW_STATE_SCHEMA,
    },
};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::Mutex;

pub const VISUAL_REVIEW_PROMPT_POLICY_VERSION: &str = "visual-review-prompt@1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualReviewBindingInput {
    pub artifact_id: String,
    pub role: RunVisualBindingRole,
    pub route: String,
    pub viewport: VisualViewport,
    pub order: u32,
}

#[derive(Debug, Clone)]
pub struct ScheduleVisualReviewRequest {
    pub project_id: String,
    pub mode: VisualReviewMode,
    pub target: RunVisualTarget,
    pub model: String,
    pub bindings: Vec<VisualReviewBindingInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualReviewResult {
    pub state: VisualReviewState,
    pub findings: Vec<VisualFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedVisualReview {
    project_id: String,
    state: VisualReviewState,
    findings: Vec<VisualFinding>,
}

#[derive(Debug, Clone)]
pub struct FileVisualReviewStore {
    log_path: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl FileVisualReviewStore {
    pub fn new(runtime_storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            log_path: runtime_storage_dir.into().join("visual-reviews.jsonl"),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn save(
        &self,
        project_id: &str,
        state: VisualReviewState,
        findings: Vec<VisualFinding>,
    ) -> Result<VisualReviewResult> {
        state
            .validate()
            .map_err(|error| anyhow!("invalid VisualReviewState: {error}"))?;
        for finding in &findings {
            finding
                .validate()
                .map_err(|error| anyhow!("invalid VisualFinding: {error}"))?;
        }
        let _guard = self.write_lock.lock().await;
        if let Some(parent) = self.log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let record = PersistedVisualReview {
            project_id: project_id.to_string(),
            state: state.clone(),
            findings: findings.clone(),
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(VisualReviewResult { state, findings })
    }

    pub fn latest(
        &self,
        project_id: &str,
        target: &RunVisualTarget,
    ) -> Result<Option<VisualReviewResult>> {
        let file = match fs::File::open(&self.log_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let target = serde_json::to_value(target)?;
        let mut latest = None;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let record: PersistedVisualReview = serde_json::from_str(&line)?;
            if record.project_id == project_id
                && record
                    .state
                    .target
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()?
                    == Some(target.clone())
            {
                latest = Some(VisualReviewResult {
                    state: record.state,
                    findings: record.findings,
                });
            }
        }
        Ok(latest)
    }

    pub fn artifact_is_referenced(&self, artifact_id: &str) -> Result<bool> {
        let file = match fs::File::open(&self.log_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let record: PersistedVisualReview = serde_json::from_str(&line)?;
            if record.findings.iter().any(|finding| {
                finding
                    .evidence_artifact_ids
                    .iter()
                    .any(|id| id == artifact_id)
            }) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[derive(Clone)]
pub struct VisualReviewService {
    store: RuntimeStore,
    model: Arc<dyn ModelClient>,
    config: RuntimeConfig,
    reviews: FileVisualReviewStore,
}

impl VisualReviewService {
    pub fn new(store: RuntimeStore, model: Arc<dyn ModelClient>, config: RuntimeConfig) -> Self {
        let reviews = FileVisualReviewStore::new(&config.runtime_storage_dir);
        Self {
            store,
            model,
            config,
            reviews,
        }
    }

    pub async fn schedule(
        &self,
        request: ScheduleVisualReviewRequest,
    ) -> Result<VisualReviewResult> {
        request
            .target
            .validate()
            .map_err(|error| anyhow!("invalid visual review target: {error}"))?;
        match &request.target {
            RunVisualTarget::StaticSnapshot { snapshot_id, .. } => {
                let snapshot = self
                    .store
                    .get_draft_snapshot(snapshot_id)
                    .await
                    .ok_or_else(|| {
                        anyhow!("visual review DraftSnapshot not found: {snapshot_id}")
                    })?;
                if snapshot.project_id != request.project_id {
                    return Err(anyhow!("visual review DraftSnapshot project mismatch"));
                }
            }
            RunVisualTarget::Version { version_id, .. } => {
                let version = self
                    .store
                    .get_project_version(version_id)
                    .await
                    .ok_or_else(|| anyhow!("visual review version not found: {version_id}"))?;
                if version.project_id != request.project_id {
                    return Err(anyhow!("visual review version project mismatch"));
                }
            }
            RunVisualTarget::Draft { .. } => {}
        }
        if request.mode == VisualReviewMode::Off {
            return self
                .reviews
                .save(
                    &request.project_id,
                    review_state(
                        request.mode,
                        VisualReviewStatus::NotRequested,
                        Some(request.target),
                        None,
                        None,
                    ),
                    vec![],
                )
                .await;
        }
        if !request
            .bindings
            .iter()
            .any(|binding| binding.role == RunVisualBindingRole::Candidate)
        {
            return self
                .reviews
                .save(
                    &request.project_id,
                    review_state(
                        request.mode,
                        VisualReviewStatus::Unavailable,
                        Some(request.target),
                        None,
                        Some("visual review requires at least one candidate artifact".to_string()),
                    ),
                    vec![],
                )
                .await;
        }

        let artifact_store =
            VisualArtifactStore::open(self.config.runtime_storage_dir.join("visual-artifacts"))?;
        let mut bound_artifacts = Vec::new();
        for binding in &request.bindings {
            let artifact = artifact_store
                .get(&binding.artifact_id)?
                .ok_or_else(|| anyhow!("VisualArtifact not found: {}", binding.artifact_id))?;
            if artifact.project_id != request.project_id {
                return Err(anyhow!("VisualArtifact belongs to a different project"));
            }
            bound_artifacts.push((binding.clone(), artifact));
        }
        bound_artifacts.sort_by_key(|(binding, _)| binding.order);

        let run = self
            .store
            .create_run(
                request.project_id.clone(),
                AgentPhase::Review,
                "visual-review-sidechain".to_string(),
                request.model.clone(),
                vec![],
            )
            .await;
        for (binding, _) in &bound_artifacts {
            self.store
                .upsert_run_visual_binding(RunVisualBinding {
                    run_id: run.id.clone(),
                    artifact_id: binding.artifact_id.clone(),
                    role: binding.role,
                    route: binding.route.clone(),
                    viewport: binding.viewport.clone(),
                    target: request.target.clone(),
                    order: binding.order,
                })
                .await?;
        }
        self.reviews
            .save(
                &request.project_id,
                review_state(
                    request.mode,
                    VisualReviewStatus::Queued,
                    Some(request.target.clone()),
                    Some(run.id.clone()),
                    None,
                ),
                vec![],
            )
            .await?;

        let mut content = vec![json!({
            "type": "text",
            "text": "Compare the bound reference and candidate images. Report only concrete, localized findings with evidence. If no finding exists, reply exactly VISUAL_REVIEW_PASS."
        })];
        for (binding, artifact) in &bound_artifacts {
            content.push(json!({
                "type": "text",
                "text": format!("role={:?}; route={}; viewport={}x{}", binding.role, binding.route, binding.viewport.width, binding.viewport.height),
            }));
            let mut image = json!({
                "type": "image",
                "artifactId": artifact.id,
                "mediaType": artifact.media_type,
                "sha256": artifact.sha256,
                "width": artifact.width,
                "height": artifact.height,
            });
            if self.config.model_provider != ModelProvider::InternalGateway {
                image["dataBase64"] = Value::String(
                    BASE64_STANDARD.encode(artifact_store.read_content(&artifact.id)?),
                );
            }
            content.push(image);
        }
        let workspace_id = self
            .store
            .get_project_access(&request.project_id)
            .await
            .map(|access| access.workspace_namespace)
            .unwrap_or_else(|| "ws-runtime-local".to_string());
        let turn = self
            .model
            .next_response_scoped_with_execution(
                ModelRequest {
                    run_id: run.id.clone(),
                    turn: 1,
                    model: request.model.clone(),
                    phase: AgentPhase::Review,
                    agent_profile: "visual-review-sidechain".to_string(),
                    system_prompt: "You are a read-only visual QA reviewer. Never propose file mutations as commands. Use review.report_finding once per evidence-backed issue. If there are no issues, reply exactly VISUAL_REVIEW_PASS.".to_string(),
                    messages: vec![json!({ "role": "user", "content": content })],
                    tools: vec![visual_finding_tool()],
                    deferred_tools: vec![],
                },
                ModelGatewayScope {
                    workspace_id,
                    project_id: request.project_id.clone(),
                },
            )
            .await;

        let turn = match turn {
            Ok(turn) => turn,
            Err(error) => {
                self.store
                    .update_run_status(&run.id, AgentRunStatus::Failed)
                    .await?;
                return self
                    .reviews
                    .save(
                        &request.project_id,
                        review_state(
                            request.mode,
                            VisualReviewStatus::Unavailable,
                            Some(request.target),
                            Some(run.id),
                            Some(format!("visual model unavailable: {error}")),
                        ),
                        vec![],
                    )
                    .await;
            }
        };
        let calls = match turn.response {
            ModelResponse::ToolCalls(calls)
            | ModelResponse::ToolCallsThenError { calls, .. }
            | ModelResponse::ToolCallsThenFallback { calls, .. } => calls,
            ModelResponse::TextOnly(text) if text.trim() == "VISUAL_REVIEW_PASS" => vec![],
            ModelResponse::Error(error) => {
                return self
                    .finish_failed(
                        &request,
                        &run.id,
                        format!("visual model returned an error: {error}"),
                    )
                    .await;
            }
            _ => {
                return self
                    .finish_failed(
                        &request,
                        &run.id,
                        "visual model response violated the review protocol".to_string(),
                    )
                    .await;
            }
        };
        let snapshot = turn.execution.map_or_else(
            || VisualFindingModelResourceSnapshot {
                model_resource_id: run.model.clone(),
                revision: 1,
                physical_model: run.model.clone(),
                capability_snapshot_hash: sha256_hex(run.model.as_bytes()),
                prompt_policy_version: VISUAL_REVIEW_PROMPT_POLICY_VERSION.to_string(),
            },
            |execution| VisualFindingModelResourceSnapshot {
                model_resource_id: execution.model_resource_id,
                revision: execution.model_resource_revision,
                physical_model: execution.physical_model,
                capability_snapshot_hash: execution.capability_snapshot_hash,
                prompt_policy_version: VISUAL_REVIEW_PROMPT_POLICY_VERSION.to_string(),
            },
        );
        let artifact_ids = bound_artifacts
            .iter()
            .map(|(_, artifact)| artifact.id.clone())
            .collect::<HashSet<_>>();
        let findings = calls
            .iter()
            .map(|call| {
                parse_visual_finding(
                    &self.store,
                    call,
                    &run.id,
                    &request.target,
                    &artifact_ids,
                    &snapshot,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        self.store
            .update_run_status(&run.id, AgentRunStatus::Completed)
            .await?;
        self.reviews
            .save(
                &request.project_id,
                review_state(
                    request.mode,
                    if findings.is_empty() {
                        VisualReviewStatus::Passed
                    } else {
                        VisualReviewStatus::Findings
                    },
                    Some(request.target),
                    Some(run.id),
                    None,
                ),
                findings,
            )
            .await
    }

    async fn finish_failed(
        &self,
        request: &ScheduleVisualReviewRequest,
        run_id: &str,
        reason: String,
    ) -> Result<VisualReviewResult> {
        self.store
            .update_run_status(run_id, AgentRunStatus::Failed)
            .await?;
        self.reviews
            .save(
                &request.project_id,
                review_state(
                    request.mode,
                    VisualReviewStatus::Failed,
                    Some(request.target.clone()),
                    Some(run_id.to_string()),
                    Some(reason),
                ),
                vec![],
            )
            .await
    }
}

fn review_state(
    mode: VisualReviewMode,
    status: VisualReviewStatus,
    target: Option<RunVisualTarget>,
    run_id: Option<String>,
    reason: Option<String>,
) -> VisualReviewState {
    VisualReviewState {
        schema_version: VISUAL_REVIEW_STATE_SCHEMA.to_string(),
        mode,
        status,
        target,
        run_id,
        reason,
        updated_at: Utc::now(),
    }
}

fn visual_finding_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        name: "review.report_finding".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "route": { "type": "string", "pattern": "^/" },
                "viewport": {
                    "type": "object",
                    "properties": {
                        "width": { "type": "integer", "minimum": 1 },
                        "height": { "type": "integer", "minimum": 1 },
                        "deviceScaleFactor": { "type": "number", "minimum": 0.1, "maximum": 4 }
                    },
                    "required": ["width", "height"],
                    "additionalProperties": false
                },
                "category": { "type": "string", "enum": ["layout", "hierarchy", "typography", "color", "spacing", "border_radius", "image_crop", "density", "consistency", "responsive", "advisory_note"] },
                "severity": { "type": "string", "enum": ["info", "warning", "blocking"] },
                "summary": { "type": "string", "minLength": 1, "maxLength": 320 },
                "evidenceArtifactIds": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                "evidenceRegion": { "type": ["object", "null"] },
                "targetObservationId": { "type": ["string", "null"] },
                "suggestedChange": { "type": "string", "minLength": 1, "maxLength": 500 }
            },
            "required": ["route", "viewport", "category", "severity", "summary", "evidenceArtifactIds", "suggestedChange"],
            "additionalProperties": false
        }),
        input_json_schema: None,
        output_schema: None,
        loading_policy: ToolLoadingPolicy::AlwaysLoad,
        mcp_info: None,
    }
}

fn parse_visual_finding(
    store: &RuntimeStore,
    call: &ToolCall,
    review_run_id: &str,
    target: &RunVisualTarget,
    bound_artifact_ids: &HashSet<String>,
    snapshot: &VisualFindingModelResourceSnapshot,
) -> Result<VisualFinding> {
    if call.name != "review.report_finding" {
        return Err(anyhow!(
            "visual review called an unsupported tool: {}",
            call.name
        ));
    }
    let evidence_artifact_ids = serde_json::from_value::<Vec<String>>(
        call.input
            .get("evidenceArtifactIds")
            .cloned()
            .ok_or_else(|| anyhow!("visual finding requires evidenceArtifactIds"))?,
    )?;
    if evidence_artifact_ids.is_empty()
        || evidence_artifact_ids
            .iter()
            .any(|artifact_id| !bound_artifact_ids.contains(artifact_id))
    {
        return Err(anyhow!(
            "visual finding evidence must reference artifacts bound to the review Run"
        ));
    }
    let finding = VisualFinding {
        schema_version: VISUAL_FINDING_SCHEMA.to_string(),
        finding_id: store.next_id("visual-finding"),
        review_run_id: review_run_id.to_string(),
        target: target.clone(),
        route: required_string(&call.input, "route")?,
        viewport: serde_json::from_value(
            call.input
                .get("viewport")
                .cloned()
                .ok_or_else(|| anyhow!("visual finding requires viewport"))?,
        )?,
        category: serde_json::from_value(
            call.input
                .get("category")
                .cloned()
                .ok_or_else(|| anyhow!("visual finding requires category"))?,
        )?,
        severity: serde_json::from_value(
            call.input
                .get("severity")
                .cloned()
                .ok_or_else(|| anyhow!("visual finding requires severity"))?,
        )?,
        summary: required_string(&call.input, "summary")?,
        evidence_artifact_ids,
        evidence_region: call
            .input
            .get("evidenceRegion")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value::<ElementBoundingBox>)
            .transpose()?,
        target_observation_id: call
            .input
            .get("targetObservationId")
            .and_then(Value::as_str)
            .map(str::to_string),
        suggested_change: required_string(&call.input, "suggestedChange")?,
        status: VisualFindingStatus::Open,
        model_resource_snapshot: snapshot.clone(),
    };
    finding
        .validate()
        .map_err(|error| anyhow!("invalid visual finding: {error}"))?;
    Ok(finding)
}

fn required_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("visual finding requires {key}"))
}
