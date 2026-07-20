use super::{
    build::{build_frozen_snapshot, verify_frozen_snapshot},
    PublishWorkflowStore, StartPublishWorkflowRequest,
};
use crate::{
    artifact_publisher::{ArtifactPublisher, FileArtifactPublisher, StagedArtifact},
    config::RuntimeConfig,
    conversation::RuntimeStore,
    publication::{
        PublicationDesiredState, PublicationIntent, PublishOperationKind, PublishOperationStatus,
        WorkRuntimeStatus,
    },
    release::{
        ReleasePackagingInput, ReleasePackagingStatus, RuntimeProfile, WorkReleaseStatus,
        STATIC_WEB_PROFILE_ID,
    },
    runtime::{RuntimeSupervisor, SupervisorError},
    templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
    types::{ArtifactPublishStatus, ProjectVersionStatus},
    visual_contracts::{
        DraftSnapshot, DraftSnapshotRetentionState, PublishSource, PublishWorkflow,
        PublishWorkflowCheckpoint, PublishWorkflowStatus, RunVisualTarget, VisualReviewMode,
        VisualReviewStatus,
    },
    visual_review::FileVisualReviewStore,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use std::{sync::Arc, time::Duration};

const EXTERNAL_PROBE_WINDOW: chrono::Duration = chrono::Duration::seconds(120);

#[derive(Clone)]
pub struct PublishWorkflowService {
    pub store: Arc<PublishWorkflowStore>,
    runtime_store: RuntimeStore,
    config: RuntimeConfig,
}

impl PublishWorkflowService {
    pub fn new(runtime_store: RuntimeStore, config: RuntimeConfig) -> Self {
        Self {
            store: runtime_store.publish_workflow_store(),
            runtime_store,
            config,
        }
    }

    pub async fn start(
        &self,
        project_id: &str,
        request: &StartPublishWorkflowRequest,
    ) -> Result<(PublishWorkflow, bool)> {
        self.validate_runtime_configuration()?;
        let snapshot = self.resolve_source(&request.source).await?;
        self.validate_visual_review(request, &snapshot)?;
        self.store
            .start(project_id, request)
            .map_err(anyhow::Error::new)
    }

    pub fn get(&self, id: &str) -> Option<PublishWorkflow> {
        self.store.get(id)
    }

    pub fn list_for_project(&self, project_id: &str) -> Vec<PublishWorkflow> {
        self.store.list_for_project(project_id)
    }

    pub fn cancel(&self, id: &str) -> Result<PublishWorkflow> {
        self.store.cancel(id).map_err(anyhow::Error::new)
    }

    fn validate_runtime_configuration(&self) -> Result<()> {
        if self.config.release_base_image_digest.is_none()
            || self.config.release_packager_version.is_none()
            || self.config.release_registry_repository.is_none()
            || self.config.release_scan_policy_version.is_none()
            || self.config.release_packaging_helper_path.is_none()
            || self.config.release_packaging_helper_sha256.is_none()
        {
            bail!("publish workflow release packaging is not configured");
        }
        if self.config.works_base_domain.is_none() {
            bail!("publish workflow public URL domain is not configured");
        }
        Ok(())
    }

    async fn resolve_source(&self, source: &PublishSource) -> Result<DraftSnapshot> {
        source.validate().map_err(|error| anyhow!(error))?;
        if let PublishSource::DraftRevision {
            project_id,
            session_id,
            session_epoch,
            revision,
            snapshot_id,
            ..
        } = source
        {
            let session = self
                .runtime_store
                .draft_preview_store()
                .get(session_id)
                .ok_or_else(|| anyhow!("publish.draft_session_not_found"))?;
            if session.project_id != *project_id
                || session.session_epoch != *session_epoch
                || session.durable_revision != *revision
                || session.durable_snapshot_id != *snapshot_id
                || session.last_ready_revision < *revision
            {
                bail!("publish.source_identity_stale");
            }
            self.runtime_store
                .draft_preview_store()
                .mark_publish_revision(session_id, *session_epoch, *revision, snapshot_id)
                .map_err(|_| anyhow!("publish.source_identity_stale"))?;
        }
        let snapshot = self
            .runtime_store
            .get_draft_snapshot(source.snapshot_id())
            .await
            .with_context(|| format!("DraftSnapshot not found: {}", source.snapshot_id()))?;
        if snapshot.project_id != source.project_id()
            || snapshot.source_hash != source.expected_source_hash()
        {
            bail!("publish.source_identity_stale");
        }
        if snapshot.retention_state == DraftSnapshotRetentionState::DeletionPending {
            bail!("publish source is pending deletion");
        }
        verify_frozen_snapshot(&self.config.runtime_storage_dir, &snapshot)?;
        Ok(snapshot)
    }

    fn validate_visual_review(
        &self,
        request: &StartPublishWorkflowRequest,
        snapshot: &DraftSnapshot,
    ) -> Result<()> {
        if request.visual_review_mode != VisualReviewMode::Required {
            return Ok(());
        }
        let target = visual_target(&request.source);
        let review = FileVisualReviewStore::new(&self.config.runtime_storage_dir)
            .latest(&snapshot.project_id, &target)?
            .filter(|review| review.state.status == VisualReviewStatus::Passed)
            .ok_or_else(|| anyhow!("required visual review has not passed for PublishSource"))?;
        if review.state.mode != VisualReviewMode::Required
            && review.state.mode != VisualReviewMode::Advisory
        {
            bail!("required visual review evidence is invalid");
        }
        Ok(())
    }

    async fn reconcile_workflow(&self, workflow: PublishWorkflow) -> Result<()> {
        match workflow.checkpoint {
            PublishWorkflowCheckpoint::Requested => self.freeze_source(&workflow).await,
            PublishWorkflowCheckpoint::SourceFrozen => {
                self.advance_to_building(&workflow).map(|_| ())
            }
            PublishWorkflowCheckpoint::Building => self.build_and_promote_version(&workflow).await,
            PublishWorkflowCheckpoint::Validating => self.prepare_release(&workflow).await,
            PublishWorkflowCheckpoint::ReleasePackaging => self.observe_release(&workflow),
            PublishWorkflowCheckpoint::ReleaseValidated => self.commit_publication(&workflow).await,
            PublishWorkflowCheckpoint::DesiredStateCommitted
            | PublishWorkflowCheckpoint::Reconciling
            | PublishWorkflowCheckpoint::WorkloadReady
            | PublishWorkflowCheckpoint::TrafficSwitched
            | PublishWorkflowCheckpoint::ExternalProbePassed => {
                self.observe_publication(&workflow).await
            }
            PublishWorkflowCheckpoint::RollingBack => self.observe_rollback(&workflow),
            PublishWorkflowCheckpoint::Completed | PublishWorkflowCheckpoint::RolledBack => Ok(()),
        }
    }

    async fn freeze_source(&self, workflow: &PublishWorkflow) -> Result<()> {
        let snapshot = self.resolve_source(&workflow.source).await?;
        let request = StartPublishWorkflowRequest {
            source: workflow.source.clone(),
            idempotency_key: "persisted".to_string(),
            expected_current_release_id: workflow.expected_current_release_id.clone(),
            expected_generation: workflow.expected_generation,
            visual_review_mode: workflow.visual_review_mode,
            runtime_profile_id: STATIC_WEB_PROFILE_ID.to_string(),
        };
        self.validate_visual_review(&request, &snapshot)?;
        self.store.advance(
            &workflow.id,
            PublishWorkflowCheckpoint::Requested,
            PublishWorkflowStatus::SourceFrozen,
            PublishWorkflowCheckpoint::SourceFrozen,
            &snapshot.source_hash,
            None,
            None,
            None,
            None,
            None,
        )?;
        Ok(())
    }

    fn advance_to_building(&self, workflow: &PublishWorkflow) -> Result<PublishWorkflow> {
        self.store
            .advance(
                &workflow.id,
                PublishWorkflowCheckpoint::SourceFrozen,
                PublishWorkflowStatus::Building,
                PublishWorkflowCheckpoint::Building,
                workflow.source.expected_source_hash(),
                None,
                None,
                None,
                None,
                None,
            )
            .map_err(anyhow::Error::new)
    }

    async fn build_and_promote_version(&self, workflow: &PublishWorkflow) -> Result<()> {
        let snapshot = self.resolve_source(&workflow.source).await?;
        let output =
            build_frozen_snapshot(&self.config.runtime_storage_dir, &workflow.id, &snapshot)
                .await?;
        let version_id = format!("version-{}", &workflow.request_hash[..32]);
        let immutable_preview_uri = format!(
            "runtime://artifacts/{}/versions/{version_id}",
            workflow.project_id
        );
        let version = self
            .runtime_store
            .create_project_version_candidate_with_id(
                &version_id,
                &workflow.project_id,
                &snapshot.created_by_run_id,
                immutable_preview_uri,
                None,
                Some(snapshot.source_snapshot_uri.clone()),
            )
            .await?;
        let expected_current_version_id = self
            .runtime_store
            .list_project_versions(&workflow.project_id)
            .await
            .into_iter()
            .filter(|candidate| {
                candidate.id != version.id && candidate.status == ProjectVersionStatus::Promoted
            })
            .max_by_key(|candidate| candidate.promoted_at.unwrap_or(candidate.created_at))
            .map(|candidate| candidate.id);
        let mut publish = self
            .runtime_store
            .begin_artifact_publish(
                &workflow.project_id,
                &snapshot.created_by_run_id,
                &workflow.id,
                &version.id,
                &output.output_hash,
                &snapshot.source_snapshot_uri,
                expected_current_version_id.as_deref(),
            )
            .await?;
        let publisher = FileArtifactPublisher::new(&self.config.runtime_storage_dir);
        if publish.status == ArtifactPublishStatus::Staging {
            let registry = BuiltInTemplateRegistry::built_in();
            let template = registry.current(&TemplateId::parse("next-app")?)?;
            let staged = publisher
                .stage_directory(
                    &workflow.project_id,
                    &version.id,
                    &output.output_hash,
                    &output.output_root,
                    &template,
                )
                .await?;
            publish = self
                .runtime_store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::Staged,
                    Some(&staged.artifact_manifest_hash),
                    Some(&staged.staged_uri),
                    None,
                    None,
                )
                .await?;
        }
        if publish.status == ArtifactPublishStatus::Staged {
            publish = self
                .runtime_store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::Validating,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
        }
        if publish.status == ArtifactPublishStatus::Validating {
            publish = self
                .runtime_store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::Ready,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
        }
        if matches!(
            publish.status,
            ArtifactPublishStatus::Ready | ArtifactPublishStatus::ReconcileRequired
        ) {
            publish = self
                .runtime_store
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
        if publish.status == ArtifactPublishStatus::Promoting {
            let staged = staged_from_publish(&publish)?;
            let immutable_uri = publisher.promote(&staged).await?;
            self.runtime_store
                .transition_artifact_publish(
                    &publish.id,
                    ArtifactPublishStatus::Promoting,
                    None,
                    None,
                    Some(&immutable_uri),
                    None,
                )
                .await?;
            self.runtime_store
                .commit_artifact_promotion_cas(
                    &workflow.project_id,
                    &snapshot.created_by_run_id,
                    &version.id,
                    &publish.id,
                    expected_current_version_id.as_deref(),
                )
                .await?;
        } else if publish.status != ArtifactPublishStatus::Promoted {
            bail!(
                "production artifact cannot be promoted from {:?}",
                publish.status
            );
        }
        self.store.advance(
            &workflow.id,
            PublishWorkflowCheckpoint::Building,
            PublishWorkflowStatus::Validating,
            PublishWorkflowCheckpoint::Validating,
            &output.output_hash,
            Some(publish.id),
            Some(version.id),
            None,
            None,
            None,
        )?;
        Ok(())
    }

    async fn prepare_release(&self, workflow: &PublishWorkflow) -> Result<()> {
        let version_id = workflow
            .version_id
            .as_deref()
            .context("PublishWorkflow has no production WorkVersion")?;
        let version = self
            .runtime_store
            .get_project_version(version_id)
            .await
            .context("production WorkVersion is missing")?;
        if version.status != ProjectVersionStatus::Promoted {
            bail!("production WorkVersion is not promoted");
        }
        let publish = self
            .runtime_store
            .artifact_publish_for_version(
                &workflow.project_id,
                &version.created_by_run_id,
                version_id,
            )
            .await
            .context("promoted artifact evidence is missing")?;
        if publish.status != ArtifactPublishStatus::Promoted {
            bail!("production artifact evidence is not promoted");
        }
        let snapshot = self.resolve_source(&workflow.source).await?;
        let profile = configured_profile(&self.config)?;
        let input = ReleasePackagingInput {
            project_id: workflow.project_id.clone(),
            version_id: version_id.to_string(),
            run_id: version.created_by_run_id,
            template_id: snapshot.template_id,
            template_version: snapshot.template_version,
            artifact_manifest_hash: publish
                .artifact_manifest_hash
                .context("artifact manifest hash is missing")?,
            runtime_manifest_hash: profile.manifest.sha256()?,
            source_snapshot_uri: version
                .source_snapshot_uri
                .unwrap_or(publish.source_snapshot_uri),
            runtime_profile_id: profile.id,
            base_image_digest: profile.base_image_digest,
            packager_version: profile.packager_version,
            registry_repository: self
                .config
                .release_registry_repository
                .clone()
                .context("release registry repository is missing")?,
            scan_policy_version: profile.scan_policy_version,
        };
        let (release, packaging) = self
            .runtime_store
            .release_store()
            .prepare(&input)
            .map_err(anyhow::Error::new)?;
        self.store.advance(
            &workflow.id,
            PublishWorkflowCheckpoint::Validating,
            PublishWorkflowStatus::ReleasePackaging,
            PublishWorkflowCheckpoint::ReleasePackaging,
            &input.artifact_manifest_hash,
            Some(packaging.id),
            None,
            Some(release.id),
            None,
            None,
        )?;
        Ok(())
    }

    fn observe_release(&self, workflow: &PublishWorkflow) -> Result<()> {
        let release_id = workflow
            .release_id
            .as_deref()
            .context("release id is missing")?;
        let release_store = self.runtime_store.release_store();
        let release = release_store
            .release(release_id)
            .context("WorkRelease is missing")?;
        let packaging = release_store
            .packaging_for_release(release_id)
            .context("Release packaging is missing")?;
        if release.status == WorkReleaseStatus::Failed
            || packaging.status == ReleasePackagingStatus::Failed
        {
            bail!(
                "release packaging failed: {}",
                packaging
                    .last_error
                    .unwrap_or_else(|| "unknown failure".to_string())
            );
        }
        if release.status != WorkReleaseStatus::Validated
            || packaging.status != ReleasePackagingStatus::Validated
        {
            return Ok(());
        }
        self.store.advance(
            &workflow.id,
            PublishWorkflowCheckpoint::ReleasePackaging,
            PublishWorkflowStatus::ReleaseValidated,
            PublishWorkflowCheckpoint::ReleaseValidated,
            &release.artifact_manifest_hash,
            Some(packaging.id),
            None,
            None,
            None,
            None,
        )?;
        Ok(())
    }

    async fn commit_publication(&self, workflow: &PublishWorkflow) -> Result<()> {
        let release_id = workflow
            .release_id
            .clone()
            .context("release id is missing")?;
        let access = self
            .runtime_store
            .get_project_access(&workflow.project_id)
            .await
            .context("project workspace is not provisioned")?;
        let kind = if workflow.expected_current_release_id.is_some() {
            PublishOperationKind::Update
        } else {
            PublishOperationKind::Publish
        };
        let intent = PublicationIntent {
            project_id: workflow.project_id.clone(),
            workspace_namespace: access.workspace_namespace,
            kind,
            release_id: Some(release_id.clone()),
            expected_current_release_id: workflow.expected_current_release_id.clone(),
            expected_generation: Some(workflow.expected_generation),
            runtime_profile_id: STATIC_WEB_PROFILE_ID.to_string(),
            idempotency_key: format!("publish-workflow:{}", workflow.id),
        };
        let (operation, _) = self
            .runtime_store
            .publication_store()
            .commit_intent(&intent)
            .map_err(anyhow::Error::new)?;
        self.store.advance(
            &workflow.id,
            PublishWorkflowCheckpoint::ReleaseValidated,
            PublishWorkflowStatus::DesiredStateCommitted,
            PublishWorkflowCheckpoint::DesiredStateCommitted,
            &operation.request_hash,
            Some(operation.id.clone()),
            None,
            None,
            Some(operation.id),
            None,
        )?;
        Ok(())
    }

    async fn observe_publication(&self, workflow: &PublishWorkflow) -> Result<()> {
        let operation_id = workflow
            .publication_operation_id
            .as_deref()
            .context("publication operation id is missing")?;
        let publication = self.runtime_store.publication_store();
        let operation = publication
            .operation(operation_id)
            .context("publication operation is missing")?;
        match operation.status {
            PublishOperationStatus::DesiredStateCommitted => Ok(()),
            PublishOperationStatus::ReconcileRequired
                if publication_failure_window_expired(workflow) =>
            {
                self.begin_rollback(workflow).await
            }
            PublishOperationStatus::Reconciling | PublishOperationStatus::ReconcileRequired => self
                .advance_publication_checkpoint(
                    workflow,
                    PublishWorkflowCheckpoint::Reconciling,
                    PublishWorkflowStatus::Reconciling,
                    &operation.request_hash,
                ),
            PublishOperationStatus::WorkloadReady => self.advance_publication_checkpoint(
                workflow,
                PublishWorkflowCheckpoint::WorkloadReady,
                PublishWorkflowStatus::WorkloadReady,
                &operation.request_hash,
            ),
            PublishOperationStatus::TrafficSwitched => self.advance_publication_checkpoint(
                workflow,
                PublishWorkflowCheckpoint::TrafficSwitched,
                PublishWorkflowStatus::TrafficSwitched,
                &operation.request_hash,
            ),
            PublishOperationStatus::ExternalProbePassed => self.advance_publication_checkpoint(
                workflow,
                PublishWorkflowCheckpoint::ExternalProbePassed,
                PublishWorkflowStatus::ExternalProbePassed,
                &operation.request_hash,
            ),
            PublishOperationStatus::Completed => {
                let runtime = publication
                    .runtime(&workflow.project_id)
                    .context("publication runtime is missing")?;
                if runtime.status != WorkRuntimeStatus::Published
                    || runtime.current_release_id != workflow.release_id
                {
                    bail!("completed publication does not match the workflow release");
                }
                let domain = self
                    .config
                    .works_base_domain
                    .as_deref()
                    .context("works base domain is missing")?;
                let public_url = format!("https://{}.{}", runtime.host_slug, domain);
                self.complete_publication_checkpoints(workflow, &operation.request_hash, public_url)
            }
            PublishOperationStatus::Failed | PublishOperationStatus::Cancelled => {
                bail!(
                    "publication operation ended as {:?}: {}",
                    operation.status,
                    operation.last_error.unwrap_or_default()
                )
            }
            PublishOperationStatus::Requested
            | PublishOperationStatus::Packaging
            | PublishOperationStatus::ReleaseValidated => Ok(()),
        }
    }

    async fn begin_rollback(&self, workflow: &PublishWorkflow) -> Result<()> {
        let publication = self.runtime_store.publication_store();
        let runtime = publication
            .runtime(&workflow.project_id)
            .context("publication runtime is missing before rollback")?;
        let access = self
            .runtime_store
            .get_project_access(&workflow.project_id)
            .await
            .context("project workspace is not provisioned for rollback")?;
        let (kind, release_id, expected_current_release_id) =
            if let Some(previous) = runtime.current_release_id.clone() {
                (
                    PublishOperationKind::Rollback,
                    Some(previous.clone()),
                    Some(previous),
                )
            } else {
                (PublishOperationKind::Unpublish, None, None)
            };
        let intent = PublicationIntent {
            project_id: workflow.project_id.clone(),
            workspace_namespace: access.workspace_namespace,
            kind,
            release_id,
            expected_current_release_id,
            expected_generation: Some(runtime.desired_generation),
            runtime_profile_id: STATIC_WEB_PROFILE_ID.to_string(),
            idempotency_key: format!("publish-workflow:{}:rollback", workflow.id),
        };
        let (operation, _) = publication
            .commit_intent(&intent)
            .map_err(anyhow::Error::new)?;
        self.store.advance(
            &workflow.id,
            workflow.checkpoint,
            PublishWorkflowStatus::RollingBack,
            PublishWorkflowCheckpoint::RollingBack,
            &operation.request_hash,
            Some(operation.id.clone()),
            None,
            None,
            Some(operation.id),
            None,
        )?;
        Ok(())
    }

    fn observe_rollback(&self, workflow: &PublishWorkflow) -> Result<()> {
        let operation_id = workflow
            .publication_operation_id
            .as_deref()
            .context("rollback publication operation id is missing")?;
        let publication = self.runtime_store.publication_store();
        let operation = publication
            .operation(operation_id)
            .context("rollback publication operation is missing")?;
        if matches!(
            operation.status,
            PublishOperationStatus::Failed | PublishOperationStatus::Cancelled
        ) {
            self.store.set_status(
                &workflow.id,
                PublishWorkflowStatus::RollbackFailed,
                Some(
                    operation
                        .last_error
                        .unwrap_or_else(|| "rollback operation failed".to_string()),
                ),
            )?;
            return Ok(());
        }
        if operation.status != PublishOperationStatus::Completed {
            return Ok(());
        }
        let runtime = publication
            .runtime(&workflow.project_id)
            .context("rollback runtime is missing")?;
        let restored = match operation.kind {
            PublishOperationKind::Rollback => {
                runtime.status == WorkRuntimeStatus::Published
                    && runtime.current_release_id == operation.release_id
            }
            PublishOperationKind::Unpublish => {
                runtime.status == WorkRuntimeStatus::Unpublished
                    && runtime.desired_publication == PublicationDesiredState::Unpublished
                    && runtime.current_release_id.is_none()
            }
            _ => false,
        };
        if !restored {
            bail!("rollback operation completed without restoring the expected runtime state");
        }
        self.store.advance(
            &workflow.id,
            PublishWorkflowCheckpoint::RollingBack,
            PublishWorkflowStatus::RolledBack,
            PublishWorkflowCheckpoint::RolledBack,
            &operation.request_hash,
            Some(operation.id),
            None,
            None,
            None,
            None,
        )?;
        Ok(())
    }

    fn advance_publication_checkpoint(
        &self,
        workflow: &PublishWorkflow,
        target: PublishWorkflowCheckpoint,
        status: PublishWorkflowStatus,
        input_hash: &str,
    ) -> Result<()> {
        if checkpoint_rank(workflow.checkpoint) >= checkpoint_rank(target) {
            return Ok(());
        }
        let next = next_publication_checkpoint(workflow.checkpoint)
            .context("workflow cannot advance to publication checkpoint")?;
        if checkpoint_rank(next) > checkpoint_rank(target) {
            return Ok(());
        }
        self.store.advance(
            &workflow.id,
            workflow.checkpoint,
            if next == target {
                status
            } else {
                status_for(next)
            },
            next,
            input_hash,
            workflow.publication_operation_id.clone(),
            None,
            None,
            None,
            None,
        )?;
        Ok(())
    }

    fn complete_publication_checkpoints(
        &self,
        workflow: &PublishWorkflow,
        input_hash: &str,
        public_url: String,
    ) -> Result<()> {
        let mut current = workflow.clone();
        for (checkpoint, status) in [
            (
                PublishWorkflowCheckpoint::Reconciling,
                PublishWorkflowStatus::Reconciling,
            ),
            (
                PublishWorkflowCheckpoint::WorkloadReady,
                PublishWorkflowStatus::WorkloadReady,
            ),
            (
                PublishWorkflowCheckpoint::TrafficSwitched,
                PublishWorkflowStatus::TrafficSwitched,
            ),
            (
                PublishWorkflowCheckpoint::ExternalProbePassed,
                PublishWorkflowStatus::ExternalProbePassed,
            ),
            (
                PublishWorkflowCheckpoint::Completed,
                PublishWorkflowStatus::Completed,
            ),
        ] {
            if checkpoint_rank(current.checkpoint) >= checkpoint_rank(checkpoint) {
                continue;
            }
            current = self.store.advance(
                &current.id,
                current.checkpoint,
                status,
                checkpoint,
                input_hash,
                current.publication_operation_id.clone(),
                None,
                None,
                None,
                (checkpoint == PublishWorkflowCheckpoint::Completed).then(|| public_url.clone()),
            )?;
        }
        Ok(())
    }
}

pub struct PublishWorkflowController {
    service: Arc<PublishWorkflowService>,
    interval: Duration,
}

impl PublishWorkflowController {
    pub fn new(service: Arc<PublishWorkflowService>, interval: Duration) -> Self {
        Self { service, interval }
    }

    pub fn spawn(self, supervisor: &RuntimeSupervisor) -> Result<(), SupervisorError> {
        supervisor.spawn_with_shutdown(
            "controller/publish-workflow",
            true,
            move |mut shutdown| async move {
                loop {
                    self.reconcile_once().await;
                    tokio::select! {
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() { break; }
                        }
                        _ = tokio::time::sleep(self.interval) => {}
                    }
                }
                Ok(())
            },
        )
    }

    pub async fn reconcile_once(&self) -> usize {
        let workflows = self.service.store.nonterminal();
        let mut processed = 0;
        for workflow in workflows {
            let id = workflow.id.clone();
            if let Err(error) = self.service.reconcile_workflow(workflow).await {
                let status = self
                    .service
                    .store
                    .get(&id)
                    .map(|current| {
                        if checkpoint_rank(current.checkpoint)
                            >= checkpoint_rank(PublishWorkflowCheckpoint::TrafficSwitched)
                        {
                            PublishWorkflowStatus::RollingBack
                        } else {
                            PublishWorkflowStatus::Failed
                        }
                    })
                    .unwrap_or(PublishWorkflowStatus::Failed);
                let _ = self
                    .service
                    .store
                    .set_status(&id, status, Some(error.to_string()));
            }
            processed += 1;
        }
        processed
    }
}

