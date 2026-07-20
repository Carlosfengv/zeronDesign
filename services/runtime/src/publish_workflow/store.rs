use crate::{
    types::sha256_hex,
    visual_contracts::{
        PublishSource, PublishWorkflow, PublishWorkflowCheckpoint, PublishWorkflowStageEvidence,
        PublishWorkflowStatus, VisualReviewMode, PUBLISH_WORKFLOW_SCHEMA,
    },
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::Mutex,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartPublishWorkflowRequest {
    pub source: PublishSource,
    pub idempotency_key: String,
    #[serde(default)]
    pub expected_current_release_id: Option<String>,
    pub expected_generation: u64,
    #[serde(default = "default_visual_review_mode")]
    pub visual_review_mode: VisualReviewMode,
    #[serde(default = "default_runtime_profile_id")]
    pub runtime_profile_id: String,
}

fn default_visual_review_mode() -> VisualReviewMode {
    VisualReviewMode::Advisory
}

fn default_runtime_profile_id() -> String {
    crate::release::STATIC_WEB_PROFILE_ID.to_string()
}

impl StartPublishWorkflowRequest {
    pub fn validate(&self, project_id: &str) -> Result<(), PublishWorkflowStoreError> {
        self.source
            .validate()
            .map_err(PublishWorkflowStoreError::InvalidInput)?;
        if self.source.project_id() != project_id {
            return Err(PublishWorkflowStoreError::InvalidInput(
                "PublishSource project does not match request path".to_string(),
            ));
        }
        if self.idempotency_key.trim().is_empty() || self.idempotency_key.len() > 200 {
            return Err(PublishWorkflowStoreError::InvalidInput(
                "idempotencyKey must contain 1-200 characters".to_string(),
            ));
        }
        if self.runtime_profile_id != crate::release::STATIC_WEB_PROFILE_ID {
            return Err(PublishWorkflowStoreError::InvalidInput(
                "only static-web-v1 is supported".to_string(),
            ));
        }
        Ok(())
    }

    fn request_hash(&self, project_id: &str) -> Result<String, PublishWorkflowStoreError> {
        let value = serde_json::json!({
            "projectId": project_id,
            "source": self.source,
            "expectedCurrentReleaseId": self.expected_current_release_id,
            "expectedGeneration": self.expected_generation,
            "visualReviewMode": self.visual_review_mode,
            "runtimeProfileId": self.runtime_profile_id,
        });
        serde_json::to_vec(&value)
            .map(|bytes| sha256_hex(&bytes))
            .map_err(|error| PublishWorkflowStoreError::Storage(error.to_string()))
    }
}

#[derive(Debug)]
pub enum PublishWorkflowStoreError {
    InvalidInput(String),
    NotFound(String),
    Conflict(String),
    InvalidTransition(String),
    Storage(String),
}

impl std::fmt::Display for PublishWorkflowStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid publish workflow: {message}"),
            Self::NotFound(message) => write!(formatter, "publish workflow not found: {message}"),
            Self::Conflict(message) => write!(formatter, "publish workflow conflict: {message}"),
            Self::InvalidTransition(message) => {
                write!(formatter, "invalid publish workflow transition: {message}")
            }
            Self::Storage(message) => {
                write!(formatter, "publish workflow storage failure: {message}")
            }
        }
    }
}

impl std::error::Error for PublishWorkflowStoreError {}

impl From<std::io::Error> for PublishWorkflowStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<serde_json::Error> for PublishWorkflowStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Default)]
struct State {
    workflows: BTreeMap<String, PublishWorkflow>,
    by_idempotency: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct PublishWorkflowStore {
    log_path: PathBuf,
    state: Mutex<State>,
}

impl PublishWorkflowStore {
    pub fn open(
        runtime_storage_dir: impl Into<PathBuf>,
    ) -> Result<Self, PublishWorkflowStoreError> {
        let log_path = runtime_storage_dir
            .into()
            .join("publish-workflows")
            .join("workflows.jsonl");
        let mut state = State::default();
        if let Ok(file) = fs::File::open(&log_path) {
            for line in BufReader::new(file).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let workflow: PublishWorkflow = serde_json::from_str(&line)?;
                workflow
                    .validate()
                    .map_err(PublishWorkflowStoreError::Storage)?;
                let scoped_key =
                    scoped_idempotency_key(&workflow.project_id, &workflow.idempotency_key_hash);
                state.by_idempotency.insert(scoped_key, workflow.id.clone());
                state.workflows.insert(workflow.id.clone(), workflow);
            }
        }
        Ok(Self {
            log_path,
            state: Mutex::new(state),
        })
    }

