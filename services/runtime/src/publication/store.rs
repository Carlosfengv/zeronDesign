use super::{
    model::{
        framed_hash, PUBLICATION_OUTBOX_SCHEMA, PUBLISH_OPERATION_SCHEMA, WORK_RUNTIME_STATE_SCHEMA,
    },
    PublicationDesiredState, PublicationIntent, PublicationOutboxEvent, PublicationOutboxStatus,
    PublishCheckpoint, PublishOperation, PublishOperationKind, PublishOperationStatus,
    WorkRuntimeState, WorkRuntimeStatus,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

const JOURNAL_FILE: &str = "publication-commits.jsonl";
const CHECKPOINT_FILE: &str = "publication-checkpoint.json";

#[derive(Debug)]
pub enum PublicationStoreError {
    InvalidInput(String),
    NotFound(String),
    Conflict(String),
    InvalidTransition(String),
    Storage(String),
}

impl fmt::Display for PublicationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => {
                write!(formatter, "invalid publication input: {message}")
            }
            Self::NotFound(message) => write!(formatter, "publication record not found: {message}"),
            Self::Conflict(message) => write!(formatter, "publication conflict: {message}"),
            Self::InvalidTransition(message) => {
                write!(formatter, "invalid publication transition: {message}")
            }
            Self::Storage(message) => write!(formatter, "publication storage failure: {message}"),
        }
    }
}

impl Error for PublicationStoreError {}