fn configured_profile(config: &RuntimeConfig) -> Result<RuntimeProfile> {
    RuntimeProfile::static_web_v1(
        config
            .release_base_image_digest
            .clone()
            .context("release base image digest is missing")?,
        config
            .release_packager_version
            .clone()
            .context("release packager version is missing")?,
        config
            .release_scan_policy_version
            .clone()
            .context("release scan policy version is missing")?,
    )
}

fn staged_from_publish(publish: &crate::types::ArtifactPublishRecord) -> Result<StagedArtifact> {
    Ok(StagedArtifact {
        project_id: publish.project_id.clone(),
        version_id: publish.version_id.clone(),
        candidate_manifest_hash: publish.candidate_manifest_hash.clone(),
        artifact_manifest_hash: publish
            .artifact_manifest_hash
            .clone()
            .context("artifact manifest hash is missing")?,
        staged_uri: publish
            .staged_uri
            .clone()
            .context("staged artifact URI is missing")?,
        file_count: 0,
    })
}

fn visual_target(source: &PublishSource) -> RunVisualTarget {
    match source {
        PublishSource::StaticSnapshot {
            snapshot_id,
            expected_source_hash,
            ..
        } => RunVisualTarget::StaticSnapshot {
            snapshot_id: snapshot_id.clone(),
            source_hash: expected_source_hash.clone(),
        },
        PublishSource::DraftRevision {
            session_id,
            session_epoch,
            revision,
            expected_source_hash,
            ..
        } => RunVisualTarget::Draft {
            session_id: session_id.clone(),
            session_epoch: *session_epoch,
            source_revision: *revision,
            source_hash: expected_source_hash.clone(),
        },
    }
}