    pub fn start(
        &self,
        project_id: &str,
        request: &StartPublishWorkflowRequest,
    ) -> Result<(PublishWorkflow, bool), PublishWorkflowStoreError> {
        request.validate(project_id)?;
        let key_hash = sha256_hex(request.idempotency_key.as_bytes());
        let request_hash = request.request_hash(project_id)?;
        let scoped_key = scoped_idempotency_key(project_id, &key_hash);
        let mut state = self.state.lock().unwrap();
        if let Some(id) = state.by_idempotency.get(&scoped_key) {
            let existing = state.workflows.get(id).ok_or_else(|| {
                PublishWorkflowStoreError::Storage("idempotency index is corrupt".to_string())
            })?;
            if existing.request_hash != request_hash {
                return Err(PublishWorkflowStoreError::Conflict(
                    "idempotencyKey is already bound to a different request".to_string(),
                ));
            }
            return Ok((existing.clone(), false));
        }
        let now = Utc::now();
        let id = format!(
            "publish-workflow-{}",
            &sha256_hex(format!("{project_id}:{key_hash}").as_bytes())[..32]
        );
        let workflow = PublishWorkflow {
            schema_version: PUBLISH_WORKFLOW_SCHEMA.to_string(),
            id: id.clone(),
            idempotency_key_hash: key_hash,
            request_hash,
            project_id: project_id.to_string(),
            source: request.source.clone(),
            status: PublishWorkflowStatus::Requested,
            checkpoint: PublishWorkflowCheckpoint::Requested,
            visual_review_mode: request.visual_review_mode,
            expected_current_release_id: request.expected_current_release_id.clone(),
            expected_generation: request.expected_generation,
            version_id: None,
            release_id: None,
            publication_operation_id: None,
            public_url: None,
            evidence: Vec::new(),
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        workflow
            .validate()
            .map_err(PublishWorkflowStoreError::InvalidInput)?;
        self.append(&workflow)?;
        state.by_idempotency.insert(scoped_key, id.clone());
        state.workflows.insert(id, workflow.clone());
        Ok((workflow, true))
    }

    pub fn get(&self, id: &str) -> Option<PublishWorkflow> {
        self.state.lock().unwrap().workflows.get(id).cloned()
    }

    pub fn list_for_project(&self, project_id: &str) -> Vec<PublishWorkflow> {
        self.state
            .lock()
            .unwrap()
            .workflows
            .values()
            .filter(|workflow| workflow.project_id == project_id)
            .cloned()
            .collect()
    }

    pub fn nonterminal(&self) -> Vec<PublishWorkflow> {
        self.state
            .lock()
            .unwrap()
            .workflows
            .values()
            .filter(|workflow| !terminal(workflow.status))
            .cloned()
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn advance(
        &self,
        id: &str,
        expected_checkpoint: PublishWorkflowCheckpoint,
        status: PublishWorkflowStatus,
        checkpoint: PublishWorkflowCheckpoint,
        input_hash: &str,
        child_operation_id: Option<String>,
        version_id: Option<String>,
        release_id: Option<String>,
        publication_operation_id: Option<String>,
        public_url: Option<String>,
    ) -> Result<PublishWorkflow, PublishWorkflowStoreError> {
        if input_hash.len() != 64 || !input_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PublishWorkflowStoreError::InvalidInput(
                "stage input hash must be SHA-256".to_string(),
            ));
        }
        let mut state = self.state.lock().unwrap();
        let mut workflow = state
            .workflows
            .get(id)
            .cloned()
            .ok_or_else(|| PublishWorkflowStoreError::NotFound(id.to_string()))?;
        if workflow.checkpoint != expected_checkpoint {
            if workflow.checkpoint == checkpoint {
                return Ok(workflow);
            }
            return Err(PublishWorkflowStoreError::Conflict(format!(
                "expected checkpoint {expected_checkpoint:?}, found {:?}",
                workflow.checkpoint
            )));
        }
        if checkpoint_rank(checkpoint) < checkpoint_rank(expected_checkpoint) {
            return Err(PublishWorkflowStoreError::InvalidTransition(format!(
                "cannot move backward from {expected_checkpoint:?} to {checkpoint:?}"
            )));
        }
        let attempt = workflow
            .evidence
            .iter()
            .filter(|evidence| evidence.stage == checkpoint)
            .count() as u32
            + 1;
        workflow.status = status;
        workflow.checkpoint = checkpoint;
        workflow.version_id = version_id.or(workflow.version_id);
        workflow.release_id = release_id.or(workflow.release_id);
        workflow.publication_operation_id =
            publication_operation_id.or(workflow.publication_operation_id);
        workflow.public_url = public_url.or(workflow.public_url);
        workflow.last_error = None;
        workflow.updated_at = Utc::now();
        workflow.evidence.push(PublishWorkflowStageEvidence {
            stage: checkpoint,
            input_hash: input_hash.to_ascii_lowercase(),
            child_operation_id,
            attempt,
            completed_at: workflow.updated_at,
        });
        workflow
            .validate()
            .map_err(PublishWorkflowStoreError::InvalidTransition)?;
        self.append(&workflow)?;
        state.workflows.insert(id.to_string(), workflow.clone());
        Ok(workflow)
    }

