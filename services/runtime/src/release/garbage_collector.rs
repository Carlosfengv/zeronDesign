use super::{ReleaseGarbageCollectionEvidence, ReleasePackagingRecord, ReleaseStore, WorkRelease};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait TrustedReleaseGarbageCollectionBackend: Send + Sync {
    async fn garbage_collect(
        &self,
        release: &WorkRelease,
        packaging: &ReleasePackagingRecord,
    ) -> Result<ReleaseGarbageCollectionEvidence>;
}

pub struct ReleaseGarbageCollector<B> {
    store: Arc<ReleaseStore>,
    backend: Arc<B>,
}

impl<B> ReleaseGarbageCollector<B>
where
    B: TrustedReleaseGarbageCollectionBackend,
{
    pub fn new(store: Arc<ReleaseStore>, backend: Arc<B>) -> Self {
        Self { store, backend }
    }

    /// Collects only a release already rejected by scan policy.
    ///
    /// Validated releases are deliberately ineligible here. G5/G8 must supply durable desired,
    /// active, and rollback references before broader Registry garbage collection is possible.
    pub async fn collect_failed(
        &self,
        packaging_id: &str,
        expected_image_digest: &str,
    ) -> Result<WorkRelease> {
        let (release, packaging) = self
            .store
            .mark_failed_garbage_collectable(packaging_id, expected_image_digest)?;
        let evidence = self.backend.garbage_collect(&release, &packaging).await?;
        if !evidence.registry_manifest_deleted || !evidence.packaging_evidence_deleted {
            return Err(anyhow!("release garbage collection evidence is incomplete"));
        }
        let (release, _) = self
            .store
            .record_garbage_collected(packaging_id, expected_image_digest)?;
        Ok(release)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::{PackagingScanEvidence, ReleasePackagingInput, WorkReleaseStatus};
    use std::fs;

    struct FakeGarbageCollector;

    #[async_trait]
    impl TrustedReleaseGarbageCollectionBackend for FakeGarbageCollector {
        async fn garbage_collect(
            &self,
            _: &WorkRelease,
            _: &ReleasePackagingRecord,
        ) -> Result<ReleaseGarbageCollectionEvidence> {
            Ok(ReleaseGarbageCollectionEvidence {
                registry_manifest_deleted: true,
                packaging_evidence_deleted: true,
            })
        }
    }

    fn digest(character: char) -> String {
        format!("sha256:{}", character.to_string().repeat(64))
    }

    #[tokio::test]
    async fn collects_failed_release_without_making_validated_releases_eligible() {
        let root = std::env::temp_dir().join(format!(
            "release-gc-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = Arc::new(ReleaseStore::open(&root).unwrap());
        let input = ReleasePackagingInput {
            project_id: "project-gc".into(),
            version_id: "version-gc".into(),
            run_id: "run-gc".into(),
            template_id: "generic".into(),
            template_version: "1".into(),
            artifact_manifest_hash: "a".repeat(64),
            runtime_manifest_hash: "b".repeat(64),
            source_snapshot_uri: "fixture://gc".into(),
            runtime_profile_id: "static-web-v1".into(),
            base_image_digest: digest('c'),
            packager_version: "packager@1".into(),
            registry_repository: "registry.example/works".into(),
            scan_policy_version: "scan@1".into(),
        };
        let (_, packaging) = store.prepare(&input).unwrap();
        store.begin_build(&packaging.id).unwrap();
        store.record_built(&packaging.id, &digest('d')).unwrap();
        store.record_pushed(&packaging.id, &digest('d')).unwrap();
        store.begin_scan(&packaging.id).unwrap();
        store
            .record_scan(
                &packaging.id,
                &digest('1'),
                &digest('2'),
                PackagingScanEvidence {
                    policy_version: "scan@1".into(),
                    passed: false,
                    critical_vulnerabilities: 1,
                    high_vulnerabilities: 0,
                    secret_findings: 0,
                    report_digest: digest('3'),
                },
            )
            .unwrap();
        let release = ReleaseGarbageCollector::new(store, Arc::new(FakeGarbageCollector))
            .collect_failed(&packaging.id, &digest('d'))
            .await
            .unwrap();
        assert_eq!(release.status, WorkReleaseStatus::GarbageCollected);
        fs::remove_dir_all(root).unwrap();
    }
}