fn next_publication_checkpoint(
    checkpoint: PublishWorkflowCheckpoint,
) -> Option<PublishWorkflowCheckpoint> {
    match checkpoint {
        PublishWorkflowCheckpoint::DesiredStateCommitted => {
            Some(PublishWorkflowCheckpoint::Reconciling)
        }
        PublishWorkflowCheckpoint::Reconciling => Some(PublishWorkflowCheckpoint::WorkloadReady),
        PublishWorkflowCheckpoint::WorkloadReady => {
            Some(PublishWorkflowCheckpoint::TrafficSwitched)
        }
        PublishWorkflowCheckpoint::TrafficSwitched => {
            Some(PublishWorkflowCheckpoint::ExternalProbePassed)
        }
        PublishWorkflowCheckpoint::ExternalProbePassed => {
            Some(PublishWorkflowCheckpoint::Completed)
        }
        _ => None,
    }
}

fn status_for(checkpoint: PublishWorkflowCheckpoint) -> PublishWorkflowStatus {
    match checkpoint {
        PublishWorkflowCheckpoint::Reconciling => PublishWorkflowStatus::Reconciling,
        PublishWorkflowCheckpoint::WorkloadReady => PublishWorkflowStatus::WorkloadReady,
        PublishWorkflowCheckpoint::TrafficSwitched => PublishWorkflowStatus::TrafficSwitched,
        PublishWorkflowCheckpoint::ExternalProbePassed => {
            PublishWorkflowStatus::ExternalProbePassed
        }
        PublishWorkflowCheckpoint::Completed => PublishWorkflowStatus::Completed,
        _ => PublishWorkflowStatus::Failed,
    }
}