impl From<std::io::Error> for PublicationStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<serde_json::Error> for PublicationStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicationSnapshot {
    sequence: u64,
    operations: BTreeMap<String, PublishOperation>,
    runtimes: BTreeMap<String, WorkRuntimeState>,
    outbox: BTreeMap<String, PublicationOutboxEvent>,
    operation_by_idempotency: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicationCommit {
    sequence: u64,
    operation: PublishOperation,
    runtime: WorkRuntimeState,
    outbox: PublicationOutboxEvent,
}

#[derive(Debug)]
pub struct PublicationStore {
    root: PathBuf,
    state: Mutex<PublicationSnapshot>,
}

impl PublicationStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, PublicationStoreError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        let mut state = read_checkpoint(&root).unwrap_or_default();
        for commit in read_journal(&root)? {
            if commit.sequence > state.sequence {
                apply_commit(&mut state, commit)?;
            }
        }
        validate_snapshot(&state)?;
        Ok(Self {
            root,
            state: Mutex::new(state),
        })
    }

    pub fn commit_intent(
        &self,
        intent: &PublicationIntent,
    ) -> Result<(PublishOperation, WorkRuntimeState), PublicationStoreError> {
        intent
            .validate()
            .map_err(PublicationStoreError::InvalidInput)?;
        let key_hash = intent.idempotency_key_hash();
        let request_hash = intent.request_hash();
        let scoped_key = framed_hash(&[intent.project_id.as_str(), key_hash.as_str()]);
        let mut snapshot = self.state.lock().unwrap();
        if let Some(operation_id) = snapshot.operation_by_idempotency.get(&scoped_key) {
            let operation = snapshot.operations.get(operation_id).ok_or_else(|| {
                PublicationStoreError::Storage("idempotency index is corrupt".to_string())
            })?;
            if operation.request_hash != request_hash {
                return Err(PublicationStoreError::Conflict(
                    "Idempotency-Key is already bound to a different request body".to_string(),
                ));
            }
            let runtime = snapshot.runtimes.get(&intent.project_id).ok_or_else(|| {
                PublicationStoreError::Storage("operation runtime link is corrupt".to_string())
            })?;
            return Ok((operation.clone(), runtime.clone()));
        }

        let existing = snapshot.runtimes.get(&intent.project_id).cloned();
        validate_compare_and_set(existing.as_ref(), intent)?;
        let now = Utc::now();
        let generation = existing
            .as_ref()
            .map_or(1, |runtime| runtime.desired_generation.saturating_add(1));
        let operation_id = format!(
            "operation-{}",
            &framed_hash(&[intent.project_id.as_str(), key_hash.as_str()])[..32]
        );
        let host_slug = allocate_host_slug(&snapshot)?;
        let mut runtime = existing.unwrap_or_else(|| initial_runtime(intent, &host_slug, now));
        runtime.desired_generation = generation;
        runtime.desired_release_id = intent.release_id.clone();
        runtime.desired_publication = if intent.kind == PublishOperationKind::Unpublish {
            PublicationDesiredState::Unpublished
        } else {
            PublicationDesiredState::Published
        };
        runtime.runtime_profile_id = intent.runtime_profile_id.clone();
        runtime.status = match intent.kind {
            PublishOperationKind::Publish => WorkRuntimeStatus::Publishing,
            PublishOperationKind::Update | PublishOperationKind::Rollback => {
                WorkRuntimeStatus::Updating
            }
            PublishOperationKind::Unpublish => WorkRuntimeStatus::Unpublishing,
        };
        runtime.last_error = None;
        runtime.updated_at = now;
        let operation = PublishOperation {
            schema_version: PUBLISH_OPERATION_SCHEMA.to_string(),
            id: operation_id.clone(),
            idempotency_key_hash: key_hash,
            request_hash,
            project_id: intent.project_id.clone(),
            release_id: intent.release_id.clone(),
            expected_current_release_id: intent.expected_current_release_id.clone(),
            desired_generation: generation,
            kind: intent.kind,
            status: PublishOperationStatus::DesiredStateCommitted,
            checkpoint: PublishCheckpoint::DesiredStateCommitted,
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        let outbox = PublicationOutboxEvent {
            schema_version: PUBLICATION_OUTBOX_SCHEMA.to_string(),
            id: format!("publication-outbox-{operation_id}"),
            project_id: intent.project_id.clone(),
            operation_id,
            desired_generation: generation,
            status: PublicationOutboxStatus::Pending,
            attempts: 0,
            last_error: None,
            next_attempt_at: now,
            created_at: now,
            updated_at: now,
            delivered_at: None,
        };
        persist_commit(
            &self.root,
            &mut snapshot,
            operation.clone(),
            runtime.clone(),
            outbox,
            scoped_key,
        )?;
        Ok((operation, runtime))
    }

    pub fn operation(&self, id: &str) -> Option<PublishOperation> {
        self.state.lock().unwrap().operations.get(id).cloned()
    }

    pub fn runtime(&self, project_id: &str) -> Option<WorkRuntimeState> {
        self.state.lock().unwrap().runtimes.get(project_id).cloned()
    }

    pub fn operations_for_project(&self, project_id: &str) -> Vec<PublishOperation> {
        self.state
            .lock()
            .unwrap()
            .operations
            .values()
            .filter(|operation| operation.project_id == project_id)
            .cloned()
            .collect()
    }

    pub fn pending_outbox(&self) -> Vec<PublicationOutboxEvent> {
        self.state
            .lock()
            .unwrap()
            .outbox
            .values()
            .filter(|event| {
                event.status == PublicationOutboxStatus::Pending
                    && event.next_attempt_at <= Utc::now()
            })
            .cloned()
            .collect()
    }

    pub fn nonterminal_operations(&self) -> Vec<PublishOperation> {
        self.state
            .lock()
            .unwrap()
            .operations
            .values()
            .filter(|operation| !operation.status.is_terminal())
            .cloned()
            .collect()
    }

    pub fn replay_nonterminal_outbox(&self) -> Result<usize, PublicationStoreError> {
        let candidates = {
            let snapshot = self.state.lock().unwrap();
            snapshot
                .outbox
                .values()
                .filter(|event| {
                    event.status == PublicationOutboxStatus::Delivered
                        && snapshot
                            .operations
                            .get(&event.operation_id)
                            .is_some_and(|operation| !operation.status.is_terminal())
                        && snapshot
                            .runtimes
                            .get(&event.project_id)
                            .is_some_and(|runtime| {
                                runtime.observed_generation < event.desired_generation
                            })
                })
                .map(|event| event.id.clone())
                .collect::<Vec<_>>()
        };
        for outbox_id in &candidates {
            self.update_outbox(outbox_id, |operation, _, outbox| {
                outbox.status = PublicationOutboxStatus::Pending;
                outbox.delivered_at = None;
                outbox.next_attempt_at = Utc::now();
                outbox.last_error = Some("replayed nonterminal operation at startup".to_string());
                operation.status = PublishOperationStatus::ReconcileRequired;
                Ok(())
            })?;
        }
        Ok(candidates.len())
    }

    pub fn record_delivery_attempt(
        &self,
        outbox_id: &str,
        error: Option<String>,
    ) -> Result<PublicationOutboxEvent, PublicationStoreError> {
        self.update_outbox(outbox_id, |operation, runtime, outbox| {
            if outbox.status == PublicationOutboxStatus::Delivered {
                return Ok(());
            }
            outbox.attempts = outbox.attempts.saturating_add(1);
            outbox.last_error = error.clone();
            if let Some(error) = &error {
                operation.status = PublishOperationStatus::ReconcileRequired;
                operation.last_error = Some(error.clone());
                runtime.status = WorkRuntimeStatus::ReconcileRequired;
                runtime.last_error = Some(error.clone());
            }
            let backoff_seconds = 1_i64 << outbox.attempts.min(6);
            outbox.next_attempt_at = Utc::now() + chrono::Duration::seconds(backoff_seconds);
            Ok(())
        })
        .map(|(_, _, outbox)| outbox)
    }

    pub fn record_delivered(
        &self,
        outbox_id: &str,
    ) -> Result<(PublishOperation, WorkRuntimeState), PublicationStoreError> {
        self.update_outbox(outbox_id, |operation, _, outbox| {
            outbox.status = PublicationOutboxStatus::Delivered;
            outbox.last_error = None;
            outbox.delivered_at = Some(Utc::now());
            outbox.next_attempt_at = Utc::now();
            operation.status = PublishOperationStatus::Reconciling;
            operation.checkpoint = PublishCheckpoint::Reconciling;
            Ok(())
        })
        .map(|(operation, runtime, _)| (operation, runtime))
    }

    fn update_outbox<F>(
        &self,
        outbox_id: &str,
        update: F,
    ) -> Result<(PublishOperation, WorkRuntimeState, PublicationOutboxEvent), PublicationStoreError>
    where
        F: FnOnce(
            &mut PublishOperation,
            &mut WorkRuntimeState,
            &mut PublicationOutboxEvent,
        ) -> Result<(), PublicationStoreError>,
    {
        let mut snapshot = self.state.lock().unwrap();
        let mut outbox = snapshot
            .outbox
            .get(outbox_id)
            .cloned()
            .ok_or_else(|| PublicationStoreError::NotFound(outbox_id.to_string()))?;
        let mut operation = snapshot
            .operations
            .get(&outbox.operation_id)
            .cloned()
            .ok_or_else(|| PublicationStoreError::NotFound(outbox.operation_id.clone()))?;
        let mut runtime = snapshot
            .runtimes
            .get(&outbox.project_id)
            .cloned()
            .ok_or_else(|| PublicationStoreError::NotFound(outbox.project_id.clone()))?;
        update(&mut operation, &mut runtime, &mut outbox)?;
        let now = Utc::now();
        operation.updated_at = now;
        runtime.updated_at = now;
        outbox.updated_at = now;
        let scoped_key = framed_hash(&[
            operation.project_id.as_str(),
            operation.idempotency_key_hash.as_str(),
        ]);
        persist_commit(
            &self.root,
            &mut snapshot,
            operation.clone(),
            runtime.clone(),
            outbox.clone(),
            scoped_key,
        )?;
        Ok((operation, runtime, outbox))
    }
}

