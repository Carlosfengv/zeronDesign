use crate::{
    profiles::policy,
    repair_loop::RepairAttempt,
    types::{
        sha256_hex, AgentCheckpoint, AgentEvent, AgentPhase, AgentRun, AgentRunStatus,
        ArtifactPublishRecord, ArtifactPublishStatus, AuditRecord, Brief, BriefStatus,
        ChannelLeaseRecord, ChannelLeaseStatus, ContentSource, ConversationItem, DesignProfile,
        DesignProfileConversionReport, DesignProfileDraft, DesignSourceArtifact, DesignSourceIndex,
        OutboxDeliveryStatus, PendingPermission, PreviewLeaseRecord, PreviewLeaseStatus,
        ProjectAccessRecord, ProjectRuntimeState, ProjectVersion, ProjectVersionStatus,
        ReviewFinding, ReviewFindingCategory, ReviewFindingEvidence, ReviewFindingSeverity,
        ReviewFindingStatus, RuntimeOutboxEvent, SandboxBinding, SandboxBindingStatus,
        SandboxChannelProtocol, MAX_DESIGN_SOURCE_BYTES,
    },
};
use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::{broadcast, RwLock};

static DEFAULT_STORAGE_ROOT_COUNTER: AtomicU64 = AtomicU64::new(1);
const RUN_EVENT_BROADCAST_CAPACITY: usize = 512;

#[derive(Debug, Clone)]
pub struct SequencedAgentEvent {
    pub sequence: usize,
    pub event: AgentEvent,
}

#[derive(Debug, Clone)]
pub struct RuntimeStore {
    inner: Arc<RwLock<RuntimeStoreInner>>,
    ids: Arc<AtomicU64>,
    checkpoint_dir: Arc<PathBuf>,
    run_log_dir: Arc<PathBuf>,
    run_state_log_path: Arc<PathBuf>,
    brief_log_path: Arc<PathBuf>,
    conversation_log_dir: Arc<PathBuf>,
    content_source_log_path: Arc<PathBuf>,
    project_version_log_path: Arc<PathBuf>,
    project_runtime_state_log_path: Arc<PathBuf>,
    project_access_log_path: Arc<PathBuf>,
    preview_lease_log_path: Arc<PathBuf>,
    channel_lease_log_path: Arc<PathBuf>,
    artifact_publish_log_path: Arc<PathBuf>,
    promotion_commit_log_path: Arc<PathBuf>,
    outbox_log_path: Arc<PathBuf>,
    review_finding_log_path: Arc<PathBuf>,
    repair_attempt_log_path: Arc<PathBuf>,
    pending_permission_log_path: Arc<PathBuf>,
    sandbox_binding_log_path: Arc<PathBuf>,
    design_profile_log_path: Arc<PathBuf>,
    design_profile_draft_log_path: Arc<PathBuf>,
    design_profile_conversion_report_log_path: Arc<PathBuf>,
    project_design_profile_log_path: Arc<PathBuf>,
    design_source_artifact_log_path: Arc<PathBuf>,
    design_source_blob_dir: Arc<PathBuf>,
    audit_log_path: Arc<PathBuf>,
}

