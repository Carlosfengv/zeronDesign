use crate::types::sha256_hex;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkReleaseStatus {
    Packaging,
    Packaged,
    Validated,
    Failed,
    GarbageCollectable,
    GarbageCollected,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReleasePackagingStatus {
    Prepared,
    Building,
    Pushed,
    Scanning,
    Signing,
    Validated,
    Failed,
    ReconcileRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackagingScanEvidence {
    pub policy_version: String,
    pub passed: bool,
    pub critical_vulnerabilities: u32,
    pub high_vulnerabilities: u32,
    pub secret_findings: u32,
    pub report_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseGarbageCollectionEvidence {
    pub registry_manifest_deleted: bool,
    pub packaging_evidence_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePackagingInput {
    pub project_id: String,
    pub version_id: String,
    pub run_id: String,
    pub template_id: String,
    pub template_version: String,
    pub artifact_manifest_hash: String,
    pub runtime_manifest_hash: String,
    pub source_snapshot_uri: String,
    pub runtime_profile_id: String,
    pub base_image_digest: String,
    pub packager_version: String,
    pub registry_repository: String,
    pub scan_policy_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkRelease {
    pub id: String,
    pub project_id: String,
    pub version_id: String,
    pub run_id: String,
    pub template_id: String,
    pub template_version: String,
    pub artifact_manifest_hash: String,
    pub runtime_manifest_hash: String,
    pub source_snapshot_uri: String,
    pub runtime_profile_id: String,
    pub runtime_image_ref: Option<String>,
    pub runtime_image_digest: Option<String>,
    pub status: WorkReleaseStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleasePackagingRecord {
    pub id: String,
    pub idempotency_key: String,
    pub project_id: String,
    pub release_id: String,
    pub artifact_manifest_hash: String,
    pub runtime_manifest_hash: String,
    pub base_image_digest: String,
    pub packager_version: String,
    pub registry_repository: String,
    pub built_image_digest: Option<String>,
    pub pushed_image_digest: Option<String>,
    pub sbom_digest: Option<String>,
    pub provenance_digest: Option<String>,
    pub signature_identity: Option<String>,
    pub signature_digest: Option<String>,
    pub scan_policy_version: String,
    pub scan_evidence: Option<PackagingScanEvidence>,
    pub status: ReleasePackagingStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn packaging_idempotency_key(input: &ReleasePackagingInput) -> String {
    let fields = [
        input.artifact_manifest_hash.as_str(),
        input.runtime_manifest_hash.as_str(),
        input.base_image_digest.as_str(),
        input.packager_version.as_str(),
        input.scan_policy_version.as_str(),
    ];
    let mut framed = Vec::new();
    for field in fields {
        framed.extend_from_slice(&(field.len() as u64).to_be_bytes());
        framed.extend_from_slice(field.as_bytes());
    }
    sha256_hex(&framed)
}

impl ReleasePackagingInput {
    pub fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("projectId", self.project_id.as_str()),
            ("versionId", self.version_id.as_str()),
            ("runId", self.run_id.as_str()),
            ("templateId", self.template_id.as_str()),
            ("templateVersion", self.template_version.as_str()),
            ("sourceSnapshotUri", self.source_snapshot_uri.as_str()),
            ("runtimeProfileId", self.runtime_profile_id.as_str()),
            ("packagerVersion", self.packager_version.as_str()),
            ("registryRepository", self.registry_repository.as_str()),
            ("scanPolicyVersion", self.scan_policy_version.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("release packaging {field} must not be empty"));
            }
        }
        validate_sha256(&self.artifact_manifest_hash, "artifact manifest hash")?;
        validate_sha256(&self.runtime_manifest_hash, "runtime manifest hash")?;
        validate_digest(&self.base_image_digest, "base image digest")?;
        if self.registry_repository.contains("://")
            || self.registry_repository.contains('@')
            || self.registry_repository.ends_with('/')
        {
            return Err("release registry repository is invalid".to_string());
        }
        Ok(())
    }
}

impl WorkRelease {
    pub(crate) fn packaging(input: &ReleasePackagingInput, key: &str, now: DateTime<Utc>) -> Self {
        Self {
            id: format!("release-{}", &key[..32]),
            project_id: input.project_id.clone(),
            version_id: input.version_id.clone(),
            run_id: input.run_id.clone(),
            template_id: input.template_id.clone(),
            template_version: input.template_version.clone(),
            artifact_manifest_hash: input.artifact_manifest_hash.clone(),
            runtime_manifest_hash: input.runtime_manifest_hash.clone(),
            source_snapshot_uri: input.source_snapshot_uri.clone(),
            runtime_profile_id: input.runtime_profile_id.clone(),
            runtime_image_ref: None,
            runtime_image_digest: None,
            status: WorkReleaseStatus::Packaging,
            created_at: now,
            updated_at: now,
        }
    }
}

impl ReleasePackagingRecord {
    pub(crate) fn prepared(
        input: &ReleasePackagingInput,
        release_id: &str,
        key: &str,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id: format!("packaging-{}", &key[..32]),
            idempotency_key: key.to_string(),
            project_id: input.project_id.clone(),
            release_id: release_id.to_string(),
            artifact_manifest_hash: input.artifact_manifest_hash.clone(),
            runtime_manifest_hash: input.runtime_manifest_hash.clone(),
            base_image_digest: input.base_image_digest.clone(),
            packager_version: input.packager_version.clone(),
            registry_repository: input.registry_repository.clone(),
            built_image_digest: None,
            pushed_image_digest: None,
            sbom_digest: None,
            provenance_digest: None,
            signature_identity: None,
            signature_digest: None,
            scan_policy_version: input.scan_policy_version.clone(),
            scan_evidence: None,
            status: ReleasePackagingStatus::Prepared,
            attempts: 0,
            last_error: None,
            created_at: now,
            updated_at: now,
        }
    }
}

pub(crate) fn validate_sha256(value: &str, field: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!("{field} is invalid"));
    }
    Ok(())
}

pub(crate) fn validate_digest(value: &str, field: &str) -> Result<(), String> {
    let Some(hash) = value.strip_prefix("sha256:") else {
        return Err(format!("{field} must be sha256-pinned"));
    };
    validate_sha256(hash, field)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ReleasePackagingInput {
        ReleasePackagingInput {
            project_id: "project-1".to_string(),
            version_id: "version-1".to_string(),
            run_id: "run-1".to_string(),
            template_id: "astro-website".to_string(),
            template_version: "astro-website@runtime-p3".to_string(),
            artifact_manifest_hash: "a".repeat(64),
            runtime_manifest_hash: "b".repeat(64),
            source_snapshot_uri: "runtime://source-snapshots/project-1/build-1".to_string(),
            runtime_profile_id: "static-web-v1".to_string(),
            base_image_digest: format!("sha256:{}", "c".repeat(64)),
            packager_version: "packager@1".to_string(),
            registry_repository: "registry.example/works".to_string(),
            scan_policy_version: "scan@1".to_string(),
        }
    }

    #[test]
    fn packaging_key_is_stable_and_changes_with_trust_inputs() {
        let first = input();
        let second = first.clone();
        assert_eq!(
            packaging_idempotency_key(&first),
            packaging_idempotency_key(&second)
        );
        let mut changed = second;
        changed.scan_policy_version = "scan@2".to_string();
        assert_ne!(
            packaging_idempotency_key(&first),
            packaging_idempotency_key(&changed)
        );
    }

    #[test]
    fn packaging_input_requires_digest_pinned_trust_inputs() {
        let mut value = input();
        assert!(value.validate().is_ok());
        value.base_image_digest = "nginx:latest".to_string();
        assert!(value.validate().is_err());
    }

    #[test]
    fn persisted_status_enums_match_the_frozen_schemas() {
        let release_schema: serde_json::Value =
            serde_json::from_str(include_str!("../../contracts/work-release-v1.schema.json"))
                .unwrap();
        let packaging_schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../contracts/release-packaging-v1.schema.json"
        ))
        .unwrap();
        let release_statuses = [
            WorkReleaseStatus::Packaging,
            WorkReleaseStatus::Packaged,
            WorkReleaseStatus::Validated,
            WorkReleaseStatus::Failed,
            WorkReleaseStatus::GarbageCollectable,
            WorkReleaseStatus::GarbageCollected,
        ]
        .into_iter()
        .map(|status| serde_json::to_value(status).unwrap())
        .collect::<Vec<_>>();
        let packaging_statuses = [
            ReleasePackagingStatus::Prepared,
            ReleasePackagingStatus::Building,
            ReleasePackagingStatus::Pushed,
            ReleasePackagingStatus::Scanning,
            ReleasePackagingStatus::Signing,
            ReleasePackagingStatus::Validated,
            ReleasePackagingStatus::Failed,
            ReleasePackagingStatus::ReconcileRequired,
        ]
        .into_iter()
        .map(|status| serde_json::to_value(status).unwrap())
        .collect::<Vec<_>>();
        assert_eq!(
            release_schema["properties"]["status"]["enum"],
            serde_json::Value::Array(release_statuses)
        );
        assert_eq!(
            packaging_schema["properties"]["status"]["enum"],
            serde_json::Value::Array(packaging_statuses)
        );
    }
}
