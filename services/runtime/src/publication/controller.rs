use super::{PublicationOutboxEvent, PublicationStore, WorkRuntimeState};
use crate::runtime::{RuntimeSupervisor, SupervisorError};
use anyhow::Result;
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationReconcileDisposition {
    Accepted,
    Deferred,
}

#[async_trait]
pub trait WorkRuntimeBackend: Send + Sync {
    async fn reconcile(
        &self,
        runtime: &WorkRuntimeState,
        event: &PublicationOutboxEvent,
    ) -> Result<PublicationReconcileDisposition>;
}

pub struct ControlPlaneOnlyBackend;

#[async_trait]
impl WorkRuntimeBackend for ControlPlaneOnlyBackend {
    async fn reconcile(
        &self,
        _: &WorkRuntimeState,
        _: &PublicationOutboxEvent,
    ) -> Result<PublicationReconcileDisposition> {
        Ok(PublicationReconcileDisposition::Deferred)
    }
}

pub struct WorkRuntimeController<B> {
    store: Arc<PublicationStore>,
    backend: Arc<B>,
    interval: Duration,
}

impl<B> WorkRuntimeController<B>
where
    B: WorkRuntimeBackend + 'static,
{
    pub fn new(store: Arc<PublicationStore>, backend: Arc<B>, interval: Duration) -> Self {
        Self {
            store,
            backend,
            interval,
        }
    }

    pub fn spawn(self, supervisor: &RuntimeSupervisor) -> Result<(), SupervisorError> {
        supervisor.spawn_with_shutdown(
            "controller/work-runtime",
            true,
            move |mut shutdown| async move {
                loop {
                    self.reconcile_once().await?;
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

    pub async fn reconcile_once(&self) -> Result<usize> {
        let mut delivered = 0;
        for event in self.store.pending_outbox() {
            let Some(runtime) = self.store.runtime(&event.project_id) else {
                self.store.record_delivery_attempt(
                    &event.id,
                    Some("publication runtime state is missing".to_string()),
                )?;
                continue;
            };
            match self.backend.reconcile(&runtime, &event).await {
                Ok(PublicationReconcileDisposition::Accepted) => {
                    self.store.record_delivery_attempt(&event.id, None)?;
                    self.store.record_delivered(&event.id)?;
                    delivered += 1;
                }
                Ok(PublicationReconcileDisposition::Deferred) => {
                    self.store.record_delivery_attempt(
                        &event.id,
                        Some("work runtime backend is not enabled in G5".to_string()),
                    )?;
                }
                Err(error) => {
                    self.store
                        .record_delivery_attempt(&event.id, Some(error.to_string()))?;
                }
            }
        }
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication::{
        PublicationIntent, PublicationOutboxStatus, PublishOperationKind, PublishOperationStatus,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct AcceptingBackend(AtomicUsize);

    #[async_trait]
    impl WorkRuntimeBackend for AcceptingBackend {
        async fn reconcile(
            &self,
            _: &WorkRuntimeState,
            _: &PublicationOutboxEvent,
        ) -> Result<PublicationReconcileDisposition> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(PublicationReconcileDisposition::Accepted)
        }
    }

    #[tokio::test]
    async fn restart_replays_pending_outbox_and_supervisor_owns_controller() {
        let root = std::env::temp_dir().join(format!(
            "publication-controller-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = PublicationStore::open(&root).unwrap();
        let (operation, _) = store
            .commit_intent(&PublicationIntent {
                project_id: "controller-project".into(),
                kind: PublishOperationKind::Publish,
                release_id: Some("release-controller".into()),
                expected_current_release_id: None,
                expected_generation: Some(0),
                runtime_profile_id: "static-web-v1".into(),
                idempotency_key: "controller-key".into(),
            })
            .unwrap();
        drop(store);
        let store = Arc::new(PublicationStore::open(&root).unwrap());
        assert_eq!(
            store.pending_outbox()[0].status,
            PublicationOutboxStatus::Pending
        );
        let backend = Arc::new(AcceptingBackend(AtomicUsize::new(0)));
        let controller = WorkRuntimeController::new(
            Arc::clone(&store),
            Arc::clone(&backend),
            Duration::from_millis(10),
        );
        let supervisor = RuntimeSupervisor::new();
        controller.spawn(&supervisor).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !store.pending_outbox().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(supervisor
            .readiness()
            .active_tasks
            .contains(&"controller/work-runtime".to_string()));
        assert_eq!(backend.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.operation(&operation.id).unwrap().status,
            PublishOperationStatus::Reconciling
        );
        supervisor.shutdown(Duration::from_secs(1)).await;
        std::fs::remove_dir_all(root).unwrap();
    }
}