#[derive(Debug, Default)]
struct RuntimeStoreInner {
    runs: HashMap<String, AgentRun>,
    events: HashMap<String, Vec<AgentEvent>>,
    event_broadcasters: HashMap<String, broadcast::Sender<SequencedAgentEvent>>,
    conversation_items: HashMap<String, Vec<ConversationItem>>,
    content_sources: HashMap<String, Vec<ContentSource>>,
    briefs: HashMap<String, Brief>,
    brief_statuses: HashMap<String, BriefStatus>,
    brief_run_ids: HashMap<String, String>,
    brief_content_sources: HashMap<String, Vec<ContentSource>>,
    audit_records: Vec<AuditRecord>,
    pending_permissions: HashMap<String, PendingPermission>,
    review_findings: HashMap<String, ReviewFinding>,
    project_review_findings: HashMap<String, Vec<String>>,
    repair_attempts: Vec<RepairAttempt>,
    checkpoints: HashMap<String, AgentCheckpoint>,
    run_checkpoints: HashMap<String, Vec<String>>,
    project_versions: HashMap<String, ProjectVersion>,
    project_current_versions: HashMap<String, String>,
    project_runtime_states: HashMap<String, ProjectRuntimeState>,
    project_access_records: HashMap<String, ProjectAccessRecord>,
    preview_leases: HashMap<String, PreviewLeaseRecord>,
    channel_leases: HashMap<String, ChannelLeaseRecord>,
    artifact_publishes: HashMap<String, ArtifactPublishRecord>,
    outbox_events: HashMap<String, RuntimeOutboxEvent>,
    sandbox_bindings: HashMap<String, SandboxBinding>,
    design_profiles: HashMap<String, DesignProfile>,
    design_profile_drafts: HashMap<String, DesignProfileDraft>,
    design_profile_conversion_reports: HashMap<String, DesignProfileConversionReport>,
    design_source_artifacts: HashMap<String, DesignSourceArtifact>,
    project_design_profiles: HashMap<String, String>,
    run_scoped_resources: HashMap<String, RunScopedResources>,
    continue_interrupt_requests: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunScopedResourceKind {
    McpServer,
    BackgroundShellTask,
    TemporaryHook,
    ReadFileCache,
    SandboxLock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunContentSourcesSnapshot {
    run_id: String,
    project_id: String,
    sources: Vec<ContentSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BriefSnapshot {
    brief_id: String,
    run_id: String,
    project_id: String,
    status: BriefStatus,
    brief: Brief,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactPromotionCommit {
    id: String,
    project_id: String,
    run_id: String,
    version: ProjectVersion,
    run: AgentRun,
    publish: ArtifactPublishRecord,
    outbox: RuntimeOutboxEvent,
    committed_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDesignProfileSnapshot {
    project_id: String,
    design_profile_id: String,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunScopedResources {
    pub mcp_servers: Vec<String>,
    pub background_shell_tasks: Vec<String>,
    pub temporary_hooks: Vec<String>,
    pub read_file_cache_entries: Vec<String>,
    pub sandbox_locks: Vec<String>,
}

impl RunScopedResources {
    pub fn is_empty(&self) -> bool {
        self.mcp_servers.is_empty()
            && self.background_shell_tasks.is_empty()
            && self.temporary_hooks.is_empty()
            && self.read_file_cache_entries.is_empty()
            && self.sandbox_locks.is_empty()
    }
}

fn repair_finding_status_for_run_status(status: AgentRunStatus) -> Option<ReviewFindingStatus> {
    match status {
        AgentRunStatus::Completed => Some(ReviewFindingStatus::Fixed),
        AgentRunStatus::NeedsUserInput
        | AgentRunStatus::Partial
        | AgentRunStatus::Blocked
        | AgentRunStatus::Failed => Some(ReviewFindingStatus::NeedsUserInput),
        AgentRunStatus::Queued
        | AgentRunStatus::Running
        | AgentRunStatus::Validating
        | AgentRunStatus::Cancelled => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeLevel {
    Workspace,
    Organization,
}

fn active_profiles_for_scope<'a>(
    profiles: impl Iterator<Item = &'a DesignProfile>,
    level: ScopeLevel,
    id: &str,
) -> Vec<DesignProfile> {
    profiles
        .filter(|profile| profile.status == "active")
        .filter(|profile| match level {
            ScopeLevel::Workspace => profile.workspace_id() == Some(id),
            ScopeLevel::Organization => profile.organization_id() == Some(id),
        })
        .cloned()
        .collect()
}

fn conversion_report_key(design_profile_id: &str, version: u32) -> String {
    format!("{design_profile_id}@{version}")
}

fn design_profile_capability_gaps(
    effective: &crate::types::EffectiveDesignProfile,
) -> (Vec<String>, Vec<String>) {
    let supported = match effective.template.as_str() {
        "astro-website" | "nextjs-website" => [
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
        ]
        .as_slice(),
        "fumadocs-docs" | "docusaurus-docs" => [
            "font.display",
            "font.mono",
            "type.display.letterSpacing",
            "type.body.letterSpacing",
            "spacing.pageGutter",
            "spacing.section",
            "radius.input",
            "radius.badge",
            "gradient.display",
        ]
        .as_slice(),
        _ => &[],
    };
    let mut unsupported = effective
        .profile
        .get("extendedTokenMapping")
        .and_then(Value::as_object)
        .map(|tokens| {
            tokens
                .keys()
                .filter(|token| !supported.contains(&token.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    unsupported.sort();
    let mut blocking_rules = effective
        .profile
        .get("signatureRules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|rule| rule.get("priority").and_then(Value::as_str) == Some("required"))
        .filter(|rule| {
            rule.get("verification")
                .and_then(|verification| verification.get("token"))
                .and_then(Value::as_str)
                .is_some_and(|token| unsupported.iter().any(|unsupported| unsupported == token))
        })
        .filter_map(|rule| {
            rule.get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    blocking_rules.sort();
    (unsupported, blocking_rules)
}

fn ensure_unique_project_active_profile(
    inner: &RuntimeStoreInner,
    profile: &DesignProfile,
) -> Result<()> {
    if profile.status != "active" {
        return Ok(());
    }
    let Some(project_id) = profile.project_id() else {
        return Ok(());
    };
    if let Some(existing) = inner.design_profiles.values().find(|candidate| {
        candidate.id != profile.id
            && candidate.status == "active"
            && candidate.project_id() == Some(project_id)
    }) {
        return Err(anyhow!(
            "project {project_id} already has active design profile {}; archive it before activating {}",
            existing.id,
            profile.id
        ));
    }
    Ok(())
}

fn profile_visible_to_context(
    profile: &DesignProfile,
    project_id: &str,
    workspace_id: Option<&str>,
    organization_id: Option<&str>,
) -> bool {
    if let Some(scope_project_id) = profile.project_id() {
        if scope_project_id != project_id {
            return false;
        }
    }
    if let Some(scope_workspace_id) = profile.workspace_id() {
        if Some(scope_workspace_id) != workspace_id {
            return false;
        }
    }
    if let Some(scope_organization_id) = profile.organization_id() {
        if Some(scope_organization_id) != organization_id {
            return false;
        }
    }
    true
}

impl Default for RuntimeStore {
    fn default() -> Self {
        let storage_root = default_storage_root();
        let next_id = initial_id_counter(&[&storage_root]);
        Self {
            inner: Arc::new(RwLock::new(RuntimeStoreInner::default())),
            ids: Arc::new(AtomicU64::new(next_id)),
            checkpoint_dir: Arc::new(storage_root.join("checkpoints")),
            run_log_dir: Arc::new(storage_root.join("run-logs")),
            run_state_log_path: Arc::new(storage_root.join("runs.jsonl")),
            brief_log_path: Arc::new(storage_root.join("briefs.jsonl")),
            conversation_log_dir: Arc::new(storage_root.join("conversation-items")),
            content_source_log_path: Arc::new(storage_root.join("content-sources.jsonl")),
            project_version_log_path: Arc::new(storage_root.join("project-versions.jsonl")),
            project_runtime_state_log_path: Arc::new(
                storage_root.join("project-runtime-states.jsonl"),
            ),
            project_access_log_path: Arc::new(storage_root.join("project-access.jsonl")),
            preview_lease_log_path: Arc::new(storage_root.join("preview-leases.jsonl")),
            channel_lease_log_path: Arc::new(storage_root.join("channel-leases.jsonl")),
            artifact_publish_log_path: Arc::new(storage_root.join("artifact-publishes.jsonl")),
            promotion_commit_log_path: Arc::new(storage_root.join("promotion-commits.jsonl")),
            outbox_log_path: Arc::new(storage_root.join("event-outbox.jsonl")),
            review_finding_log_path: Arc::new(storage_root.join("review-findings.jsonl")),
            repair_attempt_log_path: Arc::new(storage_root.join("repair-attempts.jsonl")),
            pending_permission_log_path: Arc::new(storage_root.join("pending-permissions.jsonl")),
            sandbox_binding_log_path: Arc::new(storage_root.join("sandbox-bindings.jsonl")),
            design_profile_log_path: Arc::new(storage_root.join("design-profiles.jsonl")),
            design_profile_draft_log_path: Arc::new(
                storage_root.join("design-profile-drafts.jsonl"),
            ),
            design_profile_conversion_report_log_path: Arc::new(
                storage_root.join("design-profile-conversion-reports.jsonl"),
            ),
            project_design_profile_log_path: Arc::new(
                storage_root.join("project-design-profiles.jsonl"),
            ),
            design_source_artifact_log_path: Arc::new(
                storage_root.join("design-source-artifacts.jsonl"),
            ),
            design_source_blob_dir: Arc::new(storage_root.join("design-source-artifacts")),
            audit_log_path: Arc::new(storage_root.join("audit-log.jsonl")),
        }
    }
}

impl RuntimeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_checkpoint_dir(checkpoint_dir: impl Into<PathBuf>) -> Self {
        let checkpoint_dir = checkpoint_dir.into();
        let next_id = initial_id_counter(&[&checkpoint_dir]);
        Self {
            inner: Arc::new(RwLock::new(RuntimeStoreInner::default())),
            ids: Arc::new(AtomicU64::new(next_id)),
            run_log_dir: Arc::new(checkpoint_dir.join("run-logs")),
            run_state_log_path: Arc::new(checkpoint_dir.join("runs.jsonl")),
            brief_log_path: Arc::new(checkpoint_dir.join("briefs.jsonl")),
            conversation_log_dir: Arc::new(checkpoint_dir.join("conversation-items")),
            content_source_log_path: Arc::new(checkpoint_dir.join("content-sources.jsonl")),
            project_version_log_path: Arc::new(checkpoint_dir.join("project-versions.jsonl")),
            project_runtime_state_log_path: Arc::new(
                checkpoint_dir.join("project-runtime-states.jsonl"),
            ),
            project_access_log_path: Arc::new(checkpoint_dir.join("project-access.jsonl")),
            preview_lease_log_path: Arc::new(checkpoint_dir.join("preview-leases.jsonl")),
            channel_lease_log_path: Arc::new(checkpoint_dir.join("channel-leases.jsonl")),
            artifact_publish_log_path: Arc::new(checkpoint_dir.join("artifact-publishes.jsonl")),
            promotion_commit_log_path: Arc::new(checkpoint_dir.join("promotion-commits.jsonl")),
            outbox_log_path: Arc::new(checkpoint_dir.join("event-outbox.jsonl")),
            review_finding_log_path: Arc::new(checkpoint_dir.join("review-findings.jsonl")),
            repair_attempt_log_path: Arc::new(checkpoint_dir.join("repair-attempts.jsonl")),
            pending_permission_log_path: Arc::new(checkpoint_dir.join("pending-permissions.jsonl")),
            sandbox_binding_log_path: Arc::new(checkpoint_dir.join("sandbox-bindings.jsonl")),
            design_profile_log_path: Arc::new(checkpoint_dir.join("design-profiles.jsonl")),
            design_profile_draft_log_path: Arc::new(
                checkpoint_dir.join("design-profile-drafts.jsonl"),
            ),
            design_profile_conversion_report_log_path: Arc::new(
                checkpoint_dir.join("design-profile-conversion-reports.jsonl"),
            ),
            project_design_profile_log_path: Arc::new(
                checkpoint_dir.join("project-design-profiles.jsonl"),
            ),
            design_source_artifact_log_path: Arc::new(
                checkpoint_dir.join("design-source-artifacts.jsonl"),
            ),
            design_source_blob_dir: Arc::new(checkpoint_dir.join("design-source-artifacts")),
            audit_log_path: Arc::new(checkpoint_dir.join("audit-log.jsonl")),
            checkpoint_dir: Arc::new(checkpoint_dir),
        }
    }

    pub fn with_storage_dirs(
        checkpoint_dir: impl Into<PathBuf>,
        run_log_dir: impl Into<PathBuf>,
    ) -> Self {
        let checkpoint_dir = checkpoint_dir.into();
        let run_log_dir = run_log_dir.into();
        let next_id = initial_id_counter(&[&checkpoint_dir, &run_log_dir]);
        Self {
            inner: Arc::new(RwLock::new(RuntimeStoreInner::default())),
            ids: Arc::new(AtomicU64::new(next_id)),
            checkpoint_dir: Arc::new(checkpoint_dir),
            audit_log_path: Arc::new(run_log_dir.join("audit-log.jsonl")),
            run_state_log_path: Arc::new(run_log_dir.join("runs.jsonl")),
            brief_log_path: Arc::new(run_log_dir.join("briefs.jsonl")),
            conversation_log_dir: Arc::new(run_log_dir.join("conversation-items")),
            content_source_log_path: Arc::new(run_log_dir.join("content-sources.jsonl")),
            project_version_log_path: Arc::new(run_log_dir.join("project-versions.jsonl")),
            project_runtime_state_log_path: Arc::new(
                run_log_dir.join("project-runtime-states.jsonl"),
            ),
            project_access_log_path: Arc::new(run_log_dir.join("project-access.jsonl")),
            preview_lease_log_path: Arc::new(run_log_dir.join("preview-leases.jsonl")),
            channel_lease_log_path: Arc::new(run_log_dir.join("channel-leases.jsonl")),
            artifact_publish_log_path: Arc::new(run_log_dir.join("artifact-publishes.jsonl")),
            promotion_commit_log_path: Arc::new(run_log_dir.join("promotion-commits.jsonl")),
            outbox_log_path: Arc::new(run_log_dir.join("event-outbox.jsonl")),
            review_finding_log_path: Arc::new(run_log_dir.join("review-findings.jsonl")),
            repair_attempt_log_path: Arc::new(run_log_dir.join("repair-attempts.jsonl")),
            pending_permission_log_path: Arc::new(run_log_dir.join("pending-permissions.jsonl")),
            sandbox_binding_log_path: Arc::new(run_log_dir.join("sandbox-bindings.jsonl")),
            design_profile_log_path: Arc::new(run_log_dir.join("design-profiles.jsonl")),
            design_profile_draft_log_path: Arc::new(
                run_log_dir.join("design-profile-drafts.jsonl"),
            ),
            design_profile_conversion_report_log_path: Arc::new(
                run_log_dir.join("design-profile-conversion-reports.jsonl"),
            ),
            project_design_profile_log_path: Arc::new(
                run_log_dir.join("project-design-profiles.jsonl"),
            ),
            design_source_artifact_log_path: Arc::new(
                run_log_dir.join("design-source-artifacts.jsonl"),
            ),
            design_source_blob_dir: Arc::new(run_log_dir.join("design-source-artifacts")),
            run_log_dir: Arc::new(run_log_dir),
        }
    }

    pub fn next_id(&self, prefix: &str) -> String {
        let id = self.ids.fetch_add(1, Ordering::SeqCst);
        format!("{prefix}-{id}")
    }

    pub async fn create_run(
        &self,
        project_id: String,
        phase: crate::types::AgentPhase,
        agent_profile: String,
        model: String,
        content_sources: Vec<ContentSource>,
    ) -> AgentRun {
        self.create_run_with_context(
            project_id,
            phase,
            agent_profile,
            model,
            content_sources,
            None,
            None,
        )
        .await
    }

    pub async fn create_run_with_context(
        &self,
        project_id: String,
        phase: crate::types::AgentPhase,
        agent_profile: String,
        model: String,
        content_sources: Vec<ContentSource>,
        brief_version: Option<String>,
        base_version_id: Option<String>,
    ) -> AgentRun {
        let now = Utc::now();
        let run_id = self.next_id("run");
        let project_state_snapshot = self.get_project_runtime_state(&project_id).await;
        let profile_snapshot = policy::snapshot_for_profile(phase, &agent_profile, None);
        let run = AgentRun {
            id: run_id.clone(),
            project_id: project_id.clone(),
            session_id: self.next_id("session"),
            parent_run_id: None,
            triggered_by_event_id: None,
            phase,
            agent_profile,
            status: AgentRunStatus::Queued,
            model,
            sandbox_id: None,
            brief_version,
            design_version: None,
            design_profile_id: None,
            design_profile_version: None,
            design_profile_hash: None,
            design_profile_surface: None,
            design_profile_template: None,
            design_profile_surface_override_hash: None,
            design_profile_template_override_hash: None,
            design_profile_effective_hash: None,
            design_profile_unsupported_extended_tokens: Vec::new(),
            design_profile_blocking_capability_rule_ids: Vec::new(),
            design_fidelity_mode: None,
            design_source_artifact_id: None,
            design_source_hash: None,
            design_source_size_bytes: None,
            design_source_budget_bytes: None,
            design_source_bytes_read: 0,
            design_source_sections: Vec::new(),
            design_source_required_section_ids: Vec::new(),
            design_source_read_section_hashes: Vec::new(),
            design_context_read_files: Vec::new(),
            base_version_id,
            output_version_id: None,
            finding_ids: None,
            input_message_ids: vec![self.next_id("message")],
            checkpoint_id: None,
            project_state_snapshot,
            profile_snapshot,
            started_at: now,
            updated_at: now,
            completed_at: None,
        };

        let mut inner = self.inner.write().await;
        inner.runs.insert(run_id.clone(), run.clone());
        inner.events.insert(run_id.clone(), Vec::new());
        let content_source_snapshot = RunContentSourcesSnapshot {
            run_id: run_id.clone(),
            project_id,
            sources: content_sources.clone(),
        };
        inner.content_sources.insert(run_id, content_sources);
        drop(inner);
        if let Err(error) = self.append_run_snapshot(&run) {
            eprintln!("failed to append run snapshot {}: {error}", run.id);
        }
        if let Err(error) = self.append_content_source_snapshot(&content_source_snapshot) {
            eprintln!(
                "failed to append content source snapshot {}: {error}",
                run.id
            );
        }
        run
    }

    pub async fn create_child_run(
        &self,
        parent_run_id: &str,
        phase: crate::types::AgentPhase,
        agent_profile: String,
        model: String,
        triggered_by_event_id: Option<String>,
        finding_ids: Vec<String>,
    ) -> Result<AgentRun> {
        let now = Utc::now();
        let mut inner = self.inner.write().await;
        if !inner.runs.contains_key(parent_run_id) {
            if let Some(run) = self.read_run(parent_run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let parent = inner
            .runs
            .get(parent_run_id)
            .ok_or_else(|| anyhow!("parent run not found: {parent_run_id}"))?
            .clone();
        let run_id = self.next_id("run");
        let profile_snapshot =
            policy::snapshot_for_profile(phase, &agent_profile, parent.checkpoint_id.clone());
        let run = AgentRun {
            id: run_id.clone(),
            project_id: parent.project_id.clone(),
            session_id: self.next_id("session"),
            parent_run_id: Some(parent.id.clone()),
            triggered_by_event_id,
            phase,
            agent_profile,
            status: AgentRunStatus::Queued,
            model,
            sandbox_id: parent.sandbox_id.clone(),
            brief_version: parent.brief_version.clone(),
            design_version: parent.design_version.clone(),
            design_profile_id: parent.design_profile_id.clone(),
            design_profile_version: parent.design_profile_version,
            design_profile_hash: parent.design_profile_hash.clone(),
            design_profile_surface: parent.design_profile_surface.clone(),
            design_profile_template: parent.design_profile_template.clone(),
            design_profile_surface_override_hash: parent
                .design_profile_surface_override_hash
                .clone(),
            design_profile_template_override_hash: parent
                .design_profile_template_override_hash
                .clone(),
            design_profile_effective_hash: parent.design_profile_effective_hash.clone(),
            design_profile_unsupported_extended_tokens: parent
                .design_profile_unsupported_extended_tokens
                .clone(),
            design_profile_blocking_capability_rule_ids: parent
                .design_profile_blocking_capability_rule_ids
                .clone(),
            design_fidelity_mode: parent.design_fidelity_mode.clone(),
            design_source_artifact_id: parent.design_source_artifact_id.clone(),
            design_source_hash: parent.design_source_hash.clone(),
            design_source_size_bytes: parent.design_source_size_bytes,
            design_source_budget_bytes: parent.design_source_budget_bytes,
            design_source_bytes_read: 0,
            design_source_sections: parent.design_source_sections.clone(),
            design_source_required_section_ids: parent.design_source_required_section_ids.clone(),
            design_source_read_section_hashes: Vec::new(),
            design_context_read_files: Vec::new(),
            base_version_id: parent
                .output_version_id
                .clone()
                .or(parent.base_version_id.clone()),
            output_version_id: None,
            finding_ids: if finding_ids.is_empty() {
                None
            } else {
                Some(finding_ids)
            },
            input_message_ids: vec![self.next_id("message")],
            checkpoint_id: parent.checkpoint_id.clone(),
            project_state_snapshot: parent.project_state_snapshot.clone(),
            profile_snapshot,
            started_at: now,
            updated_at: now,
            completed_at: None,
        };

        inner.runs.insert(run_id.clone(), run.clone());
        inner.events.insert(run_id, Vec::new());
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn create_repair_run_for_finding(
        &self,
        parent_run_id: &str,
        finding_id: &str,
        triggered_by_event_id: Option<String>,
    ) -> Result<AgentRun> {
        self.create_repair_run_for_findings(
            parent_run_id,
            &[finding_id.to_string()],
            triggered_by_event_id,
            "repair".to_string(),
            "internal-balanced".to_string(),
        )
        .await
    }

    pub async fn create_repair_run_for_findings(
        &self,
        parent_run_id: &str,
        finding_ids: &[String],
        triggered_by_event_id: Option<String>,
        agent_profile: String,
        model: String,
    ) -> Result<AgentRun> {
        if finding_ids.is_empty() {
            return Err(anyhow!("repair run requires at least one finding"));
        }
        {
            let mut inner = self.inner.write().await;
            self.hydrate_persisted_runs(&mut inner)?;
            self.hydrate_review_findings(&mut inner)?;
            let parent = inner
                .runs
                .get(parent_run_id)
                .ok_or_else(|| anyhow!("parent run not found: {parent_run_id}"))?;
            for finding_id in finding_ids {
                let finding = inner
                    .review_findings
                    .get(finding_id)
                    .ok_or_else(|| anyhow!("review finding not found: {finding_id}"))?;
                if finding.run_id != parent_run_id {
                    return Err(anyhow!(
                        "review finding {finding_id} does not belong to parent run {parent_run_id}"
                    ));
                }
                if finding.project_id != parent.project_id {
                    return Err(anyhow!(
                        "review finding {finding_id} project mismatch for parent run {parent_run_id}"
                    ));
                }
                if !finding.repairable {
                    return Err(anyhow!("review finding {finding_id} is not repairable"));
                }
                if finding.status != ReviewFindingStatus::Open {
                    return Err(anyhow!("repair run requires an open finding: {finding_id}"));
                }
            }
        }

        let repair_run = self
            .create_child_run(
                parent_run_id,
                crate::types::AgentPhase::Repair,
                agent_profile,
                model,
                triggered_by_event_id,
                finding_ids.to_vec(),
            )
            .await?;
        for finding_id in finding_ids {
            self.update_review_finding_status(finding_id, ReviewFindingStatus::Repairing)
                .await?;
        }
        Ok(repair_run)
    }

    pub async fn get_run(&self, run_id: &str) -> Option<AgentRun> {
        if let Some(run) = self.inner.read().await.runs.get(run_id).cloned() {
            return Some(run);
        }
        {
            let mut inner = self.inner.write().await;
            self.hydrate_persisted_runs(&mut inner).ok()?;
            self.hydrate_artifact_transactions(&mut inner).ok()?;
            if let Some(run) = inner.runs.get(run_id).cloned() {
                return Some(run);
            }
        }
        let run = self.read_run(run_id).ok().flatten()?;
        self.inner
            .write()
            .await
            .runs
            .insert(run.id.clone(), run.clone());
        Some(run)
    }

    pub async fn create_design_source_artifact(
        &self,
        scope: Value,
        file_name: String,
        media_type: String,
        content: Vec<u8>,
    ) -> Result<DesignSourceArtifact> {
        if content.is_empty() {
            return Err(anyhow!(
                "invalid design source artifact: content must not be empty"
            ));
        }
        if content.len() > MAX_DESIGN_SOURCE_BYTES {
            return Err(anyhow!(
                "invalid design source artifact: content exceeds {MAX_DESIGN_SOURCE_BYTES} bytes"
            ));
        }
        std::str::from_utf8(&content)
            .map_err(|_| anyhow!("invalid design source artifact: content must be UTF-8"))?;

        let artifact = DesignSourceArtifact {
            id: self.next_id("design-source"),
            scope,
            file_name,
            media_type,
            content_encoding: "identity".to_string(),
            size_bytes: content.len() as u64,
            sha256: sha256_hex(&content),
            created_at: Utc::now(),
        };
        artifact
            .validate_for_runtime()
            .map_err(|error| anyhow!("invalid design source artifact: {error}"))?;

        let artifact_dir = self.design_source_blob_dir.join(&artifact.id);
        fs::create_dir_all(&artifact_dir)?;
        let final_path = artifact_dir.join("source.md");
        let temporary_path = artifact_dir.join(format!(
            ".source.md.tmp-{}-{}",
            std::process::id(),
            Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or_else(|| Utc::now().timestamp_micros())
        ));
        let write_result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary_path)?;
            file.write_all(&content)?;
            file.sync_all()?;
            fs::rename(&temporary_path, &final_path)?;
            fs::File::open(&artifact_dir)?.sync_all()?;
            self.append_design_source_artifact_snapshot(&artifact)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        write_result?;

        self.inner
            .write()
            .await
            .design_source_artifacts
            .insert(artifact.id.clone(), artifact.clone());
        Ok(artifact)
    }

    pub async fn get_design_source_artifact(
        &self,
        artifact_id: &str,
    ) -> Option<DesignSourceArtifact> {
        if let Some(artifact) = self
            .inner
            .read()
            .await
            .design_source_artifacts
            .get(artifact_id)
            .cloned()
        {
            return Some(artifact);
        }
        let artifact = self
            .read_design_source_artifact(artifact_id)
            .ok()
            .flatten()?;
        self.inner
            .write()
            .await
            .design_source_artifacts
            .insert(artifact.id.clone(), artifact.clone());
        Some(artifact)
    }

    pub async fn read_design_source_artifact_content(&self, artifact_id: &str) -> Result<Vec<u8>> {
        let artifact = self
            .get_design_source_artifact(artifact_id)
            .await
            .ok_or_else(|| anyhow!("design source artifact not found: {artifact_id}"))?;
        let path = self.design_source_blob_path(artifact_id);
        let content = fs::read(&path).map_err(|error| {
            anyhow!("design source artifact content missing: {artifact_id}: {error}")
        })?;
        if content.len() as u64 != artifact.size_bytes || sha256_hex(&content) != artifact.sha256 {
            return Err(anyhow!(
                "design source artifact integrity check failed: {artifact_id}"
            ));
        }
        std::str::from_utf8(&content)
            .map_err(|_| anyhow!("design source artifact content is not UTF-8: {artifact_id}"))?;
        Ok(content)
    }

    pub async fn create_design_profile_draft(
        &self,
        draft: DesignProfileDraft,
        report: DesignProfileConversionReport,
    ) -> Result<(DesignProfileDraft, DesignProfileConversionReport)> {
        if draft.schema_version != crate::types::DESIGN_PROFILE_SCHEMA_V2
            || draft.status != "draft"
            || draft.version == 0
            || draft.name.trim().is_empty()
        {
            return Err(anyhow!("invalid design profile draft metadata"));
        }
        if report.design_profile_id != draft.id
            || report.profile_version != draft.version
            || report.id != draft.conversion_report_id
        {
            return Err(anyhow!(
                "design profile conversion report does not match draft"
            ));
        }
        let mut inner = self.inner.write().await;
        self.hydrate_design_profile_drafts(&mut inner)?;
        if inner.design_profile_drafts.contains_key(&draft.id)
            || inner.design_profiles.contains_key(&draft.id)
        {
            return Err(anyhow!("design profile already exists: {}", draft.id));
        }
        inner
            .design_profile_drafts
            .insert(draft.id.clone(), draft.clone());
        inner.design_profile_conversion_reports.insert(
            conversion_report_key(&draft.id, draft.version),
            report.clone(),
        );
        drop(inner);
        self.append_design_profile_draft_snapshot(&draft)?;
        self.append_design_profile_conversion_report_snapshot(&report)?;
        Ok((draft, report))
    }

    pub async fn get_design_profile_draft(
        &self,
        design_profile_id: &str,
    ) -> Option<DesignProfileDraft> {
        if let Some(draft) = self
            .inner
            .read()
            .await
            .design_profile_drafts
            .get(design_profile_id)
            .cloned()
        {
            return Some(draft);
        }
        let draft = self
            .read_design_profile_draft(design_profile_id)
            .ok()
            .flatten()?;
        self.inner
            .write()
            .await
            .design_profile_drafts
            .insert(draft.id.clone(), draft.clone());
        Some(draft)
    }

    pub async fn list_design_profile_drafts(
        &self,
        project_id: Option<&str>,
        workspace_id: Option<&str>,
        organization_id: Option<&str>,
    ) -> Vec<DesignProfileDraft> {
        let mut inner = self.inner.write().await;
        self.hydrate_design_profile_drafts(&mut inner).ok();
        let scope_filter_present =
            project_id.is_some() || workspace_id.is_some() || organization_id.is_some();
        let mut drafts = inner
            .design_profile_drafts
            .values()
            .filter(|draft| {
                if !scope_filter_present {
                    return true;
                }
                project_id.is_some_and(|id| {
                    draft.scope.get("projectId").and_then(Value::as_str) == Some(id)
                }) || workspace_id.is_some_and(|id| {
                    draft.scope.get("workspaceId").and_then(Value::as_str) == Some(id)
                }) || organization_id.is_some_and(|id| {
                    draft.scope.get("organizationId").and_then(Value::as_str) == Some(id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        drafts.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        drafts
    }

    pub async fn update_design_profile_draft(
        &self,
        design_profile_id: &str,
        expected_version: u32,
        name: String,
        candidate: Value,
        validation_issues: Vec<crate::types::DesignProfileValidationIssue>,
    ) -> Result<DesignProfileDraft> {
        let mut inner = self.inner.write().await;
        self.hydrate_design_profile_drafts(&mut inner)?;
        let current = inner
            .design_profile_drafts
            .get(design_profile_id)
            .cloned()
            .ok_or_else(|| anyhow!("design profile draft not found: {design_profile_id}"))?;
        if current.version != expected_version {
            return Err(anyhow!(
                "design profile version conflict: expected {expected_version}, current {}",
                current.version
            ));
        }
        let now = Utc::now();
        let report_id = self.next_id("design-profile-conversion-report");
        let draft = DesignProfileDraft {
            id: current.id,
            schema_version: current.schema_version,
            version: current.version + 1,
            name,
            status: "draft".to_string(),
            scope: current.scope,
            source: current.source,
            candidate,
            conversion_report_id: report_id.clone(),
            validation_issues,
            created_at: current.created_at,
            updated_at: now,
        };
        let previous_report = self
            .read_design_profile_conversion_report(design_profile_id, expected_version)?
            .ok_or_else(|| anyhow!("conversion report not found for draft revision"))?;
        let report = DesignProfileConversionReport {
            id: report_id,
            design_profile_id: draft.id.clone(),
            profile_version: draft.version,
            required_signature_rule_count: draft
                .candidate
                .get("signatureRules")
                .and_then(Value::as_array)
                .map(|rules| {
                    rules
                        .iter()
                        .filter(|rule| {
                            rule.get("priority").and_then(Value::as_str) == Some("required")
                        })
                        .count()
                })
                .unwrap_or(0),
            created_at: now,
            ..previous_report
        };
        inner
            .design_profile_drafts
            .insert(draft.id.clone(), draft.clone());
        inner.design_profile_conversion_reports.insert(
            conversion_report_key(&draft.id, draft.version),
            report.clone(),
        );
        drop(inner);
        self.append_design_profile_draft_snapshot(&draft)?;
        self.append_design_profile_conversion_report_snapshot(&report)?;
        Ok(draft)
    }

    pub async fn design_profile_draft_versions(
        &self,
        design_profile_id: &str,
    ) -> Result<Vec<DesignProfileDraft>> {
        self.read_design_profile_draft_history(design_profile_id)
    }

    pub async fn design_profile_conversion_report(
        &self,
        design_profile_id: &str,
        version: Option<u32>,
    ) -> Result<Option<DesignProfileConversionReport>> {
        let version = match version {
            Some(version) => version,
            None => self
                .get_design_profile_draft(design_profile_id)
                .await
                .map(|draft| draft.version)
                .ok_or_else(|| anyhow!("design profile draft not found: {design_profile_id}"))?,
        };
        self.read_design_profile_conversion_report(design_profile_id, version)
    }

    pub async fn create_design_profile(&self, profile: DesignProfile) -> Result<DesignProfile> {
        profile
            .validate_for_runtime()
            .map_err(|error| anyhow!("invalid design profile: {error}"))?;
        let mut inner = self.inner.write().await;
        self.hydrate_design_profiles(&mut inner)?;
        ensure_unique_project_active_profile(&inner, &profile)?;
        inner
            .design_profiles
            .insert(profile.id.clone(), profile.clone());
        drop(inner);
        self.append_design_profile_snapshot(&profile)?;
        Ok(profile)
    }

    pub async fn get_design_profile(&self, design_profile_id: &str) -> Option<DesignProfile> {
        if let Some(profile) = self
            .inner
            .read()
            .await
            .design_profiles
            .get(design_profile_id)
            .cloned()
        {
            return Some(profile);
        }
        let profile = self.read_design_profile(design_profile_id).ok().flatten()?;
        self.inner
            .write()
            .await
            .design_profiles
            .insert(profile.id.clone(), profile.clone());
        Some(profile)
    }

    pub async fn design_profile_versions(
        &self,
        design_profile_id: &str,
    ) -> Result<Vec<DesignProfile>> {
        let versions = self.read_design_profile_history(design_profile_id)?;
        if let Some(latest) = versions.last().cloned() {
            self.inner
                .write()
                .await
                .design_profiles
                .insert(latest.id.clone(), latest);
        }
        Ok(versions)
    }

    pub async fn list_design_profiles(
        &self,
        project_id: Option<&str>,
        workspace_id: Option<&str>,
        organization_id: Option<&str>,
        include_archived: bool,
    ) -> Vec<DesignProfile> {
        let mut inner = self.inner.write().await;
        self.hydrate_design_profiles(&mut inner).ok();
        let scope_filter_present =
            project_id.is_some() || workspace_id.is_some() || organization_id.is_some();
        let mut profiles = inner
            .design_profiles
            .values()
            .filter(|profile| include_archived || profile.status != "archived")
            .filter(|profile| {
                if !scope_filter_present {
                    return true;
                }
                project_id.is_some_and(|id| profile.project_id() == Some(id))
                    || workspace_id.is_some_and(|id| profile.workspace_id() == Some(id))
                    || organization_id.is_some_and(|id| profile.organization_id() == Some(id))
            })
            .cloned()
            .collect::<Vec<_>>();
        profiles.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| left.version.cmp(&right.version))
        });
        profiles
    }

    pub async fn archive_design_profile(&self, design_profile_id: &str) -> Result<DesignProfile> {
        let mut inner = self.inner.write().await;
        self.hydrate_design_profiles(&mut inner)?;
        let mut profile = inner
            .design_profiles
            .get(design_profile_id)
            .cloned()
            .ok_or_else(|| anyhow!("design profile not found: {design_profile_id}"))?;
        profile.status = "archived".to_string();
        profile.version += 1;
        profile.updated_at = Utc::now();
        inner
            .design_profiles
            .insert(profile.id.clone(), profile.clone());
        drop(inner);
        self.append_design_profile_snapshot(&profile)?;
        Ok(profile)
    }

    pub async fn bind_project_design_profile(
        &self,
        project_id: &str,
        design_profile_id: &str,
    ) -> Result<DesignProfile> {
        let mut inner = self.inner.write().await;
        self.hydrate_design_profiles(&mut inner)?;
        self.hydrate_project_design_profiles(&mut inner)?;
        let profile = inner
            .design_profiles
            .get(design_profile_id)
            .cloned()
            .ok_or_else(|| anyhow!("design profile not found: {design_profile_id}"))?;
        if profile.status != "active" {
            return Err(anyhow!(
                "project active design profile must have status=active: {design_profile_id}"
            ));
        }
        if let Some(scope_project_id) = profile.project_id() {
            if scope_project_id != project_id {
                return Err(anyhow!(
                    "design profile {design_profile_id} is scoped to project {scope_project_id}"
                ));
            }
        }
        inner
            .project_design_profiles
            .insert(project_id.to_string(), design_profile_id.to_string());
        drop(inner);
        self.append_project_design_profile_snapshot(&ProjectDesignProfileSnapshot {
            project_id: project_id.to_string(),
            design_profile_id: design_profile_id.to_string(),
            updated_at: Utc::now(),
        })?;
        Ok(profile)
    }

    pub async fn project_design_profile(&self, project_id: &str) -> Option<DesignProfile> {
        let mut inner = self.inner.write().await;
        self.hydrate_design_profiles(&mut inner).ok()?;
        self.hydrate_project_design_profiles(&mut inner).ok()?;
        let profile_id = inner.project_design_profiles.get(project_id)?.clone();
        inner.design_profiles.get(&profile_id).cloned()
    }

    pub async fn resolve_design_profile(
        &self,
        project_id: &str,
        workspace_id: Option<&str>,
        organization_id: Option<&str>,
        explicit_design_profile_id: Option<&str>,
    ) -> Result<Option<DesignProfile>> {
        let mut inner = self.inner.write().await;
        self.hydrate_design_profiles(&mut inner)?;
        self.hydrate_project_design_profiles(&mut inner)?;
        if let Some(design_profile_id) = explicit_design_profile_id {
            let profile = inner
                .design_profiles
                .get(design_profile_id)
                .cloned()
                .ok_or_else(|| anyhow!("design profile not found: {design_profile_id}"))?;
            if !profile_visible_to_context(&profile, project_id, workspace_id, organization_id) {
                return Err(anyhow!(
                    "design profile {design_profile_id} is not visible to project {project_id}"
                ));
            }
            return Ok(Some(profile));
        }
        let Some(profile_id) = inner.project_design_profiles.get(project_id).cloned() else {
            if let Some(workspace_id) = workspace_id {
                let workspace_matches = active_profiles_for_scope(
                    inner.design_profiles.values(),
                    ScopeLevel::Workspace,
                    workspace_id,
                );
                if workspace_matches.len() > 1 {
                    return Err(anyhow!(
                        "multiple workspace default design profiles match workspace {workspace_id}; pass designProfileId explicitly"
                    ));
                }
                if let Some(profile) = workspace_matches.into_iter().next() {
                    return Ok(Some(profile));
                }
            }
            if let Some(organization_id) = organization_id {
                let organization_matches = active_profiles_for_scope(
                    inner.design_profiles.values(),
                    ScopeLevel::Organization,
                    organization_id,
                );
                if organization_matches.len() > 1 {
                    return Err(anyhow!(
                        "multiple organization default design profiles match organization {organization_id}; pass designProfileId explicitly"
                    ));
                }
                if let Some(profile) = organization_matches.into_iter().next() {
                    return Ok(Some(profile));
                }
            }
            return Ok(None);
        };
        let profile = inner
            .design_profiles
            .get(&profile_id)
            .cloned()
            .ok_or_else(|| anyhow!("design profile not found: {profile_id}"))?;
        if profile.status != "active" {
            return Err(anyhow!(
                "project active design profile is not active: {profile_id}"
            ));
        }
        Ok(Some(profile))
    }

    pub async fn attach_run_design_profile(
        &self,
        run_id: &str,
        profile: &DesignProfile,
    ) -> Result<AgentRun> {
        self.attach_run_effective_design_profile(run_id, profile, None, None)
            .await
    }

    pub async fn attach_run_effective_design_profile(
        &self,
        run_id: &str,
        profile: &DesignProfile,
        surface: Option<&str>,
        template: Option<&str>,
    ) -> Result<AgentRun> {
        let effective = match (surface, template) {
            (Some(surface), Some(template)) => Some(
                profile
                    .effective_for(surface, template)
                    .map_err(|error| anyhow!(error))?,
            ),
            (None, None) => None,
            _ => return Err(anyhow!("surface and template must be provided together")),
        };
        let (unsupported_extended_tokens, blocking_capability_rule_ids) = effective
            .as_ref()
            .map(design_profile_capability_gaps)
            .unwrap_or_default();
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        run.design_profile_id = Some(profile.id.clone());
        run.design_profile_version = Some(profile.version);
        run.design_profile_hash = Some(profile.stable_hash());
        run.design_profile_surface = effective.as_ref().map(|value| value.surface.clone());
        run.design_profile_template = effective.as_ref().map(|value| value.template.clone());
        run.design_profile_surface_override_hash = effective
            .as_ref()
            .and_then(|value| value.surface_override_hash.clone());
        run.design_profile_template_override_hash = effective
            .as_ref()
            .and_then(|value| value.template_override_hash.clone());
        run.design_profile_effective_hash = effective
            .as_ref()
            .map(|value| value.effective_profile_hash.clone());
        run.design_profile_unsupported_extended_tokens = unsupported_extended_tokens;
        run.design_profile_blocking_capability_rule_ids = blocking_capability_rule_ids;
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn configure_run_design_fidelity(
        &self,
        run_id: &str,
        profile: &DesignProfile,
        requested_mode: Option<&str>,
    ) -> Result<AgentRun> {
        if let Some(mode) = requested_mode {
            if !matches!(mode, "profile_only" | "source_fallback") {
                return Err(anyhow!(
                    "designFidelityMode must be profile_only or source_fallback"
                ));
            }
        }
        let imported = profile.source.get("kind").and_then(Value::as_str) == Some("imported");
        let source_artifact_id = profile
            .source
            .get("primarySourceArtifactId")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let artifact = match source_artifact_id.as_deref() {
            Some(artifact_id) => Some(
                self.get_design_source_artifact(artifact_id)
                    .await
                    .ok_or_else(|| anyhow!("design source artifact not found: {artifact_id}"))?,
            ),
            None => None,
        };
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        let mode = requested_mode.unwrap_or(if imported && run.phase == AgentPhase::Build {
            "source_fallback"
        } else {
            "profile_only"
        });
        if mode == "source_fallback" && !imported {
            return Err(anyhow!(
                "source_fallback requires an imported design profile"
            ));
        }
        run.design_fidelity_mode = Some(mode.to_string());
        if let Some(artifact) = artifact {
            run.design_source_artifact_id = Some(artifact.id);
            run.design_source_hash = Some(artifact.sha256);
            run.design_source_size_bytes = Some(artifact.size_bytes);
            run.design_source_budget_bytes = Some(48 * 1024);
        }
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn set_run_design_source_index(
        &self,
        run_id: &str,
        index: &DesignSourceIndex,
        required_section_ids: Vec<String>,
    ) -> Result<AgentRun> {
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        if run.design_source_artifact_id.as_deref() != Some(&index.source_artifact_id)
            || run.design_source_hash.as_deref() != Some(&index.source_hash)
        {
            return Err(anyhow!("design source index does not match run snapshot"));
        }
        run.design_source_sections = index.sections.clone();
        run.design_source_required_section_ids = required_section_ids;
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn record_design_context_file_read(
        &self,
        run_id: &str,
        path: &str,
    ) -> Result<AgentRun> {
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        if !run
            .design_context_read_files
            .iter()
            .any(|value| value == path)
        {
            run.design_context_read_files.push(path.to_string());
            run.design_context_read_files.sort();
        }
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn record_design_source_sections_read(
        &self,
        run_id: &str,
        section_hashes: &[String],
        bytes_read: u64,
    ) -> Result<AgentRun> {
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        let budget = run.design_source_budget_bytes.unwrap_or(48 * 1024);
        if run.design_source_bytes_read.saturating_add(bytes_read) > budget {
            return Err(anyhow!("design profile source budget exceeded"));
        }
        run.design_source_bytes_read += bytes_read;
        for hash in section_hashes {
            if !run
                .design_source_read_section_hashes
                .iter()
                .any(|value| value == hash)
            {
                run.design_source_read_section_hashes.push(hash.clone());
            }
        }
        run.design_source_read_section_hashes.sort();
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn child_runs(&self, parent_run_id: &str) -> Vec<AgentRun> {
        self.inner
            .read()
            .await
            .runs
            .values()
            .filter(|run| run.parent_run_id.as_deref() == Some(parent_run_id))
            .cloned()
            .collect()
    }

    pub async fn active_review_or_repair_runs_for_candidate(
        &self,
        parent_run_id: &str,
        candidate_version_id: &str,
    ) -> Vec<AgentRun> {
        let trigger = format!("preview.candidate:{candidate_version_id}");
        let inner = self.inner.read().await;
        let candidate_roots = inner
            .runs
            .values()
            .filter(|run| {
                run.parent_run_id.as_deref() == Some(parent_run_id)
                    && run.triggered_by_event_id.as_deref() == Some(trigger.as_str())
                    && matches!(
                        run.phase,
                        crate::types::AgentPhase::Review | crate::types::AgentPhase::Repair
                    )
            })
            .map(|run| run.id.clone())
            .collect::<HashSet<_>>();
        inner
            .runs
            .values()
            .filter(|run| {
                matches!(
                    run.phase,
                    crate::types::AgentPhase::Review | crate::types::AgentPhase::Repair
                ) && !run.status.is_terminal()
                    && run_is_candidate_review_descendant(&inner, &run.id, &candidate_roots)
            })
            .cloned()
            .collect()
    }

    pub async fn bind_run_to_sandbox(
        &self,
        run_id: &str,
        sandbox_binding_id: &str,
    ) -> Result<AgentRun> {
        let mut inner = self.inner.write().await;
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        if !inner.sandbox_bindings.contains_key(sandbox_binding_id) {
            if let Some(binding) = self.read_sandbox_binding(sandbox_binding_id)? {
                inner.sandbox_bindings.insert(binding.id.clone(), binding);
            }
        }
        let binding = inner
            .sandbox_bindings
            .get(sandbox_binding_id)
            .cloned()
            .ok_or_else(|| anyhow!("sandbox binding not found: {sandbox_binding_id}"))?;
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        if run.project_id != binding.project_id {
            return Err(anyhow!(
                "sandbox binding project mismatch: run project {} cannot use binding project {}",
                run.project_id,
                binding.project_id
            ));
        }
        run.sandbox_id = Some(sandbox_binding_id.to_string());
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn ensure_sandbox_binding_available(
        &self,
        sandbox_binding_id: &str,
        allowed_parent_run_id: Option<&str>,
    ) -> Result<SandboxBinding> {
        {
            let mut inner = self.inner.write().await;
            self.hydrate_persisted_runs(&mut inner)?;
            if !inner.sandbox_bindings.contains_key(sandbox_binding_id) {
                if let Some(binding) = self.read_sandbox_binding(sandbox_binding_id)? {
                    inner.sandbox_bindings.insert(binding.id.clone(), binding);
                }
            }
        }
        let inner = self.inner.read().await;
        let binding = inner
            .sandbox_bindings
            .get(sandbox_binding_id)
            .cloned()
            .ok_or_else(|| anyhow!("sandbox binding not found: {sandbox_binding_id}"))?;
        if !matches!(
            binding.status,
            SandboxBindingStatus::Ready | SandboxBindingStatus::Busy | SandboxBindingStatus::Idle
        ) {
            return Err(anyhow!(
                "sandbox binding {sandbox_binding_id} is not ready: status={:?}; wait_ready must complete before starting a sandbox run",
                binding.status
            ));
        }
        let allowed_workspace_holders = allowed_workspace_holder_ids(&inner, allowed_parent_run_id);
        if let Some(active) = inner.runs.values().find(|run| {
            run.sandbox_id.as_deref() == Some(sandbox_binding_id)
                && !run.status.is_terminal()
                && !allowed_workspace_holders.contains(run.id.as_str())
        }) {
            return Err(anyhow!(
                "sandbox binding {sandbox_binding_id} is already in use by active run {}",
                active.id
            ));
        }
        Ok(binding)
    }

    pub async fn mark_sandbox_binding_busy(
        &self,
        sandbox_binding_id: &str,
    ) -> Result<SandboxBinding> {
        let binding = {
            let mut inner = self.inner.write().await;
            if !inner.sandbox_bindings.contains_key(sandbox_binding_id) {
                if let Some(binding) = self.read_sandbox_binding(sandbox_binding_id)? {
                    inner.sandbox_bindings.insert(binding.id.clone(), binding);
                }
            }
            let binding = inner
                .sandbox_bindings
                .get_mut(sandbox_binding_id)
                .ok_or_else(|| anyhow!("sandbox binding not found: {sandbox_binding_id}"))?;
            binding.status = SandboxBindingStatus::Busy;
            binding.last_seen_at = Utc::now();
            binding.clone()
        };
        self.append_sandbox_binding_snapshot(&binding)?;
        Ok(binding)
    }

    pub async fn acquire_sandbox_binding_for_run(
        &self,
        run_id: &str,
        allowed_parent_run_id: Option<&str>,
    ) -> Result<SandboxBinding> {
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let run = inner
            .runs
            .get(run_id)
            .cloned()
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        let sandbox_binding_id = run
            .sandbox_id
            .as_deref()
            .ok_or_else(|| anyhow!("run {run_id} is not bound to a sandbox"))?;
        if !inner.sandbox_bindings.contains_key(sandbox_binding_id) {
            if let Some(binding) = self.read_sandbox_binding(sandbox_binding_id)? {
                inner.sandbox_bindings.insert(binding.id.clone(), binding);
            }
        }
        let binding = inner
            .sandbox_bindings
            .get(sandbox_binding_id)
            .cloned()
            .ok_or_else(|| anyhow!("sandbox binding not found: {sandbox_binding_id}"))?;
        if run.project_id != binding.project_id {
            return Err(anyhow!(
                "sandbox binding project mismatch: run project {} cannot use binding project {}",
                run.project_id,
                binding.project_id
            ));
        }
        if !matches!(
            binding.status,
            SandboxBindingStatus::Ready | SandboxBindingStatus::Busy | SandboxBindingStatus::Idle
        ) {
            return Err(anyhow!(
                "sandbox binding {sandbox_binding_id} is not ready: status={:?}; wait_ready must complete before starting a sandbox run",
                binding.status
            ));
        }
        let allowed_workspace_holders = allowed_workspace_holder_ids(&inner, allowed_parent_run_id);
        if let Some(active) = inner.runs.values().find(|other| {
            other.id != run_id
                && other.sandbox_id.as_deref() == Some(sandbox_binding_id)
                && !other.status.is_terminal()
                && !allowed_workspace_holders.contains(other.id.as_str())
        }) {
            return Err(anyhow!(
                "sandbox binding {sandbox_binding_id} is already in use by active run {}",
                active.id
            ));
        }
        let binding = inner
            .sandbox_bindings
            .get_mut(sandbox_binding_id)
            .ok_or_else(|| anyhow!("sandbox binding not found: {sandbox_binding_id}"))?;
        binding.status = SandboxBindingStatus::Busy;
        binding.last_seen_at = Utc::now();
        let binding = binding.clone();
        drop(inner);
        self.append_sandbox_binding_snapshot(&binding)?;
        Ok(binding)
    }

    pub async fn runs_requiring_recovery(&self) -> Vec<AgentRun> {
        let mut runs_by_id = self
            .read_runs()
            .unwrap_or_default()
            .into_iter()
            .map(|run| (run.id.clone(), run))
            .collect::<HashMap<_, _>>();
        for run in self.inner.read().await.runs.values().cloned() {
            match runs_by_id.entry(run.id.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(run);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if run_snapshot_is_newer(&run, entry.get()) {
                        entry.insert(run);
                    }
                }
            }
        }
        let recoverable = runs_by_id
            .values()
            .filter(|run| {
                matches!(
                    run.status,
                    AgentRunStatus::Queued | AgentRunStatus::Running | AgentRunStatus::Validating
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut inner = self.inner.write().await;
        for run in runs_by_id.into_values() {
            match inner.runs.entry(run.id.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(run);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if run_snapshot_is_newer(&run, entry.get()) {
                        entry.insert(run);
                    }
                }
            }
        }
        recoverable
    }

    pub async fn update_run_status(
        &self,
        run_id: &str,
        status: AgentRunStatus,
    ) -> Result<AgentRun> {
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let mut should_cleanup_child_resources = false;
        let mut sandbox_binding_to_persist = None;
        let mut review_findings_to_persist = Vec::new();
        let mut pending_permissions_to_persist = Vec::new();
        let run = {
            let run = inner
                .runs
                .get_mut(run_id)
                .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
            if run.status.is_terminal() && run.status != status {
                return Err(anyhow!(
                    "run {run_id} is already terminal with status {:?}",
                    run.status
                ));
            }
            if status == AgentRunStatus::Partial && run.checkpoint_id.is_none() {
                return Err(anyhow!(
                    "run {run_id} cannot enter partial without a checkpoint"
                ));
            }
            run.status = status;
            run.updated_at = Utc::now();
            if status.is_terminal() {
                if run.completed_at.is_none() {
                    run.completed_at = Some(run.updated_at);
                }
                should_cleanup_child_resources = run.parent_run_id.is_some();
            }
            run.clone()
        };
        if should_cleanup_child_resources {
            inner.run_scoped_resources.remove(run_id);
        }
        if status.is_terminal() {
            if let Some(sandbox_binding_id) = run.sandbox_id.as_deref() {
                let has_active_run = inner.runs.values().any(|other| {
                    other.id != run_id
                        && other.sandbox_id.as_deref() == Some(sandbox_binding_id)
                        && !other.status.is_terminal()
                });
                if !has_active_run {
                    if let Some(binding) = inner.sandbox_bindings.get_mut(sandbox_binding_id) {
                        if binding.status == SandboxBindingStatus::Busy {
                            binding.status = SandboxBindingStatus::Idle;
                            binding.last_seen_at = run.updated_at;
                            sandbox_binding_to_persist = Some(binding.clone());
                        }
                    }
                }
            }
            inner.continue_interrupt_requests.remove(run_id);
            self.hydrate_pending_permissions(&mut inner)?;
            for permission in inner.pending_permissions.values_mut() {
                if permission.run_id == run_id && permission.status == "pending" {
                    permission.status = "expired".to_string();
                    permission.resolved_at = Some(run.updated_at);
                    pending_permissions_to_persist.push(permission.clone());
                }
            }
        }
        if run.phase == crate::types::AgentPhase::Repair {
            self.hydrate_review_findings(&mut inner)?;
            if let Some(next_finding_status) = repair_finding_status_for_run_status(status) {
                if let Some(finding_ids) = run.finding_ids.as_ref() {
                    for finding_id in finding_ids {
                        if let Some(finding) = inner.review_findings.get_mut(finding_id) {
                            finding.status = next_finding_status;
                            review_findings_to_persist.push(finding.clone());
                        }
                    }
                }
            }
        }
        drop(inner);
        if let Some(binding) = sandbox_binding_to_persist {
            self.append_sandbox_binding_snapshot(&binding)?;
        }
        for finding in review_findings_to_persist {
            self.append_review_finding_snapshot(&finding)?;
        }
        for permission in pending_permissions_to_persist {
            self.append_pending_permission_snapshot(&permission)?;
        }
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn set_run_brief_version(&self, run_id: &str, brief_version: String) -> Result<()> {
        let mut inner = self.inner.write().await;
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        run.brief_version = Some(brief_version);
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(())
    }

    pub async fn set_run_checkpoint(
        &self,
        run_id: &str,
        checkpoint_id: String,
    ) -> Result<AgentRun> {
        let mut inner = self.inner.write().await;
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        run.checkpoint_id = Some(checkpoint_id);
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn set_run_output_version(
        &self,
        run_id: &str,
        output_version_id: String,
    ) -> Result<AgentRun> {
        let mut inner = self.inner.write().await;
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        run.output_version_id = Some(output_version_id);
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    pub async fn request_continue_interrupt(&self, run_id: &str) {
        self.inner
            .write()
            .await
            .continue_interrupt_requests
            .insert(run_id.to_string());
    }

    pub async fn continue_interrupt_requested(&self, run_id: &str) -> bool {
        self.inner
            .read()
            .await
            .continue_interrupt_requests
            .contains(run_id)
    }

    pub async fn clear_continue_interrupt(&self, run_id: &str) {
        self.inner
            .write()
            .await
            .continue_interrupt_requests
            .remove(run_id);
    }

    pub async fn content_sources(&self, run_id: &str) -> Vec<ContentSource> {
        if let Some(sources) = self.inner.read().await.content_sources.get(run_id).cloned() {
            return sources;
        }
        let snapshot = self
            .read_content_source_snapshot(run_id)
            .unwrap_or(None)
            .unwrap_or_else(|| RunContentSourcesSnapshot {
                run_id: run_id.to_string(),
                project_id: String::new(),
                sources: Vec::new(),
            });
        if !snapshot.sources.is_empty() {
            self.inner
                .write()
                .await
                .content_sources
                .insert(run_id.to_string(), snapshot.sources.clone());
        }
        snapshot.sources
    }

    pub async fn content_sources_for_brief(&self, brief_id: &str) -> Vec<ContentSource> {
        if let Some(sources) = self
            .inner
            .read()
            .await
            .brief_content_sources
            .get(brief_id)
            .cloned()
        {
            return sources;
        }
        if let Some(run_id) = self.inner.read().await.brief_run_ids.get(brief_id).cloned() {
            let sources = self.content_sources(&run_id).await;
            self.inner
                .write()
                .await
                .brief_content_sources
                .insert(brief_id.to_string(), sources.clone());
            return sources;
        }
        let run_id = self
            .inner
            .read()
            .await
            .runs
            .values()
            .find(|run| {
                run.phase == AgentPhase::Brief && run.brief_version.as_deref() == Some(brief_id)
            })
            .map(|run| run.id.clone());
        if let Some(run_id) = run_id {
            return self.content_sources(&run_id).await;
        }
        let Ok(Some(snapshot)) = self.read_brief_snapshot(brief_id) else {
            return Vec::new();
        };
        self.inner
            .write()
            .await
            .brief_run_ids
            .insert(brief_id.to_string(), snapshot.run_id.clone());
        let sources = self.content_sources(&snapshot.run_id).await;
        self.inner
            .write()
            .await
            .brief_content_sources
            .insert(brief_id.to_string(), sources.clone());
        sources
    }

    pub async fn write_brief(&self, run_id: &str, brief: Brief) -> Result<String> {
        brief.validate_for_runtime().map_err(|err| anyhow!(err))?;
        let brief_id = self.next_id("brief");
        let run = self
            .get_run(run_id)
            .await
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        let brief_checkpoint_summary = json!({
            "projectType": brief.project_type.clone(),
            "recommendedTemplate": brief.recommended_template.clone(),
            "audience": brief.audience.clone(),
            "contentHierarchy": brief.content_hierarchy.clone(),
            "missingInformation": brief.missing_information.clone(),
        });
        let snapshot = BriefSnapshot {
            brief_id: brief_id.clone(),
            run_id: run_id.to_string(),
            project_id: run.project_id.clone(),
            status: BriefStatus::Confirmed,
            brief: brief.clone(),
        };
        let content_sources = self.content_sources(run_id).await;
        let mut inner = self.inner.write().await;
        inner.briefs.insert(brief_id.clone(), brief);
        inner
            .brief_statuses
            .insert(brief_id.clone(), BriefStatus::Confirmed);
        inner
            .brief_run_ids
            .insert(brief_id.clone(), run_id.to_string());
        inner
            .brief_content_sources
            .insert(brief_id.clone(), content_sources);
        drop(inner);
        self.append_brief_snapshot(&snapshot)?;
        self.set_run_brief_version(run_id, brief_id.clone()).await?;
        self.save_checkpoint(AgentCheckpoint {
            id: self.next_id("checkpoint"),
            run_id: run_id.to_string(),
            project_id: run.project_id,
            phase: run.phase,
            message_window: vec![json!({
                "role": "system",
                "text": "Brief confirmed.",
                "briefId": brief_id,
                "brief": brief_checkpoint_summary,
            })],
            conversation_range: Some(crate::types::CheckpointConversationRange {
                start_index: 0,
                end_index_exclusive: 1,
                retained_count: 1,
            }),
            task_list: Vec::new(),
            workspace_snapshot_uri: None,
            build_result: None,
            brief_version: Some(brief_id.clone()),
            design_version: run.design_version,
            last_known_preview_url: None,
            context_summary: "Brief confirmed and stored.".to_string(),
            created_at: Utc::now(),
        })
        .await?;
        Ok(brief_id)
    }

    pub async fn write_brief_draft(&self, run_id: &str, brief: Brief) -> Result<String> {
        brief.validate_for_runtime().map_err(|err| anyhow!(err))?;
        let brief_id = self.next_id("brief");
        let run = self
            .get_run(run_id)
            .await
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        let snapshot = BriefSnapshot {
            brief_id: brief_id.clone(),
            run_id: run_id.to_string(),
            project_id: run.project_id,
            status: BriefStatus::Draft,
            brief: brief.clone(),
        };
        let content_sources = self.content_sources(run_id).await;
        let mut inner = self.inner.write().await;
        inner.briefs.insert(brief_id.clone(), brief);
        inner
            .brief_statuses
            .insert(brief_id.clone(), BriefStatus::Draft);
        inner
            .brief_run_ids
            .insert(brief_id.clone(), run_id.to_string());
        inner
            .brief_content_sources
            .insert(brief_id.clone(), content_sources);
        drop(inner);
        self.append_brief_snapshot(&snapshot)?;
        self.set_run_brief_version(run_id, brief_id.clone()).await?;
        Ok(brief_id)
    }

    pub async fn confirm_brief(&self, run_id: &str, brief_id: &str) -> Result<()> {
        let run = self
            .get_run(run_id)
            .await
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        let brief = self
            .get_brief(brief_id)
            .await
            .ok_or_else(|| anyhow!("brief not found: {brief_id}"))?;
        let snapshot = BriefSnapshot {
            brief_id: brief_id.to_string(),
            run_id: run_id.to_string(),
            project_id: run.project_id.clone(),
            status: BriefStatus::Confirmed,
            brief: brief.clone(),
        };
        let content_sources = self.content_sources(run_id).await;
        let brief_checkpoint_summary = json!({
            "projectType": brief.project_type.clone(),
            "recommendedTemplate": brief.recommended_template.clone(),
            "audience": brief.audience.clone(),
            "contentHierarchy": brief.content_hierarchy.clone(),
            "missingInformation": brief.missing_information.clone(),
        });
        {
            let mut inner = self.inner.write().await;
            inner.briefs.insert(brief_id.to_string(), brief);
            inner
                .brief_statuses
                .insert(brief_id.to_string(), BriefStatus::Confirmed);
            inner
                .brief_run_ids
                .insert(brief_id.to_string(), run_id.to_string());
            inner
                .brief_content_sources
                .insert(brief_id.to_string(), content_sources);
        }
        self.append_brief_snapshot(&snapshot)?;
        self.save_checkpoint(AgentCheckpoint {
            id: self.next_id("checkpoint"),
            run_id: run_id.to_string(),
            project_id: run.project_id,
            phase: run.phase,
            message_window: vec![json!({
                "role": "system",
                "text": "Brief confirmed.",
                "briefId": brief_id,
                "brief": brief_checkpoint_summary,
            })],
            conversation_range: Some(crate::types::CheckpointConversationRange {
                start_index: 0,
                end_index_exclusive: 1,
                retained_count: 1,
            }),
            task_list: Vec::new(),
            workspace_snapshot_uri: None,
            build_result: None,
            brief_version: Some(brief_id.to_string()),
            design_version: run.design_version,
            last_known_preview_url: None,
            context_summary: "Brief confirmed and stored.".to_string(),
            created_at: Utc::now(),
        })
        .await?;
        Ok(())
    }

    pub async fn get_brief(&self, brief_id: &str) -> Option<Brief> {
        if let Some(brief) = self.inner.read().await.briefs.get(brief_id).cloned() {
            return Some(brief);
        }
        let snapshot = self.read_brief_snapshot(brief_id).ok().flatten()?;
        let mut inner = self.inner.write().await;
        inner
            .brief_statuses
            .insert(brief_id.to_string(), snapshot.status);
        inner
            .brief_run_ids
            .insert(brief_id.to_string(), snapshot.run_id);
        inner
            .briefs
            .insert(brief_id.to_string(), snapshot.brief.clone());
        Some(snapshot.brief)
    }

    pub async fn brief_status(&self, brief_id: &str) -> Option<BriefStatus> {
        if let Some(status) = self
            .inner
            .read()
            .await
            .brief_statuses
            .get(brief_id)
            .copied()
        {
            return Some(status);
        }
        let snapshot = self.read_brief_snapshot(brief_id).ok().flatten()?;
        let mut inner = self.inner.write().await;
        inner
            .briefs
            .insert(brief_id.to_string(), snapshot.brief.clone());
        inner
            .brief_statuses
            .insert(brief_id.to_string(), snapshot.status);
        inner
            .brief_run_ids
            .insert(brief_id.to_string(), snapshot.run_id);
        Some(snapshot.status)
    }

    pub async fn is_brief_confirmed(&self, brief_id: &str) -> bool {
        self.brief_status(brief_id).await == Some(BriefStatus::Confirmed)
    }

    pub async fn append_event(&self, event: AgentEvent) -> Result<()> {
        let run_id = event.run_id().to_string();
        self.append_run_log_event(&run_id, &event)?;
        let event_is_terminal = event.is_run_completed();
        let mut inner = self.inner.write().await;
        let sequence = if inner.events.contains_key(&run_id) {
            let events = inner.events.get_mut(&run_id).expect("events checked");
            events.push(event.clone());
            events.len()
        } else {
            let events = self.read_run_log_events(&run_id)?;
            let sequence = events.len();
            inner.events.insert(run_id.clone(), events);
            sequence
        };
        let broadcaster = if event_is_terminal {
            inner.event_broadcasters.remove(&run_id)
        } else {
            inner.event_broadcasters.get(&run_id).cloned()
        };
        drop(inner);
        if let Some(broadcaster) = broadcaster {
            let _ = broadcaster.send(SequencedAgentEvent { sequence, event });
        }
        Ok(())
    }

    fn append_run_log_event(&self, run_id: &str, event: &AgentEvent) -> Result<()> {
        let path = self.run_log_path(run_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, event)
    }

    pub async fn events(&self, run_id: &str) -> Vec<AgentEvent> {
        let memory_events = self.inner.read().await.events.get(run_id).cloned();
        match memory_events {
            Some(events) if !events.is_empty() => events,
            _ => self.read_run_log_events(run_id).unwrap_or_default(),
        }
    }

    pub async fn subscribe_events(
        &self,
        run_id: &str,
    ) -> Option<broadcast::Receiver<SequencedAgentEvent>> {
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner).ok()?;
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id).ok().flatten() {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let run = inner.runs.get(run_id)?;
        if run.status.is_terminal() {
            let has_terminal_event = match inner.events.get(run_id) {
                Some(events) => events.iter().any(AgentEvent::is_run_completed),
                None => {
                    let events = self.read_run_log_events(run_id).ok()?;
                    let has_terminal_event = events.iter().any(AgentEvent::is_run_completed);
                    inner.events.insert(run_id.to_string(), events);
                    has_terminal_event
                }
            };
            if has_terminal_event {
                return None;
            }
        }
        let sender = inner
            .event_broadcasters
            .entry(run_id.to_string())
            .or_insert_with(|| broadcast::channel(RUN_EVENT_BROADCAST_CAPACITY).0)
            .clone();
        Some(sender.subscribe())
    }

    fn read_run_log_events(&self, run_id: &str) -> Result<Vec<AgentEvent>> {
        let file = match fs::File::open(self.run_log_path(run_id)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            events.push(serde_json::from_str(&line)?);
        }
        Ok(events)
    }

    pub async fn append_conversation_item(
        &self,
        project_id: &str,
        run_id: Option<&str>,
        kind: &str,
        role: Option<&str>,
        text: impl Into<String>,
        metadata: Option<Value>,
    ) {
        self.append_conversation_item_with_visibility(
            project_id, run_id, kind, role, text, metadata, "user",
        )
        .await;
    }

    pub async fn append_conversation_item_with_visibility(
        &self,
        project_id: &str,
        run_id: Option<&str>,
        kind: &str,
        role: Option<&str>,
        text: impl Into<String>,
        metadata: Option<Value>,
        visibility: &str,
    ) {
        let visibility = match visibility {
            "debug" => "debug",
            _ => "user",
        };
        let item = ConversationItem {
            id: self.next_id("conversation"),
            project_id: project_id.to_string(),
            run_id: run_id.map(str::to_string),
            version_id: None,
            checkpoint_id: None,
            kind: kind.to_string(),
            role: role.map(str::to_string),
            text: text.into(),
            metadata,
            visibility: visibility.to_string(),
            created_at: Utc::now(),
        };

        if let Err(error) = self.append_conversation_log_item(&item) {
            eprintln!("failed to append conversation item {}: {error}", item.id);
        }

        let mut inner = self.inner.write().await;
        inner
            .conversation_items
            .entry(project_id.to_string())
            .or_default()
            .push(item);
    }

    pub async fn conversation_items(&self, project_id: &str) -> Vec<ConversationItem> {
        let memory_items = self
            .inner
            .read()
            .await
            .conversation_items
            .get(project_id)
            .cloned();
        match memory_items {
            Some(items) if !items.is_empty() => items,
            _ => self
                .read_conversation_log_items(project_id)
                .unwrap_or_default(),
        }
    }

    pub fn conversation_log_path(&self, project_id: &str) -> PathBuf {
        self.conversation_log_dir
            .join(project_log_segment(project_id))
            .join("conversation-items.jsonl")
    }

    fn append_conversation_log_item(&self, item: &ConversationItem) -> Result<()> {
        let path = self.conversation_log_path(&item.project_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, item)
    }

    fn read_conversation_log_items(&self, project_id: &str) -> Result<Vec<ConversationItem>> {
        let file = match fs::File::open(self.conversation_log_path(project_id)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut items = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            items.push(serde_json::from_str(&line)?);
        }
        Ok(items)
    }

    pub fn brief_log_path(&self) -> PathBuf {
        (*self.brief_log_path).clone()
    }

    fn append_brief_snapshot(&self, snapshot: &BriefSnapshot) -> Result<()> {
        let path = self.brief_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, snapshot)
    }

    fn read_brief_snapshot(&self, brief_id: &str) -> Result<Option<BriefSnapshot>> {
        let file = match fs::File::open(self.brief_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut snapshot = None;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let candidate: BriefSnapshot = serde_json::from_str(&line)?;
            if candidate.brief_id == brief_id {
                snapshot = Some(candidate);
            }
        }
        Ok(snapshot)
    }

    pub fn content_source_log_path(&self) -> PathBuf {
        (*self.content_source_log_path).clone()
    }

    fn append_content_source_snapshot(&self, snapshot: &RunContentSourcesSnapshot) -> Result<()> {
        let path = self.content_source_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, snapshot)
    }

    fn read_content_source_snapshot(
        &self,
        run_id: &str,
    ) -> Result<Option<RunContentSourcesSnapshot>> {
        let file = match fs::File::open(self.content_source_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut snapshot = None;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let candidate: RunContentSourcesSnapshot = serde_json::from_str(&line)?;
            if candidate.run_id == run_id {
                snapshot = Some(candidate);
            }
        }
        Ok(snapshot)
    }

    pub fn sandbox_binding_log_path(&self) -> PathBuf {
        (*self.sandbox_binding_log_path).clone()
    }

    pub fn design_profile_log_path(&self) -> PathBuf {
        (*self.design_profile_log_path).clone()
    }

    pub fn design_profile_draft_log_path(&self) -> PathBuf {
        (*self.design_profile_draft_log_path).clone()
    }

    pub fn design_profile_conversion_report_log_path(&self) -> PathBuf {
        (*self.design_profile_conversion_report_log_path).clone()
    }

    pub fn design_source_artifact_log_path(&self) -> PathBuf {
        (*self.design_source_artifact_log_path).clone()
    }

    fn design_source_blob_path(&self, artifact_id: &str) -> PathBuf {
        self.design_source_blob_dir
            .join(artifact_id)
            .join("source.md")
    }

    fn append_design_source_artifact_snapshot(
        &self,
        artifact: &DesignSourceArtifact,
    ) -> Result<()> {
        let path = self.design_source_artifact_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl_synced(&path, artifact)
    }

    fn read_design_source_artifact(
        &self,
        artifact_id: &str,
    ) -> Result<Option<DesignSourceArtifact>> {
        Ok(self
            .read_design_source_artifacts()?
            .into_iter()
            .find(|artifact| artifact.id == artifact_id))
    }

    fn read_design_source_artifacts(&self) -> Result<Vec<DesignSourceArtifact>> {
        let file = match fs::File::open(self.design_source_artifact_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut artifacts_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let artifact: DesignSourceArtifact = serde_json::from_str(&line)?;
            artifacts_by_id.insert(artifact.id.clone(), artifact);
        }
        Ok(artifacts_by_id.into_values().collect())
    }

    pub fn project_design_profile_log_path(&self) -> PathBuf {
        (*self.project_design_profile_log_path).clone()
    }

    fn append_design_profile_snapshot(&self, profile: &DesignProfile) -> Result<()> {
        let path = self.design_profile_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, profile)
    }

    fn append_design_profile_draft_snapshot(&self, draft: &DesignProfileDraft) -> Result<()> {
        let path = self.design_profile_draft_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl_synced(&path, draft)
    }

    fn append_design_profile_conversion_report_snapshot(
        &self,
        report: &DesignProfileConversionReport,
    ) -> Result<()> {
        let path = self.design_profile_conversion_report_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl_synced(&path, report)
    }

    fn read_design_profile_draft(
        &self,
        design_profile_id: &str,
    ) -> Result<Option<DesignProfileDraft>> {
        Ok(self
            .read_design_profile_drafts()?
            .into_iter()
            .find(|draft| draft.id == design_profile_id))
    }

    fn read_design_profile_draft_history(
        &self,
        design_profile_id: &str,
    ) -> Result<Vec<DesignProfileDraft>> {
        let file = match fs::File::open(self.design_profile_draft_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut drafts = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let draft: DesignProfileDraft = serde_json::from_str(&line)?;
            if draft.id == design_profile_id {
                drafts.push(draft);
            }
        }
        drafts.sort_by_key(|draft| draft.version);
        Ok(drafts)
    }

    fn read_design_profile_drafts(&self) -> Result<Vec<DesignProfileDraft>> {
        let file = match fs::File::open(self.design_profile_draft_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut drafts_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let draft: DesignProfileDraft = serde_json::from_str(&line)?;
            drafts_by_id.insert(draft.id.clone(), draft);
        }
        Ok(drafts_by_id.into_values().collect())
    }

    fn hydrate_design_profile_drafts(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        for draft in self.read_design_profile_drafts()? {
            inner.design_profile_drafts.insert(draft.id.clone(), draft);
        }
        Ok(())
    }

    fn read_design_profile_conversion_report(
        &self,
        design_profile_id: &str,
        version: u32,
    ) -> Result<Option<DesignProfileConversionReport>> {
        let file = match fs::File::open(self.design_profile_conversion_report_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut report = None;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let candidate: DesignProfileConversionReport = serde_json::from_str(&line)?;
            if candidate.design_profile_id == design_profile_id
                && candidate.profile_version == version
            {
                report = Some(candidate);
            }
        }
        Ok(report)
    }

    fn read_design_profile(&self, design_profile_id: &str) -> Result<Option<DesignProfile>> {
        Ok(self
            .read_design_profiles()?
            .into_iter()
            .find(|profile| profile.id == design_profile_id))
    }

    fn read_design_profile_history(&self, design_profile_id: &str) -> Result<Vec<DesignProfile>> {
        let file = match fs::File::open(self.design_profile_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut profiles = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let profile: DesignProfile = serde_json::from_str(&line)?;
            if profile.id == design_profile_id {
                profiles.push(profile);
            }
        }
        profiles.sort_by(|left, right| {
            left.version
                .cmp(&right.version)
                .then_with(|| left.updated_at.cmp(&right.updated_at))
        });
        Ok(profiles)
    }

    fn read_design_profiles(&self) -> Result<Vec<DesignProfile>> {
        let file = match fs::File::open(self.design_profile_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut profiles_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let profile: DesignProfile = serde_json::from_str(&line)?;
            profiles_by_id.insert(profile.id.clone(), profile);
        }
        Ok(profiles_by_id.into_values().collect())
    }

    fn hydrate_design_profiles(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        for profile in self.read_design_profiles()? {
            inner.design_profiles.insert(profile.id.clone(), profile);
        }
        Ok(())
    }

    fn append_project_design_profile_snapshot(
        &self,
        snapshot: &ProjectDesignProfileSnapshot,
    ) -> Result<()> {
        let path = self.project_design_profile_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, snapshot)
    }

    fn read_project_design_profile_snapshots(&self) -> Result<Vec<ProjectDesignProfileSnapshot>> {
        let file = match fs::File::open(self.project_design_profile_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut bindings_by_project = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let snapshot: ProjectDesignProfileSnapshot = serde_json::from_str(&line)?;
            bindings_by_project.insert(snapshot.project_id.clone(), snapshot);
        }
        Ok(bindings_by_project.into_values().collect())
    }

    fn hydrate_project_design_profiles(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        for snapshot in self.read_project_design_profile_snapshots()? {
            inner
                .project_design_profiles
                .entry(snapshot.project_id)
                .or_insert(snapshot.design_profile_id);
        }
        Ok(())
    }

    fn append_sandbox_binding_snapshot(&self, binding: &SandboxBinding) -> Result<()> {
        let path = self.sandbox_binding_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, binding)
    }

    fn read_sandbox_binding(&self, binding_id: &str) -> Result<Option<SandboxBinding>> {
        Ok(self
            .read_sandbox_bindings()?
            .into_iter()
            .find(|binding| binding.id == binding_id))
    }

    fn read_sandbox_binding_with_workspace_pvc(
        &self,
        workspace_pvc_name: &str,
    ) -> Result<Option<SandboxBinding>> {
        Ok(self
            .read_sandbox_bindings()?
            .into_iter()
            .find(|binding| binding.workspace_pvc_name == workspace_pvc_name))
    }

    fn read_sandbox_bindings(&self) -> Result<Vec<SandboxBinding>> {
        let file = match fs::File::open(self.sandbox_binding_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut bindings_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let binding: SandboxBinding = serde_json::from_str(&line)?;
            bindings_by_id.insert(binding.id.clone(), binding);
        }
        Ok(bindings_by_id.into_values().collect())
    }

    pub async fn append_audit_record(
        &self,
        project_id: &str,
        run_id: &str,
        tool: &str,
        input_summary: impl Into<String>,
        decision: impl Into<String>,
        reason: impl Into<String>,
    ) -> AuditRecord {
        let record = AuditRecord {
            id: self.next_id("audit"),
            project_id: project_id.to_string(),
            run_id: run_id.to_string(),
            tool: tool.to_string(),
            input_summary: input_summary.into(),
            decision: decision.into(),
            reason: reason.into(),
            created_at: Utc::now(),
        };
        if let Err(error) = self.append_audit_log_record(&record) {
            eprintln!("failed to append audit log record {}: {error}", record.id);
        }
        let mut inner = self.inner.write().await;
        inner.audit_records.push(record.clone());
        record
    }

    pub async fn audit_records(&self) -> Vec<AuditRecord> {
        let memory_records = self.inner.read().await.audit_records.clone();
        if !memory_records.is_empty() {
            return memory_records;
        }
        self.read_audit_log_records().unwrap_or_default()
    }

    fn append_audit_log_record(&self, record: &AuditRecord) -> Result<()> {
        if let Some(parent) = self.audit_log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&self.audit_log_path, record)
    }

    fn read_audit_log_records(&self) -> Result<Vec<AuditRecord>> {
        let file = match fs::File::open(&*self.audit_log_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            records.push(serde_json::from_str(&line)?);
        }
        Ok(records)
    }

    pub fn pending_permission_log_path(&self) -> PathBuf {
        (*self.pending_permission_log_path).clone()
    }

    fn append_pending_permission_snapshot(&self, permission: &PendingPermission) -> Result<()> {
        let path = self.pending_permission_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, permission)
    }

    fn read_pending_permission(&self, permission_id: &str) -> Result<Option<PendingPermission>> {
        Ok(self
            .read_pending_permissions()?
            .into_iter()
            .find(|permission| permission.id == permission_id))
    }

    fn read_pending_permissions(&self) -> Result<Vec<PendingPermission>> {
        let file = match fs::File::open(self.pending_permission_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut permissions_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let permission: PendingPermission = serde_json::from_str(&line)?;
            permissions_by_id.insert(permission.id.clone(), permission);
        }
        Ok(permissions_by_id.into_values().collect())
    }

    fn hydrate_pending_permissions(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        for permission in self.read_pending_permissions()? {
            inner
                .pending_permissions
                .insert(permission.id.clone(), permission);
        }
        Ok(())
    }

    pub fn review_finding_log_path(&self) -> PathBuf {
        (*self.review_finding_log_path).clone()
    }

    fn append_review_finding_snapshot(&self, finding: &ReviewFinding) -> Result<()> {
        let path = self.review_finding_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, finding)
    }

    fn read_review_finding(&self, finding_id: &str) -> Result<Option<ReviewFinding>> {
        Ok(self
            .read_review_findings()?
            .into_iter()
            .find(|finding| finding.id == finding_id))
    }

    fn read_review_findings(&self) -> Result<Vec<ReviewFinding>> {
        let file = match fs::File::open(self.review_finding_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut findings_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let finding: ReviewFinding = serde_json::from_str(&line)?;
            findings_by_id.insert(finding.id.clone(), finding);
        }
        Ok(findings_by_id.into_values().collect())
    }

    fn hydrate_review_findings(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        for finding in self.read_review_findings()? {
            inner
                .project_review_findings
                .entry(finding.project_id.clone())
                .or_default()
                .push(finding.id.clone());
            inner
                .review_findings
                .entry(finding.id.clone())
                .or_insert(finding);
        }
        for finding_ids in inner.project_review_findings.values_mut() {
            let mut seen = HashSet::new();
            finding_ids.retain(|id| seen.insert(id.clone()));
        }
        Ok(())
    }

    pub fn repair_attempt_log_path(&self) -> PathBuf {
        (*self.repair_attempt_log_path).clone()
    }

    fn append_repair_attempt_snapshot(&self, attempt: &RepairAttempt) -> Result<()> {
        let path = self.repair_attempt_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, attempt)
    }

    fn read_repair_attempts(&self) -> Result<Vec<RepairAttempt>> {
        let file = match fs::File::open(self.repair_attempt_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut attempts = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            attempts.push(serde_json::from_str(&line)?);
        }
        Ok(attempts)
    }

    fn hydrate_repair_attempts(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        if !inner.repair_attempts.is_empty() {
            return Ok(());
        }
        for attempt in self.read_repair_attempts()? {
            inner.repair_attempts.push(attempt);
        }
        Ok(())
    }

    pub async fn register_run_scoped_resource(
        &self,
        run_id: &str,
        kind: RunScopedResourceKind,
        id: impl Into<String>,
    ) -> Result<()> {
        let mut inner = self.inner.write().await;
        if !inner.runs.contains_key(run_id) {
            return Err(anyhow!("run not found: {run_id}"));
        }
        let resources = inner
            .run_scoped_resources
            .entry(run_id.to_string())
            .or_default();
        let id = id.into();
        match kind {
            RunScopedResourceKind::McpServer => resources.mcp_servers.push(id),
            RunScopedResourceKind::BackgroundShellTask => resources.background_shell_tasks.push(id),
            RunScopedResourceKind::TemporaryHook => resources.temporary_hooks.push(id),
            RunScopedResourceKind::ReadFileCache => resources.read_file_cache_entries.push(id),
            RunScopedResourceKind::SandboxLock => resources.sandbox_locks.push(id),
        }
        Ok(())
    }

    pub async fn run_scoped_resources(&self, run_id: &str) -> RunScopedResources {
        self.inner
            .read()
            .await
            .run_scoped_resources
            .get(run_id)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn create_permission_request(
        &self,
        project_id: &str,
        run_id: &str,
        tool: &str,
    ) -> PendingPermission {
        let request = PendingPermission {
            id: self.next_id("permission"),
            project_id: project_id.to_string(),
            run_id: run_id.to_string(),
            tool: tool.to_string(),
            status: "pending".to_string(),
            created_at: Utc::now(),
            resolved_at: None,
        };
        let mut inner = self.inner.write().await;
        inner
            .pending_permissions
            .insert(request.id.clone(), request.clone());
        drop(inner);
        if let Err(error) = self.append_pending_permission_snapshot(&request) {
            eprintln!(
                "failed to append pending permission snapshot {}: {error}",
                request.id
            );
        }
        request
    }

    pub async fn resolve_permission(
        &self,
        permission_id: &str,
        decision: &str,
    ) -> Result<PendingPermission> {
        let mut inner = self.inner.write().await;
        if !inner.pending_permissions.contains_key(permission_id) {
            if let Some(permission) = self.read_pending_permission(permission_id)? {
                inner
                    .pending_permissions
                    .insert(permission.id.clone(), permission);
            }
        }
        let permission = inner
            .pending_permissions
            .get_mut(permission_id)
            .ok_or_else(|| anyhow!("permission request not found: {permission_id}"))?;
        if permission.status != "pending" {
            return Err(anyhow!(
                "permission request {permission_id} is already {}",
                permission.status
            ));
        }
        permission.status = decision.to_string();
        permission.resolved_at = Some(Utc::now());
        let permission = permission.clone();
        drop(inner);
        self.append_pending_permission_snapshot(&permission)?;
        Ok(permission)
    }

    pub async fn pending_permission(&self, permission_id: &str) -> Option<PendingPermission> {
        if let Some(permission) = self
            .inner
            .read()
            .await
            .pending_permissions
            .get(permission_id)
            .cloned()
        {
            return Some(permission);
        }
        let permission = self.read_pending_permission(permission_id).ok().flatten()?;
        self.inner
            .write()
            .await
            .pending_permissions
            .insert(permission.id.clone(), permission.clone());
        Some(permission)
    }

    pub async fn record_review_finding(
        &self,
        project_id: &str,
        run_id: &str,
        version_id: &str,
        severity: ReviewFindingSeverity,
        category: ReviewFindingCategory,
        summary: impl Into<String>,
        evidence: Option<ReviewFindingEvidence>,
        repairable: bool,
    ) -> Result<ReviewFinding> {
        {
            let mut inner = self.inner.write().await;
            self.hydrate_persisted_runs(&mut inner)?;
            self.hydrate_review_findings(&mut inner)?;
            if !inner.project_versions.contains_key(version_id) {
                if let Some(version) = self.read_project_version(version_id)? {
                    inner.project_versions.insert(version.id.clone(), version);
                }
            }
            let run = inner
                .runs
                .get(run_id)
                .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
            if run.project_id != project_id {
                return Err(anyhow!("run does not belong to project: {project_id}"));
            }
            let version = inner
                .project_versions
                .get(version_id)
                .ok_or_else(|| anyhow!("project version not found: {version_id}"))?;
            if version.project_id != project_id {
                return Err(anyhow!(
                    "project version does not belong to project: {project_id}"
                ));
            }
        }

        let summary = summary.into();
        let finding = ReviewFinding {
            id: self.next_id("finding"),
            project_id: project_id.to_string(),
            run_id: run_id.to_string(),
            version_id: version_id.to_string(),
            severity,
            category,
            summary: summary.clone(),
            evidence,
            repairable,
            status: ReviewFindingStatus::Open,
            created_at: Utc::now(),
        };
        {
            let mut inner = self.inner.write().await;
            inner
                .project_review_findings
                .entry(project_id.to_string())
                .or_default()
                .push(finding.id.clone());
            inner
                .review_findings
                .insert(finding.id.clone(), finding.clone());
        }
        self.append_review_finding_snapshot(&finding)?;
        let _ = self
            .append_event(AgentEvent::ReviewFinding {
                run_id: run_id.to_string(),
                finding_id: finding.id.clone(),
                severity: severity.as_str().to_string(),
                summary,
                timestamp: Utc::now(),
            })
            .await;
        self.append_conversation_item(
            project_id,
            Some(run_id),
            "review_finding",
            Some("assistant"),
            finding.summary.clone(),
            Some(serde_json::json!({ "findingId": finding.id, "severity": severity.as_str() })),
        )
        .await;
        Ok(finding)
    }

    pub async fn get_review_finding(&self, finding_id: &str) -> Option<ReviewFinding> {
        if let Some(finding) = self
            .inner
            .read()
            .await
            .review_findings
            .get(finding_id)
            .cloned()
        {
            return Some(finding);
        }
        let finding = self.read_review_finding(finding_id).ok().flatten()?;
        let mut inner = self.inner.write().await;
        inner
            .project_review_findings
            .entry(finding.project_id.clone())
            .or_default()
            .push(finding.id.clone());
        inner
            .review_findings
            .insert(finding.id.clone(), finding.clone());
        Some(finding)
    }

    pub async fn update_review_finding_status(
        &self,
        finding_id: &str,
        status: ReviewFindingStatus,
    ) -> Result<ReviewFinding> {
        let mut inner = self.inner.write().await;
        self.hydrate_review_findings(&mut inner)?;
        let finding = inner
            .review_findings
            .get_mut(finding_id)
            .ok_or_else(|| anyhow!("review finding not found: {finding_id}"))?;
        finding.status = status;
        let finding = finding.clone();
        drop(inner);
        self.append_review_finding_snapshot(&finding)?;
        Ok(finding)
    }

    pub async fn open_blocking_findings(
        &self,
        project_id: &str,
        version_id: &str,
    ) -> Vec<ReviewFinding> {
        let mut findings_by_id = self
            .read_review_findings()
            .unwrap_or_default()
            .into_iter()
            .map(|finding| (finding.id.clone(), finding))
            .collect::<HashMap<_, _>>();
        for finding in self.inner.read().await.review_findings.values().cloned() {
            findings_by_id.insert(finding.id.clone(), finding);
        }
        findings_by_id
            .into_values()
            .filter(|finding| {
                finding.project_id == project_id
                    && finding.version_id == version_id
                    && finding.severity == ReviewFindingSeverity::Blocking
                    && matches!(
                        finding.status,
                        ReviewFindingStatus::Open
                            | ReviewFindingStatus::Repairing
                            | ReviewFindingStatus::NeedsUserInput
                    )
            })
            .collect()
    }

    pub async fn record_repair_attempt(&self, attempt: RepairAttempt) -> Result<()> {
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        self.hydrate_review_findings(&mut inner)?;
        self.hydrate_repair_attempts(&mut inner)?;
        let repair_run = inner
            .runs
            .get(&attempt.repair_run_id)
            .ok_or_else(|| anyhow!("repair run not found: {}", attempt.repair_run_id))?;
        if repair_run.parent_run_id.as_deref() != Some(attempt.parent_run_id.as_str()) {
            return Err(anyhow!(
                "repair run does not belong to parent run: {}",
                attempt.parent_run_id
            ));
        }
        let finding = inner
            .review_findings
            .get(&attempt.finding_id)
            .ok_or_else(|| anyhow!("review finding not found: {}", attempt.finding_id))?;
        if !matches!(
            finding.status,
            ReviewFindingStatus::Open | ReviewFindingStatus::Repairing
        ) {
            return Err(anyhow!(
                "repair attempt requires an open or repairing finding: {}",
                attempt.finding_id
            ));
        }
        inner.repair_attempts.push(attempt.clone());
        drop(inner);
        self.append_repair_attempt_snapshot(&attempt)?;
        Ok(())
    }

    pub async fn repair_attempts_for_finding(
        &self,
        parent_run_id: &str,
        finding_id: &str,
    ) -> Vec<RepairAttempt> {
        let memory_attempts = self.inner.read().await.repair_attempts.clone();
        let attempts = if memory_attempts.is_empty() {
            self.read_repair_attempts().unwrap_or_default()
        } else {
            memory_attempts
        };
        attempts
            .into_iter()
            .filter(|attempt| {
                attempt.parent_run_id == parent_run_id && attempt.finding_id == finding_id
            })
            .collect()
    }

    pub async fn save_checkpoint(&self, checkpoint: AgentCheckpoint) -> Result<()> {
        fs::create_dir_all(&*self.checkpoint_dir)?;
        let path = self.checkpoint_path(&checkpoint.id);
        let json = serde_json::to_string_pretty(&checkpoint)?;
        fs::write(path, json)?;

        {
            let mut inner = self.inner.write().await;
            inner
                .run_checkpoints
                .entry(checkpoint.run_id.clone())
                .or_default()
                .push(checkpoint.id.clone());
            inner
                .checkpoints
                .insert(checkpoint.id.clone(), checkpoint.clone());
        }
        self.set_run_checkpoint(&checkpoint.run_id, checkpoint.id.clone())
            .await?;
        Ok(())
    }

    pub async fn get_checkpoint(&self, checkpoint_id: &str) -> Option<AgentCheckpoint> {
        if let Some(checkpoint) = self
            .inner
            .read()
            .await
            .checkpoints
            .get(checkpoint_id)
            .cloned()
        {
            return Some(checkpoint);
        }

        let path = self.checkpoint_path(checkpoint_id);
        let json = fs::read_to_string(path).ok()?;
        serde_json::from_str(&json).ok()
    }

    pub async fn latest_checkpoint_for_run(&self, run_id: &str) -> Option<AgentCheckpoint> {
        let checkpoint_id = self
            .inner
            .read()
            .await
            .run_checkpoints
            .get(run_id)
            .and_then(|ids| ids.last())
            .cloned();
        match checkpoint_id {
            Some(checkpoint_id) => self.get_checkpoint(&checkpoint_id).await,
            None => self
                .get_run(run_id)
                .await
                .and_then(|run| run.checkpoint_id)
                .and_then(|checkpoint_id| {
                    fs::read_to_string(self.checkpoint_path(&checkpoint_id))
                        .ok()
                        .and_then(|json| serde_json::from_str(&json).ok())
                }),
        }
    }

    pub fn checkpoint_path(&self, checkpoint_id: &str) -> PathBuf {
        checkpoint_path(&self.checkpoint_dir, checkpoint_id)
    }

    pub fn run_log_path(&self, run_id: &str) -> PathBuf {
        self.run_log_dir.join(run_id).join("run-log.jsonl")
    }

    pub fn run_state_log_path(&self) -> PathBuf {
        (*self.run_state_log_path).clone()
    }

    fn append_run_snapshot(&self, run: &AgentRun) -> Result<()> {
        let path = self.run_state_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, run)
    }

    fn read_run(&self, run_id: &str) -> Result<Option<AgentRun>> {
        Ok(self.read_runs()?.into_iter().find(|run| run.id == run_id))
    }

    fn read_runs(&self) -> Result<Vec<AgentRun>> {
        let file = match fs::File::open(self.run_state_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut runs_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let run: AgentRun = serde_json::from_str(&line)?;
            runs_by_id.insert(run.id.clone(), run);
        }
        Ok(runs_by_id.into_values().collect())
    }

    fn hydrate_persisted_runs(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        for run in self.read_runs()? {
            match inner.runs.entry(run.id.clone()) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(run);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if run_snapshot_is_newer(&run, entry.get()) {
                        entry.insert(run);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn audit_log_path(&self) -> PathBuf {
        (*self.audit_log_path).clone()
    }

    pub fn project_version_log_path(&self) -> PathBuf {
        (*self.project_version_log_path).clone()
    }

    fn append_project_version_snapshot(&self, version: &ProjectVersion) -> Result<()> {
        let path = self.project_version_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, version)
    }

    fn project_runtime_state_log_path(&self) -> PathBuf {
        (*self.project_runtime_state_log_path).clone()
    }

    fn append_project_runtime_state_snapshot(&self, state: &ProjectRuntimeState) -> Result<()> {
        let path = self.project_runtime_state_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, state)
    }

    fn read_project_runtime_states(&self) -> Result<Vec<ProjectRuntimeState>> {
        let file = match fs::File::open(self.project_runtime_state_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut states_by_project = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let state: ProjectRuntimeState = serde_json::from_str(&line)?;
            states_by_project.insert(state.project_id.clone(), state);
        }
        Ok(states_by_project.into_values().collect())
    }

    pub async fn get_project_runtime_state(&self, project_id: &str) -> Option<ProjectRuntimeState> {
        if let Some(state) = self
            .inner
            .read()
            .await
            .project_runtime_states
            .get(project_id)
            .cloned()
        {
            return Some(state);
        }
        let state = self
            .read_project_runtime_states()
            .ok()?
            .into_iter()
            .find(|state| state.project_id == project_id)?;
        self.inner
            .write()
            .await
            .project_runtime_states
            .insert(project_id.to_string(), state.clone());
        Some(state)
    }

    fn project_access_log_path(&self) -> PathBuf {
        (*self.project_access_log_path).clone()
    }

    fn append_project_access_snapshot(&self, record: &ProjectAccessRecord) -> Result<()> {
        let path = self.project_access_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, record)
    }

    fn read_project_access_records(&self) -> Result<Vec<ProjectAccessRecord>> {
        read_latest_jsonl_by_key(
            &self.project_access_log_path,
            |record: &ProjectAccessRecord| record.project_id.clone(),
        )
    }

    pub async fn get_project_access(&self, project_id: &str) -> Option<ProjectAccessRecord> {
        if let Some(record) = self
            .inner
            .read()
            .await
            .project_access_records
            .get(project_id)
            .cloned()
        {
            return Some(record);
        }
        let record = self
            .read_project_access_records()
            .ok()?
            .into_iter()
            .find(|record| record.project_id == project_id)?;
        self.inner
            .write()
            .await
            .project_access_records
            .insert(project_id.to_string(), record.clone());
        Some(record)
    }

    pub async fn upsert_project_access(
        &self,
        project_id: &str,
        owner_principal_id: String,
        workspace_id: Option<String>,
        organization_id: Option<String>,
    ) -> Result<ProjectAccessRecord> {
        if project_id.trim().is_empty() || owner_principal_id.trim().is_empty() {
            return Err(anyhow!(
                "project access requires project and owner principal ids"
            ));
        }
        let now = Utc::now();
        let created_at = self
            .get_project_access(project_id)
            .await
            .map(|record| record.created_at)
            .unwrap_or(now);
        let record = ProjectAccessRecord {
            project_id: project_id.to_string(),
            owner_principal_id,
            workspace_id,
            organization_id,
            created_at,
            updated_at: now,
        };
        self.inner
            .write()
            .await
            .project_access_records
            .insert(project_id.to_string(), record.clone());
        self.append_project_access_snapshot(&record)?;
        Ok(record)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_project_runtime_state(
        &self,
        project_id: &str,
        app_root: String,
        template_key: String,
        template_version: String,
        framework: String,
        package_manager: String,
        lockfile: String,
        registry: String,
    ) -> Result<ProjectRuntimeState> {
        let revision = self
            .get_project_runtime_state(project_id)
            .await
            .map(|state| state.revision + 1)
            .unwrap_or(1);
        let state = ProjectRuntimeState {
            project_id: project_id.to_string(),
            revision,
            app_root,
            template_key,
            template_version,
            framework,
            package_manager,
            lockfile,
            registry,
            updated_at: Utc::now(),
        };
        self.inner
            .write()
            .await
            .project_runtime_states
            .insert(project_id.to_string(), state.clone());
        self.append_project_runtime_state_snapshot(&state)?;
        Ok(state)
    }

    pub async fn set_run_project_state_snapshot(
        &self,
        run_id: &str,
        state: ProjectRuntimeState,
    ) -> Result<AgentRun> {
        let run = {
            let mut inner = self.inner.write().await;
            self.hydrate_persisted_runs(&mut inner)?;
            let run = inner
                .runs
                .get_mut(run_id)
                .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
            run.project_state_snapshot = Some(state);
            run.updated_at = Utc::now();
            run.clone()
        };
        self.append_run_snapshot(&run)?;
        Ok(run)
    }

    fn preview_lease_log_path(&self) -> PathBuf {
        (*self.preview_lease_log_path).clone()
    }

    fn append_preview_lease_snapshot(&self, lease: &PreviewLeaseRecord) -> Result<()> {
        let path = self.preview_lease_log_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl(&path, lease)
    }

    fn read_preview_leases(&self) -> Result<Vec<PreviewLeaseRecord>> {
        let file = match fs::File::open(self.preview_lease_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut leases_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let lease: PreviewLeaseRecord = serde_json::from_str(&line)?;
            leases_by_id.insert(lease.id.clone(), lease);
        }
        Ok(leases_by_id.into_values().collect())
    }

    pub async fn create_preview_lease(
        &self,
        run_id: &str,
        build_id: String,
        candidate_manifest_hash: String,
        ttl_seconds: u64,
    ) -> Result<PreviewLeaseRecord> {
        let run = self
            .get_run(run_id)
            .await
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        let binding_id = run
            .sandbox_id
            .as_deref()
            .ok_or_else(|| anyhow!("run is not bound to a sandbox"))?;
        let binding = self
            .get_sandbox_binding(binding_id)
            .await
            .ok_or_else(|| anyhow!("sandbox binding not found: {binding_id}"))?;
        let pod_uid = binding
            .pod_uid
            .clone()
            .ok_or_else(|| anyhow!("sandbox binding has no verified pod UID"))?;
        let now = Utc::now();
        let lease = PreviewLeaseRecord {
            id: sha256_hex(&rand::random::<[u8; 32]>()),
            project_id: run.project_id,
            run_id: run.id,
            sandbox_binding_id: binding.id,
            sandbox_name: binding.sandbox_name,
            pod_uid,
            build_id,
            candidate_manifest_hash,
            status: PreviewLeaseStatus::Active,
            created_at: now,
            expires_at: now + Duration::seconds(ttl_seconds.clamp(30, 3600) as i64),
        };
        self.inner
            .write()
            .await
            .preview_leases
            .insert(lease.id.clone(), lease.clone());
        self.append_preview_lease_snapshot(&lease)?;
        Ok(lease)
    }

    pub async fn get_preview_lease(&self, lease_id: &str) -> Option<PreviewLeaseRecord> {
        let mut lease = self
            .inner
            .read()
            .await
            .preview_leases
            .get(lease_id)
            .cloned()
            .or_else(|| {
                self.read_preview_leases()
                    .ok()?
                    .into_iter()
                    .find(|lease| lease.id == lease_id)
            })?;
        if lease.status == PreviewLeaseStatus::Active && lease.expires_at <= Utc::now() {
            lease.status = PreviewLeaseStatus::Expired;
            self.inner
                .write()
                .await
                .preview_leases
                .insert(lease.id.clone(), lease.clone());
            self.append_preview_lease_snapshot(&lease).ok();
        }
        Some(lease)
    }

    pub async fn stop_preview_lease(&self, lease_id: &str) -> Result<PreviewLeaseRecord> {
        let mut lease = self
            .get_preview_lease(lease_id)
            .await
            .ok_or_else(|| anyhow!("preview lease not found: {lease_id}"))?;
        lease.status = PreviewLeaseStatus::Stopped;
        self.inner
            .write()
            .await
            .preview_leases
            .insert(lease.id.clone(), lease.clone());
        self.append_preview_lease_snapshot(&lease)?;
        Ok(lease)
    }

    pub async fn stop_preview_leases_for_binding(&self, binding_id: &str) -> Result<usize> {
        let leases = self.active_preview_leases_for_binding(binding_id).await?;
        for lease in &leases {
            self.stop_preview_lease(&lease.id).await?;
        }
        Ok(leases.len())
    }

    pub async fn active_preview_leases_for_binding(
        &self,
        binding_id: &str,
    ) -> Result<Vec<PreviewLeaseRecord>> {
        let mut leases = self.read_preview_leases()?;
        {
            let inner = self.inner.read().await;
            for lease in inner.preview_leases.values() {
                if let Some(existing) = leases.iter_mut().find(|item| item.id == lease.id) {
                    *existing = lease.clone();
                } else {
                    leases.push(lease.clone());
                }
            }
        }
        let leases = leases
            .into_iter()
            .filter(|lease| {
                lease.sandbox_binding_id == binding_id && lease.status == PreviewLeaseStatus::Active
            })
            .collect::<Vec<_>>();
        Ok(leases)
    }

    pub async fn preview_lease_for_run(&self, run_id: &str) -> Option<PreviewLeaseRecord> {
        let mut leases = self.read_preview_leases().ok()?;
        leases.extend(self.inner.read().await.preview_leases.values().cloned());
        leases
            .into_iter()
            .filter(|lease| lease.run_id == run_id)
            .max_by_key(|lease| lease.created_at)
    }

    fn append_channel_lease_snapshot(&self, lease: &ChannelLeaseRecord) -> Result<()> {
        if let Some(parent) = self.channel_lease_log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl_synced(&self.channel_lease_log_path, lease)
    }

    fn read_channel_leases(&self) -> Result<Vec<ChannelLeaseRecord>> {
        read_latest_jsonl_by_key(
            &self.channel_lease_log_path,
            |lease: &ChannelLeaseRecord| lease.id.clone(),
        )
    }

    fn hydrate_channel_leases(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        for lease in self.read_channel_leases()? {
            inner.channel_leases.insert(lease.id.clone(), lease);
        }
        Ok(())
    }

    pub async fn put_channel_lease(&self, lease: ChannelLeaseRecord) -> Result<ChannelLeaseRecord> {
        {
            let mut inner = self.inner.write().await;
            self.hydrate_channel_leases(&mut inner)?;
            if let Some(previous) = inner.channel_leases.get(&lease.id) {
                if !channel_lease_transition_allowed(previous.status, lease.status) {
                    return Err(anyhow!(
                        "invalid channel lease transition {:?} -> {:?}: {}",
                        previous.status,
                        lease.status,
                        lease.id
                    ));
                }
            }
            inner.channel_leases.insert(lease.id.clone(), lease.clone());
        }
        self.append_channel_lease_snapshot(&lease)?;
        Ok(lease)
    }

    pub async fn get_channel_lease(&self, lease_id: &str) -> Option<ChannelLeaseRecord> {
        let mut inner = self.inner.write().await;
        self.hydrate_channel_leases(&mut inner).ok()?;
        inner.channel_leases.get(lease_id).cloned()
    }

    pub async fn channel_leases(&self) -> Result<Vec<ChannelLeaseRecord>> {
        let mut inner = self.inner.write().await;
        self.hydrate_channel_leases(&mut inner)?;
        Ok(inner.channel_leases.values().cloned().collect())
    }

    pub async fn active_channel_lease(
        &self,
        binding_id: &str,
        target_port: u16,
    ) -> Result<Option<ChannelLeaseRecord>> {
        Ok(self.channel_leases().await?.into_iter().find(|lease| {
            lease.sandbox_binding_id == binding_id
                && lease.target_port == target_port
                && lease.status == ChannelLeaseStatus::Ready
        }))
    }

    fn append_artifact_publish_snapshot(&self, publish: &ArtifactPublishRecord) -> Result<()> {
        if let Some(parent) = self.artifact_publish_log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl_synced(&self.artifact_publish_log_path, publish)
    }

    fn read_artifact_publishes(&self) -> Result<Vec<ArtifactPublishRecord>> {
        read_latest_jsonl_by_key(
            &self.artifact_publish_log_path,
            |publish: &ArtifactPublishRecord| publish.id.clone(),
        )
    }

    fn read_promotion_commits(&self) -> Result<Vec<ArtifactPromotionCommit>> {
        read_latest_jsonl_by_key(
            &self.promotion_commit_log_path,
            |commit: &ArtifactPromotionCommit| commit.id.clone(),
        )
    }

    fn read_outbox_events(&self) -> Result<Vec<RuntimeOutboxEvent>> {
        read_latest_jsonl_by_key(&self.outbox_log_path, |event: &RuntimeOutboxEvent| {
            event.id.clone()
        })
    }

    fn hydrate_artifact_transactions(&self, inner: &mut RuntimeStoreInner) -> Result<()> {
        for publish in self.read_artifact_publishes()? {
            inner.artifact_publishes.insert(publish.id.clone(), publish);
        }
        let mut commits = self.read_promotion_commits()?;
        commits.sort_by_key(|commit| commit.committed_at);
        for commit in commits {
            inner
                .project_current_versions
                .insert(commit.project_id.clone(), commit.version.id.clone());
            inner
                .project_versions
                .insert(commit.version.id.clone(), commit.version);
            let replace_run = inner
                .runs
                .get(&commit.run.id)
                .is_none_or(|run| run.updated_at <= commit.run.updated_at);
            if replace_run {
                inner.runs.insert(commit.run.id.clone(), commit.run);
            }
            let replace_publish = inner
                .artifact_publishes
                .get(&commit.publish.id)
                .is_none_or(|publish| publish.revision <= commit.publish.revision);
            if replace_publish {
                inner
                    .artifact_publishes
                    .insert(commit.publish.id.clone(), commit.publish);
            }
            inner
                .outbox_events
                .entry(commit.outbox.id.clone())
                .or_insert(commit.outbox);
        }
        for event in self.read_outbox_events()? {
            inner.outbox_events.insert(event.id.clone(), event);
        }
        Ok(())
    }

    pub async fn begin_artifact_publish(
        &self,
        project_id: &str,
        run_id: &str,
        build_id: &str,
        version_id: &str,
        candidate_manifest_hash: &str,
        source_snapshot_uri: &str,
        expected_current_version_id: Option<&str>,
    ) -> Result<ArtifactPublishRecord> {
        let idempotency_key = format!("{project_id}/{run_id}/{build_id}");
        let run = self
            .get_run(run_id)
            .await
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        if run.project_id != project_id {
            return Err(anyhow!("run does not belong to project: {project_id}"));
        }
        let binding = match run.sandbox_id.as_deref() {
            Some(binding_id) => self.get_sandbox_binding(binding_id).await,
            None => None,
        };
        let mut inner = self.inner.write().await;
        self.hydrate_artifact_transactions(&mut inner)?;
        if let Some(existing) = inner
            .artifact_publishes
            .values()
            .find(|publish| publish.idempotency_key == idempotency_key)
            .cloned()
        {
            if existing.version_id != version_id
                || existing.candidate_manifest_hash != candidate_manifest_hash
                || existing.source_snapshot_uri != source_snapshot_uri
            {
                return Err(anyhow!(
                    "artifact publish idempotency key already belongs to different content"
                ));
            }
            return Ok(existing);
        }
        let now = Utc::now();
        let publish = ArtifactPublishRecord {
            id: self.next_id("publish"),
            idempotency_key,
            project_id: project_id.to_string(),
            run_id: run_id.to_string(),
            build_id: build_id.to_string(),
            version_id: version_id.to_string(),
            sandbox_binding_id: binding.as_ref().map(|binding| binding.id.clone()),
            pod_uid: binding.and_then(|binding| binding.pod_uid),
            candidate_manifest_hash: candidate_manifest_hash.to_string(),
            artifact_manifest_hash: None,
            source_snapshot_uri: source_snapshot_uri.to_string(),
            expected_current_version_id: expected_current_version_id.map(str::to_string),
            status: ArtifactPublishStatus::Staging,
            revision: 1,
            staged_uri: None,
            immutable_artifact_uri: None,
            last_error: None,
            created_at: now,
            updated_at: now,
            gc_after: None,
        };
        self.append_artifact_publish_snapshot(&publish)?;
        inner
            .artifact_publishes
            .insert(publish.id.clone(), publish.clone());
        Ok(publish)
    }

    pub async fn transition_artifact_publish(
        &self,
        publish_id: &str,
        status: ArtifactPublishStatus,
        artifact_manifest_hash: Option<&str>,
        staged_uri: Option<&str>,
        immutable_artifact_uri: Option<&str>,
        last_error: Option<&str>,
    ) -> Result<ArtifactPublishRecord> {
        let mut inner = self.inner.write().await;
        self.hydrate_artifact_transactions(&mut inner)?;
        let current = inner
            .artifact_publishes
            .get(publish_id)
            .cloned()
            .ok_or_else(|| anyhow!("artifact publish not found: {publish_id}"))?;
        if !artifact_publish_transition_allowed(current.status, status) {
            return Err(anyhow!(
                "invalid artifact publish transition: {:?} -> {:?}",
                current.status,
                status
            ));
        }
        let mut publish = current;
        publish.status = status;
        publish.revision += 1;
        publish.updated_at = Utc::now();
        if let Some(hash) = artifact_manifest_hash {
            publish.artifact_manifest_hash = Some(hash.to_string());
        }
        if let Some(uri) = staged_uri {
            publish.staged_uri = Some(uri.to_string());
        }
        if let Some(uri) = immutable_artifact_uri {
            publish.immutable_artifact_uri = Some(uri.to_string());
        }
        if let Some(error) = last_error {
            publish.last_error = Some(error.to_string());
        }
        if matches!(
            status,
            ArtifactPublishStatus::GarbageCollectable | ArtifactPublishStatus::Failed
        ) {
            publish.gc_after = Some(Utc::now() + Duration::hours(24));
        }
        self.append_artifact_publish_snapshot(&publish)?;
        inner
            .artifact_publishes
            .insert(publish.id.clone(), publish.clone());
        Ok(publish)
    }

    pub async fn get_artifact_publish(&self, publish_id: &str) -> Option<ArtifactPublishRecord> {
        let mut inner = self.inner.write().await;
        self.hydrate_artifact_transactions(&mut inner).ok()?;
        inner.artifact_publishes.get(publish_id).cloned()
    }

    pub async fn garbage_collectable_artifact_publishes(
        &self,
        now: chrono::DateTime<Utc>,
    ) -> Result<Vec<ArtifactPublishRecord>> {
        let mut inner = self.inner.write().await;
        self.hydrate_artifact_transactions(&mut inner)?;
        Ok(inner
            .artifact_publishes
            .values()
            .filter(|publish| {
                publish.status == ArtifactPublishStatus::GarbageCollectable
                    && publish.gc_after.is_some_and(|gc_after| gc_after <= now)
            })
            .cloned()
            .collect())
    }

    pub async fn artifact_publish_for_version(
        &self,
        project_id: &str,
        run_id: &str,
        version_id: &str,
    ) -> Option<ArtifactPublishRecord> {
        let mut inner = self.inner.write().await;
        self.hydrate_artifact_transactions(&mut inner).ok()?;
        inner
            .artifact_publishes
            .values()
            .filter(|publish| {
                publish.project_id == project_id
                    && publish.run_id == run_id
                    && publish.version_id == version_id
            })
            .max_by_key(|publish| publish.revision)
            .cloned()
    }

    pub async fn commit_artifact_promotion_cas(
        &self,
        project_id: &str,
        run_id: &str,
        version_id: &str,
        publish_id: &str,
        expected_current_version_id: Option<&str>,
    ) -> Result<(ProjectVersion, RuntimeOutboxEvent)> {
        let persisted_current = self
            .read_current_project_version(project_id)?
            .map(|version| version.id);
        let mut inner = self.inner.write().await;
        self.hydrate_persisted_runs(&mut inner)?;
        self.hydrate_artifact_transactions(&mut inner)?;
        let actual_current = inner
            .project_current_versions
            .get(project_id)
            .cloned()
            .or(persisted_current);
        if actual_current.as_deref() == Some(version_id) {
            let version = inner.project_versions.get(version_id).cloned();
            let publish = inner.artifact_publishes.get(publish_id);
            if let (Some(version), Some(publish)) = (version, publish) {
                if version.project_id == project_id
                    && version.created_by_run_id == run_id
                    && version.status == ProjectVersionStatus::Promoted
                    && publish.project_id == project_id
                    && publish.run_id == run_id
                    && publish.version_id == version_id
                    && publish.status == ArtifactPublishStatus::Promoted
                {
                    let outbox_id = preview_updated_outbox_id(project_id, version_id);
                    let outbox = inner
                        .outbox_events
                        .get(&outbox_id)
                        .cloned()
                        .ok_or_else(|| anyhow!("promoted artifact is missing outbox event"))?;
                    return Ok((version, outbox));
                }
            }
        }
        if actual_current.as_deref() != expected_current_version_id {
            return Err(anyhow!(
                "project current version compare-and-swap failed: expected {:?}, actual {:?}",
                expected_current_version_id,
                actual_current
            ));
        }
        if !inner.project_versions.contains_key(version_id) {
            if let Some(version) = self.read_project_version(version_id)? {
                inner.project_versions.insert(version.id.clone(), version);
            }
        }
        let mut version = inner
            .project_versions
            .get(version_id)
            .cloned()
            .ok_or_else(|| anyhow!("project version not found: {version_id}"))?;
        if version.project_id != project_id || version.created_by_run_id != run_id {
            return Err(anyhow!("project version ownership mismatch"));
        }
        let mut run = inner
            .runs
            .get(run_id)
            .cloned()
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        let mut publish = inner
            .artifact_publishes
            .get(publish_id)
            .cloned()
            .ok_or_else(|| anyhow!("artifact publish not found: {publish_id}"))?;
        if publish.project_id != project_id
            || publish.run_id != run_id
            || publish.version_id != version_id
        {
            return Err(anyhow!("artifact publish ownership mismatch"));
        }
        if publish.status == ArtifactPublishStatus::Promoted
            && version.status == ProjectVersionStatus::Promoted
        {
            let outbox_id = preview_updated_outbox_id(project_id, version_id);
            let outbox = inner
                .outbox_events
                .get(&outbox_id)
                .cloned()
                .ok_or_else(|| anyhow!("promoted artifact is missing outbox event"))?;
            return Ok((version, outbox));
        }
        if version.status != ProjectVersionStatus::Candidate {
            return Err(anyhow!("only candidate versions can be promoted"));
        }
        if publish.status != ArtifactPublishStatus::Promoting
            || publish.immutable_artifact_uri.is_none()
        {
            return Err(anyhow!(
                "artifact publish must be promoting with immutable bytes before CAS"
            ));
        }
        let now = Utc::now();
        version.status = ProjectVersionStatus::Promoted;
        version.promoted_at = Some(now);
        run.output_version_id = Some(version_id.to_string());
        run.updated_at = now;
        publish.status = ArtifactPublishStatus::Promoted;
        publish.revision += 1;
        publish.updated_at = now;
        let outbox = RuntimeOutboxEvent {
            id: preview_updated_outbox_id(project_id, version_id),
            project_id: project_id.to_string(),
            run_id: run_id.to_string(),
            event: AgentEvent::PreviewUpdated {
                run_id: run_id.to_string(),
                url: version.preview_url.clone(),
                version_id: version.id.clone(),
                screenshot_id: version.screenshot_id.clone(),
                timestamp: now,
            },
            status: OutboxDeliveryStatus::Pending,
            delivery_attempts: 0,
            created_at: now,
            delivered_at: None,
        };
        let commit = ArtifactPromotionCommit {
            id: format!("promotion:{project_id}:{version_id}"),
            project_id: project_id.to_string(),
            run_id: run_id.to_string(),
            version: version.clone(),
            run: run.clone(),
            publish: publish.clone(),
            outbox: outbox.clone(),
            committed_at: now,
        };
        if let Some(parent) = self.promotion_commit_log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl_synced(&self.promotion_commit_log_path, &commit)?;
        inner
            .project_current_versions
            .insert(project_id.to_string(), version_id.to_string());
        inner
            .project_versions
            .insert(version_id.to_string(), version.clone());
        inner.runs.insert(run_id.to_string(), run.clone());
        inner
            .artifact_publishes
            .insert(publish_id.to_string(), publish.clone());
        inner
            .outbox_events
            .insert(outbox.id.clone(), outbox.clone());
        drop(inner);
        self.append_project_version_snapshot(&version).ok();
        self.append_run_snapshot(&run).ok();
        self.append_artifact_publish_snapshot(&publish).ok();
        Ok((version, outbox))
    }

    pub async fn dispatch_outbox_event(&self, outbox_id: &str) -> Result<RuntimeOutboxEvent> {
        let mut outbox = {
            let mut inner = self.inner.write().await;
            self.hydrate_artifact_transactions(&mut inner)?;
            inner
                .outbox_events
                .get(outbox_id)
                .cloned()
                .ok_or_else(|| anyhow!("outbox event not found: {outbox_id}"))?
        };
        if outbox.status == OutboxDeliveryStatus::Delivered {
            return Ok(outbox);
        }
        let already_persisted = self.events(&outbox.run_id).await.iter().any(|event| {
            matches!(
                (event, &outbox.event),
                (
                    AgentEvent::PreviewUpdated { version_id: left, .. },
                    AgentEvent::PreviewUpdated { version_id: right, .. }
                ) if left == right
            )
        });
        outbox.delivery_attempts += 1;
        if !already_persisted {
            if let Err(error) = self.append_event(outbox.event.clone()).await {
                self.persist_outbox_snapshot(&outbox)?;
                self.inner
                    .write()
                    .await
                    .outbox_events
                    .insert(outbox.id.clone(), outbox);
                return Err(error);
            }
        }
        outbox.status = OutboxDeliveryStatus::Delivered;
        outbox.delivered_at = Some(Utc::now());
        self.persist_outbox_snapshot(&outbox)?;
        self.inner
            .write()
            .await
            .outbox_events
            .insert(outbox.id.clone(), outbox.clone());
        Ok(outbox)
    }

    fn persist_outbox_snapshot(&self, outbox: &RuntimeOutboxEvent) -> Result<()> {
        if let Some(parent) = self.outbox_log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        append_jsonl_synced(&self.outbox_log_path, outbox)
    }

    pub async fn reconcile_artifact_promotions(&self) -> Result<usize> {
        let recoverable_publishes = {
            let mut inner = self.inner.write().await;
            self.hydrate_artifact_transactions(&mut inner)?;
            inner
                .artifact_publishes
                .values()
                .filter(|publish| {
                    matches!(
                        publish.status,
                        ArtifactPublishStatus::Promoting | ArtifactPublishStatus::ReconcileRequired
                    ) && publish.immutable_artifact_uri.is_some()
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        for mut publish in recoverable_publishes {
            if publish.status == ArtifactPublishStatus::ReconcileRequired {
                publish = self
                    .transition_artifact_publish(
                        &publish.id,
                        ArtifactPublishStatus::Promoting,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await?;
            }
            if let Err(error) = self
                .commit_artifact_promotion_cas(
                    &publish.project_id,
                    &publish.run_id,
                    &publish.version_id,
                    &publish.id,
                    publish.expected_current_version_id.as_deref(),
                )
                .await
            {
                let error_text = error.to_string();
                let status = if error_text.contains("compare-and-swap failed") {
                    ArtifactPublishStatus::GarbageCollectable
                } else {
                    ArtifactPublishStatus::ReconcileRequired
                };
                self.transition_artifact_publish(
                    &publish.id,
                    status,
                    None,
                    None,
                    None,
                    Some(&error_text),
                )
                .await?;
            }
        }
        let pending = {
            let mut inner = self.inner.write().await;
            self.hydrate_artifact_transactions(&mut inner)?;
            inner
                .outbox_events
                .values()
                .filter(|event| event.status == OutboxDeliveryStatus::Pending)
                .map(|event| event.id.clone())
                .collect::<Vec<_>>()
        };
        let mut delivered = 0;
        for outbox_id in pending {
            self.dispatch_outbox_event(&outbox_id).await?;
            delivered += 1;
        }
        Ok(delivered)
    }

    fn read_project_version(&self, version_id: &str) -> Result<Option<ProjectVersion>> {
        Ok(self
            .read_project_versions()?
            .into_iter()
            .find(|version| version.id == version_id))
    }

    fn read_project_versions(&self) -> Result<Vec<ProjectVersion>> {
        let file = match fs::File::open(self.project_version_log_path()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut versions_by_id = HashMap::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let version: ProjectVersion = serde_json::from_str(&line)?;
            versions_by_id.insert(version.id.clone(), version);
        }
        Ok(versions_by_id.into_values().collect())
    }

    fn read_current_project_version(&self, project_id: &str) -> Result<Option<ProjectVersion>> {
        Ok(self
            .read_project_versions()?
            .into_iter()
            .filter(|version| {
                version.project_id == project_id && version.status == ProjectVersionStatus::Promoted
            })
            .max_by_key(|version| version.promoted_at.unwrap_or(version.created_at)))
    }

    pub async fn create_project_version_candidate(
        &self,
        project_id: &str,
        run_id: &str,
        preview_url: String,
        screenshot_id: Option<String>,
        source_snapshot_uri: Option<String>,
    ) -> ProjectVersion {
        let version = ProjectVersion {
            id: self.next_id("version"),
            project_id: project_id.to_string(),
            source_snapshot_uri,
            preview_url,
            screenshot_uri: screenshot_id
                .as_ref()
                .map(|id| format!("screenshots/{id}.png")),
            screenshot_id,
            status: ProjectVersionStatus::Candidate,
            created_by_run_id: run_id.to_string(),
            created_at: Utc::now(),
            promoted_at: None,
        };
        let mut inner = self.inner.write().await;
        inner
            .project_versions
            .insert(version.id.clone(), version.clone());
        drop(inner);
        if let Err(error) = self.append_project_version_snapshot(&version) {
            eprintln!(
                "failed to append project version snapshot {}: {error}",
                version.id
            );
        }
        version
    }

    pub async fn get_project_version(&self, version_id: &str) -> Option<ProjectVersion> {
        {
            let mut inner = self.inner.write().await;
            self.hydrate_artifact_transactions(&mut inner).ok()?;
            if let Some(version) = inner.project_versions.get(version_id).cloned() {
                return Some(version);
            }
        }
        let version = self.read_project_version(version_id).ok().flatten()?;
        self.inner
            .write()
            .await
            .project_versions
            .insert(version.id.clone(), version.clone());
        Some(version)
    }

    pub async fn promote_project_version(
        &self,
        project_id: &str,
        run_id: &str,
        version_id: &str,
    ) -> Result<ProjectVersion> {
        self.promote_project_version_inner(project_id, run_id, version_id, None)
            .await
    }

    pub async fn promote_project_version_cas(
        &self,
        project_id: &str,
        run_id: &str,
        version_id: &str,
        expected_current_version_id: Option<&str>,
    ) -> Result<ProjectVersion> {
        self.promote_project_version_inner(
            project_id,
            run_id,
            version_id,
            Some(expected_current_version_id.map(str::to_string)),
        )
        .await
    }

    async fn promote_project_version_inner(
        &self,
        project_id: &str,
        run_id: &str,
        version_id: &str,
        expected_current_version_id: Option<Option<String>>,
    ) -> Result<ProjectVersion> {
        let persisted_current = self
            .read_current_project_version(project_id)?
            .map(|version| version.id);
        let mut inner = self.inner.write().await;
        let actual_current = inner
            .project_current_versions
            .get(project_id)
            .cloned()
            .or(persisted_current);
        if let Some(expected_current) = expected_current_version_id {
            if actual_current != expected_current {
                return Err(anyhow!(
                    "project current version compare-and-swap failed: expected {:?}, actual {:?}",
                    expected_current,
                    actual_current
                ));
            }
        }
        if !inner.project_versions.contains_key(version_id) {
            if let Some(version) = self.read_project_version(version_id)? {
                inner.project_versions.insert(version.id.clone(), version);
            }
        }
        if !inner.runs.contains_key(run_id) {
            if let Some(run) = self.read_run(run_id)? {
                inner.runs.insert(run.id.clone(), run);
            }
        }
        let version = inner
            .project_versions
            .get_mut(version_id)
            .ok_or_else(|| anyhow!("project version not found: {version_id}"))?;
        if version.project_id != project_id {
            return Err(anyhow!(
                "project version does not belong to project: {project_id}"
            ));
        }
        if version.created_by_run_id != run_id {
            return Err(anyhow!("project version does not belong to run: {run_id}"));
        }
        if version.status != ProjectVersionStatus::Candidate {
            return Err(anyhow!("only candidate versions can be promoted"));
        }
        version.status = ProjectVersionStatus::Promoted;
        version.promoted_at = Some(Utc::now());
        let promoted = version.clone();
        inner
            .project_current_versions
            .insert(project_id.to_string(), version_id.to_string());
        let run = inner
            .runs
            .get_mut(run_id)
            .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
        run.output_version_id = Some(version_id.to_string());
        run.updated_at = Utc::now();
        let run = run.clone();
        drop(inner);
        self.append_project_version_snapshot(&promoted)?;
        self.append_run_snapshot(&run)?;
        Ok(promoted)
    }

    pub async fn current_project_version(&self, project_id: &str) -> Option<ProjectVersion> {
        let current_id = {
            let mut inner = self.inner.write().await;
            self.hydrate_artifact_transactions(&mut inner).ok()?;
            inner.project_current_versions.get(project_id).cloned()
        };
        if let Some(current_id) = current_id {
            return self.get_project_version(&current_id).await;
        }
        let version = self
            .read_current_project_version(project_id)
            .ok()
            .flatten()?;
        let mut inner = self.inner.write().await;
        inner
            .project_current_versions
            .insert(project_id.to_string(), version.id.clone());
        inner
            .project_versions
            .insert(version.id.clone(), version.clone());
        Some(version)
    }

    pub async fn current_project_sandbox_binding(
        &self,
        project_id: &str,
    ) -> Option<SandboxBinding> {
        if let Some(version) = self.current_project_version(project_id).await {
            if let Some(run) = self.get_run(&version.created_by_run_id).await {
                if let Some(binding_id) = run.sandbox_id.as_deref() {
                    if let Some(binding) = self.get_sandbox_binding(binding_id).await {
                        return Some(binding);
                    }
                }
            }
        }
        let mut bindings = self
            .read_sandbox_bindings()
            .unwrap_or_default()
            .into_iter()
            .filter(|binding| {
                binding.project_id == project_id
                    && !matches!(
                        binding.status,
                        SandboxBindingStatus::Failed | SandboxBindingStatus::Deleted
                    )
            })
            .collect::<Vec<_>>();
        bindings.sort_by_key(|binding| binding.last_seen_at);
        bindings.pop()
    }

    pub async fn active_mutable_run_for_project(&self, project_id: &str) -> Option<AgentRun> {
        let mut runs_by_id = self
            .read_runs()
            .unwrap_or_default()
            .into_iter()
            .map(|run| (run.id.clone(), run))
            .collect::<HashMap<_, _>>();
        for run in self.inner.read().await.runs.values().cloned() {
            runs_by_id.insert(run.id.clone(), run);
        }
        runs_by_id.into_values().find(|run| {
            run.project_id == project_id && is_mutable_phase(run.phase) && !run.status.is_terminal()
        })
    }

    pub async fn create_sandbox_binding(
        &self,
        project_id: &str,
        sandbox_name: String,
        sandbox_claim_name: String,
        workspace_pvc_name: String,
        warm_pool_name: String,
        namespace: String,
        channel_protocol: SandboxChannelProtocol,
    ) -> Result<SandboxBinding> {
        if let Some(existing) = self.read_sandbox_binding_with_workspace_pvc(&workspace_pvc_name)? {
            return Err(anyhow!(
                "workspace PVC {} is already bound to project {} via sandbox binding {}",
                workspace_pvc_name,
                existing.project_id,
                existing.id
            ));
        }
        let mut inner = self.inner.write().await;
        if let Some(existing) = inner
            .sandbox_bindings
            .values()
            .find(|binding| binding.workspace_pvc_name == workspace_pvc_name)
        {
            return Err(anyhow!(
                "workspace PVC {} is already bound to project {} via sandbox binding {}",
                workspace_pvc_name,
                existing.project_id,
                existing.id
            ));
        }
        let binding = SandboxBinding {
            id: self.next_id("sandbox-binding"),
            project_id: project_id.to_string(),
            sandbox_name,
            sandbox_claim_name,
            workspace_pvc_name,
            channel_service_name: None,
            sandbox_uid: None,
            pod_uid: None,
            warm_pool_name,
            namespace,
            status: SandboxBindingStatus::Claiming,
            channel_protocol,
            last_seen_at: Utc::now(),
        };
        inner
            .sandbox_bindings
            .insert(binding.id.clone(), binding.clone());
        drop(inner);
        self.append_sandbox_binding_snapshot(&binding)?;
        Ok(binding)
    }

    pub async fn update_sandbox_binding_status(
        &self,
        binding_id: &str,
        status: SandboxBindingStatus,
    ) -> Result<SandboxBinding> {
        let binding = {
            let mut inner = self.inner.write().await;
            if !inner.sandbox_bindings.contains_key(binding_id) {
                if let Some(binding) = self.read_sandbox_binding(binding_id)? {
                    inner.sandbox_bindings.insert(binding.id.clone(), binding);
                }
            }
            let binding = inner
                .sandbox_bindings
                .get_mut(binding_id)
                .ok_or_else(|| anyhow!("sandbox binding not found: {binding_id}"))?;
            binding.status = status;
            binding.last_seen_at = Utc::now();
            binding.clone()
        };
        self.append_sandbox_binding_snapshot(&binding)?;
        Ok(binding)
    }

    pub async fn update_sandbox_binding_channel_service_name(
        &self,
        binding_id: &str,
        channel_service_name: Option<String>,
    ) -> Result<SandboxBinding> {
        let binding = {
            let mut inner = self.inner.write().await;
            if !inner.sandbox_bindings.contains_key(binding_id) {
                if let Some(binding) = self.read_sandbox_binding(binding_id)? {
                    inner.sandbox_bindings.insert(binding.id.clone(), binding);
                }
            }
            let binding = inner
                .sandbox_bindings
                .get_mut(binding_id)
                .ok_or_else(|| anyhow!("sandbox binding not found: {binding_id}"))?;
            binding.channel_service_name = channel_service_name;
            binding.last_seen_at = Utc::now();
            binding.clone()
        };
        self.append_sandbox_binding_snapshot(&binding)?;
        Ok(binding)
    }

    pub async fn update_sandbox_binding_runtime_identity(
        &self,
        binding_id: &str,
        sandbox_name: String,
        channel_service_name: Option<String>,
    ) -> Result<SandboxBinding> {
        self.update_sandbox_binding_runtime_identity_with_uids(
            binding_id,
            sandbox_name,
            channel_service_name,
            None,
            None,
        )
        .await
    }

    pub async fn update_sandbox_binding_runtime_identity_with_uids(
        &self,
        binding_id: &str,
        sandbox_name: String,
        channel_service_name: Option<String>,
        sandbox_uid: Option<String>,
        pod_uid: Option<String>,
    ) -> Result<SandboxBinding> {
        let binding = {
            let mut inner = self.inner.write().await;
            if !inner.sandbox_bindings.contains_key(binding_id) {
                if let Some(binding) = self.read_sandbox_binding(binding_id)? {
                    inner.sandbox_bindings.insert(binding.id.clone(), binding);
                }
            }
            let binding = inner
                .sandbox_bindings
                .get_mut(binding_id)
                .ok_or_else(|| anyhow!("sandbox binding not found: {binding_id}"))?;
            binding.sandbox_name = sandbox_name;
            binding.channel_service_name = channel_service_name;
            binding.sandbox_uid = sandbox_uid;
            binding.pod_uid = pod_uid;
            binding.last_seen_at = Utc::now();
            binding.clone()
        };
        self.append_sandbox_binding_snapshot(&binding)?;
        Ok(binding)
    }

    pub async fn get_sandbox_binding(&self, binding_id: &str) -> Option<SandboxBinding> {
        if let Some(binding) = self
            .inner
            .read()
            .await
            .sandbox_bindings
            .get(binding_id)
            .cloned()
        {
            return Some(binding);
        }
        let binding = self.read_sandbox_binding(binding_id).ok().flatten()?;
        self.inner
            .write()
            .await
            .sandbox_bindings
            .insert(binding.id.clone(), binding.clone());
        Some(binding)
    }
}

fn checkpoint_path(checkpoint_dir: &Path, checkpoint_id: &str) -> PathBuf {
    checkpoint_dir.join(format!("{checkpoint_id}.json"))
}

fn run_snapshot_is_newer(candidate: &AgentRun, current: &AgentRun) -> bool {
    candidate.updated_at > current.updated_at
        || (candidate.updated_at == current.updated_at
            && candidate.status.is_terminal()
            && !current.status.is_terminal())
}

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn append_jsonl_synced<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.sync_all()?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn read_latest_jsonl_by_key<T, F>(path: &Path, key: F) -> Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
    F: Fn(&T) -> String,
{
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut values = HashMap::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: T = serde_json::from_str(&line)?;
        values.insert(key(&value), value);
    }
    Ok(values.into_values().collect())
}

fn artifact_publish_transition_allowed(
    from: ArtifactPublishStatus,
    to: ArtifactPublishStatus,
) -> bool {
    from == to
        || matches!(
            (from, to),
            (
                ArtifactPublishStatus::Staging,
                ArtifactPublishStatus::Staged
            ) | (
                ArtifactPublishStatus::Staging,
                ArtifactPublishStatus::Failed
            ) | (
                ArtifactPublishStatus::Staging,
                ArtifactPublishStatus::GarbageCollectable
            ) | (
                ArtifactPublishStatus::Staged,
                ArtifactPublishStatus::Validating
            ) | (
                ArtifactPublishStatus::Staged,
                ArtifactPublishStatus::GarbageCollectable
            ) | (
                ArtifactPublishStatus::Validating,
                ArtifactPublishStatus::Promoting
            ) | (
                ArtifactPublishStatus::Validating,
                ArtifactPublishStatus::GarbageCollectable
            ) | (
                ArtifactPublishStatus::Promoting,
                ArtifactPublishStatus::ReconcileRequired
            ) | (
                ArtifactPublishStatus::Promoting,
                ArtifactPublishStatus::GarbageCollectable
            ) | (
                ArtifactPublishStatus::ReconcileRequired,
                ArtifactPublishStatus::Promoting
            ) | (
                ArtifactPublishStatus::ReconcileRequired,
                ArtifactPublishStatus::GarbageCollectable
            ) | (
                ArtifactPublishStatus::GarbageCollectable,
                ArtifactPublishStatus::GarbageCollected
            )
        )
}

fn channel_lease_transition_allowed(from: ChannelLeaseStatus, to: ChannelLeaseStatus) -> bool {
    from == to
        || matches!(
            (from, to),
            (ChannelLeaseStatus::Acquiring, ChannelLeaseStatus::Ready)
                | (ChannelLeaseStatus::Acquiring, ChannelLeaseStatus::Failed)
                | (ChannelLeaseStatus::Acquiring, ChannelLeaseStatus::Stale)
                | (ChannelLeaseStatus::Ready, ChannelLeaseStatus::Stale)
                | (ChannelLeaseStatus::Ready, ChannelLeaseStatus::Releasing)
                | (ChannelLeaseStatus::Ready, ChannelLeaseStatus::Failed)
                | (ChannelLeaseStatus::Stale, ChannelLeaseStatus::Acquiring)
                | (ChannelLeaseStatus::Stale, ChannelLeaseStatus::Releasing)
                | (ChannelLeaseStatus::Stale, ChannelLeaseStatus::Released)
                | (ChannelLeaseStatus::Releasing, ChannelLeaseStatus::Released)
                | (ChannelLeaseStatus::Releasing, ChannelLeaseStatus::Failed)
                | (ChannelLeaseStatus::Failed, ChannelLeaseStatus::Acquiring)
                | (ChannelLeaseStatus::Released, ChannelLeaseStatus::Acquiring)
        )
}

fn preview_updated_outbox_id(project_id: &str, version_id: &str) -> String {
    format!("preview.updated:{project_id}:{version_id}")
}

fn default_storage_root() -> PathBuf {
    let sequence = DEFAULT_STORAGE_ROOT_COUNTER.fetch_add(1, Ordering::SeqCst);
    let unique = format!(
        "anydesign-runtime-{}-{}-{}",
        std::process::id(),
        sequence,
        Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(|| Utc::now().timestamp_micros())
    );
    std::env::temp_dir().join(unique)
}

fn initial_id_counter(paths: &[&Path]) -> u64 {
    paths
        .iter()
        .map(|path| max_numeric_suffix_in_path(path))
        .max()
        .unwrap_or(0)
        + 1
}

fn max_numeric_suffix_in_path(path: &Path) -> u64 {
    if path.is_dir() {
        return fs::read_dir(path)
            .ok()
            .into_iter()
            .flat_map(|entries| entries.flatten())
            .map(|entry| max_numeric_suffix_in_path(&entry.path()))
            .max()
            .unwrap_or(0);
    }
    fs::read_to_string(path)
        .map(|text| max_numeric_suffix_in_text(&text))
        .unwrap_or(0)
}

fn max_numeric_suffix_in_text(text: &str) -> u64 {
    let mut max = 0;
    for prefix in [
        "run-",
        "session-",
        "message-",
        "brief-",
        "checkpoint-",
        "conversation-",
        "audit-",
        "permission-",
        "finding-",
        "version-",
        "publish-",
        "sandbox-binding-",
        "sandbox-",
        "screenshot-",
    ] {
        let mut start = 0;
        while let Some(relative_index) = text[start..].find(prefix) {
            let value_start = start + relative_index + prefix.len();
            let bytes = text.as_bytes();
            if value_start >= bytes.len() || !bytes[value_start].is_ascii_digit() {
                start = value_start;
                continue;
            }
            let mut cursor = value_start;
            let mut value = 0u64;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                value = value
                    .saturating_mul(10)
                    .saturating_add((bytes[cursor] - b'0') as u64);
                cursor += 1;
            }
            max = max.max(value);
            start = cursor;
        }
    }
    max
}

fn project_log_segment(project_id: &str) -> String {
    project_id
        .chars()
        .map(|character| match character {
            '/' | '\\' => '_',
            _ => character,
        })
        .collect()
}

fn allowed_workspace_holder_ids(
    inner: &RuntimeStoreInner,
    allowed_parent_run_id: Option<&str>,
) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut next = allowed_parent_run_id;
    while let Some(run_id) = next {
        if !ids.insert(run_id.to_string()) {
            break;
        }
        next = inner
            .runs
            .get(run_id)
            .and_then(|run| run.parent_run_id.as_deref());
    }
    ids
}

fn is_mutable_phase(phase: AgentPhase) -> bool {
    matches!(
        phase,
        AgentPhase::Build | AgentPhase::Edit | AgentPhase::Repair | AgentPhase::Export
    )
}

fn run_is_candidate_review_descendant(
    inner: &RuntimeStoreInner,
    run_id: &str,
    candidate_roots: &HashSet<String>,
) -> bool {
    let mut seen = HashSet::new();
    let mut next = Some(run_id);
    while let Some(current_id) = next {
        if candidate_roots.contains(current_id) {
            return true;
        }
        if !seen.insert(current_id.to_string()) {
            return false;
        }
        next = inner
            .runs
            .get(current_id)
            .and_then(|run| run.parent_run_id.as_deref());
    }
    false
}
