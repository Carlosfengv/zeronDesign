use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::types::ProjectVersion;

pub const DRAFT_SNAPSHOT_SCHEMA: &str = "draft-snapshot@1";
pub const VISUAL_ARTIFACT_SCHEMA: &str = "visual-artifact@1";
pub const VISUAL_REVIEW_STATE_SCHEMA: &str = "visual-review-state@1";
pub const DRAFT_PREVIEW_SESSION_SCHEMA: &str = "draft-preview-session@1";
pub const ELEMENT_OBSERVATION_SCHEMA: &str = "element-observation@1";
pub const EDIT_IMPACT_PLAN_SCHEMA: &str = "edit-impact-plan@1";
pub const VISUAL_FINDING_SCHEMA: &str = "visual-finding@1";
pub const PUBLISH_WORKFLOW_SCHEMA: &str = "publish-workflow@1";
pub const PROJECT_ASSET_SCHEMA: &str = "project-asset@1";
pub const RUNTIME_DEPENDENCY_POLICY_VERSION: &str = "runtime-dependency-policy@1";

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_root_relative_route(value: &str) -> bool {
    value.starts_with('/') && !value.starts_with("//")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAssetSource {
    Upload,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAsset {
    pub schema_version: String,
    pub asset_id: String,
    pub project_id: String,
    pub source_artifact_id: String,
    pub source: ProjectAssetSource,
    pub target_path: String,
    pub content_hash: String,
    pub license: String,
    pub provenance: Value,
    pub width: u32,
    pub height: u32,
    pub alt_text: String,
    pub created_by_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ProjectAsset {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != PROJECT_ASSET_SCHEMA
            || self.asset_id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || self.source_artifact_id.trim().is_empty()
            || !self.target_path.starts_with("public/assets/")
            || !is_sha256(&self.content_hash)
            || self.license.trim().is_empty()
            || self.width == 0
            || self.height == 0
            || self.alt_text.trim().is_empty()
        {
            return Err("ProjectAsset is invalid".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftSnapshotRetentionState {
    Active,
    DeletionPending,
    Protected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftSnapshot {
    pub schema_version: String,
    pub snapshot_id: String,
    pub project_id: String,
    pub source_snapshot_uri: String,
    pub source_hash: String,
    pub template_id: String,
    pub template_version: String,
    pub dependency_policy_version: String,
    pub design_context_hash: String,
    pub created_by_run_id: String,
    pub based_on_snapshot_id: Option<String>,
    pub restored_from_version_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub retention_state: DraftSnapshotRetentionState,
    pub delete_after: Option<DateTime<Utc>>,
}

impl DraftSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != DRAFT_SNAPSHOT_SCHEMA {
            return Err("unsupported DraftSnapshot schemaVersion".to_string());
        }
        if self.snapshot_id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || self.source_snapshot_uri.trim().is_empty()
            || self.template_id.trim().is_empty()
            || self.template_version.trim().is_empty()
            || self.dependency_policy_version.trim().is_empty()
            || self.created_by_run_id.trim().is_empty()
        {
            return Err("DraftSnapshot required identity fields must be non-empty".to_string());
        }
        if !is_sha256(&self.source_hash) || !is_sha256(&self.design_context_hash) {
            return Err("DraftSnapshot hashes must be SHA-256 hex".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum PublishSource {
    #[serde(rename = "static-snapshot", rename_all = "camelCase")]
    StaticSnapshot {
        project_id: String,
        snapshot_id: String,
        expected_source_hash: String,
    },
    #[serde(rename = "draft-revision", rename_all = "camelCase")]
    DraftRevision {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        revision: u64,
        snapshot_id: String,
        expected_source_hash: String,
    },
}

impl PublishSource {
    pub fn project_id(&self) -> &str {
        match self {
            Self::StaticSnapshot { project_id, .. } | Self::DraftRevision { project_id, .. } => {
                project_id
            }
        }
    }

    pub fn snapshot_id(&self) -> &str {
        match self {
            Self::StaticSnapshot { snapshot_id, .. } | Self::DraftRevision { snapshot_id, .. } => {
                snapshot_id
            }
        }
    }

    pub fn expected_source_hash(&self) -> &str {
        match self {
            Self::StaticSnapshot {
                expected_source_hash,
                ..
            }
            | Self::DraftRevision {
                expected_source_hash,
                ..
            } => expected_source_hash,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.project_id().trim().is_empty()
            || self.snapshot_id().trim().is_empty()
            || !is_sha256(self.expected_source_hash())
        {
            return Err("PublishSource identity is invalid".to_string());
        }
        if let Self::DraftRevision {
            session_id,
            session_epoch,
            ..
        } = self
        {
            if session_id.trim().is_empty() || *session_epoch == 0 {
                return Err("draft PublishSource session identity is invalid".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum EditBase {
    #[serde(rename = "work-version", rename_all = "camelCase")]
    WorkVersion { version_id: String },
    #[serde(rename = "draft", rename_all = "camelCase")]
    Draft {
        snapshot_id: String,
        session_id: String,
        expected_session_epoch: u64,
        expected_workspace_revision: u64,
        writer_lease_id: String,
    },
}

impl EditBase {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::WorkVersion { version_id } if version_id.trim().is_empty() => {
                Err("work-version EditBase requires versionId".to_string())
            }
            Self::Draft {
                snapshot_id,
                session_id,
                expected_session_epoch,
                writer_lease_id,
                ..
            } if snapshot_id.trim().is_empty()
                || session_id.trim().is_empty()
                || *expected_session_epoch == 0
                || writer_lease_id.trim().is_empty() =>
            {
                Err("draft EditBase identity is invalid".to_string())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisualMediaType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualArtifactOrigin {
    Upload,
    Browser,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualArtifact {
    pub schema_version: String,
    pub id: String,
    pub project_id: String,
    pub media_type: VisualMediaType,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    pub storage_uri: String,
    pub origin: VisualArtifactOrigin,
    #[serde(default)]
    pub origin_metadata: BTreeMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub retention_state: DraftSnapshotRetentionState,
    pub delete_after: Option<DateTime<Utc>>,
}

impl VisualArtifact {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VISUAL_ARTIFACT_SCHEMA {
            return Err("unsupported VisualArtifact schemaVersion".to_string());
        }
        if self.id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || self.storage_uri.trim().is_empty()
            || self.size_bytes == 0
            || self.width == 0
            || self.height == 0
            || self.width > 16_384
            || self.height > 16_384
            || !is_sha256(&self.sha256)
        {
            return Err("VisualArtifact metadata is invalid".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualViewport {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_device_scale_factor")]
    pub device_scale_factor: f64,
}

fn default_device_scale_factor() -> f64 {
    1.0
}

impl VisualViewport {
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0
            || self.height == 0
            || self.width > 16_384
            || self.height > 16_384
            || !self.device_scale_factor.is_finite()
            || self.device_scale_factor <= 0.0
            || self.device_scale_factor > 4.0
        {
            return Err("visual viewport is outside supported bounds".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum RunVisualTarget {
    #[serde(rename = "draft", rename_all = "camelCase")]
    Draft {
        session_id: String,
        session_epoch: u64,
        source_revision: u64,
        source_hash: String,
    },
    #[serde(rename = "version", rename_all = "camelCase")]
    Version {
        version_id: String,
        artifact_manifest_hash: String,
    },
    #[serde(rename = "static-snapshot", rename_all = "camelCase")]
    StaticSnapshot {
        snapshot_id: String,
        source_hash: String,
    },
}

impl RunVisualTarget {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Draft {
                session_id,
                session_epoch,
                source_hash,
                ..
            } if session_id.trim().is_empty() || *session_epoch == 0 || !is_sha256(source_hash) => {
                Err("draft visual target identity is invalid".to_string())
            }
            Self::Version {
                version_id,
                artifact_manifest_hash,
            } if version_id.trim().is_empty() || !is_sha256(artifact_manifest_hash) => {
                Err("version visual target identity is invalid".to_string())
            }
            Self::StaticSnapshot {
                snapshot_id,
                source_hash,
            } if snapshot_id.trim().is_empty() || !is_sha256(source_hash) => {
                Err("static visual target identity is invalid".to_string())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunVisualBindingRole {
    Reference,
    Candidate,
}

/// A visual binding supplied atomically with StartRun. The Runtime adds the
/// durable Run identity before persisting it, so Generation Context can freeze
/// the complete reference set before the first model turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRunVisualBinding {
    pub artifact_id: String,
    pub role: RunVisualBindingRole,
    pub route: String,
    pub viewport: VisualViewport,
    pub target: RunVisualTarget,
    pub order: u32,
}

impl StartRunVisualBinding {
    pub fn validate(&self) -> Result<(), String> {
        RunVisualBinding {
            run_id: "start-run-validation".to_string(),
            artifact_id: self.artifact_id.clone(),
            role: self.role,
            route: self.route.clone(),
            viewport: self.viewport.clone(),
            target: self.target.clone(),
            order: self.order,
        }
        .validate()
    }

    pub fn bind_to_run(&self, run_id: &str) -> RunVisualBinding {
        RunVisualBinding {
            run_id: run_id.to_string(),
            artifact_id: self.artifact_id.clone(),
            role: self.role,
            route: self.route.clone(),
            viewport: self.viewport.clone(),
            target: self.target.clone(),
            order: self.order,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunVisualBinding {
    pub run_id: String,
    pub artifact_id: String,
    pub role: RunVisualBindingRole,
    pub route: String,
    pub viewport: VisualViewport,
    pub target: RunVisualTarget,
    pub order: u32,
}

impl RunVisualBinding {
    pub fn validate(&self) -> Result<(), String> {
        if self.run_id.trim().is_empty()
            || self.artifact_id.trim().is_empty()
            || !is_root_relative_route(&self.route)
        {
            return Err("RunVisualBinding identity is invalid".to_string());
        }
        self.viewport.validate()?;
        self.target.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolResultContentBlock {
    Text {
        text: String,
    },
    Json {
        value: Value,
    },
    Image {
        #[serde(rename = "artifactId")]
        artifact_id: String,
        #[serde(rename = "mediaType")]
        media_type: VisualMediaType,
        sha256: String,
        width: u32,
        height: u32,
    },
}

impl ToolResultContentBlock {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Image {
                artifact_id,
                sha256,
                width,
                height,
                ..
            } if artifact_id.trim().is_empty()
                || !is_sha256(sha256)
                || *width == 0
                || *height == 0
                || *width > 16_384
                || *height > 16_384 =>
            {
                Err("image content block metadata is invalid".to_string())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelVisionCapability {
    #[serde(default)]
    pub vision_input: bool,
    #[serde(default)]
    pub supported_image_media_types: Vec<VisualMediaType>,
    #[serde(default)]
    pub max_image_bytes: u64,
    #[serde(default)]
    pub max_image_count: u32,
}

impl ModelVisionCapability {
    pub fn validate(&self) -> Result<(), String> {
        if !self.vision_input {
            return Ok(());
        }
        if self.supported_image_media_types.is_empty() {
            return Err(
                "vision input requires at least one supported image media type".to_string(),
            );
        }
        if self.max_image_bytes == 0 {
            return Err("vision input requires a positive maxImageBytes".to_string());
        }
        if self.max_image_count == 0 {
            return Err("vision input requires a positive maxImageCount".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualReviewMode {
    Off,
    Advisory,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualReviewStatus {
    NotRequested,
    Queued,
    Passed,
    Findings,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualReviewState {
    pub schema_version: String,
    pub mode: VisualReviewMode,
    pub status: VisualReviewStatus,
    pub target: Option<RunVisualTarget>,
    pub run_id: Option<String>,
    pub reason: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl VisualReviewState {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VISUAL_REVIEW_STATE_SCHEMA {
            return Err("unsupported VisualReviewState schemaVersion".to_string());
        }
        if let Some(target) = &self.target {
            target.validate()?;
        }
        if matches!(self.status, VisualReviewStatus::Queued) && self.run_id.is_none() {
            return Err("queued visual review requires runId".to_string());
        }
        if matches!(
            self.status,
            VisualReviewStatus::Unavailable | VisualReviewStatus::Failed
        ) && self.reason.as_deref().is_none_or(str::is_empty)
        {
            return Err("unavailable or failed visual review requires reason".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftPreviewSessionStatus {
    Starting,
    Ready,
    Updating,
    CompileError,
    Crashed,
    Restarting,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftPreviewSession {
    pub schema_version: String,
    pub session_id: String,
    pub project_id: String,
    pub sandbox_binding_id: String,
    pub template_id: String,
    pub base_snapshot_id: String,
    pub base_version_id: Option<String>,
    pub writer_lease_id: String,
    pub writer_lease_expires_at: DateTime<Utc>,
    pub workspace_revision: u64,
    pub last_ready_revision: u64,
    pub durable_revision: u64,
    pub durable_snapshot_id: String,
    pub publish_revision: Option<u64>,
    pub session_epoch: u64,
    pub status: DraftPreviewSessionStatus,
    pub proxy_url: String,
    pub started_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub restart_count: u32,
    pub last_error: Option<String>,
}

impl DraftPreviewSession {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != DRAFT_PREVIEW_SESSION_SCHEMA
            || self.session_id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || self.sandbox_binding_id.trim().is_empty()
            || self.template_id.trim().is_empty()
            || self.base_snapshot_id.trim().is_empty()
            || self.writer_lease_id.trim().is_empty()
            || self.durable_snapshot_id.trim().is_empty()
            || !(self.proxy_url.starts_with("http://") || self.proxy_url.starts_with("https://"))
        {
            return Err("DraftPreviewSession identity is invalid".to_string());
        }
        if self.last_ready_revision > self.workspace_revision {
            return Err("lastReadyRevision cannot exceed workspaceRevision".to_string());
        }
        if self.durable_revision > self.workspace_revision {
            return Err("durableRevision cannot exceed workspaceRevision".to_string());
        }
        if self.session_epoch == 0 {
            return Err("sessionEpoch must be positive".to_string());
        }
        if self.restart_count > 2 {
            return Err("restartCount exceeds the automatic restart budget".to_string());
        }
        if matches!(
            self.status,
            DraftPreviewSessionStatus::CompileError
                | DraftPreviewSessionStatus::Crashed
                | DraftPreviewSessionStatus::Failed
        ) && self.last_error.as_deref().is_none_or(str::is_empty)
        {
            return Err("failed DraftPreviewSession status requires lastError".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DraftPreviewEvent {
    #[serde(rename = "preview.dev_starting", rename_all = "camelCase")]
    DevStarting {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.dev_ready", rename_all = "camelCase")]
    DevReady {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        proxy_url: String,
        ready_revision: u64,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.dev_updating", rename_all = "camelCase")]
    DevUpdating {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.dev_compile_error", rename_all = "camelCase")]
    DevCompileError {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        error: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.dev_restarting", rename_all = "camelCase")]
    DevRestarting {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        restart_count: u32,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.dev_failed", rename_all = "camelCase")]
    DevFailed {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        error: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.dev_stopped", rename_all = "camelCase")]
    DevStopped {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "source.revision_committed", rename_all = "camelCase")]
    SourceRevisionCommitted {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "source.revision_durable", rename_all = "camelCase")]
    SourceRevisionDurable {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        snapshot_id: String,
        source_hash: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "source.snapshot_created", rename_all = "camelCase")]
    SourceSnapshotCreated {
        project_id: String,
        session_id: String,
        session_epoch: u64,
        workspace_revision: u64,
        snapshot_id: String,
        source_hash: String,
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ElementBoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ElementSourceCandidate {
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub export_name: Option<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ElementObservation {
    pub schema_version: String,
    pub observation_id: String,
    pub project_id: String,
    pub session_id: String,
    pub session_epoch: u64,
    pub workspace_revision: u64,
    pub route: String,
    pub viewport: VisualViewport,
    pub dom_path: String,
    pub data_slot: Option<String>,
    pub accessible_name: Option<String>,
    pub visible_text_hash: Option<String>,
    pub bounding_box: ElementBoundingBox,
    pub source_candidates: Vec<ElementSourceCandidate>,
    pub confidence: f64,
    pub screenshot_crop_artifact_id: String,
    pub expires_at: DateTime<Utc>,
    pub signature: String,
}

impl ElementObservation {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != ELEMENT_OBSERVATION_SCHEMA
            || self.observation_id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || self.session_id.trim().is_empty()
            || self.session_epoch == 0
            || !is_root_relative_route(&self.route)
            || self.screenshot_crop_artifact_id.trim().is_empty()
            || self.signature.trim().is_empty()
            || !self.confidence.is_finite()
            || !(0.0..=1.0).contains(&self.confidence)
            || self.bounding_box.width < 0.0
            || self.bounding_box.height < 0.0
        {
            return Err("ElementObservation metadata is invalid".to_string());
        }
        if self
            .visible_text_hash
            .as_deref()
            .is_some_and(|hash| !is_sha256(hash))
        {
            return Err("ElementObservation visibleTextHash is invalid".to_string());
        }
        self.viewport.validate()?;
        if self.source_candidates.iter().any(|candidate| {
            candidate.path.trim().is_empty()
                || !candidate.confidence.is_finite()
                || !(0.0..=1.0).contains(&candidate.confidence)
        }) {
            return Err("ElementObservation source candidate is invalid".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditImpactScope {
    Local,
    Page,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditImpactOperation {
    Copy,
    Style,
    Layout,
    Component,
    Navigation,
    Delete,
    Dependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditImpactRisk {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditImpactPlan {
    pub schema_version: String,
    pub scope: EditImpactScope,
    pub targets: Vec<String>,
    pub operations: Vec<EditImpactOperation>,
    pub risk: EditImpactRisk,
    pub requires_confirmation: bool,
    pub edit_base: EditBase,
    pub session_id: String,
    pub session_epoch: u64,
    pub workspace_revision: u64,
    pub plan_hash: String,
}

impl EditImpactPlan {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != EDIT_IMPACT_PLAN_SCHEMA
            || self.session_id.trim().is_empty()
            || self.session_epoch == 0
            || !is_sha256(&self.plan_hash)
        {
            return Err("EditImpactPlan identity is invalid".to_string());
        }
        if self.targets.is_empty() || self.operations.is_empty() {
            return Err("edit impact plan requires targets and operations".to_string());
        }
        if self.risk == EditImpactRisk::High && !self.requires_confirmation {
            return Err("high-risk edits require confirmation".to_string());
        }
        self.edit_base.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryItem {
    DraftSnapshot {
        snapshot: DraftSnapshot,
        recoverable: bool,
        publishable: bool,
    },
    WorkVersion {
        version: ProjectVersion,
        recoverable: bool,
        publishable: bool,
    },
}

impl HistoryItem {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::DraftSnapshot {
                recoverable,
                publishable,
                ..
            } if !recoverable || *publishable => {
                Err("draft snapshots must be recoverable and not publishable".to_string())
            }
            Self::WorkVersion {
                version,
                recoverable,
                publishable,
            } if *recoverable != version.source_snapshot_uri.is_some()
                || *publishable
                    != (version.status == crate::types::ProjectVersionStatus::Promoted) =>
            {
                Err("work version history flags do not match version state".to_string())
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualFindingCategory {
    Layout,
    Hierarchy,
    Typography,
    Color,
    Spacing,
    BorderRadius,
    ImageCrop,
    Density,
    Consistency,
    Responsive,
    AdvisoryNote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualFindingSeverity {
    Info,
    Warning,
    Blocking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualFindingStatus {
    Open,
    Repairing,
    Fixed,
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualFindingModelResourceSnapshot {
    pub model_resource_id: String,
    pub revision: u64,
    pub physical_model: String,
    pub capability_snapshot_hash: String,
    pub prompt_policy_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualFinding {
    pub schema_version: String,
    pub finding_id: String,
    pub review_run_id: String,
    pub target: RunVisualTarget,
    pub route: String,
    pub viewport: VisualViewport,
    pub category: VisualFindingCategory,
    pub severity: VisualFindingSeverity,
    pub summary: String,
    pub evidence_artifact_ids: Vec<String>,
    pub evidence_region: Option<ElementBoundingBox>,
    pub target_observation_id: Option<String>,
    pub suggested_change: String,
    pub status: VisualFindingStatus,
    pub model_resource_snapshot: VisualFindingModelResourceSnapshot,
}

impl VisualFinding {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VISUAL_FINDING_SCHEMA
            || self.finding_id.trim().is_empty()
            || self.review_run_id.trim().is_empty()
            || !is_root_relative_route(&self.route)
            || self.summary.trim().is_empty()
            || self.evidence_artifact_ids.is_empty()
            || self.suggested_change.trim().is_empty()
            || self
                .evidence_artifact_ids
                .iter()
                .any(|artifact_id| artifact_id.trim().is_empty())
            || self
                .model_resource_snapshot
                .model_resource_id
                .trim()
                .is_empty()
            || self.model_resource_snapshot.revision == 0
            || self
                .model_resource_snapshot
                .physical_model
                .trim()
                .is_empty()
            || !is_sha256(&self.model_resource_snapshot.capability_snapshot_hash)
            || self
                .model_resource_snapshot
                .prompt_policy_version
                .trim()
                .is_empty()
        {
            return Err("VisualFinding metadata is invalid".to_string());
        }
        self.viewport.validate()?;
        self.target.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishWorkflowStatus {
    Requested,
    SourceFrozen,
    Building,
    Validating,
    ReleasePackaging,
    ReleaseValidated,
    DesiredStateCommitted,
    Reconciling,
    WorkloadReady,
    TrafficSwitched,
    ExternalProbePassed,
    RollingBack,
    Completed,
    Failed,
    Cancelled,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishWorkflowCheckpoint {
    Requested,
    SourceFrozen,
    Building,
    Validating,
    ReleasePackaging,
    ReleaseValidated,
    DesiredStateCommitted,
    Reconciling,
    WorkloadReady,
    TrafficSwitched,
    ExternalProbePassed,
    RollingBack,
    Completed,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishWorkflowStageEvidence {
    pub stage: PublishWorkflowCheckpoint,
    pub input_hash: String,
    pub child_operation_id: Option<String>,
    pub attempt: u32,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishWorkflow {
    pub schema_version: String,
    pub id: String,
    pub idempotency_key_hash: String,
    pub request_hash: String,
    pub project_id: String,
    pub source: PublishSource,
    pub status: PublishWorkflowStatus,
    pub checkpoint: PublishWorkflowCheckpoint,
    pub visual_review_mode: VisualReviewMode,
    pub expected_current_release_id: Option<String>,
    pub expected_generation: u64,
    pub version_id: Option<String>,
    pub release_id: Option<String>,
    pub publication_operation_id: Option<String>,
    pub public_url: Option<String>,
    pub evidence: Vec<PublishWorkflowStageEvidence>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PublishWorkflow {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != PUBLISH_WORKFLOW_SCHEMA
            || self.id.trim().is_empty()
            || self.project_id.trim().is_empty()
            || !is_sha256(&self.idempotency_key_hash)
            || !is_sha256(&self.request_hash)
            || self.project_id != self.source.project_id()
        {
            return Err("PublishWorkflow identity is invalid".to_string());
        }
        self.source.validate()?;
        if self
            .evidence
            .iter()
            .any(|evidence| evidence.attempt == 0 || !is_sha256(&evidence.input_hash))
        {
            return Err("PublishWorkflow stage evidence is invalid".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct VisualContractFixture {
        draft_snapshot: DraftSnapshot,
        publish_sources: Vec<PublishSource>,
        edit_bases: Vec<EditBase>,
        visual_artifact: VisualArtifact,
        run_visual_bindings: Vec<RunVisualBinding>,
        tool_result_content_blocks: Vec<ToolResultContentBlock>,
        model_vision_capabilities: Vec<ModelVisionCapability>,
        visual_review_states: Vec<VisualReviewState>,
        draft_preview_session: DraftPreviewSession,
        draft_preview_events: Vec<DraftPreviewEvent>,
        element_observation: ElementObservation,
        edit_impact_plan: EditImpactPlan,
        history_items: Vec<HistoryItem>,
        visual_finding: VisualFinding,
        project_asset: ProjectAsset,
        publish_workflow: PublishWorkflow,
    }

    #[test]
    fn shared_visual_contract_fixture_round_trips() {
        let fixture: VisualContractFixture = serde_json::from_str(include_str!(
            "../contracts/visual-runtime-contract-v1.fixture.json"
        ))
        .unwrap();

        assert_eq!(fixture.draft_snapshot.schema_version, DRAFT_SNAPSHOT_SCHEMA);
        fixture.draft_snapshot.validate().unwrap();
        assert_eq!(
            fixture.visual_artifact.schema_version,
            VISUAL_ARTIFACT_SCHEMA
        );
        assert_eq!(fixture.publish_sources.len(), 2);
        assert_eq!(fixture.edit_bases.len(), 2);
        assert_eq!(fixture.run_visual_bindings.len(), 3);
        assert_eq!(fixture.tool_result_content_blocks.len(), 3);
        assert_eq!(fixture.model_vision_capabilities.len(), 2);
        assert_eq!(fixture.visual_review_states.len(), 2);
        for source in &fixture.publish_sources {
            source.validate().unwrap();
        }
        for edit_base in &fixture.edit_bases {
            edit_base.validate().unwrap();
        }
        fixture.visual_artifact.validate().unwrap();
        for binding in &fixture.run_visual_bindings {
            binding.validate().unwrap();
        }
        for block in &fixture.tool_result_content_blocks {
            block.validate().unwrap();
        }
        for state in &fixture.visual_review_states {
            state.validate().unwrap();
        }
        assert_eq!(
            fixture.draft_preview_session.schema_version,
            DRAFT_PREVIEW_SESSION_SCHEMA
        );
        fixture.draft_preview_session.validate().unwrap();
        assert_eq!(fixture.draft_preview_events.len(), 2);
        assert_eq!(
            fixture.element_observation.schema_version,
            ELEMENT_OBSERVATION_SCHEMA
        );
        fixture.element_observation.validate().unwrap();
        assert_eq!(
            fixture.edit_impact_plan.schema_version,
            EDIT_IMPACT_PLAN_SCHEMA
        );
        fixture.edit_impact_plan.validate().unwrap();
        assert_eq!(fixture.history_items.len(), 2);
        for item in &fixture.history_items {
            item.validate().unwrap();
        }
        assert_eq!(fixture.visual_finding.schema_version, VISUAL_FINDING_SCHEMA);
        fixture.visual_finding.validate().unwrap();
        assert_eq!(fixture.project_asset.schema_version, PROJECT_ASSET_SCHEMA);
        fixture.project_asset.validate().unwrap();
        assert_eq!(
            fixture.publish_workflow.schema_version,
            PUBLISH_WORKFLOW_SCHEMA
        );
        fixture.publish_workflow.validate().unwrap();

        for capability in &fixture.model_vision_capabilities {
            capability.validate().unwrap();
        }

        let encoded = serde_json::to_value(&fixture.publish_workflow).unwrap();
        let decoded: PublishWorkflow = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, fixture.publish_workflow);
    }

    #[test]
    fn vision_capability_requires_bounded_inputs_when_enabled() {
        let capability = ModelVisionCapability {
            vision_input: true,
            ..Default::default()
        };
        assert!(capability.validate().is_err());
    }

    #[test]
    fn frozen_publish_workflow_schema_matches_runtime_statuses() {
        let schema: Value =
            serde_json::from_str(include_str!("../contracts/publish-workflow-v1.schema.json"))
                .unwrap();
        assert_eq!(
            schema["properties"]["schemaVersion"]["const"],
            PUBLISH_WORKFLOW_SCHEMA
        );
        let statuses = schema["properties"]["status"]["enum"].as_array().unwrap();
        for status in [
            PublishWorkflowStatus::Requested,
            PublishWorkflowStatus::SourceFrozen,
            PublishWorkflowStatus::Building,
            PublishWorkflowStatus::Validating,
            PublishWorkflowStatus::ReleasePackaging,
            PublishWorkflowStatus::ReleaseValidated,
            PublishWorkflowStatus::DesiredStateCommitted,
            PublishWorkflowStatus::Reconciling,
            PublishWorkflowStatus::WorkloadReady,
            PublishWorkflowStatus::TrafficSwitched,
            PublishWorkflowStatus::ExternalProbePassed,
            PublishWorkflowStatus::RollingBack,
            PublishWorkflowStatus::Completed,
            PublishWorkflowStatus::Failed,
            PublishWorkflowStatus::Cancelled,
            PublishWorkflowStatus::RolledBack,
            PublishWorkflowStatus::RollbackFailed,
        ] {
            assert!(statuses.contains(&serde_json::to_value(status).unwrap()));
        }
    }
}