    pub fn set_status(
        &self,
        id: &str,
        status: PublishWorkflowStatus,
        error: Option<String>,
    ) -> Result<PublishWorkflow, PublishWorkflowStoreError> {
        let mut state = self.state.lock().unwrap();
        let mut workflow = state
            .workflows
            .get(id)
            .cloned()
            .ok_or_else(|| PublishWorkflowStoreError::NotFound(id.to_string()))?;
        workflow.status = status;
        workflow.last_error = error;
        workflow.updated_at = Utc::now();
        self.append(&workflow)?;
        state.workflows.insert(id.to_string(), workflow.clone());
        Ok(workflow)
    }

    pub fn cancel(&self, id: &str) -> Result<PublishWorkflow, PublishWorkflowStoreError> {
        let mut state = self.state.lock().unwrap();
        let mut workflow = state
            .workflows
            .get(id)
            .cloned()
            .ok_or_else(|| PublishWorkflowStoreError::NotFound(id.to_string()))?;
        if workflow.status == PublishWorkflowStatus::Cancelled {
            return Ok(workflow);
        }
        if terminal(workflow.status)
            || checkpoint_rank(workflow.checkpoint)
                >= checkpoint_rank(PublishWorkflowCheckpoint::DesiredStateCommitted)
        {
            return Err(PublishWorkflowStoreError::Conflict(
                "workflow can only be cancelled before desired state is committed".to_string(),
            ));
        }
        workflow.status = PublishWorkflowStatus::Cancelled;
        workflow.last_error = None;
        workflow.updated_at = Utc::now();
        self.append(&workflow)?;
        state.workflows.insert(id.to_string(), workflow.clone());
        Ok(workflow)
    }

    fn append(&self, workflow: &PublishWorkflow) -> Result<(), PublishWorkflowStoreError> {
        if let Some(parent) = self.log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        serde_json::to_writer(&mut file, workflow)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    }
}

fn scoped_idempotency_key(project_id: &str, key_hash: &str) -> String {
    sha256_hex(format!("{project_id}\0{key_hash}").as_bytes())
}