fn validate_compare_and_set(
    current: Option<&WorkRuntimeState>,
    intent: &PublicationIntent,
) -> Result<(), PublicationStoreError> {
    if let Some(expected) = intent.expected_generation {
        if current.map_or(0, |runtime| runtime.desired_generation) != expected {
            return Err(PublicationStoreError::Conflict(
                "expected generation does not match current desired generation".to_string(),
            ));
        }
    }
    if intent.expected_current_release_id.as_deref()
        != current.and_then(|runtime| runtime.current_release_id.as_deref())
        && intent.expected_current_release_id.is_some()
    {
        return Err(PublicationStoreError::Conflict(
            "expected current release does not match observed current release".to_string(),
        ));
    }
    if intent.kind == PublishOperationKind::Publish
        && current.is_some_and(|runtime| {
            runtime.desired_publication == PublicationDesiredState::Published
        })
    {
        return Err(PublicationStoreError::Conflict(
            "published work requires update or rollback intent".to_string(),
        ));
    }
    if matches!(
        intent.kind,
        PublishOperationKind::Update | PublishOperationKind::Rollback
    ) && current.is_none_or(|runtime| runtime.current_release_id.is_none())
    {
        return Err(PublicationStoreError::Conflict(
            "update or rollback requires an observed current release".to_string(),
        ));
    }
    Ok(())
}

