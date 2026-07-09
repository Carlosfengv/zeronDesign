use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPhase {
    Brief,
    Build,
    Repair,
    Review,
    Edit,
    Export,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Queued,
    Running,
    NeedsUserInput,
    Completed,
    Partial,
    Blocked,
    Failed,
    Cancelled,
}

impl AgentRunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Partial | Self::Blocked | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectVersionStatus {
    Candidate,
    Promoted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFindingSeverity {
    Info,
    Warning,
    Blocking,
}

impl ReviewFindingSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Blocking => "blocking",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFindingCategory {
    Build,
    Runtime,
    Visual,
    Content,
    Safety,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFindingStatus {
    Open,
    Repairing,
    Fixed,
    Accepted,
    NeedsUserInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBindingStatus {
    Claiming,
    Starting,
    Ready,
    Busy,
    Idle,
    Paused,
    Failed,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxChannelProtocol {
    Grpc,
    Websocket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSource {
    pub id: String,
    pub kind: String,
    pub text: String,
    #[serde(default = "default_readable")]
    pub readable: bool,
}

fn default_readable() -> bool {
    true
}

impl ContentSource {
    pub fn readable(
        id: impl Into<String>,
        kind: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            text: text.into(),
            readable: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Normal,
    ReadOnly,
    ScopedRepair,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptMode {
    Main,
    Sidechain,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunProfileSnapshot {
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub permission_mode: PermissionMode,
    pub transcript_mode: TranscriptMode,
    pub source_checkpoint_id: Option<String>,
    pub mcp_server_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    pub id: String,
    pub project_id: String,
    pub session_id: String,
    pub parent_run_id: Option<String>,
    pub triggered_by_event_id: Option<String>,
    pub phase: AgentPhase,
    pub agent_profile: String,
    pub status: AgentRunStatus,
    pub model: String,
    pub sandbox_id: Option<String>,
    pub brief_version: Option<String>,
    pub design_version: Option<String>,
    pub base_version_id: Option<String>,
    pub output_version_id: Option<String>,
    pub finding_ids: Option<Vec<String>>,
    pub input_message_ids: Vec<String>,
    pub checkpoint_id: Option<String>,
    pub profile_snapshot: RunProfileSnapshot,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brief {
    pub project_type: String,
    pub audience: String,
    pub content_hierarchy: Vec<String>,
    pub page_structure: Value,
    pub visual_direction: String,
    pub recommended_template: String,
    pub assumptions: Vec<String>,
    pub missing_information: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BriefStatus {
    Draft,
    Confirmed,
    Superseded,
}

impl Brief {
    pub fn validate_for_runtime(&self) -> Result<(), String> {
        if !matches!(self.project_type.as_str(), "website" | "docs") {
            return Err("projectType must be website or docs".to_string());
        }
        if self.audience.trim().is_empty() {
            return Err("audience is required".to_string());
        }
        if self.content_hierarchy.is_empty() {
            return Err("contentHierarchy must not be empty".to_string());
        }
        if self.visual_direction.trim().is_empty() {
            return Err("visualDirection is required".to_string());
        }
        if !matches!(
            self.recommended_template.as_str(),
            "astro-website" | "fumadocs-docs" | "nextjs-website" | "docusaurus-docs"
        ) {
            return Err("recommendedTemplate is not supported".to_string());
        }
        if !self.page_structure.is_array() {
            return Err("pageStructure must be an array".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationItem {
    pub id: String,
    pub project_id: String,
    pub run_id: Option<String>,
    pub version_id: Option<String>,
    pub checkpoint_id: Option<String>,
    pub kind: String,
    pub role: Option<String>,
    pub text: String,
    pub metadata: Option<Value>,
    pub visibility: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFindingEvidence {
    pub screenshot_id: Option<String>,
    pub file_path: Option<String>,
    pub log_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFinding {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub version_id: String,
    pub severity: ReviewFindingSeverity,
    pub category: ReviewFindingCategory,
    pub summary: String,
    pub evidence: Option<ReviewFindingEvidence>,
    pub repairable: bool,
    pub status: ReviewFindingStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentEvent {
    #[serde(rename = "run.started", rename_all = "camelCase")]
    RunStarted {
        run_id: String,
        label: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "agent.message", rename_all = "camelCase")]
    AgentMessage {
        run_id: String,
        text: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "tool.started", rename_all = "camelCase")]
    ToolStarted {
        run_id: String,
        tool: String,
        summary: String,
        tool_use_id: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "tool.completed", rename_all = "camelCase")]
    ToolCompleted {
        run_id: String,
        tool: String,
        summary: String,
        tool_use_id: String,
        metadata: Option<Value>,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "tool.output", rename_all = "camelCase")]
    ToolOutput {
        run_id: String,
        tool: String,
        tool_use_id: String,
        stream: String,
        text: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "tool.failed", rename_all = "camelCase")]
    ToolFailed {
        run_id: String,
        tool: String,
        error: String,
        tool_use_id: String,
        recoverable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "tool.recovery_suggested", rename_all = "camelCase")]
    ToolRecoverySuggested {
        run_id: String,
        tool: String,
        error_kind: String,
        fingerprint: String,
        attempt: u32,
        guidance: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "chunk.received", rename_all = "camelCase")]
    ChunkReceived {
        run_id: String,
        path: String,
        session_id: String,
        index: u64,
        total: u64,
        bytes: usize,
        chars: usize,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "chunk.committed", rename_all = "camelCase")]
    ChunkCommitted {
        run_id: String,
        path: String,
        session_id: String,
        total: u64,
        bytes: usize,
        chars: usize,
        sha256: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "metric.recorded", rename_all = "camelCase")]
    MetricRecorded {
        run_id: String,
        name: String,
        value: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "permission.requested", rename_all = "camelCase")]
    PermissionRequested {
        run_id: String,
        permission_id: String,
        tool: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "permission.denied", rename_all = "camelCase")]
    PermissionDenied {
        run_id: String,
        tool: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "state.changed", rename_all = "camelCase")]
    StateChanged {
        run_id: String,
        state: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.rebuilding", rename_all = "camelCase")]
    PreviewRebuilding {
        run_id: String,
        previous_version_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.candidate", rename_all = "camelCase")]
    PreviewCandidate {
        run_id: String,
        url: String,
        version_id: String,
        screenshot_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "preview.updated", rename_all = "camelCase")]
    PreviewUpdated {
        run_id: String,
        url: String,
        version_id: String,
        screenshot_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "review.finding", rename_all = "camelCase")]
    ReviewFinding {
        run_id: String,
        finding_id: String,
        severity: String,
        summary: String,
        timestamp: DateTime<Utc>,
    },
    #[serde(rename = "run.completed", rename_all = "camelCase")]
    RunCompleted {
        run_id: String,
        status: String,
        summary: String,
        timestamp: DateTime<Utc>,
    },
}

impl AgentEvent {
    pub fn run_id(&self) -> &str {
        match self {
            Self::RunStarted { run_id, .. }
            | Self::AgentMessage { run_id, .. }
            | Self::ToolStarted { run_id, .. }
            | Self::ToolCompleted { run_id, .. }
            | Self::ToolOutput { run_id, .. }
            | Self::ToolFailed { run_id, .. }
            | Self::ToolRecoverySuggested { run_id, .. }
            | Self::ChunkReceived { run_id, .. }
            | Self::ChunkCommitted { run_id, .. }
            | Self::MetricRecorded { run_id, .. }
            | Self::PermissionRequested { run_id, .. }
            | Self::PermissionDenied { run_id, .. }
            | Self::StateChanged { run_id, .. }
            | Self::PreviewRebuilding { run_id, .. }
            | Self::PreviewCandidate { run_id, .. }
            | Self::PreviewUpdated { run_id, .. }
            | Self::ReviewFinding { run_id, .. }
            | Self::RunCompleted { run_id, .. } => run_id,
        }
    }

    pub fn is_run_completed(&self) -> bool {
        matches!(self, Self::RunCompleted { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub tool: String,
    pub input_summary: String,
    pub decision: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPermission {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub tool: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTask {
    pub id: String,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointConversationRange {
    pub start_index: u64,
    pub end_index_exclusive: u64,
    pub retained_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointBuildResult {
    pub version_id: String,
    pub status: ProjectVersionStatus,
    pub preview_url: String,
    pub source_snapshot_uri: Option<String>,
    pub screenshot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCheckpoint {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub phase: AgentPhase,
    pub message_window: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_range: Option<CheckpointConversationRange>,
    pub task_list: Vec<AgentTask>,
    pub workspace_snapshot_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_result: Option<CheckpointBuildResult>,
    pub brief_version: Option<String>,
    pub design_version: Option<String>,
    pub last_known_preview_url: Option<String>,
    pub context_summary: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectVersion {
    pub id: String,
    pub project_id: String,
    pub source_snapshot_uri: Option<String>,
    pub preview_url: String,
    pub screenshot_uri: Option<String>,
    pub screenshot_id: Option<String>,
    pub status: ProjectVersionStatus,
    pub created_by_run_id: String,
    pub created_at: DateTime<Utc>,
    pub promoted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxBinding {
    pub id: String,
    pub project_id: String,
    pub sandbox_name: String,
    pub sandbox_claim_name: String,
    pub workspace_pvc_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_service_name: Option<String>,
    pub warm_pool_name: String,
    pub namespace: String,
    pub status: SandboxBindingStatus,
    pub channel_protocol: SandboxChannelProtocol,
    pub last_seen_at: DateTime<Utc>,
}
