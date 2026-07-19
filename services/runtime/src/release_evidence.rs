use crate::{
    conversation::RuntimeStore,
    runtime_storage::RuntimeEvidenceStore,
    types::{AgentEvent, AgentPhase, ReviewFindingStatus},
};
use serde_json::{json, Value};
use std::{error::Error, fmt, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseEvidenceError {
    NotFound(String),
    Conflict(String),
    Internal(String),
}

impl fmt::Display for ReleaseEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(message) | Self::Conflict(message) | Self::Internal(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl Error for ReleaseEvidenceError {}

#[derive(Clone)]
pub struct ReleaseEvidenceService {
    store: RuntimeStore,
    evidence: Arc<dyn RuntimeEvidenceStore>,
}

impl ReleaseEvidenceService {
    pub fn new(store: RuntimeStore, evidence: Arc<dyn RuntimeEvidenceStore>) -> Self {
        Self { store, evidence }
    }

    pub async fn project_release_evidence(
        &self,
        project_id: &str,
    ) -> Result<Value, ReleaseEvidenceError> {
        let current = self
            .store
            .current_project_version(project_id)
            .await
            .ok_or_else(|| {
                ReleaseEvidenceError::NotFound(format!("current version not found: {project_id}"))
            })?;
        let edit_run_id = current.created_by_run_id.clone();
        let current_run = self.store.get_run(&edit_run_id).await.ok_or_else(|| {
            ReleaseEvidenceError::NotFound(format!("current version run not found: {edit_run_id}"))
        })?;
        let review_repair = if current_run.phase == AgentPhase::Repair {
            let review_run_id = current_run.parent_run_id.as_deref().ok_or_else(|| {
                ReleaseEvidenceError::Conflict(
                    "repair release evidence requires a Review parent run".to_string(),
                )
            })?;
            let review_run = self.store.get_run(review_run_id).await.ok_or_else(|| {
                ReleaseEvidenceError::NotFound(format!(
                    "repair Review parent run not found: {review_run_id}"
                ))
            })?;
            if review_run.phase != AgentPhase::Review {
                return Err(ReleaseEvidenceError::Conflict(
                    "repair release evidence parent must be a Review run".to_string(),
                ));
            }
            let source_edit_run_id = review_run.parent_run_id.clone().ok_or_else(|| {
                ReleaseEvidenceError::Conflict(
                    "repair Review evidence requires an Edit parent run".to_string(),
                )
            })?;
            let finding_ids = current_run.finding_ids.as_deref().ok_or_else(|| {
                ReleaseEvidenceError::Conflict(
                    "repair release evidence requires scoped finding IDs".to_string(),
                )
            })?;
            let mut findings = Vec::with_capacity(finding_ids.len());
            for finding_id in finding_ids {
                let finding = self
                    .store
                    .get_review_finding(finding_id)
                    .await
                    .ok_or_else(|| {
                        ReleaseEvidenceError::NotFound(format!(
                            "repair review finding not found: {finding_id}"
                        ))
                    })?;
                if finding.run_id != review_run.id || finding.status != ReviewFindingStatus::Fixed {
                    return Err(ReleaseEvidenceError::Conflict(format!(
                        "repair review finding is not fixed by the scoped Review run: {finding_id}"
                    )));
                }
                findings.push(json!({
                    "findingId": finding.id,
                    "versionId": finding.version_id,
                    "severity": finding.severity,
                    "category": finding.category,
                    "repairable": finding.repairable,
                    "status": finding.status,
                }));
            }
            Some(json!({
                "editRunId": source_edit_run_id,
                "reviewRunId": review_run.id,
                "repairRunId": current_run.id,
                "findings": findings,
            }))
        } else {
            None
        };
        let publish = self
            .store
            .artifact_publish_for_version(project_id, &edit_run_id, &current.id)
            .await
            .ok_or_else(|| {
                ReleaseEvidenceError::NotFound(format!(
                    "artifact publish not found: {}",
                    current.id
                ))
            })?;
        let base_version_id = publish.expected_current_version_id.clone().ok_or_else(|| {
            ReleaseEvidenceError::Conflict(
                "release evidence requires an Edit promotion with a base version".to_string(),
            )
        })?;
        let base_version = self
            .store
            .get_project_version(&base_version_id)
            .await
            .ok_or_else(|| {
                ReleaseEvidenceError::NotFound(format!("base version not found: {base_version_id}"))
            })?;
        let lease = self
            .store
            .preview_lease_for_run(&edit_run_id)
            .await
            .ok_or_else(|| {
                ReleaseEvidenceError::NotFound(format!("preview lease not found: {edit_run_id}"))
            })?;
        let binding = self
            .store
            .get_sandbox_binding(&lease.sandbox_binding_id)
            .await
            .ok_or_else(|| {
                ReleaseEvidenceError::NotFound(format!(
                    "sandbox binding not found: {}",
                    lease.sandbox_binding_id
                ))
            })?;
        let events = self.store.events(&edit_run_id).await;
        let build_events = self.store.events(&base_version.created_by_run_id).await;
        let failure_counts = build_events.iter().chain(events.iter()).fold(
            (0_u64, 0_u64),
            |(recoverable, terminal), event| match event {
                AgentEvent::ToolFailed {
                    recoverable: true, ..
                } => (recoverable + 1, terminal),
                AgentEvent::ToolFailed {
                    recoverable: false, ..
                } => (recoverable, terminal + 1),
                _ => (recoverable, terminal),
            },
        );
        let preview_index = events
            .iter()
            .position(|event| matches!(event, AgentEvent::PreviewUpdated { .. }))
            .ok_or_else(|| {
                ReleaseEvidenceError::Conflict("preview.updated event missing".to_string())
            })?;
        let completed_index = events
            .iter()
            .position(|event| matches!(event, AgentEvent::RunCompleted { .. }))
            .ok_or_else(|| {
                ReleaseEvidenceError::Conflict("run.completed event missing".to_string())
            })?;
        let screenshot_id = current
            .screenshot_id
            .clone()
            .ok_or_else(|| ReleaseEvidenceError::Conflict("screenshot ID missing".to_string()))?;
        let screenshot = self
            .evidence
            .read_screenshot(project_id, &edit_run_id, &screenshot_id)
            .map_err(|error| ReleaseEvidenceError::Internal(error.to_string()))?;
        Ok(json!({
            "projectId": project_id,
            "buildRunId": base_version.created_by_run_id,
            "editRunId": edit_run_id,
            "bindingId": binding.id,
            "podUid": binding.pod_uid,
            "workspaceNamespace": binding.namespace,
            "buildId": publish.build_id,
            "candidateManifestHash": publish.candidate_manifest_hash,
            "sourceSnapshotUri": publish.source_snapshot_uri,
            "previewLeaseId": lease.id,
            "previewLeaseStatus": lease.status,
            "screenshotId": screenshot_id,
            "nonblankPixelRatio": screenshot["nonblankPixelRatio"],
            "screenshotPngSha256": screenshot["pngSha256"],
            "screenshotDocumentSha256": screenshot["documentSha256"],
            "versionBeforeCas": base_version_id,
            "versionAfterCas": current.id,
            "artifactManifestHash": publish.artifact_manifest_hash,
            "artifactUrl": format!("/artifacts/{project_id}/current/"),
            "events": {
                "previewUpdated": format!("{}/{}", current.created_by_run_id, preview_index),
                "runCompleted": format!("{}/{}", current.created_by_run_id, completed_index),
                "sequenceValid": preview_index < completed_index,
            },
            "recoverableToolFailureCount": failure_counts.0,
            "terminalToolFailureCount": failure_counts.1,
            "reviewRepair": review_repair,
            "sandboxStatus": binding.status,
            "sandboxReleasedAt": binding.last_seen_at,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnusedEvidenceStore;

    impl RuntimeEvidenceStore for UnusedEvidenceStore {
        fn read_screenshot(
            &self,
            _project_id: &str,
            _run_id: &str,
            _screenshot_id: &str,
        ) -> anyhow::Result<Value> {
            panic!("missing project must fail before Runtime evidence access")
        }
    }

    #[tokio::test]
    async fn missing_project_fails_closed_before_evidence_access() {
        let service =
            ReleaseEvidenceService::new(RuntimeStore::new(), Arc::new(UnusedEvidenceStore));
        assert_eq!(
            service.project_release_evidence("missing-project").await,
            Err(ReleaseEvidenceError::NotFound(
                "current version not found: missing-project".to_string()
            ))
        );
    }
}
