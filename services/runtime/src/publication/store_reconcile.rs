use super::*;
use crate::publication::ObservedWorkRuntime;

impl PublicationStore {
    pub fn published_reconcile_outbox(&self) -> Vec<PublicationOutboxEvent> {
        let snapshot = self.state.lock().unwrap();
        snapshot
            .runtimes
            .values()
            .filter(|runtime| runtime.desired_publication == PublicationDesiredState::Published)
            .filter_map(|runtime| {
                snapshot
                    .outbox
                    .values()
                    .find(|event| {
                        event.project_id == runtime.project_id
                            && event.desired_generation == runtime.desired_generation
                    })
                    .cloned()
            })
            .collect()
    }

    pub fn record_workload_ready(
        &self,
        outbox_id: &str,
        observed: &ObservedWorkRuntime,
    ) -> Result<(PublishOperation, WorkRuntimeState), PublicationStoreError> {
        if !observed.ready || !observed.release_identity_verified {
            return Err(PublicationStoreError::InvalidTransition(
                "workload readiness and release identity evidence are required".to_string(),
            ));
        }
        if observed.ingress.is_some() != observed.external_release_identity_verified {
            return Err(PublicationStoreError::InvalidTransition(
                "Ingress identity and external release probe evidence must advance together"
                    .to_string(),
            ));
        }
        self.update_outbox(outbox_id, |operation, runtime, outbox| {
            if operation.desired_generation != runtime.desired_generation
                || operation.desired_generation != outbox.desired_generation
            {
                return Err(PublicationStoreError::Conflict(
                    "stale reconcile result cannot update a newer desired generation".to_string(),
                ));
            }
            let desired_release = runtime.desired_release_id.clone().ok_or_else(|| {
                PublicationStoreError::InvalidTransition(
                    "Published runtime is missing desired release".to_string(),
                )
            })?;
            if runtime.current_release_id.as_deref() != Some(desired_release.as_str()) {
                runtime.previous_release_id = runtime.current_release_id.clone();
            }
            runtime.current_release_id = Some(desired_release.clone());
            runtime.last_successful_release_id = Some(desired_release);
            if runtime.current_deployment_name.as_deref() != Some(observed.deployment.name.as_str())
            {
                runtime.previous_deployment_name = runtime.current_deployment_name.clone();
            }
            runtime.current_deployment_name = Some(observed.deployment.name.clone());
            runtime.deployment_uid = Some(observed.deployment.uid.clone());
            runtime.deployment_resource_version =
                Some(observed.deployment.resource_version.clone());
            runtime.service_uid = Some(observed.service.uid.clone());
            runtime.service_resource_version = Some(observed.service.resource_version.clone());
            if let Some(ingress) = &observed.ingress {
                runtime.ingress_uid = Some(ingress.uid.clone());
                runtime.ingress_resource_version = Some(ingress.resource_version.clone());
            }
            runtime.observed_generation = runtime.desired_generation;
            runtime.last_error = None;
            if observed.external_release_identity_verified {
                runtime.status = WorkRuntimeStatus::Published;
            } else if !matches!(runtime.status, WorkRuntimeStatus::Published) {
                runtime.status = if runtime.previous_release_id.is_some() {
                    WorkRuntimeStatus::Updating
                } else {
                    WorkRuntimeStatus::Publishing
                };
            }
            if observed.external_release_identity_verified {
                operation.status = PublishOperationStatus::Completed;
                operation.checkpoint = PublishCheckpoint::Completed;
            } else if !matches!(
                operation.status,
                PublishOperationStatus::TrafficSwitched
                    | PublishOperationStatus::ExternalProbePassed
                    | PublishOperationStatus::Completed
            ) {
                operation.status = PublishOperationStatus::WorkloadReady;
                operation.checkpoint = PublishCheckpoint::WorkloadReady;
            }
            operation.last_error = None;
            outbox.status = PublicationOutboxStatus::Delivered;
            outbox.last_error = None;
            outbox.delivered_at = Some(Utc::now());
            outbox.next_attempt_at = Utc::now();
            Ok(())
        })
        .map(|(operation, runtime, _)| (operation, runtime))
    }

