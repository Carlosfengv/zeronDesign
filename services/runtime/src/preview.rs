use crate::{
    conversation::RuntimeStore,
    types::{AgentCheckpoint, AgentEvent, CheckpointBuildResult, ProjectVersion},
};
use anyhow::{anyhow, Result};
use chrono::Utc;

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
