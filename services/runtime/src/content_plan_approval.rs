use crate::types::sha256_hex;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const CONTENT_PLAN_APPROVAL_SCHEMA: &str = "content-plan-approval@1";
pub const CONTENT_PLAN_APPROVAL_TRANSACTION_SCHEMA: &str = "content-plan-approval-transaction@1";
pub const CONTENT_PLAN_APPROVAL_PRODUCER_SCHEMA: &str = "content-plan-approval-producer@1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPlanApprovalDecision {
    Approved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentPlanApproval {
    pub schema_version: String,
    pub approval_id: String,
    pub project_id: String,
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub decision: ContentPlanApprovalDecision,
    pub confirmation_event_id: String,
    pub approved_at: DateTime<Utc>,
    pub invalidated_at: Option<DateTime<Utc>>,
    pub invalidation_reason: Option<String>,
}

impl ContentPlanApproval {
    pub fn validate(&self) -> Result<(), ContentPlanApprovalError> {
        if self.schema_version != CONTENT_PLAN_APPROVAL_SCHEMA {
            return Err(ContentPlanApprovalError::InvalidInput(format!(
                "schemaVersion must be {CONTENT_PLAN_APPROVAL_SCHEMA}"
            )));
        }
        require_identifier("approvalId", &self.approval_id)?;
        require_identifier("projectId", &self.project_id)?;
        require_identifier("planId", &self.plan_id)?;
        require_identifier("confirmationEventId", &self.confirmation_event_id)?;
        if self.revision == 0 {
            return Err(ContentPlanApprovalError::InvalidInput(
                "revision must be positive".to_string(),
            ));
        }
        validate_sha256_hex("contentHash", &self.content_hash)?;
        if self.invalidated_at.is_some() != self.invalidation_reason.is_some() {
            return Err(ContentPlanApprovalError::InvalidInput(
                "invalidatedAt and invalidationReason must be set together".to_string(),
            ));
        }
        if self
            .invalidation_reason
            .as_deref()
            .is_some_and(|reason| reason.trim().is_empty())
        {
            return Err(ContentPlanApprovalError::InvalidInput(
                "invalidationReason must not be empty".to_string(),
            ));
        }
        Ok(())
    }