    pub fn record_traffic_switched(
        &self,
        outbox_id: &str,
        observed: &ObservedWorkRuntime,
    ) -> Result<(PublishOperation, WorkRuntimeState), PublicationStoreError> {
        if !observed.ready
            || !observed.release_identity_verified
            || observed.external_release_identity_verified
        {
            return Err(PublicationStoreError::InvalidTransition(
                "traffic switch requires ready target and internal identity evidence before external verification"
                    .to_string(),
            ));
        }
        self.update_outbox(outbox_id, |operation, runtime, outbox| {
            if operation.desired_generation != runtime.desired_generation
                || operation.desired_generation != outbox.desired_generation
                || runtime.current_release_id == runtime.desired_release_id
            {
                return Err(PublicationStoreError::Conflict(
                    "stale or non-switch reconcile result cannot record traffic checkpoint"
                        .to_string(),
                ));
            }
            runtime.service_uid = Some(observed.service.uid.clone());
            runtime.service_resource_version = Some(observed.service.resource_version.clone());
            if let Some(ingress) = &observed.ingress {
                runtime.ingress_uid = Some(ingress.uid.clone());
                runtime.ingress_resource_version = Some(ingress.resource_version.clone());
            }
            runtime.status = WorkRuntimeStatus::Updating;
            runtime.last_error = None;
            operation.status = PublishOperationStatus::TrafficSwitched;
            operation.checkpoint = PublishCheckpoint::TrafficSwitched;
            operation.last_error = None;
            outbox.status = PublicationOutboxStatus::Pending;
            outbox.delivered_at = None;
            outbox.last_error = None;
            outbox.next_attempt_at = Utc::now();
            Ok(())
        })
        .map(|(operation, runtime, _)| (operation, runtime))
    }

    pub fn record_unpublished(
        &self,
        outbox_id: &str,
    ) -> Result<(PublishOperation, WorkRuntimeState), PublicationStoreError> {
        self.update_outbox(outbox_id, |operation, runtime, outbox| {
            if runtime.desired_publication != PublicationDesiredState::Unpublished
                || runtime.desired_release_id.is_some()
                || operation.desired_generation != runtime.desired_generation
                || outbox.desired_generation != runtime.desired_generation
            {
                return Err(PublicationStoreError::Conflict(
                    "stale or invalid Unpublish result cannot be committed".to_string(),
                ));
            }
            runtime.previous_release_id = runtime.current_release_id.take();
            runtime.previous_deployment_name = runtime.current_deployment_name.take();
            runtime.deployment_uid = None;
            runtime.deployment_resource_version = None;
            runtime.service_uid = None;
            runtime.service_resource_version = None;
            runtime.ingress_uid = None;
            runtime.ingress_resource_version = None;
            runtime.observed_generation = runtime.desired_generation;
            runtime.status = WorkRuntimeStatus::Unpublished;
            runtime.last_error = None;
            operation.status = PublishOperationStatus::Completed;
            operation.checkpoint = PublishCheckpoint::Completed;
            operation.last_error = None;
            outbox.status = PublicationOutboxStatus::Delivered;
            outbox.delivered_at = Some(Utc::now());
            outbox.last_error = None;
            outbox.next_attempt_at = Utc::now();
            Ok(())
        })
        .map(|(operation, runtime, _)| (operation, runtime))
    }

    pub fn record_reconcile_failure(
        &self,
        outbox_id: &str,
        error: impl Into<String>,
    ) -> Result<(), PublicationStoreError> {
        let error = error.into();
        self.update_outbox(outbox_id, |operation, runtime, outbox| {
            operation.status = PublishOperationStatus::ReconcileRequired;
            operation.last_error = Some(error.clone());
            runtime.status = WorkRuntimeStatus::ReconcileRequired;
            runtime.last_error = Some(error.clone());
            outbox.status = PublicationOutboxStatus::Pending;
            outbox.delivered_at = None;
            outbox.last_error = Some(error.clone());
            outbox.next_attempt_at = Utc::now() + chrono::Duration::seconds(2);
            Ok(())
        })?;
        Ok(())
    }

