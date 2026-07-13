use super::{
    PackagingScanEvidence, ReleasePackagingRecord, ReleasePackagingStatus, ReleaseStore,
    RuntimeManifest, RuntimeProfile, WorkRelease,
};
use crate::artifact_manifest::ArtifactResolver;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseImageBuildRequest {
    pub release_id: String,
    pub project_id: String,
    pub version_id: String,
    pub artifact_root: PathBuf,
    pub artifact_manifest_hash: String,
    pub runtime_manifest: RuntimeManifest,
    pub runtime_manifest_hash: String,
    pub base_image_digest: String,
    pub image_repository: String,
    pub packager_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltReleaseImage {
    pub digest: String,
    pub layout_uri: String,
    pub artifact_manifest_hash: String,
    pub runtime_manifest_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackagingEvidence {
    pub sbom_digest: String,
    pub provenance_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseSignatureEvidence {
    pub identity: String,
    pub signature_digest: String,
}

#[async_trait]
pub trait TrustedReleasePackagingBackend: Send + Sync {
    async fn build(&self, request: &ReleaseImageBuildRequest) -> Result<BuiltReleaseImage>;

    async fn registry_digest(
        &self,
        image_repository: &str,
        release_id: &str,
    ) -> Result<Option<String>>;

    async fn push(
        &self,
        request: &ReleaseImageBuildRequest,
        image: &BuiltReleaseImage,
    ) -> Result<String>;

    async fn generate_evidence(
        &self,
        request: &ReleaseImageBuildRequest,
        image_digest: &str,
    ) -> Result<PackagingEvidence>;

    async fn scan(
        &self,
        image_digest: &str,
        evidence: &PackagingEvidence,
        policy_version: &str,
    ) -> Result<PackagingScanEvidence>;

    async fn sign(
        &self,
        image_digest: &str,
        provenance_digest: &str,
    ) -> Result<ReleaseSignatureEvidence>;
}

pub struct ReleasePackager<B: ?Sized> {
    store: Arc<ReleaseStore>,
    backend: Arc<B>,
}

impl<B> ReleasePackager<B>
where
    B: TrustedReleasePackagingBackend + ?Sized,
{
    pub fn new(store: Arc<ReleaseStore>, backend: Arc<B>) -> Self {
        Self { store, backend }
    }

    pub async fn reconcile(
        &self,
        packaging_id: &str,
        artifact_root: PathBuf,
        profile: &RuntimeProfile,
    ) -> Result<WorkRelease> {
        profile.validate()?;
        for _ in 0..8 {
            let packaging = self
                .store
                .packaging(packaging_id)
                .ok_or_else(|| anyhow!("release packaging record not found: {packaging_id}"))?;
            let release = self
                .store
                .release(&packaging.release_id)
                .ok_or_else(|| anyhow!("work release not found: {}", packaging.release_id))?;
            if release.runtime_profile_id != profile.id
                || packaging.base_image_digest != profile.base_image_digest
                || packaging.packager_version != profile.packager_version
                || packaging.scan_policy_version != profile.scan_policy_version
                || packaging.runtime_manifest_hash != profile.manifest.sha256()?
            {
                return Err(anyhow!("release packaging profile identity mismatch"));
            }
            let request = ReleaseImageBuildRequest {
                release_id: release.id.clone(),
                project_id: release.project_id.clone(),
                version_id: release.version_id.clone(),
                artifact_root: artifact_root.clone(),
                artifact_manifest_hash: release.artifact_manifest_hash.clone(),
                runtime_manifest: profile.manifest.clone(),
                runtime_manifest_hash: release.runtime_manifest_hash.clone(),
                base_image_digest: packaging.base_image_digest.clone(),
                image_repository: packaging.registry_repository.clone(),
                packager_version: packaging.packager_version.clone(),
            };
            match packaging.status {
                ReleasePackagingStatus::Prepared
                | ReleasePackagingStatus::Failed
                | ReleasePackagingStatus::ReconcileRequired => {
                    self.store.begin_build(packaging_id)?;
                }
                ReleasePackagingStatus::Building => {
                    if let Err(error) = self.build_and_push(&request, &packaging).await {
                        self.store
                            .mark_reconcile_required(packaging_id, error.to_string())?;
                        return Err(error);
                    }
                }
                ReleasePackagingStatus::Pushed => {
                    self.store.begin_scan(packaging_id)?;
                }
                ReleasePackagingStatus::Scanning => {
                    if let Err(error) = self.scan(&request, &packaging).await {
                        if self
                            .store
                            .packaging(packaging_id)
                            .is_some_and(|current| current.status != ReleasePackagingStatus::Failed)
                        {
                            self.store
                                .mark_reconcile_required(packaging_id, error.to_string())?;
                        }
                        return Err(error);
                    }
                }
                ReleasePackagingStatus::Signing => {
                    if let Err(error) = self.sign(&packaging).await {
                        self.store
                            .mark_reconcile_required(packaging_id, error.to_string())?;
                        return Err(error);
                    }
                }
                ReleasePackagingStatus::Validated => return Ok(release),
            }
        }
        Err(anyhow!("release packaging reconciliation did not converge"))
    }

    async fn build_and_push(
        &self,
        request: &ReleaseImageBuildRequest,
        packaging: &ReleasePackagingRecord,
    ) -> Result<()> {
        let resolver = ArtifactResolver::load_for_version(
            &request.artifact_root,
            &request.artifact_manifest_hash,
            &request.project_id,
            &request.version_id,
        )?
        .ok_or_else(|| anyhow!("release artifact manifest is missing"))?;
        resolver.verify_all()?;
        if resolver.manifest().sha256()? != request.artifact_manifest_hash {
            return Err(anyhow!(
                "release artifact manifest changed before packaging"
            ));
        }
        if request.runtime_manifest.sha256()? != request.runtime_manifest_hash {
            return Err(anyhow!("release runtime manifest changed before packaging"));
        }

        let built = self.backend.build(request).await?;
        if built.artifact_manifest_hash != request.artifact_manifest_hash
            || built.runtime_manifest_hash != request.runtime_manifest_hash
        {
            return Err(anyhow!("release builder output provenance is invalid"));
        }
        self.store.record_built(&packaging.id, &built.digest)?;
        let existing = self
            .backend
            .registry_digest(&request.image_repository, &request.release_id)
            .await?;
        let pushed = match existing {
            Some(digest) if digest == built.digest => digest,
            Some(_) => return Err(anyhow!("release registry tag points to a different digest")),
            None => self.backend.push(request, &built).await?,
        };
        if pushed != built.digest {
            return Err(anyhow!(
                "release registry returned a different image digest"
            ));
        }
        self.store.record_pushed(&packaging.id, &pushed)?;
        Ok(())
    }

    async fn scan(
        &self,
        request: &ReleaseImageBuildRequest,
        packaging: &ReleasePackagingRecord,
    ) -> Result<()> {
        let image_digest = packaging
            .pushed_image_digest
            .as_deref()
            .ok_or_else(|| anyhow!("release scan requires a pushed image digest"))?;
        let evidence = self
            .backend
            .generate_evidence(request, image_digest)
            .await?;
        let scan = self
            .backend
            .scan(image_digest, &evidence, &packaging.scan_policy_version)
            .await?;
        let passed = scan.passed;
        self.store.record_scan(
            &packaging.id,
            &evidence.sbom_digest,
            &evidence.provenance_digest,
            scan,
        )?;
        if !passed {
            return Err(anyhow!("release image failed scan policy"));
        }
        Ok(())
    }

    async fn sign(&self, packaging: &ReleasePackagingRecord) -> Result<()> {
        let image_digest = packaging
            .pushed_image_digest
            .as_deref()
            .ok_or_else(|| anyhow!("release signing requires a pushed image digest"))?;
        let provenance_digest = packaging
            .provenance_digest
            .as_deref()
            .ok_or_else(|| anyhow!("release signing requires provenance"))?;
        let signature = self.backend.sign(image_digest, provenance_digest).await?;
        self.store.record_signature(
            &packaging.id,
            &signature.identity,
            &signature.signature_digest,
        )?;
        Ok(())
    }

    pub fn recoverable_packagings(&self) -> Vec<ReleasePackagingRecord> {
        self.store.recoverable_packagings()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifact_manifest::{
            manifest_file, ArtifactDeliverySpec, ArtifactManifest, ARTIFACT_MANIFEST_FILE,
        },
        release::{
            ReleasePackagingInput, ReleasePackagingStatus, RuntimeProfile, WorkReleaseStatus,
        },
        types::sha256_hex,
    };
    use std::{
        fs,
        path::Path,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    struct FakeBackend {
        digest: String,
        registry: Mutex<Option<String>>,
        builds: AtomicUsize,
        pushes: AtomicUsize,
        scan_passed: bool,
        sign_failures: AtomicUsize,
    }

    #[async_trait]
    impl TrustedReleasePackagingBackend for FakeBackend {
        async fn build(&self, request: &ReleaseImageBuildRequest) -> Result<BuiltReleaseImage> {
            self.builds.fetch_add(1, Ordering::SeqCst);
            Ok(BuiltReleaseImage {
                digest: self.digest.clone(),
                layout_uri: "file:///trusted/oci-layout".to_string(),
                artifact_manifest_hash: request.artifact_manifest_hash.clone(),
                runtime_manifest_hash: request.runtime_manifest_hash.clone(),
            })
        }

        async fn registry_digest(&self, _: &str, _: &str) -> Result<Option<String>> {
            Ok(self.registry.lock().unwrap().clone())
        }

        async fn push(
            &self,
            _: &ReleaseImageBuildRequest,
            image: &BuiltReleaseImage,
        ) -> Result<String> {
            self.pushes.fetch_add(1, Ordering::SeqCst);
            *self.registry.lock().unwrap() = Some(image.digest.clone());
            Ok(image.digest.clone())
        }

        async fn generate_evidence(
            &self,
            _: &ReleaseImageBuildRequest,
            _: &str,
        ) -> Result<PackagingEvidence> {
            Ok(PackagingEvidence {
                sbom_digest: digest('1'),
                provenance_digest: digest('2'),
            })
        }

        async fn scan(
            &self,
            _: &str,
            _: &PackagingEvidence,
            policy_version: &str,
        ) -> Result<PackagingScanEvidence> {
            Ok(PackagingScanEvidence {
                policy_version: policy_version.to_string(),
                passed: self.scan_passed,
                critical_vulnerabilities: 0,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: digest('3'),
            })
        }

        async fn sign(&self, _: &str, _: &str) -> Result<ReleaseSignatureEvidence> {
            if self
                .sign_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(anyhow!("injected signing failure"));
            }
            Ok(ReleaseSignatureEvidence {
                identity: "cosign://trusted-builder".to_string(),
                signature_digest: digest('4'),
            })
        }
    }

    fn digest(character: char) -> String {
        format!("sha256:{}", character.to_string().repeat(64))
    }

    fn artifact(root: &Path) -> String {
        fs::create_dir_all(root).unwrap();
        let bytes = b"<h1>release</h1>";
        fs::write(root.join("index.html"), bytes).unwrap();
        let manifest = ArtifactManifest::build(
            "project-1",
            "version-1",
            &"a".repeat(64),
            "astro-website",
            "astro-website@runtime-p3",
            ArtifactDeliverySpec::HOST_ROOT,
            vec![manifest_file(
                Path::new("index.html"),
                bytes.len() as u64,
                sha256_hex(bytes),
            )
            .unwrap()],
        )
        .unwrap();
        fs::write(
            root.join(ARTIFACT_MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        manifest.sha256().unwrap()
    }

    #[tokio::test]
    async fn reconcile_is_content_addressed_and_skips_duplicate_registry_push() {
        let root = std::env::temp_dir().join(format!(
            "release-packager-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let artifact_root = root.join("artifact");
        let artifact_hash = artifact(&artifact_root);
        let profile = RuntimeProfile::static_web_v1(digest('b'), "packager@1", "scan@1").unwrap();
        let input = ReleasePackagingInput {
            project_id: "project-1".to_string(),
            version_id: "version-1".to_string(),
            run_id: "run-1".to_string(),
            template_id: "astro-website".to_string(),
            template_version: "astro-website@runtime-p3".to_string(),
            artifact_manifest_hash: artifact_hash,
            runtime_manifest_hash: profile.manifest.sha256().unwrap(),
            source_snapshot_uri: "runtime://source-snapshots/project-1/build-1".to_string(),
            runtime_profile_id: profile.id.clone(),
            base_image_digest: profile.base_image_digest.clone(),
            packager_version: profile.packager_version.clone(),
            registry_repository: "registry.example/works".to_string(),
            scan_policy_version: profile.scan_policy_version.clone(),
        };
        let store = Arc::new(ReleaseStore::open(root.join("store")).unwrap());
        let (_, packaging) = store.prepare(&input).unwrap();
        let backend = Arc::new(FakeBackend {
            digest: digest('d'),
            registry: Mutex::new(None),
            builds: AtomicUsize::new(0),
            pushes: AtomicUsize::new(0),
            scan_passed: true,
            sign_failures: AtomicUsize::new(0),
        });
        let packager = ReleasePackager::new(store.clone(), backend.clone());
        let release = packager
            .reconcile(&packaging.id, artifact_root.clone(), &profile)
            .await
            .unwrap();
        assert_eq!(release.status, WorkReleaseStatus::Validated);
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 1);

        let release = packager
            .reconcile(&packaging.id, artifact_root, &profile)
            .await
            .unwrap();
        assert_eq!(release.status, WorkReleaseStatus::Validated);
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.packaging(&packaging.id).unwrap().status,
            ReleasePackagingStatus::Validated
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn signing_crash_resumes_at_signing_without_another_push() {
        let root = std::env::temp_dir().join(format!(
            "release-packager-signing-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let artifact_root = root.join("artifact");
        let artifact_hash = artifact(&artifact_root);
        let profile = RuntimeProfile::static_web_v1(digest('b'), "packager@1", "scan@1").unwrap();
        let input = release_input(&profile, artifact_hash);
        let store = Arc::new(ReleaseStore::open(root.join("store")).unwrap());
        let (_, packaging) = store.prepare(&input).unwrap();
        let backend = Arc::new(FakeBackend {
            digest: digest('d'),
            registry: Mutex::new(None),
            builds: AtomicUsize::new(0),
            pushes: AtomicUsize::new(0),
            scan_passed: true,
            sign_failures: AtomicUsize::new(1),
        });
        let packager = ReleasePackager::new(store.clone(), backend.clone());
        assert!(packager
            .reconcile(&packaging.id, artifact_root.clone(), &profile)
            .await
            .is_err());
        assert_eq!(
            store.packaging(&packaging.id).unwrap().status,
            ReleasePackagingStatus::ReconcileRequired
        );
        let validated = packager
            .reconcile(&packaging.id, artifact_root, &profile)
            .await
            .unwrap();
        assert_eq!(validated.status, WorkReleaseStatus::Validated);
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 1);
        assert_eq!(backend.builds.load(Ordering::SeqCst), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn scan_policy_failure_never_validates_the_release() {
        let root = std::env::temp_dir().join(format!(
            "release-packager-scan-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let artifact_root = root.join("artifact");
        let artifact_hash = artifact(&artifact_root);
        let profile = RuntimeProfile::static_web_v1(digest('b'), "packager@1", "scan@1").unwrap();
        let input = release_input(&profile, artifact_hash);
        let store = Arc::new(ReleaseStore::open(root.join("store")).unwrap());
        let (release, packaging) = store.prepare(&input).unwrap();
        let backend = Arc::new(FakeBackend {
            digest: digest('d'),
            registry: Mutex::new(None),
            builds: AtomicUsize::new(0),
            pushes: AtomicUsize::new(0),
            scan_passed: false,
            sign_failures: AtomicUsize::new(0),
        });
        let packager = ReleasePackager::new(store.clone(), backend);
        assert!(packager
            .reconcile(&packaging.id, artifact_root, &profile)
            .await
            .is_err());
        assert_eq!(
            store.packaging(&packaging.id).unwrap().status,
            ReleasePackagingStatus::Failed
        );
        assert_eq!(
            store.release(&release.id).unwrap().status,
            WorkReleaseStatus::Failed
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn registry_digest_recovers_push_before_store_commit() {
        let root = std::env::temp_dir().join(format!(
            "release-packager-registry-recovery-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let artifact_root = root.join("artifact");
        let artifact_hash = artifact(&artifact_root);
        let profile = RuntimeProfile::static_web_v1(digest('b'), "packager@1", "scan@1").unwrap();
        let store = Arc::new(ReleaseStore::open(root.join("store")).unwrap());
        let (_, packaging) = store
            .prepare(&release_input(&profile, artifact_hash))
            .unwrap();
        let backend = Arc::new(FakeBackend {
            digest: digest('d'),
            registry: Mutex::new(Some(digest('d'))),
            builds: AtomicUsize::new(0),
            pushes: AtomicUsize::new(0),
            scan_passed: true,
            sign_failures: AtomicUsize::new(0),
        });
        let packager = ReleasePackager::new(store, backend.clone());
        let release = packager
            .reconcile(&packaging.id, artifact_root, &profile)
            .await
            .unwrap();
        assert_eq!(release.status, WorkReleaseStatus::Validated);
        assert_eq!(backend.pushes.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn registry_digest_conflict_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "release-packager-registry-conflict-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let artifact_root = root.join("artifact");
        let artifact_hash = artifact(&artifact_root);
        let profile = RuntimeProfile::static_web_v1(digest('b'), "packager@1", "scan@1").unwrap();
        let store = Arc::new(ReleaseStore::open(root.join("store")).unwrap());
        let (release, packaging) = store
            .prepare(&release_input(&profile, artifact_hash))
            .unwrap();
        let backend = Arc::new(FakeBackend {
            digest: digest('d'),
            registry: Mutex::new(Some(digest('e'))),
            builds: AtomicUsize::new(0),
            pushes: AtomicUsize::new(0),
            scan_passed: true,
            sign_failures: AtomicUsize::new(0),
        });
        let packager = ReleasePackager::new(store.clone(), backend);
        assert!(packager
            .reconcile(&packaging.id, artifact_root, &profile)
            .await
            .unwrap_err()
            .to_string()
            .contains("different digest"));
        assert!(store
            .release(&release.id)
            .unwrap()
            .runtime_image_digest
            .is_none());
        fs::remove_dir_all(root).unwrap();
    }

    fn release_input(profile: &RuntimeProfile, artifact_hash: String) -> ReleasePackagingInput {
        ReleasePackagingInput {
            project_id: "project-1".to_string(),
            version_id: "version-1".to_string(),
            run_id: "run-1".to_string(),
            template_id: "astro-website".to_string(),
            template_version: "astro-website@runtime-p3".to_string(),
            artifact_manifest_hash: artifact_hash,
            runtime_manifest_hash: profile.manifest.sha256().unwrap(),
            source_snapshot_uri: "runtime://source-snapshots/project-1/build-1".to_string(),
            runtime_profile_id: profile.id.clone(),
            base_image_digest: profile.base_image_digest.clone(),
            packager_version: profile.packager_version.clone(),
            registry_repository: "registry.example/works".to_string(),
            scan_policy_version: profile.scan_policy_version.clone(),
        }
    }
}
