use crate::templates::TemplateId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const RUNTIME_TOKEN_MAPPING_KEYS: [&str; 12] = [
    "color.background",
    "color.surface",
    "color.surfaceStrong",
    "color.text",
    "color.muted",
    "color.primary",
    "color.primaryContrast",
    "color.border",
    "radius.card",
    "radius.control",
    "font.sans",
    "shadow.soft",
];
pub const MAX_DESIGN_SOURCE_BYTES: usize = 256 * 1024;
pub const DESIGN_PROFILE_SCHEMA_V1: &str = "design-profile@1";
pub const DESIGN_PROFILE_SCHEMA_V2: &str = "design-profile@2";

fn default_design_profile_schema_version() -> String {
    DESIGN_PROFILE_SCHEMA_V1.to_string()
}

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
    Validating,
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
    #[serde(default)]
    pub design_profile_id: Option<String>,
    #[serde(default)]
    pub design_profile_version: Option<u32>,
    #[serde(default)]
    pub design_profile_hash: Option<String>,
    #[serde(default)]
    pub design_profile_surface: Option<String>,
    #[serde(default)]
    pub design_profile_template: Option<String>,
    #[serde(default)]
    pub design_profile_surface_override_hash: Option<String>,
    #[serde(default)]
    pub design_profile_template_override_hash: Option<String>,
    #[serde(default)]
    pub design_profile_effective_hash: Option<String>,
    #[serde(default)]
    pub design_profile_unsupported_extended_tokens: Vec<String>,
    #[serde(default)]
    pub design_profile_blocking_capability_rule_ids: Vec<String>,
    #[serde(default)]
    pub design_fidelity_mode: Option<String>,
    #[serde(default)]
    pub design_source_artifact_id: Option<String>,
    #[serde(default)]
    pub design_source_hash: Option<String>,
    #[serde(default)]
    pub design_source_size_bytes: Option<u64>,
    #[serde(default)]
    pub design_source_budget_bytes: Option<u64>,
    #[serde(default)]
    pub design_source_bytes_read: u64,
    #[serde(default)]
    pub design_source_sections: Vec<DesignSourceIndexSection>,
    #[serde(default)]
    pub design_source_required_section_ids: Vec<String>,
    #[serde(default)]
    pub design_source_read_section_hashes: Vec<String>,
    #[serde(default)]
    pub design_context_read_files: Vec<String>,
    pub base_version_id: Option<String>,
    pub output_version_id: Option<String>,
    pub finding_ids: Option<Vec<String>>,
    pub input_message_ids: Vec<String>,
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub project_state_snapshot: Option<ProjectRuntimeState>,
    pub profile_snapshot: RunProfileSnapshot,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuntimeState {
    pub project_id: String,
    pub revision: u64,
    pub app_root: String,
    pub template_key: String,
    pub template_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_manifest_sha256: Option<String>,
    pub framework: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_execution_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_execution_profile_version: Option<String>,
    pub package_manager: String,
    pub lockfile: String,
    pub registry: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAccessRecord {
    pub project_id: String,
    pub owner_principal_id: String,
    pub workspace_id: Option<String>,
    pub organization_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreviewLeaseStatus {
    Active,
    Stopped,
    Expired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPublishStatus {
    Staging,
    Staged,
    Validating,
    Promoting,
    Promoted,
    Failed,
    GarbageCollectable,
    GarbageCollected,
    ReconcileRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPublishRecord {
    pub id: String,
    pub idempotency_key: String,
    pub project_id: String,
    pub run_id: String,
    pub build_id: String,
    pub version_id: String,
    pub sandbox_binding_id: Option<String>,
    pub pod_uid: Option<String>,
    pub candidate_manifest_hash: String,
    pub artifact_manifest_hash: Option<String>,
    pub source_snapshot_uri: String,
    pub expected_current_version_id: Option<String>,
    pub status: ArtifactPublishStatus,
    pub revision: u64,
    pub staged_uri: Option<String>,
    pub immutable_artifact_uri: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub gc_after: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboxDeliveryStatus {
    Pending,
    Delivered,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeOutboxEvent {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub event: AgentEvent,
    pub status: OutboxDeliveryStatus,
    pub delivery_attempts: u32,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewLeaseRecord {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub sandbox_binding_id: String,
    pub sandbox_name: String,
    pub pod_uid: String,
    pub build_id: String,
    pub candidate_manifest_hash: String,
    pub status: PreviewLeaseStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLeaseTransport {
    PortForward,
    ServiceDns,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelLeaseStatus {
    Acquiring,
    Ready,
    Stale,
    Releasing,
    Released,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelLeaseRecord {
    pub id: String,
    pub owner_runtime_epoch: String,
    pub sandbox_binding_id: String,
    pub sandbox_uid: Option<String>,
    pub pod_uid: String,
    pub project_id: String,
    pub run_id: String,
    pub transport: ChannelLeaseTransport,
    pub target_port: u16,
    pub local_port: Option<u16>,
    pub service_endpoint: Option<String>,
    pub child_pid: Option<u32>,
    pub child_started_at: Option<String>,
    pub status: ChannelLeaseStatus,
    pub created_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfile {
    pub id: String,
    #[serde(default = "default_design_profile_schema_version")]
    pub schema_version: String,
    pub name: String,
    pub status: String,
    pub version: u32,
    pub scope: Value,
    pub source: Value,
    pub product: Value,
    pub brand: Value,
    pub visual: Value,
    pub tokens: Value,
    pub runtime_token_mapping: Value,
    #[serde(default)]
    pub extended_token_mapping: Value,
    pub components: Value,
    pub content: Value,
    pub accessibility: Value,
    pub technical: Value,
    pub governance: Value,
    #[serde(default)]
    pub signature_rules: Vec<Value>,
    #[serde(default)]
    pub overrides: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileValidationIssue {
    pub path: String,
    pub code: String,
    pub message: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileDraft {
    pub id: String,
    pub schema_version: String,
    pub version: u32,
    pub name: String,
    pub status: String,
    pub scope: Value,
    pub source: Value,
    pub candidate: Value,
    pub conversion_report_id: String,
    pub validation_issues: Vec<DesignProfileValidationIssue>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileUnmappedItem {
    pub source_section: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub excerpt: String,
    pub excerpt_hash: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileConversionReport {
    pub id: String,
    pub design_profile_id: String,
    pub profile_version: u32,
    pub converter_version: String,
    pub deterministic_parser_version: String,
    pub source_artifact_id: String,
    pub source_hash: String,
    pub extracted_sections: Vec<String>,
    pub extracted_token_count: usize,
    pub extracted_component_count: usize,
    pub required_signature_rule_count: usize,
    pub unmapped_items: Vec<DesignProfileUnmappedItem>,
    pub warnings: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveDesignProfile {
    pub design_profile_id: String,
    pub version: u32,
    pub surface: String,
    pub template: String,
    pub base_profile_hash: String,
    pub surface_override_hash: Option<String>,
    pub template_override_hash: Option<String>,
    pub effective_profile_hash: String,
    pub profile: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignProfileFidelityReport {
    pub design_profile_id: String,
    pub version: u32,
    pub schema_version: String,
    pub surface: String,
    pub template: String,
    pub style_contract_version: String,
    pub effective_profile_hash: String,
    pub source_integrity: String,
    pub source_hash_matches: Option<bool>,
    pub required_signature_rule_ids: Vec<String>,
    pub capsule_included_rule_ids: Vec<String>,
    pub capsule_missing_rule_ids: Vec<String>,
    pub unsupported_extended_tokens: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DesignSourceArtifact {
    pub id: String,
    pub scope: Value,
    pub file_name: String,
    pub media_type: String,
    pub content_encoding: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DesignSourceIndexSection {
    pub id: String,
    pub heading: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub sha256: String,
    pub required_by_rule_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DesignSourceIndex {
    pub source_artifact_id: String,
    pub source_hash: String,
    pub size_bytes: u64,
    pub profile_hash: String,
    pub capsule_hash: String,
    pub sections: Vec<DesignSourceIndexSection>,
}

impl DesignSourceArtifact {
    pub fn validate_for_runtime(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("id is required".to_string());
        }
        validate_design_source_scope(&self.scope)?;
        validate_design_source_file_name(&self.file_name)?;
        if !matches!(self.media_type.as_str(), "text/markdown" | "text/plain") {
            return Err("mediaType must be text/markdown or text/plain".to_string());
        }
        if self.content_encoding != "identity" {
            return Err("contentEncoding must be identity".to_string());
        }
        if self.size_bytes == 0 {
            return Err("sizeBytes must be positive".to_string());
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("sha256 must be a 64-character hexadecimal digest".to_string());
        }
        Ok(())
    }
}

impl DesignProfile {
    pub fn validate_for_runtime(&self) -> Result<(), String> {
        if !matches!(
            self.schema_version.as_str(),
            DESIGN_PROFILE_SCHEMA_V1 | DESIGN_PROFILE_SCHEMA_V2
        ) {
            return Err("schemaVersion must be design-profile@1 or design-profile@2".to_string());
        }
        if self.name.trim().is_empty() {
            return Err("name is required".to_string());
        }
        if !matches!(self.status.as_str(), "draft" | "active" | "archived") {
            return Err("status must be draft, active, or archived".to_string());
        }
        if self.version == 0 {
            return Err("version must be positive".to_string());
        }
        if !object_string(&self.scope, "projectId").is_some()
            && !object_string(&self.scope, "workspaceId").is_some()
            && !object_string(&self.scope, "organizationId").is_some()
        {
            return Err("scope requires projectId, workspaceId, or organizationId".to_string());
        }
        if object_string(&self.product, "name")
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            return Err("product.name is required".to_string());
        }
        if object_string(&self.product, "category")
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            return Err("product.category is required".to_string());
        }
        if object_string(&self.visual, "direction")
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            return Err("visual.direction is required".to_string());
        }
        validate_runtime_token_mapping(&self.runtime_token_mapping)?;
        let allowed_templates = self
            .technical
            .get("allowedTemplates")
            .and_then(Value::as_array)
            .ok_or_else(|| "technical.allowedTemplates is required".to_string())?;
        if allowed_templates.is_empty() {
            return Err("technical.allowedTemplates must not be empty".to_string());
        }
        if !allowed_templates.iter().all(|template| {
            template
                .as_str()
                .is_some_and(|template| TemplateId::parse(template).is_ok())
        }) {
            return Err("technical.allowedTemplates must contain valid template ids".to_string());
        }
        match object_string(&self.governance, "conflictBehavior") {
            Some("prefer-user" | "ask" | "block") => {}
            Some(_) => {
                return Err(
                    "governance.conflictBehavior must be prefer-user, ask, or block".to_string(),
                );
            }
            None => return Err("governance.conflictBehavior is required".to_string()),
        }
        if self.schema_version == DESIGN_PROFILE_SCHEMA_V2 {
            validate_design_profile_v2(self)?;
        }
        Ok(())
    }

    pub fn project_id(&self) -> Option<&str> {
        object_string(&self.scope, "projectId")
    }

    pub fn workspace_id(&self) -> Option<&str> {
        object_string(&self.scope, "workspaceId")
    }

    pub fn organization_id(&self) -> Option<&str> {
        object_string(&self.scope, "organizationId")
    }

    pub fn stable_hash(&self) -> String {
        canonical_json_hash(&serde_json::to_value(self).unwrap_or(Value::Null))
    }

    pub fn effective_for(
        &self,
        surface: &str,
        template: &str,
    ) -> Result<EffectiveDesignProfile, String> {
        if !matches!(surface, "website" | "docs") {
            return Err("surface must be website or docs".to_string());
        }
        let allowed = self
            .technical
            .get("allowedTemplates")
            .and_then(Value::as_array)
            .is_some_and(|templates| {
                templates
                    .iter()
                    .any(|value| value.as_str() == Some(template))
            });
        if !allowed {
            return Err(format!(
                "template {template} is not allowed by design profile"
            ));
        }

        let mut effective = serde_json::to_value(self).map_err(|error| error.to_string())?;
        let surface_override = self
            .overrides
            .get("surfaces")
            .and_then(|value| value.get(surface))
            .filter(|value| !value.is_null());
        let template_override = self
            .overrides
            .get("templates")
            .and_then(|value| value.get(template))
            .filter(|value| !value.is_null());
        if let Some(value) = surface_override {
            merge_design_profile_value(&mut effective, value, "")?;
        }
        if let Some(value) = template_override {
            merge_design_profile_value(&mut effective, value, "")?;
        }
        let base_profile_hash = self.stable_hash();
        Ok(EffectiveDesignProfile {
            design_profile_id: self.id.clone(),
            version: self.version,
            surface: surface.to_string(),
            template: template.to_string(),
            base_profile_hash,
            surface_override_hash: surface_override.map(canonical_json_hash),
            template_override_hash: template_override.map(canonical_json_hash),
            effective_profile_hash: canonical_json_hash(&effective),
            profile: effective,
        })
    }
}

pub fn canonical_json_hash(value: &Value) -> String {
    let canonical = canonical_json_value(value);
    sha256_hex(&serde_json::to_vec(&canonical).unwrap_or_default())
}

fn canonical_json_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json_value(&object[key]));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical_json_value).collect()),
        _ => value.clone(),
    }
}

fn merge_design_profile_value(
    target: &mut Value,
    override_value: &Value,
    path: &str,
) -> Result<(), String> {
    if override_value.is_null() {
        return Err(format!("design profile override cannot set {path} to null"));
    }
    match (target, override_value) {
        (Value::Object(target), Value::Object(override_object)) => {
            for (key, value) in override_object {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if key == "signatureRules" {
                    let target_rules = target
                        .entry(key.clone())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    merge_signature_rules(target_rules, value, &child_path)?;
                } else if let Some(target_value) = target.get_mut(key) {
                    merge_design_profile_value(target_value, value, &child_path)?;
                } else {
                    target.insert(key.clone(), value.clone());
                }
            }
            Ok(())
        }
        (target, value) => {
            *target = value.clone();
            Ok(())
        }
    }
}

fn merge_signature_rules(
    target: &mut Value,
    override_value: &Value,
    path: &str,
) -> Result<(), String> {
    let target_rules = target
        .as_array_mut()
        .ok_or_else(|| format!("{path} must be an array"))?;
    let override_rules = override_value
        .as_array()
        .ok_or_else(|| format!("{path} must be an array"))?;
    for rule in override_rules {
        let id = rule
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{path} rule id is required"))?;
        if let Some(index) = target_rules
            .iter()
            .position(|candidate| candidate.get("id").and_then(Value::as_str) == Some(id))
        {
            if target_rules[index].get("priority").and_then(Value::as_str) == Some("required")
                && rule.get("priority").and_then(Value::as_str) != Some("required")
            {
                return Err(format!(
                    "{path} cannot downgrade required rule {id} without an explicit conflict decision"
                ));
            }
            target_rules[index] = rule.clone();
        } else {
            target_rules.push(rule.clone());
        }
    }
    Ok(())
}

fn validate_design_profile_v2(profile: &DesignProfile) -> Result<(), String> {
    if profile.signature_rules.len() > 64 {
        return Err("signatureRules must contain at most 64 rules".to_string());
    }
    let mut required_count = 0usize;
    let mut ids = std::collections::HashSet::new();
    for (index, rule) in profile.signature_rules.iter().enumerate() {
        let object = rule
            .as_object()
            .ok_or_else(|| format!("signatureRules[{index}] must be an object"))?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("signatureRules[{index}].id is required"))?;
        if !ids.insert(id.to_string()) {
            return Err(format!("signatureRules contains duplicate id: {id}"));
        }
        let statement = object
            .get("statement")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("signatureRules[{index}].statement is required"))?;
        if statement.chars().count() > 240 {
            return Err(format!(
                "signatureRules[{index}].statement must be at most 240 characters"
            ));
        }
        match object.get("priority").and_then(Value::as_str) {
            Some("required") => required_count += 1,
            Some("preferred") => {}
            _ => {
                return Err(format!(
                    "signatureRules[{index}].priority must be required or preferred"
                ))
            }
        }
        validate_signature_rule_applies_to(index, object.get("appliesTo"))?;
        let verification = object
            .get("verification")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("signatureRules[{index}].verification is required"))?;
        validate_signature_verification(index, verification)?;
        if verification.get("kind").and_then(Value::as_str) == Some("visual-review") {
            let rubric = verification
                .get("rubric")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if rubric.is_empty() || rubric.chars().count() > 500 {
                return Err(format!(
                    "signatureRules[{index}].verification.rubric must be 1-500 characters"
                ));
            }
        }
    }
    if required_count > 24 {
        return Err("signatureRules must contain at most 24 required rules".to_string());
    }
    let required_capsule_chars = profile
        .signature_rules
        .iter()
        .filter(|rule| rule.get("priority").and_then(Value::as_str) == Some("required"))
        .map(design_signature_rule_capsule_line)
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .map(|line| line.chars().count() + 1)
        .sum::<usize>();
    if required_capsule_chars > 2_500 {
        return Err(
            "required signature rules exceed the 2500-character Design Capsule budget".to_string(),
        );
    }

    if object_string(&profile.source, "kind") == Some("imported") {
        if object_string(&profile.source, "primarySourceArtifactId").is_none() {
            return Err("imported source requires primarySourceArtifactId".to_string());
        }
        if object_string(&profile.source, "sourceHash").is_none() {
            return Err("imported source requires sourceHash".to_string());
        }
        if object_string(&profile.source, "converterVersion")
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            return Err("imported source requires converterVersion".to_string());
        }
        if object_string(&profile.source, "integrity") != Some("verified") {
            return Err("imported source integrity must be verified".to_string());
        }
        if required_count == 0 {
            return Err(
                "imported profile requires at least one required signature rule".to_string(),
            );
        }
    }
    Ok(())
}

fn validate_signature_verification(
    index: usize,
    verification: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let path = format!("signatureRules[{index}].verification");
    let kind = required_nonempty_string(verification, "kind", &path)?;
    match kind {
        "token" => {
            required_nonempty_string(verification, "token", &path)?;
            require_string_field(verification, "expected", &path)?;
            validate_value_comparator(verification.get("comparator"), &path, false)?;
        }
        "computed-style" => {
            validate_fidelity_route(required_nonempty_string(verification, "route", &path)?)?;
            required_nonempty_string(verification, "selector", &path)?;
            required_nonempty_string(verification, "property", &path)?;
            require_string_field(verification, "expected", &path)?;
            let comparator =
                validate_value_comparator(verification.get("comparator"), &path, true)?;
            if comparator == "numeric-ratio" {
                required_nonempty_string(verification, "referenceProperty", &path)?;
            }
            if let Some(min_matches) = verification.get("minMatches") {
                if min_matches.as_u64().is_none_or(|value| value == 0) {
                    return Err(format!("{path}.minMatches must be a positive integer"));
                }
            }
            if let Some(exclude_within) = verification.get("excludeWithin") {
                if exclude_within
                    .as_str()
                    .is_none_or(|value| value.trim().is_empty())
                {
                    return Err(format!("{path}.excludeWithin must be a non-empty string"));
                }
            }
            if let Some(match_policy) = verification.get("matchPolicy") {
                if !matches!(match_policy.as_str(), Some("all" | "any")) {
                    return Err(format!("{path}.matchPolicy must be all or any"));
                }
            }
        }
        "dom" => {
            validate_fidelity_route(required_nonempty_string(verification, "route", &path)?)?;
            required_nonempty_string(verification, "selector", &path)?;
            if verification
                .get("minMatches")
                .and_then(Value::as_u64)
                .is_none_or(|value| value == 0)
            {
                return Err(format!("{path}.minMatches must be a positive integer"));
            }
        }
        "source-pattern" => {
            let paths = verification
                .get("paths")
                .and_then(Value::as_array)
                .filter(|paths| {
                    !paths.is_empty()
                        && paths
                            .iter()
                            .all(|path| path.as_str().is_some_and(|value| !value.trim().is_empty()))
                })
                .ok_or_else(|| format!("{path}.paths must contain non-empty strings"))?;
            let _ = paths;
            let pattern = required_nonempty_string(verification, "pattern", &path)?;
            regex::Regex::new(pattern)
                .map_err(|error| format!("{path}.pattern is invalid: {error}"))?;
        }
        "visual-review" => {}
        _ => return Err(format!("{path}.kind is unsupported: {kind}")),
    }
    Ok(())
}

fn validate_value_comparator<'a>(
    value: Option<&'a Value>,
    path: &str,
    allow_numeric_ratio: bool,
) -> Result<&'a str, String> {
    let comparator = value
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{path}.comparator is required"))?;
    let kind = required_nonempty_string(comparator, "kind", &format!("{path}.comparator"))?;
    match kind {
        "exact" | "contains" | "color-equivalent" | "forbidden-anywhere" => {}
        "numeric-tolerance" => {
            if comparator
                .get("tolerance")
                .and_then(Value::as_f64)
                .is_none_or(|value| value < 0.0)
            {
                return Err(format!(
                    "{path}.comparator.tolerance must be a non-negative number"
                ));
            }
        }
        "numeric-ratio" if allow_numeric_ratio => {
            if comparator.get("ratio").and_then(Value::as_f64).is_none() {
                return Err(format!("{path}.comparator.ratio must be a number"));
            }
            if comparator
                .get("tolerance")
                .and_then(Value::as_f64)
                .is_none_or(|value| value < 0.0)
            {
                return Err(format!(
                    "{path}.comparator.tolerance must be a non-negative number"
                ));
            }
        }
        _ => return Err(format!("{path}.comparator.kind is unsupported: {kind}")),
    }
    Ok(kind)
}

fn required_nonempty_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    path: &str,
) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{path}.{field} must be a non-empty string"))
}

fn require_string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
    path: &str,
) -> Result<(), String> {
    if object.get(field).and_then(Value::as_str).is_none() {
        return Err(format!("{path}.{field} must be a string"));
    }
    Ok(())
}

fn validate_fidelity_route(route: &str) -> Result<(), String> {
    if !route.starts_with('/')
        || route.starts_with("//")
        || route.contains('\\')
        || route.chars().any(char::is_control)
    {
        return Err("signature rule route must be a root-relative path".to_string());
    }
    Ok(())
}

pub fn design_signature_rule_capsule_line(rule: &Value) -> Result<String, String> {
    let id = rule
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "signature rule id is required".to_string())?;
    let statement = rule
        .get("statement")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("signature rule {id} statement is required"))?;
    let applies_to = match rule.get("appliesTo") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(","),
        _ => "unspecified".to_string(),
    };
    let verification = rule
        .get("verification")
        .and_then(Value::as_object)
        .map(|verification| {
            let kind = verification
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let detail = ["token", "selector", "property", "pattern", "rubric"]
                .iter()
                .find_map(|key| {
                    verification
                        .get(*key)
                        .and_then(Value::as_str)
                        .map(|value| format!("{key}={value}"))
                })
                .unwrap_or_default();
            if detail.is_empty() {
                kind.to_string()
            } else {
                format!("{kind}:{detail}")
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    Ok(format!(
        "- [{id}] {statement} (appliesTo: {applies_to}; verify: {verification})"
    ))
}

fn validate_signature_rule_applies_to(index: usize, value: Option<&Value>) -> Result<(), String> {
    match value {
        Some(Value::String(value)) if value == "all" => Ok(()),
        Some(Value::Array(values)) if !values.is_empty() => {
            let mut seen = std::collections::HashSet::new();
            for value in values {
                let surface = value.as_str().ok_or_else(|| {
                    format!("signatureRules[{index}].appliesTo must contain strings")
                })?;
                if !matches!(surface, "website" | "docs") {
                    return Err(format!(
                        "signatureRules[{index}].appliesTo contains unsupported surface: {surface}"
                    ));
                }
                if !seen.insert(surface) {
                    return Err(format!(
                        "signatureRules[{index}].appliesTo contains duplicate surface: {surface}"
                    ));
                }
            }
            Ok(())
        }
        _ => Err(format!(
            "signatureRules[{index}].appliesTo must be all or a non-empty surface array"
        )),
    }
}

fn object_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

pub fn validate_design_source_scope(scope: &Value) -> Result<(), String> {
    let object = scope
        .as_object()
        .ok_or_else(|| "scope must be an object".to_string())?;
    let known = ["projectId", "workspaceId", "organizationId"];
    if object.keys().any(|key| !known.contains(&key.as_str())) {
        return Err("scope contains unsupported fields".to_string());
    }
    let mut populated = Vec::new();
    for key in known {
        let Some(value) = object.get(key) else {
            continue;
        };
        let value = value
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("scope.{key} must be a non-empty string"))?;
        populated.push(value);
    }
    if populated.len() != 1 {
        return Err(
            "scope requires exactly one of projectId, workspaceId, or organizationId".to_string(),
        );
    }
    Ok(())
}

pub fn validate_design_source_file_name(file_name: &str) -> Result<(), String> {
    let file_name = file_name.trim();
    if file_name.is_empty() {
        return Err("fileName is required".to_string());
    }
    if file_name.len() > 255 {
        return Err("fileName must be at most 255 bytes".to_string());
    }
    if matches!(file_name, "." | "..")
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.chars().any(char::is_control)
    {
        return Err("fileName must be a plain file name without path separators".to_string());
    }
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn validate_runtime_token_mapping(mapping: &Value) -> Result<(), String> {
    let object = mapping
        .as_object()
        .ok_or_else(|| "runtimeTokenMapping must be an object".to_string())?;
    for key in RUNTIME_TOKEN_MAPPING_KEYS {
        let value = object
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("runtimeTokenMapping.{key} is required"))?;
        validate_runtime_token_value(key, value)?;
    }
    Ok(())
}

fn validate_runtime_token_value(key: &str, value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("runtimeTokenMapping.{key} must not be empty"));
    }
    if trimmed.len() > 240
        || trimmed.contains(';')
        || trimmed.contains('{')
        || trimmed.contains('}')
        || trimmed.contains('\n')
        || trimmed.contains('\r')
    {
        return Err(format!(
            "runtimeTokenMapping.{key} contains an unsafe CSS value"
        ));
    }
    if key.starts_with("color.") && !is_safe_color_value(trimmed) {
        return Err(format!(
            "runtimeTokenMapping.{key} must be a safe color expression"
        ));
    }
    Ok(())
}

