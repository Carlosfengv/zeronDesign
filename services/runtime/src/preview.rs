use crate::{
    artifact_publisher::{ArtifactPublisher, FileArtifactPublisher, StagedArtifact},
    conversation::RuntimeStore,
    generation_contract::GenerationContract,
    runtime_storage::{FileAcceptanceReportStore, FileValidationReportStore},
    types::{
        canonical_json_hash, AgentCheckpoint, AgentEvent, ArtifactPublishStatus,
        CheckpointBuildResult, ProjectVersion,
    },
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PromotionGateReport {
    pub build_log_has_terminal_error: bool,
    pub preview_accessible: bool,
    pub screenshot_blank: bool,
    pub screenshot_available: bool,
    pub blocking_findings: u32,
}

impl PromotionGateReport {
    pub fn passing() -> Self {
        Self {
            build_log_has_terminal_error: false,
            preview_accessible: true,
            screenshot_blank: false,
            screenshot_available: true,
            blocking_findings: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionGateError {
    BuildFailed,
    PreviewUnreachable,
    ScreenshotMissing,
    BlankPage,
    BlockingFindings(u32),
    ReviewPending(u32),
}

impl std::fmt::Display for PromotionGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BuildFailed => write!(f, "build log contains a terminal error"),
            Self::PreviewUnreachable => write!(f, "preview server is not accessible"),
            Self::ScreenshotMissing => write!(f, "screenshot is required before promotion"),
            Self::BlankPage => write!(f, "screenshot appears blank"),
            Self::BlockingFindings(count) => write!(f, "{count} blocking review finding(s)"),
            Self::ReviewPending(count) => {
                write!(f, "{count} review/repair child run(s) still active")
            }
        }
    }
}

impl std::error::Error for PromotionGateError {}

pub fn check_promotion_gate(report: &PromotionGateReport) -> Result<(), PromotionGateError> {
    if report.build_log_has_terminal_error {
        return Err(PromotionGateError::BuildFailed);
    }
    if !report.preview_accessible {
        return Err(PromotionGateError::PreviewUnreachable);
    }
    if !report.screenshot_available {
        return Err(PromotionGateError::ScreenshotMissing);
    }
    if report.screenshot_blank {
        return Err(PromotionGateError::BlankPage);
    }
    if report.blocking_findings > 0 {
        return Err(PromotionGateError::BlockingFindings(
            report.blocking_findings,
        ));
    }
    Ok(())
}

pub async fn promote_preview(
    store: &RuntimeStore,
    project_id: &str,
    run_id: &str,
    candidate_version_id: &str,
    gate_report: PromotionGateReport,
) -> Result<ProjectVersion> {
    promote_preview_inner(
        store,
        project_id,
        run_id,
        candidate_version_id,
        gate_report,
        None,
    )
    .await
}

pub async fn promote_preview_cas(
    store: &RuntimeStore,
    project_id: &str,
    run_id: &str,
    candidate_version_id: &str,
    gate_report: PromotionGateReport,
    expected_current_version_id: Option<&str>,
) -> Result<ProjectVersion> {
    promote_preview_inner(
        store,
        project_id,
        run_id,
        candidate_version_id,
        gate_report,
        Some(expected_current_version_id),
    )
    .await
}

async fn promote_preview_inner(
    store: &RuntimeStore,
    project_id: &str,
    run_id: &str,
    candidate_version_id: &str,
    gate_report: PromotionGateReport,
    expected_current_version_id: Option<Option<&str>>,
) -> Result<ProjectVersion> {
    validate_preview_promotion(store, project_id, run_id, candidate_version_id, gate_report)
        .await?;
    let publish = store
        .artifact_publish_for_version(project_id, run_id, candidate_version_id)
        .await;
    let version = if let Some(publish) = publish {
        let expected = match expected_current_version_id {
            Some(expected) => expected,
            None => publish.expected_current_version_id.as_deref(),
        };
        let (version, outbox) = store
            .commit_artifact_promotion_cas(
                project_id,
                run_id,
                candidate_version_id,
                &publish.id,
                expected,
            )
            .await?;
        store.dispatch_outbox_event(&outbox.id).await?;
        version
    } else {
        let version = match expected_current_version_id {
            Some(expected) => {
                store
                    .promote_project_version_cas(project_id, run_id, candidate_version_id, expected)
                    .await?
            }
            None => {
                store
                    .promote_project_version(project_id, run_id, candidate_version_id)
                    .await?
            }
        };
        let _ = store
            .append_event(AgentEvent::PreviewUpdated {
                run_id: run_id.to_string(),
                url: version.preview_url.clone(),
                version_id: version.id.clone(),
                screenshot_id: version.screenshot_id.clone(),
                timestamp: Utc::now(),
            })
            .await;
        version
    };
    store
        .append_conversation_item(
            project_id,
            Some(run_id),
            "preview_update",
            None,
            format!("Preview updated: {}", version.preview_url),
            Some(serde_json::json!({ "versionId": version.id })),
        )
        .await;
    save_promotion_checkpoint(store, project_id, run_id, &version).await?;
    Ok(version)
}

pub async fn validate_preview_promotion(
    store: &RuntimeStore,
    project_id: &str,
    run_id: &str,
    candidate_version_id: &str,
    mut gate_report: PromotionGateReport,
) -> Result<()> {
    check_promotion_gate(&gate_report).map_err(|error| anyhow!(error.to_string()))?;
    let active_review_runs = store
        .active_review_or_repair_runs_for_candidate(run_id, candidate_version_id)
        .await
        .len() as u32;
    if active_review_runs > 0 {
        return Err(anyhow!(
            "{}",
            PromotionGateError::ReviewPending(active_review_runs)
        ));
    }
    let blocking_findings = store
        .open_blocking_findings(project_id, candidate_version_id)
        .await
        .len() as u32;
    gate_report.blocking_findings = gate_report.blocking_findings.max(blocking_findings);
    check_promotion_gate(&gate_report).map_err(|error| anyhow!(error.to_string()))?;
    Ok(())
}

pub async fn complete_candidate_preview(
    store: &RuntimeStore,
    runtime_storage_dir: &Path,
    project_id: &str,
    run_id: &str,
    candidate_version_id: &str,
    generation_contract: &GenerationContract,
    template_version: &str,
    summary: &str,
) -> Result<ProjectVersion> {
    let validation_report = FileValidationReportStore::new(runtime_storage_dir)
        .read(project_id, run_id, candidate_version_id)
        .map_err(|error| anyhow!("candidate validation report is missing or invalid: {error}"))?;
    if validation_report.run_id != run_id
        || validation_report.candidate_version_id != candidate_version_id
    {
        return Err(anyhow!(
            "candidate validation report identity does not match completion request"
        ));
    }
    let expected_contract_digest = generation_contract
        .digest()
        .map_err(|error| anyhow!("candidate generation contract is invalid: {error}"))?;
    if validation_report.generation_contract_digest != expected_contract_digest
        || validation_report.template_version != template_version
    {
        return Err(anyhow!(
            "candidate validation report does not match the frozen generation contract"
        ));
    }
    let validation_blockers = validation_report.promotion_blockers(generation_contract);
    if !validation_blockers.is_empty() {
        return Err(anyhow!(
            "candidate validation contract is not satisfied: {}",
            validation_blockers
                .iter()
                .map(|blocker| format!(
                    "{}={:?}{}",
                    blocker.check_id,
                    blocker.status,
                    blocker
                        .message
                        .as_deref()
                        .map(|message| format!(" ({message})"))
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let run = store
        .get_run(run_id)
        .await
        .ok_or_else(|| anyhow!("candidate run is missing"))?;
    let brief_id = run
        .brief_version
        .as_deref()
        .ok_or_else(|| anyhow!("candidate completion requires a frozen Brief"))?;
    if !store.is_brief_confirmed(brief_id).await {
        return Err(anyhow!("candidate completion requires a confirmed Brief"));
    }
    let brief = store
        .get_brief(brief_id)
        .await
        .ok_or_else(|| anyhow!("candidate Brief is missing"))?;
    let contract = store
        .get_acceptance_contract(brief_id)
        .await
        .ok_or_else(|| anyhow!("candidate acceptance contract is missing"))?;
    contract
        .validate()
        .map_err(|error| anyhow!("candidate acceptance contract is invalid: {error}"))?;
    let current_brief_digest = canonical_json_hash(&serde_json::to_value(&brief)?);
    if current_brief_digest != contract.brief_digest {
        return Err(anyhow!(
            "candidate acceptance contract does not match the frozen Brief"
        ));
    }
    let current_sources = store.content_sources_for_brief(brief_id).await;
    let current_sources_digest = canonical_json_hash(&serde_json::to_value(current_sources)?);
    if current_sources_digest != contract.content_sources_digest {
        return Err(anyhow!(
            "candidate acceptance contract does not match the frozen content sources"
        ));
    }
    let acceptance_report = FileAcceptanceReportStore::new(runtime_storage_dir)
        .read(project_id, run_id, candidate_version_id)
        .map_err(|error| anyhow!("candidate acceptance report is missing or invalid: {error}"))?;
    if acceptance_report.run_id != run_id
        || acceptance_report.candidate_version_id != candidate_version_id
        || acceptance_report.brief_id != brief_id
        || acceptance_report.contract_digest != contract.contract_digest
    {
        return Err(anyhow!(
            "candidate acceptance report identity does not match completion request"
        ));
    }
    if !acceptance_report.passed() {
        return Err(anyhow!(
            "candidate does not satisfy the frozen Brief acceptance contract"
        ));
    }
    let publish = store
        .artifact_publish_for_version(project_id, run_id, candidate_version_id)
        .await
        .ok_or_else(|| anyhow!("candidate artifact publish is missing"))?;
    let artifact_manifest_hash = publish
        .artifact_manifest_hash
        .as_deref()
        .ok_or_else(|| anyhow!("candidate artifact manifest hash is missing"))?;
    if validation_report.candidate_manifest_hash != publish.candidate_manifest_hash
        || validation_report.artifact_manifest_hash != artifact_manifest_hash
    {
        return Err(anyhow!(
            "candidate validation report manifest does not match the artifact publish"
        ));
    }
    if acceptance_report.candidate_manifest_hash != publish.candidate_manifest_hash {
        return Err(anyhow!(
            "candidate acceptance report manifest does not match the artifact publish"
        ));
    }
    if publish.status == ArtifactPublishStatus::Promoted {
        let (version, preview_outbox, completion_outbox) = store
            .complete_artifact_promotion_cas(
                project_id,
                run_id,
                candidate_version_id,
                &publish.id,
                publish.expected_current_version_id.as_deref(),
                summary,
            )
            .await?;
        return finish_completed_candidate(
            store,
            project_id,
            run_id,
            version,
            preview_outbox,
            completion_outbox,
        )
        .await;
    }
    if !matches!(
        publish.status,
        ArtifactPublishStatus::Ready
            | ArtifactPublishStatus::Promoting
            | ArtifactPublishStatus::ReconcileRequired
    ) {
        return Err(anyhow!(
            "candidate artifact is not ready for completion: {:?}",
            publish.status
        ));
    }
    let artifact_manifest_hash = publish
        .artifact_manifest_hash
        .clone()
        .ok_or_else(|| anyhow!("candidate artifact manifest hash is missing"))?;
    let staged_uri = publish
        .staged_uri
        .clone()
        .ok_or_else(|| anyhow!("candidate staged artifact URI is missing"))?;
    let staged = StagedArtifact {
        project_id: project_id.to_string(),
        version_id: candidate_version_id.to_string(),
        candidate_manifest_hash: publish.candidate_manifest_hash.clone(),
        artifact_manifest_hash,
        staged_uri,
        file_count: 0,
    };
    if matches!(
        publish.status,
        ArtifactPublishStatus::Ready | ArtifactPublishStatus::ReconcileRequired
    ) {
        store
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
    let publisher = FileArtifactPublisher::new(runtime_storage_dir);
    let artifact_uri = match publisher.promote(&staged).await {
        Ok(uri) => uri,
        Err(error) => {
            store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::ReconcileRequired,
                    None,
                    None,
                    None,
                    Some(&error.to_string()),
                )
                .await
                .ok();
            return Err(error);
        }
    };
    store
        .transition_artifact_publish(
            &publish.id,
            ArtifactPublishStatus::Promoting,
            None,
            None,
            Some(&artifact_uri),
            None,
        )
        .await?;
    let committed = store
        .complete_artifact_promotion_cas(
            project_id,
            run_id,
            candidate_version_id,
            &publish.id,
            publish.expected_current_version_id.as_deref(),
            summary,
        )
        .await;
    let (version, preview_outbox, completion_outbox) = match committed {
        Ok(committed) => committed,
        Err(error) => {
            store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::GarbageCollectable,
                    None,
                    None,
                    None,
                    Some(&error.to_string()),
                )
                .await
                .ok();
            return Err(error);
        }
    };
    finish_completed_candidate(
        store,
        project_id,
        run_id,
        version,
        preview_outbox,
        completion_outbox,
    )
    .await
}

async fn finish_completed_candidate(
    store: &RuntimeStore,
    project_id: &str,
    run_id: &str,
    version: ProjectVersion,
    preview_outbox: crate::types::RuntimeOutboxEvent,
    completion_outbox: crate::types::RuntimeOutboxEvent,
) -> Result<ProjectVersion> {
    store.dispatch_outbox_event(&preview_outbox.id).await?;
    store.dispatch_outbox_event(&completion_outbox.id).await?;
    store
        .append_conversation_item(
            project_id,
            Some(run_id),
            "preview_update",
            None,
            format!("Preview updated: {}", version.preview_url),
            Some(serde_json::json!({ "versionId": version.id })),
        )
        .await;
    save_promotion_checkpoint(store, project_id, run_id, &version).await?;
    Ok(version)
}

async fn save_promotion_checkpoint(
    store: &RuntimeStore,
    project_id: &str,
    run_id: &str,
    version: &ProjectVersion,
) -> Result<()> {
    let run = store
        .get_run(run_id)
        .await
        .ok_or_else(|| anyhow!("run not found for checkpoint: {run_id}"))?;
    store
        .save_checkpoint(AgentCheckpoint {
            id: store.next_id("checkpoint"),
            run_id: run.id,
            project_id: project_id.to_string(),
            phase: run.phase,
            message_window: Vec::new(),
            conversation_range: None,
            task_list: Vec::new(),
            workspace_snapshot_uri: version.source_snapshot_uri.clone(),
            build_result: Some(CheckpointBuildResult {
                version_id: version.id.clone(),
                status: version.status,
                preview_url: version.preview_url.clone(),
                source_snapshot_uri: version.source_snapshot_uri.clone(),
                screenshot_id: version.screenshot_id.clone(),
            }),
            brief_version: run.brief_version,
            design_version: run.design_version,
            last_known_preview_url: Some(version.preview_url.clone()),
            context_summary: format!("preview promoted: {}", version.id),
            created_at: Utc::now(),
        })
        .await
}
