use super::{
    DesiredUnpublishRuntime, DesiredWorkRuntime, PublicationDesiredState,
    PublicationReconcileDisposition, PublicationStore, WorkRuntimeBackend,
};
use crate::{
    release::ReleaseStore,
    runtime::{RuntimeSupervisor, SupervisorError},
};
use anyhow::{Context, Result};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

const DRIFT_AUDIT_INTERVAL: Duration = Duration::from_secs(300);

pub struct WorkRuntimeController<B: ?Sized> {
    publication_store: Arc<PublicationStore>,
    release_store: Arc<ReleaseStore>,
    backend: Arc<B>,
    interval: Duration,
}

impl<B> WorkRuntimeController<B>
where
    B: WorkRuntimeBackend + ?Sized + 'static,
{
    pub fn new(
        publication_store: Arc<PublicationStore>,
        release_store: Arc<ReleaseStore>,
        backend: Arc<B>,
        interval: Duration,
    ) -> Self {
        Self {
            publication_store,
            release_store,
            backend,
            interval,
        }
    }

    pub fn spawn(self, supervisor: &RuntimeSupervisor) -> Result<(), SupervisorError> {
        supervisor.spawn_with_shutdown(
            "controller/work-runtime",
            true,
            move |mut shutdown| async move {
                let mut next_drift_audit = tokio::time::Instant::now();
                loop {
                    let now = tokio::time::Instant::now();
                    let include_observed = now >= next_drift_audit;
                    if include_observed {
                        next_drift_audit = now + DRIFT_AUDIT_INTERVAL;
                    }
                    self.reconcile_once(include_observed).await?;
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

    pub async fn reconcile_once(&self, include_observed: bool) -> Result<usize> {
        let mut events = self
            .publication_store
            .pending_outbox()
            .into_iter()
            .map(|event| (event.id.clone(), event))
            .collect::<BTreeMap<_, _>>();
        if include_observed {
            for event in self.publication_store.published_reconcile_outbox() {
                events.entry(event.id.clone()).or_insert(event);
            }
        }
        let mut reconciled = 0;
        for event in events.into_values() {
            let Some(runtime) = self.publication_store.runtime(&event.project_id) else {
                self.publication_store
                    .record_reconcile_failure(&event.id, "publication runtime state is missing")?;
                continue;
            };
            let result = async {
                match runtime.desired_publication {
                    PublicationDesiredState::Published => {
                        let release_id = runtime
                            .desired_release_id
                            .as_deref()
                            .context("Published runtime is missing desired release")?;
                        let release = self
                            .release_store
                            .release(release_id)
                            .context("desired WorkRelease does not exist")?;
                        let packaging = self
                            .release_store
                            .packaging_for_release(release_id)
                            .context("desired WorkRelease packaging evidence does not exist")?;
                        let operation = self
                            .publication_store
                            .operation(&event.operation_id)
                            .context("publication operation does not exist")?;
                        let desired = DesiredWorkRuntime::from_records(
                            &runtime,
                            &release,
                            &packaging,
                            operation.checkpoint,
                        )?;
                        self.backend.reconcile(&desired).await
                    }
                    PublicationDesiredState::Unpublished => {
                        let desired = DesiredUnpublishRuntime::from_state(&runtime)?;
                        self.backend.unpublish(&desired).await
                    }
                }
            }
            .await;
            match result {
                Ok(PublicationReconcileDisposition::Applied(observed)) => {
                    self.publication_store
                        .record_workload_ready(&event.id, observed.as_ref())?;
                    reconciled += 1;
                }
                Ok(PublicationReconcileDisposition::TrafficSwitched(observed)) => {
                    self.publication_store
                        .record_traffic_switched(&event.id, observed.as_ref())?;
                    reconciled += 1;
                }
                Ok(PublicationReconcileDisposition::Unpublished) => {
                    self.publication_store.record_unpublished(&event.id)?;
                    reconciled += 1;
                }
                Ok(PublicationReconcileDisposition::Deferred) => {
                    if event.status == super::PublicationOutboxStatus::Pending {
                        self.publication_store
                            .record_delivery_attempt(&event.id, None)?;
                    }
                }
                Err(error) => {
                    self.publication_store
                        .record_reconcile_failure(&event.id, error.to_string())?;
                }
            }
        }
        Ok(reconciled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication::ControlPlaneOnlyBackend;

    #[tokio::test]
    async fn supervisor_owns_controller_lifecycle() {
        let root = std::env::temp_dir().join(format!(
            "publication-controller-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let publication_store = Arc::new(PublicationStore::open(root.join("publication")).unwrap());
        let release_store = Arc::new(ReleaseStore::open(root.join("release")).unwrap());
        let controller = WorkRuntimeController::new(
            publication_store,
            release_store,
            Arc::new(ControlPlaneOnlyBackend),
            Duration::from_millis(10),
        );
        let supervisor = RuntimeSupervisor::new();
        controller.spawn(&supervisor).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(supervisor
            .readiness()
            .active_tasks
            .contains(&"controller/work-runtime".to_string()));
        supervisor.shutdown(Duration::from_secs(1)).await;
        std::fs::remove_dir_all(root).unwrap();
    }
}
