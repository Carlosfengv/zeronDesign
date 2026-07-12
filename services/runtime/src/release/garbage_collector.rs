use super::{
    ReleaseGarbageCollectionEvidence, ReleasePackagingRecord, ReleaseProtectionSource,
    ReleaseStore, WorkRelease,
};
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
    protection: Arc<dyn ReleaseProtectionSource>,
}

impl<B> ReleaseGarbageCollector<B>
where
    B: TrustedReleaseGarbageCollectionBackend,
{
    pub fn new(
        store: Arc<ReleaseStore>,
        backend: Arc<B>,
        protection: Arc<dyn ReleaseProtectionSource>,
    ) -> Self {
        Self {
            store,
            backend,
            protection,
        }
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
        let packaging = self
            .store
            .packaging(packaging_id)
            .ok_or_else(|| anyhow!("release packaging record not found: {packaging_id}"))?;
        let release = self
            .store
            .release(&packaging.release_id)
            .ok_or_else(|| anyhow!("work release not found: {}", packaging.release_id))?;
        let protection = self.protection.snapshot().await?;
        if protection.protects(&release.id, expected_image_digest) {
            return Err(anyhow!(
                "release or image digest is protected by runtime, operation, packaging, or live workload references"
            ));
        }
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
    use crate::release::{
        PackagingScanEvidence, ReleasePackagingInput, ReleaseProtectionSet, WorkReleaseStatus,
    };
    use std::fs;

    struct FakeGarbageCollector;

    struct EmptyProtection;

    struct ProtectedRelease(String);

    struct UnavailableProtection;

    #[async_trait]
    impl ReleaseProtectionSource for EmptyProtection {
        async fn snapshot(&self) -> Result<ReleaseProtectionSet> {
            Ok(ReleaseProtectionSet::default())
        }
    }

    #[async_trait]
    impl ReleaseProtectionSource for ProtectedRelease {
        async fn snapshot(&self) -> Result<ReleaseProtectionSet> {
            Ok(ReleaseProtectionSet {
                release_ids: [self.0.clone()].into_iter().collect(),
                image_digests: Default::default(),
            })
        }
    }

    #[async_trait]
    impl ReleaseProtectionSource for UnavailableProtection {
        async fn snapshot(&self) -> Result<ReleaseProtectionSet> {
            Err(anyhow!("live workload scan unavailable"))
        }
    }

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
        let release = ReleaseGarbageCollector::new(
            store,
            Arc::new(FakeGarbageCollector),
            Arc::new(EmptyProtection),
        )
        .collect_failed(&packaging.id, &digest('d'))
        .await
        .unwrap();
        assert_eq!(release.status, WorkReleaseStatus::GarbageCollected);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn gc_fails_closed_for_protected_or_unavailable_reference_scans() {
        let root = std::env::temp_dir().join(format!(
            "release-gc-protected-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = Arc::new(ReleaseStore::open(&root).unwrap());
        let input = ReleasePackagingInput {
            project_id: "project-gc-protected".into(),
            version_id: "version-gc-protected".into(),
            run_id: "run-gc-protected".into(),
            template_id: "generic".into(),
            template_version: "1".into(),
            artifact_manifest_hash: "4".repeat(64),
            runtime_manifest_hash: "5".repeat(64),
            source_snapshot_uri: "fixture://gc-protected".into(),
            runtime_profile_id: "static-web-v1".into(),
            base_image_digest: digest('6'),
            packager_version: "packager@protected".into(),
            registry_repository: "registry.example/works".into(),
            scan_policy_version: "scan@protected".into(),
        };
        let (release, packaging) = store.prepare(&input).unwrap();
        let image_digest = digest('7');
        store.begin_build(&packaging.id).unwrap();
        store.record_built(&packaging.id, &image_digest).unwrap();
        store.record_pushed(&packaging.id, &image_digest).unwrap();
        store.begin_scan(&packaging.id).unwrap();
        store
            .record_scan(
                &packaging.id,
                &digest('8'),
                &digest('9'),
                PackagingScanEvidence {
                    policy_version: "scan@protected".into(),
                    passed: false,
                    critical_vulnerabilities: 1,
                    high_vulnerabilities: 0,
                    secret_findings: 0,
                    report_digest: digest('a'),
                },
            )
            .unwrap();

        let protected = ReleaseGarbageCollector::new(
            Arc::clone(&store),
            Arc::new(FakeGarbageCollector),
            Arc::new(ProtectedRelease(release.id.clone())),
        )
        .collect_failed(&packaging.id, &image_digest)
        .await
        .unwrap_err();
        assert!(protected.to_string().contains("protected"));
        assert_eq!(
            store.release(&release.id).unwrap().status,
            WorkReleaseStatus::Failed
        );

        let unavailable = ReleaseGarbageCollector::new(
            Arc::clone(&store),
            Arc::new(FakeGarbageCollector),
            Arc::new(UnavailableProtection),
        )
        .collect_failed(&packaging.id, &image_digest)
        .await
        .unwrap_err();
        assert!(unavailable.to_string().contains("unavailable"));
        assert_eq!(
            store.release(&release.id).unwrap().status,
            WorkReleaseStatus::Failed
        );
        fs::remove_dir_all(root).unwrap();
    }
}