fn checkpoint_rank(checkpoint: PublishWorkflowCheckpoint) -> u8 {
    match checkpoint {
        PublishWorkflowCheckpoint::Requested => 0,
        PublishWorkflowCheckpoint::SourceFrozen => 1,
        PublishWorkflowCheckpoint::Building => 2,
        PublishWorkflowCheckpoint::Validating => 3,
        PublishWorkflowCheckpoint::ReleasePackaging => 4,
        PublishWorkflowCheckpoint::ReleaseValidated => 5,
        PublishWorkflowCheckpoint::DesiredStateCommitted => 6,
        PublishWorkflowCheckpoint::Reconciling => 7,
        PublishWorkflowCheckpoint::WorkloadReady => 8,
        PublishWorkflowCheckpoint::TrafficSwitched => 9,
        PublishWorkflowCheckpoint::ExternalProbePassed => 10,
        PublishWorkflowCheckpoint::RollingBack => 11,
        PublishWorkflowCheckpoint::Completed => 12,
        PublishWorkflowCheckpoint::RolledBack => 13,
    }
}

fn terminal(status: PublishWorkflowStatus) -> bool {
    matches!(
        status,
        PublishWorkflowStatus::Completed
            | PublishWorkflowStatus::Failed
            | PublishWorkflowStatus::Cancelled
            | PublishWorkflowStatus::RolledBack
            | PublishWorkflowStatus::RollbackFailed
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(key: &str, source_hash: char) -> StartPublishWorkflowRequest {
        StartPublishWorkflowRequest {
            source: PublishSource::StaticSnapshot {
                project_id: "project-publish".to_string(),
                snapshot_id: "draft-snapshot-1".to_string(),
                expected_source_hash: source_hash.to_string().repeat(64),
            },
            idempotency_key: key.to_string(),
            expected_current_release_id: None,
            expected_generation: 0,
            visual_review_mode: VisualReviewMode::Advisory,
            runtime_profile_id: crate::release::STATIC_WEB_PROFILE_ID.to_string(),
        }
    }

    #[test]
    fn start_is_idempotent_conflict_safe_and_restart_durable() {
        let root = std::env::temp_dir().join(format!(
            "publish-workflow-store-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = PublishWorkflowStore::open(&root).unwrap();
        let (created, is_new) = store
            .start("project-publish", &request("click-1", 'a'))
            .unwrap();
        assert!(is_new);
        let (retried, is_new) = store
            .start("project-publish", &request("click-1", 'a'))
            .unwrap();
        assert!(!is_new);
        assert_eq!(retried.id, created.id);
        assert!(matches!(
            store.start("project-publish", &request("click-1", 'b')),
            Err(PublishWorkflowStoreError::Conflict(_))
        ));

        store
            .advance(
                &created.id,
                PublishWorkflowCheckpoint::Requested,
                PublishWorkflowStatus::SourceFrozen,
                PublishWorkflowCheckpoint::SourceFrozen,
                &"a".repeat(64),
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        drop(store);

        let reopened = PublishWorkflowStore::open(&root).unwrap();
        let recovered = reopened.get(&created.id).unwrap();
        assert_eq!(
            recovered.checkpoint,
            PublishWorkflowCheckpoint::SourceFrozen
        );
        assert_eq!(recovered.evidence.len(), 1);
        let cancelled = reopened.cancel(&created.id).unwrap();
        assert_eq!(cancelled.status, PublishWorkflowStatus::Cancelled);
        assert_eq!(
            reopened.cancel(&created.id).unwrap().status,
            cancelled.status
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancel_is_rejected_after_desired_state_commit() {
        let root = std::env::temp_dir().join(format!(
            "publish-workflow-cancel-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let store = PublishWorkflowStore::open(&root).unwrap();
        let (workflow, _) = store
            .start("project-publish", &request("click-committed", 'a'))
            .unwrap();
        store
            .advance(
                &workflow.id,
                PublishWorkflowCheckpoint::Requested,
                PublishWorkflowStatus::DesiredStateCommitted,
                PublishWorkflowCheckpoint::DesiredStateCommitted,
                &"a".repeat(64),
                Some("operation-1".to_string()),
                None,
                None,
                Some("operation-1".to_string()),
                None,
            )
            .unwrap();
        assert!(matches!(
            store.cancel(&workflow.id),
            Err(PublishWorkflowStoreError::Conflict(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