fn allocate_host_slug(snapshot: &PublicationSnapshot) -> Result<String, PublicationStoreError> {
    for _ in 0..16 {
        let entropy = rand::random::<[u8; 16]>();
        let candidate = format!("w-{}", &crate::types::sha256_hex(&entropy)[..20]);
        if snapshot
            .runtimes
            .values()
            .all(|runtime| runtime.host_slug != candidate)
        {
            return Ok(candidate);
        }
    }
    Err(PublicationStoreError::Storage(
        "failed to allocate a unique publication host identity".to_string(),
    ))
}

fn initial_runtime(
    intent: &PublicationIntent,
    host_slug: &str,
    now: chrono::DateTime<Utc>,
) -> WorkRuntimeState {
    let resource_key = &framed_hash(&[intent.project_id.as_str()])[..12];
    WorkRuntimeState {
        schema_version: WORK_RUNTIME_STATE_SCHEMA.to_string(),
        project_id: intent.project_id.clone(),
        desired_publication: PublicationDesiredState::Unpublished,
        desired_release_id: None,
        current_release_id: None,
        previous_release_id: None,
        last_successful_release_id: None,
        desired_generation: 0,
        host_slug: host_slug.to_string(),
        runtime_profile_id: intent.runtime_profile_id.clone(),
        current_deployment_name: None,
        previous_deployment_name: None,
        service_name: format!("work-{resource_key}"),
        ingress_name: format!("work-{resource_key}"),
        deployment_uid: None,
        deployment_resource_version: None,
        service_uid: None,
        service_resource_version: None,
        ingress_uid: None,
        ingress_resource_version: None,
        observed_generation: 0,
        status: WorkRuntimeStatus::Unpublished,
        last_error: None,
        created_at: now,
        updated_at: now,
    }
}

fn persist_commit(
    root: &Path,
    snapshot: &mut PublicationSnapshot,
    operation: PublishOperation,
    runtime: WorkRuntimeState,
    outbox: PublicationOutboxEvent,
    scoped_key: String,
) -> Result<(), PublicationStoreError> {
    let commit = PublicationCommit {
        sequence: snapshot.sequence.saturating_add(1),
        operation,
        runtime,
        outbox,
    };
    append_commit(root, &commit)?;
    apply_commit_with_key(snapshot, commit, scoped_key)?;
    write_checkpoint(root, snapshot)?;
    Ok(())
}

fn apply_commit(
    snapshot: &mut PublicationSnapshot,
    commit: PublicationCommit,
) -> Result<(), PublicationStoreError> {
    let scoped_key = framed_hash(&[
        commit.operation.project_id.as_str(),
        commit.operation.idempotency_key_hash.as_str(),
    ]);
    apply_commit_with_key(snapshot, commit, scoped_key)
}

fn apply_commit_with_key(
    snapshot: &mut PublicationSnapshot,
    commit: PublicationCommit,
    scoped_key: String,
) -> Result<(), PublicationStoreError> {
    if commit.operation.project_id != commit.runtime.project_id
        || commit.operation.project_id != commit.outbox.project_id
        || commit.operation.id != commit.outbox.operation_id
        || commit.operation.desired_generation != commit.runtime.desired_generation
        || commit.operation.desired_generation != commit.outbox.desired_generation
    {
        return Err(PublicationStoreError::Storage(
            "publication commit linkage is invalid".to_string(),
        ));
    }
    snapshot.sequence = commit.sequence;
    snapshot
        .operation_by_idempotency
        .insert(scoped_key, commit.operation.id.clone());
    snapshot
        .operations
        .insert(commit.operation.id.clone(), commit.operation);
    snapshot
        .runtimes
        .insert(commit.runtime.project_id.clone(), commit.runtime);
    snapshot
        .outbox
        .insert(commit.outbox.id.clone(), commit.outbox);
    Ok(())
}