fn checkpoint_rank(checkpoint: PublishWorkflowCheckpoint) -> u8 {
    match checkpoint {
        PublishWorkflowCheckpoint::Requested => 0,
        PublishWorkflowCheckpoint::SourceFrozen => 1,
        PublishWorkflowCheckpoint::Building => 2,
        PublishWorkflowCheckpoint::Validating => 3,
        PublishWorkflowCheckpoint::ReleasePackaging => 4,
        PublishWorkflowCheckpoint::ReleaseValidated => 5,
        PublishWorkflowCheckpoint::DesiredStateCommitted => 6,
        PublishWorkflowCheckpoint::Reconciling => 7,
        PublishWorkflowCheckpoint::WorkloadReady => 8,
        PublishWorkflowCheckpoint::TrafficSwitched => 9,
        PublishWorkflowCheckpoint::ExternalProbePassed => 10,
        PublishWorkflowCheckpoint::RollingBack => 11,
        PublishWorkflowCheckpoint::Completed => 12,
        PublishWorkflowCheckpoint::RolledBack => 13,
    }
}

fn publication_failure_window_expired(workflow: &PublishWorkflow) -> bool {
    publication_failure_window_expired_at(workflow, Utc::now())
}

fn publication_failure_window_expired_at(
    workflow: &PublishWorkflow,
    now: chrono::DateTime<Utc>,
) -> bool {
    workflow
        .evidence
        .iter()
        .rev()
        .find(|evidence| {
            matches!(
                evidence.stage,
                PublishWorkflowCheckpoint::TrafficSwitched
                    | PublishWorkflowCheckpoint::DesiredStateCommitted
            )
        })
        .is_some_and(|evidence| now - evidence.completed_at >= EXTERNAL_PROBE_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifact_publisher::{ArtifactFile, FileArtifactPublisher},
        publication::{KubernetesResourceIdentity, ObservedWorkRuntime},
        release::PackagingScanEvidence,
        templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
        visual_contracts::RUNTIME_DEPENDENCY_POLICY_VERSION,
    };
    use std::path::PathBuf;

    fn observed(external: bool) -> ObservedWorkRuntime {
        ObservedWorkRuntime {
            deployment: KubernetesResourceIdentity {
                name: "work-deployment".to_string(),
                uid: "deployment-uid".to_string(),
                resource_version: "1".to_string(),
            },
            service: KubernetesResourceIdentity {
                name: "work-service".to_string(),
                uid: "service-uid".to_string(),
                resource_version: "2".to_string(),
            },
            ingress: external.then(|| KubernetesResourceIdentity {
                name: "work-ingress".to_string(),
                uid: "ingress-uid".to_string(),
                resource_version: "3".to_string(),
            }),
            ready: true,
            release_identity_verified: true,
            external_release_identity_verified: external,
        }
    }

    #[tokio::test]
    async fn failed_update_probe_creates_compensating_rollback_and_restores_blue() {
        let root = std::env::temp_dir().join(format!(
            "publish-workflow-rollback-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let runtime_store = RuntimeStore::with_checkpoint_dir(&root);
        runtime_store
            .upsert_project_access(
                "project-rollback",
                "owner".to_string(),
                "ws-project-rollback".to_string(),
            )
            .await
            .unwrap();
        let publication = runtime_store.publication_store();
        let (blue, _) = publication
            .commit_intent(&PublicationIntent {
                project_id: "project-rollback".to_string(),
                workspace_namespace: "ws-project-rollback".to_string(),
                kind: PublishOperationKind::Publish,
                release_id: Some("release-blue".to_string()),
                expected_current_release_id: None,
                expected_generation: Some(0),
                runtime_profile_id: STATIC_WEB_PROFILE_ID.to_string(),
                idempotency_key: "publish-blue".to_string(),
            })
            .unwrap();
        let blue_outbox = publication
            .pending_outbox()
            .into_iter()
            .find(|event| event.operation_id == blue.id)
            .unwrap();
        publication
            .record_workload_ready(&blue_outbox.id, &observed(true))
            .unwrap();

        let (green, _) = publication
            .commit_intent(&PublicationIntent {
                project_id: "project-rollback".to_string(),
                workspace_namespace: "ws-project-rollback".to_string(),
                kind: PublishOperationKind::Update,
                release_id: Some("release-green".to_string()),
                expected_current_release_id: Some("release-blue".to_string()),
                expected_generation: Some(1),
                runtime_profile_id: STATIC_WEB_PROFILE_ID.to_string(),
                idempotency_key: "publish-green".to_string(),
            })
            .unwrap();
        let green_outbox = publication
            .pending_outbox()
            .into_iter()
            .find(|event| event.operation_id == green.id)
            .unwrap();
        publication
            .record_traffic_switched(&green_outbox.id, &observed(false))
            .unwrap();

        let mut config = RuntimeConfig::from_env();
        config.runtime_storage_dir = root.clone();
        config.works_base_domain = Some("works.example.test".to_string());
        let service = PublishWorkflowService::new(runtime_store.clone(), config);
        let request = StartPublishWorkflowRequest {
            source: PublishSource::StaticSnapshot {
                project_id: "project-rollback".to_string(),
                snapshot_id: "snapshot-green".to_string(),
                expected_source_hash: "a".repeat(64),
            },
            idempotency_key: "workflow-green".to_string(),
            expected_current_release_id: Some("release-blue".to_string()),
            expected_generation: 1,
            visual_review_mode: VisualReviewMode::Advisory,
            runtime_profile_id: STATIC_WEB_PROFILE_ID.to_string(),
        };
        let (mut workflow, _) = service.store.start("project-rollback", &request).unwrap();
        for (checkpoint, status) in [
            (
                PublishWorkflowCheckpoint::SourceFrozen,
                PublishWorkflowStatus::SourceFrozen,
            ),
            (
                PublishWorkflowCheckpoint::Building,
                PublishWorkflowStatus::Building,
            ),
            (
                PublishWorkflowCheckpoint::Validating,
                PublishWorkflowStatus::Validating,
            ),
            (
                PublishWorkflowCheckpoint::ReleasePackaging,
                PublishWorkflowStatus::ReleasePackaging,
            ),
            (
                PublishWorkflowCheckpoint::ReleaseValidated,
                PublishWorkflowStatus::ReleaseValidated,
            ),
            (
                PublishWorkflowCheckpoint::DesiredStateCommitted,
                PublishWorkflowStatus::DesiredStateCommitted,
            ),
            (
                PublishWorkflowCheckpoint::Reconciling,
                PublishWorkflowStatus::Reconciling,
            ),
            (
                PublishWorkflowCheckpoint::WorkloadReady,
                PublishWorkflowStatus::WorkloadReady,
            ),
            (
                PublishWorkflowCheckpoint::TrafficSwitched,
                PublishWorkflowStatus::TrafficSwitched,
            ),
        ] {
            workflow = service
                .store
                .advance(
                    &workflow.id,
                    workflow.checkpoint,
                    status,
                    checkpoint,
                    &"b".repeat(64),
                    Some(green.id.clone()),
                    (checkpoint == PublishWorkflowCheckpoint::Validating)
                        .then(|| "version-green".to_string()),
                    (checkpoint == PublishWorkflowCheckpoint::ReleasePackaging)
                        .then(|| "release-green".to_string()),
                    (checkpoint == PublishWorkflowCheckpoint::DesiredStateCommitted)
                        .then(|| green.id.clone()),
                    None,
                )
                .unwrap();
        }
        assert!(publication_failure_window_expired_at(
            &workflow,
            Utc::now() + chrono::Duration::seconds(121)
        ));

        service.begin_rollback(&workflow).await.unwrap();
        let rolling_back = service.store.get(&workflow.id).unwrap();
        assert_eq!(
            rolling_back.checkpoint,
            PublishWorkflowCheckpoint::RollingBack
        );
        let rollback_id = rolling_back.publication_operation_id.clone().unwrap();
        let rollback = publication.operation(&rollback_id).unwrap();
        assert_eq!(rollback.kind, PublishOperationKind::Rollback);
        assert_eq!(rollback.release_id.as_deref(), Some("release-blue"));
        let rollback_outbox = publication
            .pending_outbox()
            .into_iter()
            .find(|event| event.operation_id == rollback_id)
            .unwrap();
        publication
            .record_workload_ready(&rollback_outbox.id, &observed(true))
            .unwrap();
        service.observe_rollback(&rolling_back).unwrap();
        let rolled_back = service.store.get(&workflow.id).unwrap();
        assert_eq!(rolled_back.status, PublishWorkflowStatus::RolledBack);
        assert_eq!(
            publication
                .runtime("project-rollback")
                .unwrap()
                .current_release_id
                .as_deref(),
            Some("release-blue")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "full publish canary requires Node.js and npm registry/cache access"]
    async fn frozen_snapshot_reaches_real_build_release_publication_and_public_url() {
        let root = std::env::temp_dir().join(format!(
            "publish-workflow-e2e-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let runtime_store = RuntimeStore::with_checkpoint_dir(&root);
        runtime_store
            .upsert_project_access(
                "project-e2e",
                "owner".to_string(),
                "ws-project-e2e".to_string(),
            )
            .await
            .unwrap();
        let run = runtime_store
            .create_run(
                "project-e2e".to_string(),
                crate::types::AgentPhase::Build,
                "build".to_string(),
                "internal-balanced".to_string(),
                Vec::new(),
            )
            .await;
        let template = BuiltInTemplateRegistry::built_in()
            .current(&TemplateId::parse("next-app").unwrap())
            .unwrap();
        let files = template
            .files
            .iter()
            .map(|file| ArtifactFile {
                path: PathBuf::from(file.path),
                bytes: file.content_for_write().as_bytes().to_vec(),
            })
            .collect::<Vec<_>>();
        let source_hash = super::super::build::source_fingerprint(&files).unwrap();
        let source_uri = FileArtifactPublisher::new(&root)
            .publish_source_snapshot("project-e2e", "build-e2e", files)
            .await
            .unwrap();
        let snapshot = runtime_store
            .create_draft_snapshot(
                "project-e2e",
                source_uri,
                source_hash.clone(),
                "next-app".to_string(),
                "next-app@1".to_string(),
                RUNTIME_DEPENDENCY_POLICY_VERSION.to_string(),
                "d".repeat(64),
                &run.id,
                None,
                None,
            )
            .await
            .unwrap();
        let mut config = RuntimeConfig::from_env();
        config.runtime_storage_dir = root.clone();
        config.release_base_image_digest = Some(format!("sha256:{}", "1".repeat(64)));
        config.release_packager_version = Some("packager@1".to_string());
        config.release_registry_repository = Some("registry.example/works".to_string());
        config.release_scan_policy_version = Some("scan@1".to_string());
        config.release_packaging_helper_path = Some(PathBuf::from("/configured/packager"));
        config.release_packaging_helper_sha256 = Some("2".repeat(64));
        config.works_base_domain = Some("works.example.test".to_string());
        let service = Arc::new(PublishWorkflowService::new(runtime_store.clone(), config));
        let (started, _) = service
            .start(
                "project-e2e",
                &StartPublishWorkflowRequest {
                    source: PublishSource::StaticSnapshot {
                        project_id: "project-e2e".to_string(),
                        snapshot_id: snapshot.snapshot_id,
                        expected_source_hash: source_hash,
                    },
                    idempotency_key: "publish-e2e".to_string(),
                    expected_current_release_id: None,
                    expected_generation: 0,
                    visual_review_mode: VisualReviewMode::Advisory,
                    runtime_profile_id: STATIC_WEB_PROFILE_ID.to_string(),
                },
            )
            .await
            .unwrap();
        let controller = PublishWorkflowController::new(service.clone(), Duration::from_millis(1));
        for _ in 0..4 {
            controller.reconcile_once().await;
        }
        let packaging_workflow = service.store.get(&started.id).unwrap();
        assert_eq!(
            packaging_workflow.checkpoint,
            PublishWorkflowCheckpoint::ReleasePackaging,
            "workflow did not reach release packaging: {packaging_workflow:?}"
        );
        let release_id = packaging_workflow.release_id.clone().unwrap();
        let release_store = runtime_store.release_store();
        let packaging = release_store.packaging_for_release(&release_id).unwrap();
        let digest = format!("sha256:{}", "3".repeat(64));
        release_store.begin_build(&packaging.id).unwrap();
        release_store.record_built(&packaging.id, &digest).unwrap();
        release_store.record_pushed(&packaging.id, &digest).unwrap();
        release_store.begin_scan(&packaging.id).unwrap();
        release_store
            .record_scan(
                &packaging.id,
                &format!("sha256:{}", "4".repeat(64)),
                &format!("sha256:{}", "5".repeat(64)),
                PackagingScanEvidence {
                    policy_version: "scan@1".to_string(),
                    passed: true,
                    critical_vulnerabilities: 0,
                    high_vulnerabilities: 0,
                    secret_findings: 0,
                    report_digest: format!("sha256:{}", "6".repeat(64)),
                },
            )
            .unwrap();
        release_store
            .record_signature(
                &packaging.id,
                "test-signer",
                &format!("sha256:{}", "7".repeat(64)),
            )
            .unwrap();
        controller.reconcile_once().await;
        controller.reconcile_once().await;
        let committed = service.store.get(&started.id).unwrap();
        assert_eq!(
            committed.checkpoint,
            PublishWorkflowCheckpoint::DesiredStateCommitted
        );
        let publication = runtime_store.publication_store();
        let outbox = publication
            .pending_outbox()
            .into_iter()
            .find(|event| {
                Some(event.operation_id.as_str()) == committed.publication_operation_id.as_deref()
            })
            .unwrap();
        publication
            .record_workload_ready(&outbox.id, &observed(true))
            .unwrap();
        controller.reconcile_once().await;
        let completed = service.store.get(&started.id).unwrap();
        assert_eq!(completed.status, PublishWorkflowStatus::Completed);
        assert!(completed.public_url.as_deref().is_some_and(
            |url| url.starts_with("https://w-") && url.ends_with(".works.example.test")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
