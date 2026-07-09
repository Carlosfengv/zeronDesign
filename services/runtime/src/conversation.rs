use crate::{
    profiles::policy,
    repair_loop::RepairAttempt,
    types::{
        AgentCheckpoint, AgentEvent, AgentPhase, AgentRun, AgentRunStatus, AuditRecord, Brief,
        BriefStatus, ContentSource, ConversationItem, PendingPermission, ProjectVersion,
        ProjectVersionStatus, ReviewFinding, ReviewFindingCategory, ReviewFindingEvidence,
        ReviewFindingSeverity, ReviewFindingStatus, SandboxBinding, SandboxBindingStatus,
        SandboxChannelProtocol,
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
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
use tokio::sync::RwLock;

static DEFAULT_STORAGE_ROOT_COUNTER: AtomicU64 = AtomicU64::new(1);

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
    review_finding_log_path: Arc<PathBuf>,
    repair_attempt_log_path: Arc<PathBuf>,
    pending_permission_log_path: Arc<PathBuf>,
    sandbox_binding_log_path: Arc<PathBuf>,
    audit_log_path: Arc<PathBuf>,
}

#[derive(Debug, Default)]
struct RuntimeStoreInner {
    runs: HashMap<String, AgentRun>,
    events: HashMap<String, Vec<AgentEvent>>,
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
    sandbox_bindings: HashMap<String, SandboxBinding>,
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
        AgentRunStatus::Queued | AgentRunStatus::Running | AgentRunStatus::Cancelled => None,
    }
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
            review_finding_log_path: Arc::new(storage_root.join("review-findings.jsonl")),
            repair_attempt_log_path: Arc::new(storage_root.join("repair-attempts.jsonl")),
            pending_permission_log_path: Arc::new(storage_root.join("pending-permissions.jsonl")),
            sandbox_binding_log_path: Arc::new(storage_root.join("sandbox-bindings.jsonl")),
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
            review_finding_log_path: Arc::new(checkpoint_dir.join("review-findings.jsonl")),
            repair_attempt_log_path: Arc::new(checkpoint_dir.join("repair-attempts.jsonl")),
            pending_permission_log_path: Arc::new(checkpoint_dir.join("pending-permissions.jsonl")),
            sandbox_binding_log_path: Arc::new(checkpoint_dir.join("sandbox-bindings.jsonl")),
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
            review_finding_log_path: Arc::new(run_log_dir.join("review-findings.jsonl")),
            repair_attempt_log_path: Arc::new(run_log_dir.join("repair-attempts.jsonl")),
            pending_permission_log_path: Arc::new(run_log_dir.join("pending-permissions.jsonl")),
            sandbox_binding_log_path: Arc::new(run_log_dir.join("sandbox-bindings.jsonl")),
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
            base_version_id,
            output_version_id: None,
            finding_ids: None,
            input_message_ids: vec![self.next_id("message")],
            checkpoint_id: None,
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
        let run = self.read_run(run_id).ok().flatten()?;
        self.inner
            .write()
            .await
            .runs
            .insert(run.id.clone(), run.clone());
        Some(run)
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
            runs_by_id.insert(run.id.clone(), run);
        }
        let recoverable = runs_by_id
            .values()
            .filter(|run| matches!(run.status, AgentRunStatus::Queued | AgentRunStatus::Running))
            .cloned()
            .collect::<Vec<_>>();
        if !recoverable.is_empty() {
            let mut inner = self.inner.write().await;
            for run in runs_by_id.into_values() {
                inner.runs.entry(run.id.clone()).or_insert(run);
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

    pub async fn append_event(&self, event: AgentEvent) {
        let run_id = event.run_id().to_string();
        if let Err(error) = self.append_run_log_event(&run_id, &event) {
            eprintln!("failed to append run log for {run_id}: {error}");
        }
        let mut inner = self.inner.write().await;
        inner.events.entry(run_id).or_default().push(event);
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
        self.append_event(AgentEvent::ReviewFinding {
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
            inner.runs.entry(run.id.clone()).or_insert(run);
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
        if let Some(version) = self
            .inner
            .read()
            .await
            .project_versions
            .get(version_id)
            .cloned()
        {
            return Some(version);
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
        let mut inner = self.inner.write().await;
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
        if let Some(current_id) = self
            .inner
            .read()
            .await
            .project_current_versions
            .get(project_id)
            .cloned()
        {
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

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut line = serde_json::to_string(value)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
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