fn validate_snapshot(snapshot: &PublicationSnapshot) -> Result<(), PublicationStoreError> {
    if snapshot.operation_by_idempotency.len() != snapshot.operations.len() {
        return Err(PublicationStoreError::Storage(
            "publication idempotency index cardinality is invalid".to_string(),
        ));
    }
    for runtime in snapshot.runtimes.values() {
        if runtime.schema_version != WORK_RUNTIME_STATE_SCHEMA
            || runtime.observed_generation > runtime.desired_generation
            || (runtime.desired_publication == PublicationDesiredState::Published)
                != runtime.desired_release_id.is_some()
        {
            return Err(PublicationStoreError::Storage(
                "persisted work runtime state is invalid".to_string(),
            ));
        }
    }
    for (key, operation_id) in &snapshot.operation_by_idempotency {
        let operation = snapshot.operations.get(operation_id).ok_or_else(|| {
            PublicationStoreError::Storage("publication idempotency index is invalid".to_string())
        })?;
        if operation.schema_version != PUBLISH_OPERATION_SCHEMA
            || operation.idempotency_key_hash.len() != 64
            || operation.request_hash.len() != 64
        {
            return Err(PublicationStoreError::Storage(
                "persisted publication operation is invalid".to_string(),
            ));
        }
        let expected = framed_hash(&[
            operation.project_id.as_str(),
            operation.idempotency_key_hash.as_str(),
        ]);
        if key != &expected || !snapshot.runtimes.contains_key(&operation.project_id) {
            return Err(PublicationStoreError::Storage(
                "publication snapshot is inconsistent".to_string(),
            ));
        }
    }
    for operation in snapshot.operations.values() {
        let outbox = snapshot
            .outbox
            .values()
            .find(|event| event.operation_id == operation.id)
            .ok_or_else(|| {
                PublicationStoreError::Storage(
                    "publication operation is missing its outbox record".to_string(),
                )
            })?;
        if outbox.schema_version != PUBLICATION_OUTBOX_SCHEMA {
            return Err(PublicationStoreError::Storage(
                "persisted publication outbox is invalid".to_string(),
            ));
        }
        let runtime = snapshot
            .runtimes
            .get(&operation.project_id)
            .ok_or_else(|| {
                PublicationStoreError::Storage(
                    "publication operation is missing its runtime state".to_string(),
                )
            })?;
        if outbox.desired_generation != operation.desired_generation
            || runtime.desired_generation < operation.desired_generation
        {
            return Err(PublicationStoreError::Storage(
                "publication operation generation linkage is invalid".to_string(),
            ));
        }
    }
    Ok(())
}

fn append_commit(root: &Path, commit: &PublicationCommit) -> Result<(), PublicationStoreError> {
    let mut bytes = serde_json::to_vec(commit)?;
    bytes.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(JOURNAL_FILE))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_journal(root: &Path) -> Result<Vec<PublicationCommit>, PublicationStoreError> {
    let bytes = match fs::read(root.join(JOURNAL_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut commits = Vec::new();
    let chunks = bytes
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    for (index, chunk) in chunks.iter().enumerate() {
        let line = chunk.strip_suffix(b"\n").unwrap_or(chunk);
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(commit) => commits.push(commit),
            Err(_) if index + 1 == chunks.len() && !bytes.ends_with(b"\n") => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(commits)
}

fn read_checkpoint(root: &Path) -> Result<PublicationSnapshot, PublicationStoreError> {
    Ok(serde_json::from_slice(&fs::read(
        root.join(CHECKPOINT_FILE),
    )?)?)
}

fn write_checkpoint(
    root: &Path,
    snapshot: &PublicationSnapshot,
) -> Result<(), PublicationStoreError> {
    let target = root.join(CHECKPOINT_FILE);
    let temporary = root.join(format!(".{CHECKPOINT_FILE}.{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(snapshot)?)?;
    file.sync_all()?;
    fs::rename(&temporary, target)?;
    fs::File::open(root)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

#[path = "store_reconcile.rs"]
mod reconcile;