    pub fn authorize_deployment_recreation(
        &self,
        project_id: &str,
        expected_uid: &str,
    ) -> Result<(), PublicationStoreError> {
        let outbox_id = {
            let snapshot = self.state.lock().unwrap();
            let runtime = snapshot
                .runtimes
                .get(project_id)
                .ok_or_else(|| PublicationStoreError::NotFound(project_id.to_string()))?;
            if runtime.deployment_uid.as_deref() != Some(expected_uid) {
                return Err(PublicationStoreError::Conflict(
                    "deployment recreation authorization UID does not match persisted identity"
                        .to_string(),
                ));
            }
            snapshot
                .outbox
                .values()
                .find(|event| {
                    event.project_id == project_id
                        && event.desired_generation == runtime.desired_generation
                })
                .map(|event| event.id.clone())
                .ok_or_else(|| {
                    PublicationStoreError::Storage(
                        "runtime state is missing its current operation outbox".to_string(),
                    )
                })?
        };
        self.update_outbox(&outbox_id, |operation, runtime, outbox| {
            if runtime.deployment_uid.as_deref() != Some(expected_uid) {
                return Err(PublicationStoreError::Conflict(
                    "deployment identity changed before recreation authorization committed"
                        .to_string(),
                ));
            }
            runtime.deployment_uid = None;
            runtime.deployment_resource_version = None;
            runtime.status = WorkRuntimeStatus::ReconcileRequired;
            runtime.last_error = Some("deployment recreation authorized by UID CAS".to_string());
            operation.status = PublishOperationStatus::ReconcileRequired;
            operation.last_error = runtime.last_error.clone();
            outbox.status = PublicationOutboxStatus::Pending;
            outbox.delivered_at = None;
            outbox.last_error = runtime.last_error.clone();
            outbox.next_attempt_at = Utc::now();
            Ok(())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication::{KubernetesResourceIdentity, PublicationIntent, PublishOperationKind};

    fn store_with_intent() -> (PublicationStore, String) {
        let root = std::env::temp_dir().join(format!(
            "publication-observed-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = PublicationStore::open(root).unwrap();
        store
            .commit_intent(&PublicationIntent {
                project_id: "project-observed".into(),
                kind: PublishOperationKind::Publish,
                release_id: Some("release-observed".into()),
                expected_current_release_id: None,
                expected_generation: Some(0),
                runtime_profile_id: "static-web-v1".into(),
                idempotency_key: "observed-key".into(),
            })
            .unwrap();
        let outbox_id = store.pending_outbox()[0].id.clone();
        (store, outbox_id)
    }

    fn observed() -> ObservedWorkRuntime {
        ObservedWorkRuntime {
            deployment: KubernetesResourceIdentity {
                name: "work-a-release-a".into(),
                uid: "deployment-uid".into(),
                resource_version: "11".into(),
            },
            service: KubernetesResourceIdentity {
                name: "work-a".into(),
                uid: "service-uid".into(),
                resource_version: "12".into(),
            },
            ingress: None,
            ready: true,
            release_identity_verified: true,
            external_release_identity_verified: false,
        }
    }

    fn externally_observed() -> ObservedWorkRuntime {
        let mut value = observed();
        value.ingress = Some(KubernetesResourceIdentity {
            name: "work-a".into(),
            uid: "ingress-uid".into(),
            resource_version: "13".into(),
        });
        value.external_release_identity_verified = true;
        value
    }

    #[test]
    fn workload_evidence_advances_observed_generation_and_persists_identity() {
        let (store, outbox_id) = store_with_intent();
        let (operation, runtime) = store
            .record_workload_ready(&outbox_id, &observed())
            .unwrap();
        assert_eq!(operation.status, PublishOperationStatus::WorkloadReady);
        assert_eq!(runtime.observed_generation, runtime.desired_generation);
        assert_eq!(runtime.deployment_uid.as_deref(), Some("deployment-uid"));
        assert_eq!(runtime.service_resource_version.as_deref(), Some("12"));
        assert!(store.pending_outbox().is_empty());
    }

    #[test]
    fn drift_failure_requeues_operation_fail_closed() {
        let (store, outbox_id) = store_with_intent();
        store
            .record_workload_ready(&outbox_id, &observed())
            .unwrap();
        store
            .record_reconcile_failure(&outbox_id, "Kubernetes resource UID drift")
            .unwrap();
        let runtime = store.runtime("project-observed").unwrap();
        assert_eq!(runtime.status, WorkRuntimeStatus::ReconcileRequired);
        assert!(runtime.last_error.unwrap().contains("UID drift"));
        assert!(store
            .authorize_deployment_recreation("project-observed", "wrong-uid")
            .is_err());
        store
            .authorize_deployment_recreation("project-observed", "deployment-uid")
            .unwrap();
        assert!(store
            .runtime("project-observed")
            .unwrap()
            .deployment_uid
            .is_none());
    }

    #[test]
    fn external_probe_completes_publish_and_unpublish_preserves_host_and_release_history() {
        let (store, publish_outbox_id) = store_with_intent();
        let (_, published) = store
            .record_workload_ready(&publish_outbox_id, &externally_observed())
            .unwrap();
        assert_eq!(published.status, WorkRuntimeStatus::Published);
        assert_eq!(published.ingress_uid.as_deref(), Some("ingress-uid"));
        assert!(store.protected_release_ids().contains("release-observed"));
        let host_slug = published.host_slug.clone();
        let (operation, _) = store
            .commit_intent(&PublicationIntent {
                project_id: "project-observed".into(),
                kind: PublishOperationKind::Unpublish,
                release_id: None,
                expected_current_release_id: Some("release-observed".into()),
                expected_generation: Some(1),
                runtime_profile_id: "static-web-v1".into(),
                idempotency_key: "unpublish-observed-key".into(),
            })
            .unwrap();
        let unpublish_outbox = store
            .pending_outbox()
            .into_iter()
            .find(|event| event.operation_id == operation.id)
            .unwrap();
        let (operation, unpublished) = store.record_unpublished(&unpublish_outbox.id).unwrap();
        assert_eq!(operation.status, PublishOperationStatus::Completed);
        assert_eq!(unpublished.status, WorkRuntimeStatus::Unpublished);
        assert_eq!(unpublished.host_slug, host_slug);
        assert_eq!(
            unpublished.last_successful_release_id.as_deref(),
            Some("release-observed")
        );
        assert!(unpublished.current_release_id.is_none());
        assert!(unpublished.ingress_uid.is_none());
        assert!(store.protected_release_ids().contains("release-observed"));
    }

    #[test]
    fn traffic_switch_checkpoint_keeps_blue_current_until_external_probe() {
        let (store, publish_outbox_id) = store_with_intent();
        store
            .record_workload_ready(&publish_outbox_id, &externally_observed())
            .unwrap();
        let (update, _) = store
            .commit_intent(&PublicationIntent {
                project_id: "project-observed".into(),
                kind: PublishOperationKind::Update,
                release_id: Some("release-green".into()),
                expected_current_release_id: Some("release-observed".into()),
                expected_generation: Some(1),
                runtime_profile_id: "static-web-v1".into(),
                idempotency_key: "update-green".into(),
            })
            .unwrap();
        let outbox = store
            .pending_outbox()
            .into_iter()
            .find(|event| event.operation_id == update.id)
            .unwrap();
        let (operation, switched) = store
            .record_traffic_switched(&outbox.id, &observed())
            .unwrap();
        assert_eq!(operation.status, PublishOperationStatus::TrafficSwitched);
        assert_eq!(operation.checkpoint, PublishCheckpoint::TrafficSwitched);
        assert_eq!(
            switched.current_release_id.as_deref(),
            Some("release-observed")
        );
        assert_eq!(
            switched.desired_release_id.as_deref(),
            Some("release-green")
        );
        assert_eq!(store.pending_outbox().len(), 1);

        let (_, completed) = store
            .record_workload_ready(&outbox.id, &externally_observed())
            .unwrap();
        assert_eq!(
            completed.current_release_id.as_deref(),
            Some("release-green")
        );
        assert_eq!(
            completed.previous_release_id.as_deref(),
            Some("release-observed")
        );
    }
}
