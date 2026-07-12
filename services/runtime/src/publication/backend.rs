use super::{PublicationDesiredState, WorkRuntimeState};
use crate::{
    release::{
        ReleasePackagingRecord, ReleasePackagingStatus, WorkRelease, WorkReleaseStatus,
        STATIC_WEB_PROFILE_ID,
    },
    types::sha256_hex,
};
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const WORKS_NAMESPACE: &str = "anydesign-works";
pub const FIELD_MANAGER: &str = "anydesign-work-runtime-controller";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesiredWorkRuntime {
    pub namespace: String,
    pub project_id: String,
    pub work_name: String,
    pub deployment_name: String,
    pub stable_service_name: String,
    pub probe_service_name: String,
    pub network_policy_name: String,
    pub release_id: String,
    pub desired_generation: u64,
    pub runtime_profile_id: String,
    pub image: String,
    pub image_digest: String,
    pub container_port: u16,
    pub health_path: String,
    pub release_path: String,
    pub labels: BTreeMap<String, String>,
    pub trust: ReleaseTrustEvidence,
    pub expected_deployment_uid: Option<String>,
    pub expected_service_uid: Option<String>,
    pub expected_service_resource_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseTrustEvidence {
    pub signature_identity: String,
    pub signature_digest: String,
    pub provenance_digest: String,
    pub scan_policy_version: String,
    pub scan_report_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KubernetesResourceIdentity {
    pub name: String,
    pub uid: String,
    pub resource_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedWorkRuntime {
    pub deployment: KubernetesResourceIdentity,
    pub service: KubernetesResourceIdentity,
    pub ready: bool,
    pub release_identity_verified: bool,
}

impl DesiredWorkRuntime {
    pub fn from_records(
        runtime: &WorkRuntimeState,
        release: &WorkRelease,
        packaging: &ReleasePackagingRecord,
    ) -> Result<Self> {
        if runtime.desired_publication != PublicationDesiredState::Published {
            bail!("work runtime is not desired Published");
        }
        if runtime.desired_release_id.as_deref() != Some(release.id.as_str())
            || release.project_id != runtime.project_id
            || packaging.project_id != runtime.project_id
            || packaging.release_id != release.id
        {
            bail!("publication, release, and packaging ownership do not match");
        }
        if release.status != WorkReleaseStatus::Validated
            || packaging.status != ReleasePackagingStatus::Validated
        {
            bail!("release and packaging must both be Validated");
        }
        if runtime.runtime_profile_id != STATIC_WEB_PROFILE_ID
            || release.runtime_profile_id != STATIC_WEB_PROFILE_ID
        {
            bail!("G6 only supports the static-web-v1 runtime profile");
        }
        let image = release
            .runtime_image_ref
            .clone()
            .ok_or_else(|| anyhow::anyhow!("validated release is missing runtime image ref"))?;
        let image_digest = release
            .runtime_image_digest
            .clone()
            .ok_or_else(|| anyhow::anyhow!("validated release is missing runtime image digest"))?;
        if !is_sha256_digest(&image_digest)
            || !image.ends_with(&format!("@{image_digest}"))
            || packaging.pushed_image_digest.as_deref() != Some(image_digest.as_str())
        {
            bail!("release image must be immutable and match packaging push evidence");
        }
        let scan = packaging
            .scan_evidence
            .as_ref()
            .filter(|evidence| evidence.passed && evidence.critical_vulnerabilities == 0)
            .ok_or_else(|| {
                anyhow::anyhow!("release scan policy evidence is missing or rejected")
            })?;
        let trust = ReleaseTrustEvidence {
            signature_identity: required(&packaging.signature_identity, "signature identity")?,
            signature_digest: required_digest(&packaging.signature_digest, "signature digest")?,
            provenance_digest: required_digest(&packaging.provenance_digest, "provenance digest")?,
            scan_policy_version: packaging.scan_policy_version.clone(),
            scan_report_digest: scan.report_digest.clone(),
        };
        if !is_sha256_digest(&trust.scan_report_digest) {
            bail!("scan report digest is invalid");
        }

        let work_name = runtime.service_name.clone();
        let release_key = &sha256_hex(release.id.as_bytes())[..12];
        let deployment_name = format!("{work_name}-{release_key}");
        let mut labels = BTreeMap::new();
        labels.insert("anydesign.dev/work".into(), work_name.clone());
        labels.insert("anydesign.dev/project-hash".into(), work_name.clone());
        labels.insert("anydesign.dev/release-id".into(), release.id.clone());
        labels.insert(
            "anydesign.dev/desired-generation".into(),
            runtime.desired_generation.to_string(),
        );
        labels.insert(
            "anydesign.dev/owner-record-id".into(),
            format!(
                "project-{}",
                &sha256_hex(runtime.project_id.as_bytes())[..20]
            ),
        );
        labels.insert(
            "anydesign.dev/runtime-profile".into(),
            STATIC_WEB_PROFILE_ID.into(),
        );
        labels.insert("app.kubernetes.io/managed-by".into(), FIELD_MANAGER.into());
        Ok(Self {
            namespace: WORKS_NAMESPACE.into(),
            project_id: runtime.project_id.clone(),
            work_name: work_name.clone(),
            deployment_name: deployment_name.clone(),
            stable_service_name: runtime.service_name.clone(),
            probe_service_name: format!("{}-probe-{release_key}", runtime.service_name),
            network_policy_name: runtime.service_name.clone(),
            release_id: release.id.clone(),
            desired_generation: runtime.desired_generation,
            runtime_profile_id: STATIC_WEB_PROFILE_ID.into(),
            image,
            image_digest,
            container_port: 8080,
            health_path: "/.well-known/anydesign/healthz".into(),
            release_path: "/.well-known/anydesign/release".into(),
            labels,
            trust,
            expected_deployment_uid: (runtime.current_deployment_name.as_deref()
                == Some(deployment_name.as_str()))
            .then(|| runtime.deployment_uid.clone())
            .flatten(),
            expected_service_uid: runtime.service_uid.clone(),
            expected_service_resource_version: runtime.service_resource_version.clone(),
        })
    }
}

fn required(value: &Option<String>, field: &str) -> Result<String> {
    value
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("release {field} is missing"))
}

fn required_digest(value: &Option<String>, field: &str) -> Result<String> {
    let value = required(value, field)?;
    if !is_sha256_digest(&value) {
        bail!("release {field} is not sha256-pinned");
    }
    Ok(value)
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationReconcileDisposition {
    Applied(ObservedWorkRuntime),
    Deferred,
}

#[async_trait]
pub trait WorkRuntimeBackend: Send + Sync {
    async fn reconcile(
        &self,
        desired: &DesiredWorkRuntime,
    ) -> Result<PublicationReconcileDisposition>;
}

pub struct ControlPlaneOnlyBackend;

#[async_trait]
impl WorkRuntimeBackend for ControlPlaneOnlyBackend {
    async fn reconcile(&self, _: &DesiredWorkRuntime) -> Result<PublicationReconcileDisposition> {
        Ok(PublicationReconcileDisposition::Deferred)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::PackagingScanEvidence;
    use chrono::Utc;

    #[test]
    fn desired_builder_is_template_agnostic_and_digest_pinned() {
        let now = Utc::now();
        let runtime = WorkRuntimeState {
            schema_version: "work-runtime-state@1".into(),
            project_id: "project-a".into(),
            desired_publication: PublicationDesiredState::Published,
            desired_release_id: Some("release-a".into()),
            current_release_id: None,
            previous_release_id: None,
            last_successful_release_id: None,
            desired_generation: 3,
            host_slug: "w-random".into(),
            runtime_profile_id: STATIC_WEB_PROFILE_ID.into(),
            current_deployment_name: None,
            previous_deployment_name: None,
            service_name: "work-0123456789ab".into(),
            ingress_name: "work-0123456789ab".into(),
            deployment_uid: None,
            deployment_resource_version: None,
            service_uid: None,
            service_resource_version: None,
            ingress_uid: None,
            ingress_resource_version: None,
            observed_generation: 0,
            status: super::super::WorkRuntimeStatus::Publishing,
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        let digest = format!("sha256:{}", "a".repeat(64));
        let release = WorkRelease {
            id: "release-a".into(),
            project_id: "project-a".into(),
            version_id: "v1".into(),
            run_id: "run1".into(),
            template_id: "future-template".into(),
            template_version: "1".into(),
            artifact_manifest_hash: "b".repeat(64),
            runtime_manifest_hash: "c".repeat(64),
            source_snapshot_uri: "runtime://snapshot".into(),
            runtime_profile_id: STATIC_WEB_PROFILE_ID.into(),
            runtime_image_ref: Some(format!("registry.test/works/release-a@{digest}")),
            runtime_image_digest: Some(digest.clone()),
            status: WorkReleaseStatus::Validated,
            created_at: now,
            updated_at: now,
        };
        let packaging = ReleasePackagingRecord {
            id: "packaging-a".into(),
            idempotency_key: "key".into(),
            project_id: "project-a".into(),
            release_id: "release-a".into(),
            artifact_manifest_hash: "b".repeat(64),
            runtime_manifest_hash: "c".repeat(64),
            base_image_digest: digest.clone(),
            packager_version: "1".into(),
            registry_repository: "registry.test/works".into(),
            built_image_digest: Some(digest.clone()),
            pushed_image_digest: Some(digest.clone()),
            sbom_digest: Some(digest.clone()),
            provenance_digest: Some(digest.clone()),
            signature_identity: Some("release-signer".into()),
            signature_digest: Some(digest),
            scan_policy_version: "scan@1".into(),
            scan_evidence: Some(PackagingScanEvidence {
                policy_version: "scan@1".into(),
                passed: true,
                critical_vulnerabilities: 0,
                high_vulnerabilities: 0,
                secret_findings: 0,
                report_digest: format!("sha256:{}", "d".repeat(64)),
            }),
            status: ReleasePackagingStatus::Validated,
            attempts: 1,
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        let desired = DesiredWorkRuntime::from_records(&runtime, &release, &packaging).unwrap();
        assert_eq!(desired.runtime_profile_id, STATIC_WEB_PROFILE_ID);
        assert!(!desired
            .labels
            .values()
            .any(|value| value == "future-template"));
        assert!(desired.image.contains("@sha256:"));

        let mut unsigned = packaging;
        unsigned.signature_digest = None;
        assert!(DesiredWorkRuntime::from_records(&runtime, &release, &unsigned).is_err());
    }
}