fn is_safe_color_value(value: &str) -> bool {
    value.starts_with('#')
        || value.starts_with("rgb(")
        || value.starts_with("rgba(")
        || value.starts_with("hsl(")
        || value.starts_with("hsla(")
        || value.starts_with("oklch(")
        || value.starts_with("color(")
        || value.starts_with("var(")
        || matches!(value, "transparent" | "currentColor" | "black" | "white")
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
        TemplateId::parse(&self.recommended_template)
            .map_err(|_| "recommendedTemplate must be a valid template id".to_string())?;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_uid: Option<String>,
    pub warm_pool_name: String,
    pub namespace: String,
    pub status: SandboxBindingStatus,
    pub channel_protocol: SandboxChannelProtocol,
    pub last_seen_at: DateTime<Utc>,
}

#[cfg(test)]
mod design_profile_effective_tests {
    use super::*;
    use serde_json::json;

    fn profile_with_overrides() -> DesignProfile {
        serde_json::from_value(json!({
            "id": "design-profile-1",
            "schemaVersion": "design-profile@2",
            "name": "Profile",
            "status": "active",
            "version": 4,
            "scope": { "projectId": "project-1" },
            "source": { "kind": "manual" },
            "product": {},
            "brand": {},
            "visual": {},
            "tokens": { "color": { "primary": "#111111" } },
            "runtimeTokenMapping": {},
            "components": {},
            "content": {},
            "accessibility": {},
            "technical": { "allowedTemplates": ["astro-website", "fumadocs-docs"] },
            "governance": {},
            "signatureRules": [{
                "id": "primary",
                "priority": "required",
                "statement": "Base rule"
            }],
            "overrides": {
                "surfaces": {
                    "website": {
                        "tokens": { "color": { "primary": "#663af3" } },
                        "signatureRules": [{
                            "id": "primary",
                            "priority": "required",
                            "statement": "Website rule"
                        }]
                    }
                },
                "templates": {
                    "astro-website": {
                        "tokens": { "color": { "surface": "#05060f" } }
                    }
                }
            },
            "createdAt": "2026-07-10T00:00:00Z",
            "updatedAt": "2026-07-10T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn effective_profile_merges_surface_then_template_and_hashes_canonically() {
        let profile = profile_with_overrides();
        let effective = profile.effective_for("website", "astro-website").unwrap();
        assert_eq!(effective.profile["tokens"]["color"]["primary"], "#663af3");
        assert_eq!(effective.profile["tokens"]["color"]["surface"], "#05060f");
        assert_eq!(
            effective.profile["signatureRules"][0]["statement"],
            "Website rule"
        );
        assert!(effective.surface_override_hash.is_some());
        assert!(effective.template_override_hash.is_some());
        assert_eq!(
            effective.effective_profile_hash,
            profile
                .effective_for("website", "astro-website")
                .unwrap()
                .effective_profile_hash
        );
    }

    #[test]
    fn effective_profile_rejects_required_rule_downgrade() {
        let mut profile = profile_with_overrides();
        profile.overrides["surfaces"]["website"]["signatureRules"][0]["priority"] =
            json!("preferred");
        assert!(profile
            .effective_for("website", "astro-website")
            .unwrap_err()
            .contains("cannot downgrade required rule"));
    }

    #[test]
    fn authkit_and_elevenlabs_v2_fixtures_are_strict_active_profiles() {
        for fixture in [
            include_str!("../fixtures/design-profiles/authkit-v2.json"),
            include_str!("../fixtures/design-profiles/elevenlabs-v2.json"),
        ] {
            let profile: DesignProfile = serde_json::from_str(fixture).unwrap();
            profile.validate_for_runtime().unwrap();
            assert_eq!(profile.schema_version, DESIGN_PROFILE_SCHEMA_V2);
            assert_eq!(profile.status, "active");
            assert!(
                profile
                    .signature_rules
                    .iter()
                    .filter(|rule| {
                        rule.get("priority").and_then(Value::as_str) == Some("required")
                    })
                    .count()
                    >= 8
            );
            assert!(profile.effective_for("website", "astro-website").is_ok());
        }
    }

    #[test]
    fn v2_computed_style_numeric_ratio_requires_reference_property() {
        let mut profile: DesignProfile =
            serde_json::from_str(include_str!("../fixtures/design-profiles/authkit-v2.json"))
                .unwrap();
        profile.signature_rules[0]["verification"] = serde_json::json!({
            "kind": "computed-style",
            "route": "/",
            "selector": "[data-eyebrow]",
            "property": "letter-spacing",
            "expected": "0.10",
            "comparator": {
                "kind": "numeric-ratio",
                "ratio": 0.10,
                "tolerance": 0.01
            }
        });

        let error = profile.validate_for_runtime().unwrap_err();
        assert!(error.contains("referenceProperty"), "{error}");
    }

    #[test]
    fn v2_fidelity_assertions_reject_external_routes() {
        let mut profile: DesignProfile =
            serde_json::from_str(include_str!("../fixtures/design-profiles/authkit-v2.json"))
                .unwrap();
        profile.signature_rules[0]["verification"] = serde_json::json!({
            "kind": "computed-style",
            "route": "https://example.com",
            "selector": "h1",
            "property": "color",
            "expected": "#ffffff",
            "comparator": { "kind": "color-equivalent" }
        });

        let error = profile.validate_for_runtime().unwrap_err();
        assert!(error.contains("root-relative"), "{error}");
    }
}
