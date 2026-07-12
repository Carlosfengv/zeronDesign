use crate::types::sha256_hex;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const PUBLISH_OPERATION_SCHEMA: &str = "publish-operation@1";
pub const WORK_RUNTIME_STATE_SCHEMA: &str = "work-runtime-state@1";
pub const PUBLICATION_OUTBOX_SCHEMA: &str = "publication-outbox@1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublishOperationKind {
    Publish,
    Update,
    Rollback,
    Unpublish,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublishOperationStatus {
    Requested,
    Packaging,
    ReleaseValidated,
    DesiredStateCommitted,
    Reconciling,
    WorkloadReady,
    TrafficSwitched,
    ExternalProbePassed,
    Completed,
    Failed,
    Cancelled,
    ReconcileRequired,
}

impl PublishOperationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublishCheckpoint {
    Requested,
    Packaging,
    ReleaseValidated,
    DesiredStateCommitted,
    Reconciling,
    WorkloadReady,
    TrafficSwitched,
    ExternalProbePassed,
    Completed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublicationDesiredState {
    Unpublished,
    Published,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkRuntimeStatus {
    Unpublished,
    Publishing,
    Published,
    Updating,
    Unpublishing,
    PublishFailed,
    UpdateFailed,
    ReconcileRequired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublicationOutboxStatus {
    Pending,
    Delivered,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationIntent {
    pub project_id: String,
    pub kind: PublishOperationKind,
    pub release_id: Option<String>,
    pub expected_current_release_id: Option<String>,
    pub expected_generation: Option<u64>,
    pub runtime_profile_id: String,
    pub idempotency_key: String,
}

impl PublicationIntent {
    pub fn validate(&self) -> Result<(), String> {
        for (field, value) in [
            ("projectId", self.project_id.as_str()),
            ("runtimeProfileId", self.runtime_profile_id.as_str()),
            ("idempotencyKey", self.idempotency_key.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("publication {field} must not be empty"));
            }
        }
        if self.project_id.len() > 128
            || self.runtime_profile_id.len() > 128
            || self.idempotency_key.len() > 256
        {
            return Err("publication intent field exceeds its length limit".to_string());
        }
        if self.expected_generation.is_none() {
            return Err("publication expectedGeneration is required for compare-and-swap".to_string());
        }
        match self.kind {
            PublishOperationKind::Publish
            | PublishOperationKind::Update
            | PublishOperationKind::Rollback => {
                if self.release_id.as_deref().is_none_or(str::is_empty) {
                    return Err("publication releaseId is required".to_string());
                }
            }
            PublishOperationKind::Unpublish if self.release_id.is_some() => {
                return Err("unpublish must not include releaseId".to_string());
            }
            PublishOperationKind::Unpublish => {}
        }
        if matches!(self.kind, PublishOperationKind::Update | PublishOperationKind::Rollback)
            && self.expected_current_release_id.as_deref().is_none_or(str::is_empty)
        {
            return Err(
                "update or rollback requires expectedCurrentReleaseId compare-and-swap"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn idempotency_key_hash(&self) -> String {
        sha256_hex(self.idempotency_key.as_bytes())
    }

    pub fn request_hash(&self) -> String {
        let kind = match self.kind {
            PublishOperationKind::Publish => "publish",
            PublishOperationKind::Update => "update",
            PublishOperationKind::Rollback => "rollback",
            PublishOperationKind::Unpublish => "unpublish",
        };
        framed_hash(&[
            self.project_id.as_str(),
            kind,
            self.release_id.as_deref().unwrap_or(""),
            self.expected_current_release_id.as_deref().unwrap_or(""),
            &self
                .expected_generation
                .map(|value| value.to_string())
                .unwrap_or_default(),
            self.runtime_profile_id.as_str(),
        ])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishOperation {
    pub schema_version: String,
    pub id: String,
    pub idempotency_key_hash: String,
    pub request_hash: String,
    pub project_id: String,
    pub release_id: Option<String>,
    pub expected_current_release_id: Option<String>,
    pub desired_generation: u64,
    pub kind: PublishOperationKind,
    pub status: PublishOperationStatus,
    pub checkpoint: PublishCheckpoint,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkRuntimeState {
    pub schema_version: String,
    pub project_id: String,
    pub desired_publication: PublicationDesiredState,
    pub desired_release_id: Option<String>,
    pub current_release_id: Option<String>,
    pub previous_release_id: Option<String>,
    pub last_successful_release_id: Option<String>,
    pub desired_generation: u64,
    pub host_slug: String,
    pub runtime_profile_id: String,
    pub current_deployment_name: Option<String>,
    pub previous_deployment_name: Option<String>,
    pub service_name: String,
    pub ingress_name: String,
    pub deployment_uid: Option<String>,
    pub deployment_resource_version: Option<String>,
    pub service_uid: Option<String>,
    pub service_resource_version: Option<String>,
    pub ingress_uid: Option<String>,
    pub ingress_resource_version: Option<String>,
    pub observed_generation: u64,
    pub status: WorkRuntimeStatus,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationOutboxEvent {
    pub schema_version: String,
    pub id: String,
    pub project_id: String,
    pub operation_id: String,
    pub desired_generation: u64,
    pub status: PublicationOutboxStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub next_attempt_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

pub(crate) fn framed_hash(fields: &[&str]) -> String {
    let mut framed = Vec::new();
    for field in fields {
        framed.extend_from_slice(&(field.len() as u64).to_be_bytes());
        framed.extend_from_slice(field.as_bytes());
    }
    sha256_hex(&framed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_hash_is_framed_and_excludes_raw_idempotency_key() {
        let intent = PublicationIntent {
            project_id: "project-1".into(),
            kind: PublishOperationKind::Publish,
            release_id: Some("release-1".into()),
            expected_current_release_id: None,
            expected_generation: Some(0),
            runtime_profile_id: "static-web-v1".into(),
            idempotency_key: "client-secret-looking-key".into(),
        };
        assert_eq!(intent.request_hash().len(), 64);
        assert_eq!(intent.idempotency_key_hash().len(), 64);
        assert!(!intent.request_hash().contains(&intent.idempotency_key));
        let mut changed = intent.clone();
        changed.release_id = Some("release-2".into());
        assert_ne!(intent.request_hash(), changed.request_hash());
    }

    #[test]
    fn frozen_schemas_match_persisted_enum_values() {
        let operation_schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../contracts/publish-operation-v1.schema.json"
        ))
        .unwrap();
        let statuses = operation_schema["properties"]["status"]["enum"]
            .as_array()
            .unwrap();
        for status in [
            PublishOperationStatus::Requested,
            PublishOperationStatus::Packaging,
            PublishOperationStatus::ReleaseValidated,
            PublishOperationStatus::DesiredStateCommitted,
            PublishOperationStatus::Reconciling,
            PublishOperationStatus::WorkloadReady,
            PublishOperationStatus::TrafficSwitched,
            PublishOperationStatus::ExternalProbePassed,
            PublishOperationStatus::Completed,
            PublishOperationStatus::Failed,
            PublishOperationStatus::Cancelled,
            PublishOperationStatus::ReconcileRequired,
        ] {
            assert!(statuses.contains(&serde_json::to_value(status).unwrap()));
        }
        let runtime_schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../contracts/work-runtime-state-v1.schema.json"
        ))
        .unwrap();
        assert_eq!(
            runtime_schema["properties"]["schemaVersion"]["const"],
            WORK_RUNTIME_STATE_SCHEMA
        );
    }
}
