use super::{ReleasePackager, ReleaseStore, RuntimeProfile, TrustedReleasePackagingBackend};
use crate::{
    artifact_publisher::FileArtifactPublisher,
    runtime::{RuntimeSupervisor, SupervisorError},
};
use anyhow::Result;
use futures::{stream, StreamExt};
use std::{path::PathBuf, sync::Arc, time::Duration};

pub struct ReleasePackagingController<B: ?Sized> {
    store: Arc<ReleaseStore>,
    packager: ReleasePackager<B>,
    runtime_storage_dir: PathBuf,
    interval: Duration,
}

impl<B> ReleasePackagingController<B>
where
    B: TrustedReleasePackagingBackend + ?Sized + 'static,
{
    pub fn new(
        store: Arc<ReleaseStore>,
        backend: Arc<B>,
        runtime_storage_dir: PathBuf,
        interval: Duration,
    ) -> Self {
        Self {
            packager: ReleasePackager::new(store.clone(), backend),
            store,
            runtime_storage_dir,
            interval,
        }
    }

    pub fn spawn(self, supervisor: &RuntimeSupervisor) -> Result<(), SupervisorError> {
        supervisor.spawn_with_shutdown(
            "controller/release-packaging",
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
        let packagings = self.packager.recoverable_packagings();
        stream::iter(packagings)
            .map(|packaging| async move {
                let Some(release) = self.store.release(&packaging.release_id) else {
                    eprintln!("release packaging {} has no linked release", packaging.id);
                    return false;
                };
                let profile = match RuntimeProfile::static_web_v1(
                    packaging.base_image_digest.clone(),
                    packaging.packager_version.clone(),
                    packaging.scan_policy_version.clone(),
                ) {
                    Ok(profile) => profile,
                    Err(error) => {
                        eprintln!(
                            "release packaging {} profile is invalid: {error}",
                            packaging.id
                        );
                        return false;
                    }
                };
                let artifact_root = FileArtifactPublisher::version_root(
                    &self.runtime_storage_dir,
                    &release.project_id,
                    &release.version_id,
                );
                match self
                    .packager
                    .reconcile(&packaging.id, artifact_root, &profile)
                    .await
                {
                    Ok(_) => true,
                    Err(error) => {
                        eprintln!(
                            "release packaging {} for release {} reconciliation failed: {error}",
                            packaging.id, release.id
                        );
                        false
                    }
                }
            })
            .buffer_unordered(4)
            .filter(|reconciled| futures::future::ready(*reconciled))
            .count()
            .await
    }
}