    pub fn is_verified(&self) -> bool {
        self.decision == ContentPlanApprovalDecision::Approved && self.invalidated_at.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPlanApprovalVerificationState {
    Verified,
    Missing,
    Invalidated,
    IdentityMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentPlanApprovalVerification {
    pub state: ContentPlanApprovalVerificationState,
    pub approval: Option<ContentPlanApproval>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentPlanApprovalProducerStatus {
    pub ready: bool,
    pub schema_version: String,
    pub transaction_schema_version: String,
    pub last_sequence: u64,
}

#[derive(Debug, Clone)]
pub struct RecordContentPlanApproval {
    pub project_id: String,
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub confirmation_event_id: String,
}

#[derive(Debug, Clone)]
pub struct RecordContentPlanChange {
    pub project_id: String,
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub change_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentPlanChangeResult {
    pub project_id: String,
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub invalidated_approval_ids: Vec<String>,
    pub sequence: u64,
}

#[derive(Debug)]
pub enum ContentPlanApprovalError {
    InvalidInput(String),
    NotFound(String),
    Conflict { kind: &'static str, message: String },
    Storage(String),
}

impl std::fmt::Display for ContentPlanApprovalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) => {
                write!(formatter, "invalid ContentPlanApproval input: {message}")
            }
            Self::NotFound(message) => {
                write!(formatter, "ContentPlanApproval not found: {message}")
            }
            Self::Conflict { kind, message } => write!(formatter, "{kind}: {message}"),
            Self::Storage(message) => {
                write!(formatter, "ContentPlanApproval storage failure: {message}")
            }
        }
    }
}

impl std::error::Error for ContentPlanApprovalError {}

impl From<std::io::Error> for ContentPlanApprovalError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<serde_json::Error> for ContentPlanApprovalError {
    fn from(error: serde_json::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentPlanIdentity {
    revision: u64,
    content_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ContentPlanApprovalTransactionKind {
    ApprovalRecorded,
    PlanChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalInvalidation {
    approval_id: String,
    invalidated_at: DateTime<Utc>,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentPlanApprovalTransaction {
    schema_version: String,
    sequence: u64,
    kind: ContentPlanApprovalTransactionKind,
    project_id: String,
    plan_id: String,
    plan_identity: ContentPlanIdentity,
    source_event_id: String,
    occurred_at: DateTime<Utc>,
    approval: Option<ContentPlanApproval>,
    invalidations: Vec<ApprovalInvalidation>,
}

#[derive(Debug, Default)]
struct State {
    approvals: BTreeMap<String, ContentPlanApproval>,
    confirmation_events: BTreeMap<String, String>,
    latest_plan_identities: BTreeMap<(String, String), ContentPlanIdentity>,
    sequence: u64,
}

#[derive(Debug)]
pub struct ContentPlanApprovalStore {
    transaction_log: PathBuf,
    state: Mutex<State>,
}

impl ContentPlanApprovalStore {
    pub fn open(runtime_storage_dir: impl AsRef<Path>) -> Result<Self, ContentPlanApprovalError> {
        let root = runtime_storage_dir.as_ref().join("content-plan-approvals");
        fs::create_dir_all(&root)?;
        let transaction_log = root.join("transactions.jsonl");
        let mut state = State::default();
        read_jsonl(&transaction_log, |transaction| {
            apply_transaction(&mut state, transaction)
        })?;
        Ok(Self {
            transaction_log,
            state: Mutex::new(state),
        })
    }

    pub fn producer_status(&self) -> ContentPlanApprovalProducerStatus {
        ContentPlanApprovalProducerStatus {
            ready: true,
            schema_version: CONTENT_PLAN_APPROVAL_PRODUCER_SCHEMA.to_string(),
            transaction_schema_version: CONTENT_PLAN_APPROVAL_TRANSACTION_SCHEMA.to_string(),
            last_sequence: self.state.lock().unwrap().sequence,
        }
    }

    pub fn record_plan_change(
        &self,
        request: RecordContentPlanChange,
    ) -> Result<ContentPlanChangeResult, ContentPlanApprovalError> {
        validate_plan_identity(
            &request.project_id,
            &request.plan_id,
            request.revision,
            &request.content_hash,
        )?;
        require_identifier("changeEventId", &request.change_event_id)?;

        let mut state = self.state.lock().unwrap();
        let key = (request.project_id.clone(), request.plan_id.clone());
        let next_identity = ContentPlanIdentity {
            revision: request.revision,
            content_hash: request.content_hash.clone(),
        };
        if let Some(current) = state.latest_plan_identities.get(&key) {
            validate_identity_advance(current, &next_identity)?;
            if current == &next_identity {
                return Ok(ContentPlanChangeResult {
                    project_id: request.project_id,
                    plan_id: request.plan_id,
                    revision: request.revision,
                    content_hash: request.content_hash,
                    invalidated_approval_ids: Vec::new(),
                    sequence: state.sequence,
                });
            }
        }

        let occurred_at = Utc::now();
        let invalidations =
            active_approvals_for_plan(&state, &request.project_id, &request.plan_id)
                .into_iter()
                .filter(|approval| {
                    approval.revision != request.revision
                        || approval.content_hash != request.content_hash
                })
                .map(|approval| ApprovalInvalidation {
                    approval_id: approval.approval_id,
                    invalidated_at: occurred_at,
                    reason: "plan_changed".to_string(),
                })
                .collect::<Vec<_>>();
        let sequence = state.sequence.saturating_add(1);
        let transaction = ContentPlanApprovalTransaction {
            schema_version: CONTENT_PLAN_APPROVAL_TRANSACTION_SCHEMA.to_string(),
            sequence,
            kind: ContentPlanApprovalTransactionKind::PlanChanged,
            project_id: request.project_id.clone(),
            plan_id: request.plan_id.clone(),
            plan_identity: next_identity,
            source_event_id: request.change_event_id,
            occurred_at,
            approval: None,
            invalidations,
        };
        append_jsonl(&self.transaction_log, &transaction)?;
        let invalidated_approval_ids = transaction
            .invalidations
            .iter()
            .map(|invalidation| invalidation.approval_id.clone())
            .collect();
        apply_transaction(&mut state, transaction)?;
        Ok(ContentPlanChangeResult {
            project_id: request.project_id,
            plan_id: request.plan_id,
            revision: request.revision,
            content_hash: request.content_hash,
            invalidated_approval_ids,
            sequence,
        })
    }

    pub fn approve(
        &self,
        request: RecordContentPlanApproval,
    ) -> Result<ContentPlanApproval, ContentPlanApprovalError> {
        validate_plan_identity(
            &request.project_id,
            &request.plan_id,
            request.revision,
            &request.content_hash,
        )?;
        require_identifier("confirmationEventId", &request.confirmation_event_id)?;

        let mut state = self.state.lock().unwrap();
        if let Some(approval_id) = state
            .confirmation_events
            .get(&request.confirmation_event_id)
        {
            let approval = state.approvals.get(approval_id).cloned().ok_or_else(|| {
                ContentPlanApprovalError::Storage(format!(
                    "confirmation event {} references missing approval {approval_id}",
                    request.confirmation_event_id
                ))
            })?;
            if approval.project_id == request.project_id
                && approval.plan_id == request.plan_id
                && approval.revision == request.revision
                && approval.content_hash == request.content_hash
                && approval.is_verified()
            {
                return Ok(approval);
            }
            if approval.project_id == request.project_id
                && approval.plan_id == request.plan_id
                && approval.revision == request.revision
                && approval.content_hash == request.content_hash
            {
                return Err(conflict(
                    "content_plan.confirmation_event_stale",
                    "confirmation event references an approval that has been invalidated",
                ));
            }
            return Err(conflict(
                "content_plan.confirmation_event_reused",
                "confirmation event is already bound to a different Content Plan identity",
            ));
        }

        let key = (request.project_id.clone(), request.plan_id.clone());
        let identity = ContentPlanIdentity {
            revision: request.revision,
            content_hash: request.content_hash.clone(),
        };
        if let Some(current) = state.latest_plan_identities.get(&key) {
            if current != &identity {
                return Err(conflict(
                    "content_plan.approval_identity_stale",
                    "approval does not match the latest recorded Content Plan revision and hash",
                ));
            }
        }
        if active_approvals_for_plan(&state, &request.project_id, &request.plan_id)
            .into_iter()
            .any(|approval| {
                approval.revision == request.revision
                    && approval.content_hash == request.content_hash
            })
        {
            return Err(conflict(
                "content_plan.already_approved",
                "Content Plan identity is already approved by a different confirmation event",
            ));
        }

        let occurred_at = Utc::now();
        let invalidations =
            active_approvals_for_plan(&state, &request.project_id, &request.plan_id)
                .into_iter()
                .map(|approval| ApprovalInvalidation {
                    approval_id: approval.approval_id,
                    invalidated_at: occurred_at,
                    reason: "reapproved".to_string(),
                })
                .collect::<Vec<_>>();
        let approval = ContentPlanApproval {
            schema_version: CONTENT_PLAN_APPROVAL_SCHEMA.to_string(),
            approval_id: format!("content-plan-approval-{}", random_suffix()),
            project_id: request.project_id.clone(),
            plan_id: request.plan_id.clone(),
            revision: request.revision,
            content_hash: request.content_hash,
            decision: ContentPlanApprovalDecision::Approved,
            confirmation_event_id: request.confirmation_event_id.clone(),
            approved_at: occurred_at,
            invalidated_at: None,
            invalidation_reason: None,
        };
        approval.validate()?;
        let sequence = state.sequence.saturating_add(1);
        let transaction = ContentPlanApprovalTransaction {
            schema_version: CONTENT_PLAN_APPROVAL_TRANSACTION_SCHEMA.to_string(),
            sequence,
            kind: ContentPlanApprovalTransactionKind::ApprovalRecorded,
            project_id: request.project_id,
            plan_id: request.plan_id,
            plan_identity: identity,
            source_event_id: request.confirmation_event_id,
            occurred_at,
            approval: Some(approval.clone()),
            invalidations,
        };
        append_jsonl(&self.transaction_log, &transaction)?;
        apply_transaction(&mut state, transaction)?;
        Ok(approval)
    }

    pub fn verify_exact(
        &self,
        project_id: &str,
        plan_id: &str,
        revision: u64,
        content_hash: &str,
    ) -> Result<ContentPlanApprovalVerification, ContentPlanApprovalError> {
        validate_plan_identity(project_id, plan_id, revision, content_hash)?;
        let state = self.state.lock().unwrap();
        if let Some(approval) = state.approvals.values().rev().find(|approval| {
            approval.project_id == project_id
                && approval.plan_id == plan_id
                && approval.revision == revision
                && approval.content_hash == content_hash
        }) {
            return Ok(if approval.is_verified() {
                ContentPlanApprovalVerification {
                    state: ContentPlanApprovalVerificationState::Verified,
                    approval: Some(approval.clone()),
                    reason: None,
                }
            } else {
                ContentPlanApprovalVerification {
                    state: ContentPlanApprovalVerificationState::Invalidated,
                    approval: Some(approval.clone()),
                    reason: approval.invalidation_reason.clone(),
                }
            });
        }
        if let Some(active) = active_approvals_for_plan(&state, project_id, plan_id)
            .into_iter()
            .next()
        {
            return Ok(ContentPlanApprovalVerification {
                state: ContentPlanApprovalVerificationState::IdentityMismatch,
                approval: Some(active),
                reason: Some(
                    "an active approval exists for a different revision or content hash"
                        .to_string(),
                ),
            });
        }
        Ok(ContentPlanApprovalVerification {
            state: ContentPlanApprovalVerificationState::Missing,
            approval: None,
            reason: Some("no approval exists for the requested Content Plan identity".to_string()),
        })
    }

    pub fn get(&self, approval_id: &str) -> Option<ContentPlanApproval> {
        self.state
            .lock()
            .unwrap()
            .approvals
            .get(approval_id)
            .cloned()
    }

    pub fn list_project(&self, project_id: &str) -> Vec<ContentPlanApproval> {
        self.state
            .lock()
            .unwrap()
            .approvals
            .values()
            .filter(|approval| approval.project_id == project_id)
            .cloned()
            .collect()
    }
}

fn active_approvals_for_plan(
    state: &State,
    project_id: &str,
    plan_id: &str,
) -> Vec<ContentPlanApproval> {
    state
        .approvals
        .values()
        .filter(|approval| {
            approval.project_id == project_id
                && approval.plan_id == plan_id
                && approval.is_verified()
        })
        .cloned()
        .collect()
}

fn validate_plan_identity(
    project_id: &str,
    plan_id: &str,
    revision: u64,
    content_hash: &str,
) -> Result<(), ContentPlanApprovalError> {
    require_identifier("projectId", project_id)?;
    require_identifier("planId", plan_id)?;
    if revision == 0 {
        return Err(ContentPlanApprovalError::InvalidInput(
            "revision must be positive".to_string(),
        ));
    }
    validate_sha256_hex("contentHash", content_hash)
}

fn validate_identity_advance(
    current: &ContentPlanIdentity,
    next: &ContentPlanIdentity,
) -> Result<(), ContentPlanApprovalError> {
    if next.revision < current.revision {
        return Err(conflict(
            "content_plan.revision_stale",
            "Content Plan revision cannot move backwards",
        ));
    }
    if next.revision == current.revision && next.content_hash != current.content_hash {
        return Err(conflict(
            "content_plan.revision_hash_conflict",
            "the same Content Plan revision cannot be recorded with a different content hash",
        ));
    }
    Ok(())
}

fn require_identifier(field: &str, value: &str) -> Result<(), ContentPlanApprovalError> {
    if value.trim().is_empty() {
        return Err(ContentPlanApprovalError::InvalidInput(format!(
            "{field} is required"
        )));
    }
    Ok(())
}

fn validate_sha256_hex(field: &str, value: &str) -> Result<(), ContentPlanApprovalError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ContentPlanApprovalError::InvalidInput(format!(
            "{field} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn apply_transaction(
    state: &mut State,
    transaction: ContentPlanApprovalTransaction,
) -> Result<(), ContentPlanApprovalError> {
    if transaction.schema_version != CONTENT_PLAN_APPROVAL_TRANSACTION_SCHEMA {
        return Err(ContentPlanApprovalError::Storage(format!(
            "unsupported transaction schema: {}",
            transaction.schema_version
        )));
    }
    let expected_sequence = state.sequence.saturating_add(1);
    if transaction.sequence != expected_sequence {
        return Err(ContentPlanApprovalError::Storage(format!(
            "transaction sequence gap: expected {expected_sequence}, got {}",
            transaction.sequence
        )));
    }
    validate_plan_identity(
        &transaction.project_id,
        &transaction.plan_id,
        transaction.plan_identity.revision,
        &transaction.plan_identity.content_hash,
    )?;
    for invalidation in &transaction.invalidations {
        let approval = state
            .approvals
            .get_mut(&invalidation.approval_id)
            .ok_or_else(|| {
                ContentPlanApprovalError::Storage(format!(
                    "transaction invalidates missing approval {}",
                    invalidation.approval_id
                ))
            })?;
        approval.invalidated_at = Some(invalidation.invalidated_at);
        approval.invalidation_reason = Some(invalidation.reason.clone());
    }
    if let Some(approval) = transaction.approval {
        approval.validate()?;
        if approval.project_id != transaction.project_id
            || approval.plan_id != transaction.plan_id
            || approval.revision != transaction.plan_identity.revision
            || approval.content_hash != transaction.plan_identity.content_hash
            || approval.confirmation_event_id != transaction.source_event_id
        {
            return Err(ContentPlanApprovalError::Storage(
                "approval identity does not match its transaction".to_string(),
            ));
        }
        state.confirmation_events.insert(
            approval.confirmation_event_id.clone(),
            approval.approval_id.clone(),
        );
        state
            .approvals
            .insert(approval.approval_id.clone(), approval);
    }
    state.latest_plan_identities.insert(
        (transaction.project_id, transaction.plan_id),
        transaction.plan_identity,
    );
    state.sequence = transaction.sequence;
    Ok(())
}

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<(), ContentPlanApprovalError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(
    path: &Path,
    mut accept: impl FnMut(T) -> Result<(), ContentPlanApprovalError>,
) -> Result<(), ContentPlanApprovalError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for line in BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            accept(serde_json::from_str(&line)?)?;
        }
    }
    Ok(())
}

fn conflict(kind: &'static str, message: impl Into<String>) -> ContentPlanApprovalError {
    ContentPlanApprovalError::Conflict {
        kind,
        message: message.into(),
    }
}

fn random_suffix() -> String {
    sha256_hex(&rand::random::<[u8; 32]>())[..32].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "content-plan-approval-{name}-{}",
            rand::random::<u64>()
        ))
    }

    fn hash(character: char) -> String {
        std::iter::repeat(character).take(64).collect()
    }

    fn approval_request(revision: u64, content_hash: String) -> RecordContentPlanApproval {
        RecordContentPlanApproval {
            project_id: "project-1".to_string(),
            plan_id: "plan-1".to_string(),
            revision,
            content_hash,
            confirmation_event_id: format!("confirmation-{revision}"),
        }
    }

    #[test]
    fn approval_is_verified_only_for_exact_active_identity() {
        let root = root("exact");
        let store = ContentPlanApprovalStore::open(&root).unwrap();
        let approval = store.approve(approval_request(1, hash('a'))).unwrap();

        assert!(approval.is_verified());
        assert_eq!(
            store
                .verify_exact("project-1", "plan-1", 1, &hash('a'))
                .unwrap()
                .state,
            ContentPlanApprovalVerificationState::Verified
        );
        assert_eq!(
            store
                .verify_exact("project-1", "plan-1", 2, &hash('b'))
                .unwrap()
                .state,
            ContentPlanApprovalVerificationState::IdentityMismatch
        );
    }

    #[test]
    fn plan_change_invalidates_old_approval_and_rejects_stale_confirmation() {
        let root = root("invalidate");
        let store = ContentPlanApprovalStore::open(&root).unwrap();
        let first = store.approve(approval_request(1, hash('a'))).unwrap();

        let change = store
            .record_plan_change(RecordContentPlanChange {
                project_id: "project-1".to_string(),
                plan_id: "plan-1".to_string(),
                revision: 2,
                content_hash: hash('b'),
                change_event_id: "plan-change-2".to_string(),
            })
            .unwrap();

        assert_eq!(change.invalidated_approval_ids, vec![first.approval_id]);
        assert_eq!(
            store
                .verify_exact("project-1", "plan-1", 1, &hash('a'))
                .unwrap()
                .state,
            ContentPlanApprovalVerificationState::Invalidated
        );
        let error = store.approve(approval_request(1, hash('a'))).unwrap_err();
        assert!(matches!(
            error,
            ContentPlanApprovalError::Conflict {
                kind: "content_plan.confirmation_event_stale",
                ..
            }
        ));
        let error = store
            .approve(RecordContentPlanApproval {
                confirmation_event_id: "late-confirmation-1".to_string(),
                ..approval_request(1, hash('a'))
            })
            .unwrap_err();
        assert!(matches!(
            error,
            ContentPlanApprovalError::Conflict {
                kind: "content_plan.approval_identity_stale",
                ..
            }
        ));

        let second = store.approve(approval_request(2, hash('b'))).unwrap();
        assert!(second.is_verified());
    }

    #[test]
    fn producer_events_are_idempotent_and_revision_hash_conflicts_fail_closed() {
        let root = root("idempotent");
        let store = ContentPlanApprovalStore::open(&root).unwrap();
        let first = store.approve(approval_request(1, hash('a'))).unwrap();
        let duplicate = store.approve(approval_request(1, hash('a'))).unwrap();
        assert_eq!(duplicate.approval_id, first.approval_id);

        let error = store
            .record_plan_change(RecordContentPlanChange {
                project_id: "project-1".to_string(),
                plan_id: "plan-1".to_string(),
                revision: 1,
                content_hash: hash('b'),
                change_event_id: "bad-change".to_string(),
            })
            .unwrap_err();
        assert!(matches!(
            error,
            ContentPlanApprovalError::Conflict {
                kind: "content_plan.revision_hash_conflict",
                ..
            }
        ));
    }

    #[test]
    fn append_only_transactions_rehydrate_authoritative_state() {
        let root = root("rehydrate");
        let approval_id = {
            let store = ContentPlanApprovalStore::open(&root).unwrap();
            let approval = store.approve(approval_request(1, hash('a'))).unwrap();
            store
                .record_plan_change(RecordContentPlanChange {
                    project_id: "project-1".to_string(),
                    plan_id: "plan-1".to_string(),
                    revision: 2,
                    content_hash: hash('b'),
                    change_event_id: "plan-change-2".to_string(),
                })
                .unwrap();
            approval.approval_id
        };

        let reopened = ContentPlanApprovalStore::open(&root).unwrap();
        let approval = reopened.get(&approval_id).unwrap();
        assert_eq!(
            approval.invalidation_reason.as_deref(),
            Some("plan_changed")
        );
        assert_eq!(reopened.producer_status().last_sequence, 2);
    }

    #[test]
    fn concurrent_plan_updates_keep_revision_monotonic_and_fail_closed() {
        let root = root("concurrent");
        let store = std::sync::Arc::new(ContentPlanApprovalStore::open(&root).unwrap());
        store.approve(approval_request(1, hash('a'))).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let update = |revision,
                      character,
                      store: std::sync::Arc<ContentPlanApprovalStore>,
                      barrier: std::sync::Arc<std::sync::Barrier>| {
            std::thread::spawn(move || {
                barrier.wait();
                store.record_plan_change(RecordContentPlanChange {
                    project_id: "project-1".to_string(),
                    plan_id: "plan-1".to_string(),
                    revision,
                    content_hash: hash(character),
                    change_event_id: format!("change-{revision}"),
                })
            })
        };
        let revision_two = update(2, 'b', store.clone(), barrier.clone());
        let revision_three = update(3, 'c', store.clone(), barrier.clone());
        barrier.wait();
        let two = revision_two.join().unwrap();
        let three = revision_three.join().unwrap();

        assert!(three.is_ok());
        assert!(
            two.is_ok()
                || matches!(
                    two,
                    Err(ContentPlanApprovalError::Conflict {
                        kind: "content_plan.revision_stale",
                        ..
                    })
                )
        );
        assert_eq!(
            store
                .verify_exact("project-1", "plan-1", 1, &hash('a'))
                .unwrap()
                .state,
            ContentPlanApprovalVerificationState::Invalidated
        );
        let latest = store
            .approve(RecordContentPlanApproval {
                project_id: "project-1".to_string(),
                plan_id: "plan-1".to_string(),
                revision: 3,
                content_hash: hash('c'),
                confirmation_event_id: "confirmation-3".to_string(),
            })
            .unwrap();
        assert!(latest.is_verified());
        assert!(matches!(
            store.approve(RecordContentPlanApproval {
                project_id: "project-1".to_string(),
                plan_id: "plan-1".to_string(),
                revision: 2,
                content_hash: hash('b'),
                confirmation_event_id: "late-confirmation-2".to_string(),
            }),
            Err(ContentPlanApprovalError::Conflict {
                kind: "content_plan.approval_identity_stale",
                ..
            })
        ));
    }
}
