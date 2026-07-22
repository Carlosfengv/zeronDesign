use crate::{
    draft_preview::DraftPreviewStore,
    types::sha256_hex,
    visual_contracts::{
        EditBase, EditImpactOperation, EditImpactPlan, EditImpactRisk, EditImpactScope,
        ElementBoundingBox, ElementObservation, ElementSourceCandidate, VisualViewport,
        EDIT_IMPACT_PLAN_SCHEMA, ELEMENT_OBSERVATION_SCHEMA,
    },
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

type HmacSha256 = Hmac<Sha256>;
const OBSERVATION_TTL_MINUTES: i64 = 15;

#[derive(Debug)]
pub enum EditGuardError {
    InvalidInput(String),
    NotFound(String),
    Conflict { kind: &'static str, message: String },
    Storage(String),
}

impl std::fmt::Display for EditGuardError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid edit guard input: {message}"),
            Self::NotFound(message) => {
                write!(formatter, "edit guard resource not found: {message}")
            }
            Self::Conflict { kind, message } => write!(formatter, "{kind}: {message}"),
            Self::Storage(message) => write!(formatter, "edit guard storage failure: {message}"),
        }
    }
}

impl std::error::Error for EditGuardError {}

impl From<std::io::Error> for EditGuardError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<serde_json::Error> for EditGuardError {
    fn from(error: serde_json::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct CreateElementObservation {
    pub project_id: String,
    pub session_id: String,
    pub session_epoch: u64,
    pub workspace_revision: u64,
    pub route: String,
    pub viewport: VisualViewport,
    pub dom_path: String,
    pub data_slot: Option<String>,
    pub accessible_name: Option<String>,
    pub visible_text_hash: Option<String>,
    pub bounding_box: ElementBoundingBox,
    pub source_candidates: Vec<ElementSourceCandidate>,
    pub screenshot_crop_artifact_id: String,
}

#[derive(Debug, Clone)]
pub struct CreateEditImpactPlan {
    pub observation_id: Option<String>,
    pub scope: EditImpactScope,
    pub targets: Vec<String>,
    pub operations: Vec<EditImpactOperation>,
    pub risk: EditImpactRisk,
    pub edit_base: EditBase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredPlan {
    plan: EditImpactPlan,
    observation_id: Option<String>,
    project_id: String,
    #[serde(default)]
    predecessor_run_id: Option<String>,
    created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalRecord {
    plan_hash: String,
    approved_at: chrono::DateTime<Utc>,
    consumed_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Default)]
struct State {
    observations: BTreeMap<String, ElementObservation>,
    plans: BTreeMap<String, StoredPlan>,
    approvals: BTreeMap<String, ApprovalRecord>,
}

#[derive(Debug)]
pub struct EditGuardStore {
    observation_log: PathBuf,
    plan_log: PathBuf,
    approval_log: PathBuf,
    signing_key: Vec<u8>,
    state: Mutex<State>,
}

impl EditGuardStore {
    pub fn open(runtime_storage_dir: impl AsRef<Path>) -> Result<Self, EditGuardError> {
        let root = runtime_storage_dir.as_ref().join("edit-guard");
        fs::create_dir_all(&root)?;
        let signing_key = read_or_create_signing_key(&root.join("observation-signing.key"))?;
        let observation_log = root.join("observations.jsonl");
        let plan_log = root.join("plans.jsonl");
        let approval_log = root.join("approvals.jsonl");
        let mut state = State::default();
        read_jsonl(&observation_log, |observation: ElementObservation| {
            state
                .observations
                .insert(observation.observation_id.clone(), observation);
        })?;
        read_jsonl(&plan_log, |stored: StoredPlan| {
            state.plans.insert(stored.plan.plan_hash.clone(), stored);
        })?;
        read_jsonl(&approval_log, |approval: ApprovalRecord| {
            state.approvals.insert(approval.plan_hash.clone(), approval);
        })?;
        Ok(Self {
            observation_log,
            plan_log,
            approval_log,
            signing_key,
            state: Mutex::new(state),
        })
    }

    pub fn create_observation(
        &self,
        draft_store: &DraftPreviewStore,
        request: CreateElementObservation,
    ) -> Result<ElementObservation, EditGuardError> {
        let session = validate_current_session(
            draft_store,
            &request.project_id,
            &request.session_id,
            request.session_epoch,
            request.workspace_revision,
        )?;
        if request.dom_path.trim().is_empty()
            || request.screenshot_crop_artifact_id.trim().is_empty()
        {
            return Err(EditGuardError::InvalidInput(
                "DOM path and screenshot crop are required".to_string(),
            ));
        }
        let confidence = request
            .source_candidates
            .iter()
            .map(|candidate| candidate.confidence)
            .fold(0.0_f64, f64::max);
        let mut observation = ElementObservation {
            schema_version: ELEMENT_OBSERVATION_SCHEMA.to_string(),
            observation_id: format!("element-observation-{}", random_suffix()),
            project_id: request.project_id,
            session_id: session.session_id,
            session_epoch: session.session_epoch,
            workspace_revision: session.workspace_revision,
            route: request.route,
            viewport: request.viewport,
            dom_path: request.dom_path,
            data_slot: request.data_slot,
            accessible_name: request.accessible_name,
            visible_text_hash: request.visible_text_hash,
            bounding_box: request.bounding_box,
            source_candidates: request.source_candidates,
            confidence,
            screenshot_crop_artifact_id: request.screenshot_crop_artifact_id,
            expires_at: Utc::now() + Duration::minutes(OBSERVATION_TTL_MINUTES),
            signature: String::new(),
        };
        observation.signature = self.sign_observation(&observation)?;
        observation
            .validate()
            .map_err(EditGuardError::InvalidInput)?;
        append_jsonl(&self.observation_log, &observation)?;
        self.state
            .lock()
            .unwrap()
            .observations
            .insert(observation.observation_id.clone(), observation.clone());
        Ok(observation)
    }

    pub fn get_observation(
        &self,
        draft_store: &DraftPreviewStore,
        observation_id: &str,
    ) -> Result<ElementObservation, EditGuardError> {
        let observation = self
            .state
            .lock()
            .unwrap()
            .observations
            .get(observation_id)
            .cloned()
            .ok_or_else(|| EditGuardError::NotFound(observation_id.to_string()))?;
        if observation.expires_at <= Utc::now()
            || !constant_time_eq(
                observation.signature.as_bytes(),
                self.sign_observation(&observation)?.as_bytes(),
            )
        {
            return Err(conflict(
                "element.observation_stale",
                "observation expired or signature validation failed",
            ));
        }
        validate_current_session(
            draft_store,
            &observation.project_id,
            &observation.session_id,
            observation.session_epoch,
            observation.workspace_revision,
        )?;
        Ok(observation)
    }

    pub fn create_plan(
        &self,
        draft_store: &DraftPreviewStore,
        request: CreateEditImpactPlan,
    ) -> Result<EditImpactPlan, EditGuardError> {
        let (project_id, session_id, session_epoch, workspace_revision) = match &request.edit_base {
            EditBase::Draft {
                snapshot_id,
                session_id,
                expected_session_epoch,
                expected_workspace_revision,
                writer_lease_id,
            } => {
                let session = validate_current_session(
                    draft_store,
                    "",
                    session_id,
                    *expected_session_epoch,
                    *expected_workspace_revision,
                )?;
                if session.writer_lease_id != *writer_lease_id
                    || session.durable_snapshot_id != *snapshot_id
                    || session.writer_lease_expires_at <= Utc::now()
                {
                    return Err(conflict("edit.base_stale", "Draft EditBase is stale"));
                }
                (
                    session.project_id,
                    session_id.clone(),
                    *expected_session_epoch,
                    *expected_workspace_revision,
                )
            }
            EditBase::WorkVersion { .. } => {
                return Err(EditGuardError::InvalidInput(
                    "P0-B impact plans require a Draft EditBase".to_string(),
                ));
            }
        };
        if let Some(observation_id) = request.observation_id.as_deref() {
            let observation = self.get_observation(draft_store, observation_id)?;
            if observation.project_id != project_id {
                return Err(EditGuardError::NotFound(observation_id.to_string()));
            }
        }
        let requires_confirmation =
            requires_confirmation(request.scope, request.risk, &request.operations);
        let hash_payload = serde_json::json!({
            "scope": request.scope,
            "targets": request.targets,
            "operations": request.operations,
            "risk": request.risk,
            "requiresConfirmation": requires_confirmation,
            "editBase": request.edit_base,
            "sessionId": session_id,
            "sessionEpoch": session_epoch,
            "workspaceRevision": workspace_revision,
            "observationId": request.observation_id,
        });
        let plan_hash = sha256_hex(
            serde_json::to_vec(&hash_payload)
                .map_err(EditGuardError::from)?
                .as_slice(),
        );
        let plan = EditImpactPlan {
            schema_version: EDIT_IMPACT_PLAN_SCHEMA.to_string(),
            scope: request.scope,
            targets: request.targets,
            operations: request.operations,
            risk: request.risk,
            requires_confirmation,
            edit_base: request.edit_base,
            session_id,
            session_epoch,
            workspace_revision,
            plan_hash: plan_hash.clone(),
        };
        plan.validate().map_err(EditGuardError::InvalidInput)?;
        let stored = StoredPlan {
            plan: plan.clone(),
            observation_id: request.observation_id,
            project_id,
            predecessor_run_id: None,
            created_at: Utc::now(),
        };
        let mut state = self.state.lock().unwrap();
        if let Some(existing) = state.plans.get(&plan_hash) {
            return Ok(existing.plan.clone());
        }
        append_jsonl(&self.plan_log, &stored)?;
        state.plans.insert(plan_hash, stored);
        Ok(plan)
    }

    pub fn get_plan(&self, plan_hash: &str) -> Option<(String, EditImpactPlan)> {
        self.state
            .lock()
            .unwrap()
            .plans
            .get(plan_hash)
            .map(|stored| (stored.project_id.clone(), stored.plan.clone()))
    }

    pub fn bind_replan_predecessor(
        &self,
        plan_hash: &str,
        project_id: &str,
        predecessor_run_id: &str,
    ) -> Result<(), EditGuardError> {
        if predecessor_run_id.trim().is_empty() {
            return Err(EditGuardError::InvalidInput(
                "predecessorRunId must not be empty".to_string(),
            ));
        }
        let mut state = self.state.lock().unwrap();
        let mut stored = state
            .plans
            .get(plan_hash)
            .cloned()
            .ok_or_else(|| EditGuardError::NotFound(plan_hash.to_string()))?;
        if stored.project_id != project_id {
            return Err(EditGuardError::NotFound(plan_hash.to_string()));
        }
        if let Some(existing) = stored.predecessor_run_id.as_deref() {
            if existing != predecessor_run_id {
                return Err(conflict(
                    "edit.replan_predecessor_conflict",
                    "EditImpactPlan is already bound to another predecessor Run",
                ));
            }
            return Ok(());
        }
        stored.predecessor_run_id = Some(predecessor_run_id.to_string());
        append_jsonl(&self.plan_log, &stored)?;
        state.plans.insert(plan_hash.to_string(), stored);
        Ok(())
    }

    pub fn replan_predecessor(&self, plan_hash: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .plans
            .get(plan_hash)
            .and_then(|stored| stored.predecessor_run_id.clone())
    }

    pub fn confirm(
        &self,
        draft_store: &DraftPreviewStore,
        plan_hash: &str,
    ) -> Result<EditImpactPlan, EditGuardError> {
        let (_, plan) = self
            .get_plan(plan_hash)
            .ok_or_else(|| EditGuardError::NotFound(plan_hash.to_string()))?;
        self.validate_plan_current(draft_store, &plan)?;
        if !plan.requires_confirmation {
            return Ok(plan);
        }
        let mut state = self.state.lock().unwrap();
        if state
            .approvals
            .get(plan_hash)
            .is_some_and(|approval| approval.consumed_at.is_some())
        {
            return Err(conflict("edit.plan_stale", "approval was already consumed"));
        }
        let approval = ApprovalRecord {
            plan_hash: plan_hash.to_string(),
            approved_at: Utc::now(),
            consumed_at: None,
        };
        append_jsonl(&self.approval_log, &approval)?;
        state.approvals.insert(plan_hash.to_string(), approval);
        Ok(plan)
    }

    pub fn validate_executable(
        &self,
        draft_store: &DraftPreviewStore,
        plan_hash: &str,
    ) -> Result<EditImpactPlan, EditGuardError> {
        let (_, plan) = self
            .get_plan(plan_hash)
            .ok_or_else(|| EditGuardError::NotFound(plan_hash.to_string()))?;
        self.validate_plan_current(draft_store, &plan)?;
        if plan.requires_confirmation {
            let state = self.state.lock().unwrap();
            let approval = state
                .approvals
                .get(plan_hash)
                .ok_or_else(|| conflict("edit.confirmation_required", "plan is not approved"))?;
            if approval.consumed_at.is_some() {
                return Err(conflict("edit.plan_stale", "approval was already consumed"));
            }
        }
        Ok(plan)
    }

    pub fn consume(
        &self,
        draft_store: &DraftPreviewStore,
        plan_hash: &str,
    ) -> Result<EditImpactPlan, EditGuardError> {
        let (_, plan) = self
            .get_plan(plan_hash)
            .ok_or_else(|| EditGuardError::NotFound(plan_hash.to_string()))?;
        self.validate_plan_current(draft_store, &plan)?;
        if !plan.requires_confirmation {
            return Ok(plan);
        }
        let mut state = self.state.lock().unwrap();
        let mut approval = state
            .approvals
            .get(plan_hash)
            .cloned()
            .ok_or_else(|| conflict("edit.confirmation_required", "plan is not approved"))?;
        if approval.consumed_at.is_some() {
            return Err(conflict("edit.plan_stale", "approval was already consumed"));
        }
        approval.consumed_at = Some(Utc::now());
        append_jsonl(&self.approval_log, &approval)?;
        state.approvals.insert(plan_hash.to_string(), approval);
        Ok(plan)
    }

    fn validate_plan_current(
        &self,
        draft_store: &DraftPreviewStore,
        plan: &EditImpactPlan,
    ) -> Result<(), EditGuardError> {
        let session = validate_current_session(
            draft_store,
            "",
            &plan.session_id,
            plan.session_epoch,
            plan.workspace_revision,
        )?;
        if let EditBase::Draft {
            snapshot_id,
            writer_lease_id,
            ..
        } = &plan.edit_base
        {
            if session.durable_snapshot_id != *snapshot_id
                || session.writer_lease_id != *writer_lease_id
                || session.writer_lease_expires_at <= Utc::now()
            {
                return Err(conflict("edit.plan_stale", "plan EditBase is stale"));
            }
        }
        Ok(())
    }

    fn sign_observation(&self, observation: &ElementObservation) -> Result<String, EditGuardError> {
        let mut unsigned = observation.clone();
        unsigned.signature.clear();
        let bytes = serde_json::to_vec(&unsigned)?;
        let mut mac = HmacSha256::new_from_slice(&self.signing_key)
            .map_err(|error| EditGuardError::Storage(error.to_string()))?;
        mac.update(&bytes);
        Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
    }
}

#[derive(Clone)]
pub struct EditGuardService {
    store: Arc<EditGuardStore>,
    draft_store: Arc<DraftPreviewStore>,
}

impl EditGuardService {
    pub fn new(store: Arc<EditGuardStore>, draft_store: Arc<DraftPreviewStore>) -> Self {
        Self { store, draft_store }
    }

    pub fn store(&self) -> &EditGuardStore {
        &self.store
    }

    pub fn draft_store(&self) -> &DraftPreviewStore {
        &self.draft_store
    }
}

fn validate_current_session(
    draft_store: &DraftPreviewStore,
    project_id: &str,
    session_id: &str,
    session_epoch: u64,
    workspace_revision: u64,
) -> Result<crate::visual_contracts::DraftPreviewSession, EditGuardError> {
    let session = draft_store
        .get(session_id)
        .ok_or_else(|| EditGuardError::NotFound(session_id.to_string()))?;
    if (!project_id.is_empty() && session.project_id != project_id)
        || session.session_epoch != session_epoch
        || session.workspace_revision != workspace_revision
    {
        return Err(conflict(
            "edit.base_stale",
            "session epoch or workspace revision changed",
        ));
    }
    Ok(session)
}

fn requires_confirmation(
    scope: EditImpactScope,
    risk: EditImpactRisk,
    operations: &[EditImpactOperation],
) -> bool {
    scope == EditImpactScope::Global
        || risk == EditImpactRisk::High
        || operations.iter().any(|operation| {
            matches!(
                operation,
                EditImpactOperation::Navigation
                    | EditImpactOperation::Delete
                    | EditImpactOperation::Dependency
            )
        })
}

fn conflict(kind: &'static str, message: impl Into<String>) -> EditGuardError {
    EditGuardError::Conflict {
        kind,
        message: message.into(),
    }
}

fn read_or_create_signing_key(path: &Path) -> Result<Vec<u8>, EditGuardError> {
    match fs::read(path) {
        Ok(key) if key.len() >= 32 => Ok(key),
        Ok(_) => Err(EditGuardError::Storage(
            "observation signing key is too short".to_string(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let key = rand::random::<[u8; 32]>().to_vec();
            let parent = path.parent().ok_or_else(|| {
                EditGuardError::Storage("observation signing key path has no parent".to_string())
            })?;
            let temp_path = parent.join(format!(
                ".observation-signing-key-{:016x}.tmp",
                rand::random::<u64>()
            ));
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp_path)?;
            if let Err(error) = file.write_all(&key).and_then(|_| file.sync_all()) {
                fs::remove_file(&temp_path).ok();
                return Err(error.into());
            }
            drop(file);
            match fs::hard_link(&temp_path, path) {
                Ok(()) => {
                    fs::remove_file(&temp_path).ok();
                    Ok(key)
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    fs::remove_file(&temp_path).ok();
                    let winning_key = fs::read(path)?;
                    if winning_key.len() < 32 {
                        return Err(EditGuardError::Storage(
                            "observation signing key is too short".to_string(),
                        ));
                    }
                    Ok(winning_key)
                }
                Err(error) => {
                    fs::remove_file(&temp_path).ok();
                    Err(error.into())
                }
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<(), EditGuardError> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(
    path: &Path,
    mut accept: impl FnMut(T),
) -> Result<(), EditGuardError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for line in BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            accept(serde_json::from_str(&line)?);
        }
    }
    Ok(())
}

fn random_suffix() -> String {
    sha256_hex(&rand::random::<[u8; 32]>())[..32].to_string()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft_preview::StartDraftPreview;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("edit-guard-{name}-{}", rand::random::<u64>()))
    }

    fn session(
        root: &Path,
    ) -> (
        DraftPreviewStore,
        crate::visual_contracts::DraftPreviewSession,
    ) {
        let drafts = DraftPreviewStore::open(root).unwrap();
        let session = drafts
            .start(StartDraftPreview {
                project_id: "project-1".to_string(),
                sandbox_binding_id: "binding-1".to_string(),
                template_id: "next-app".to_string(),
                base_snapshot_id: "snapshot-0".to_string(),
                base_version_id: None,
                proxy_url: "https://runtime.test/previews/lease-1/".to_string(),
                writer_ttl_seconds: 120,
            })
            .unwrap();
        (drafts, session)
    }

    fn observation_request(
        session: &crate::visual_contracts::DraftPreviewSession,
    ) -> CreateElementObservation {
        CreateElementObservation {
            project_id: session.project_id.clone(),
            session_id: session.session_id.clone(),
            session_epoch: session.session_epoch,
            workspace_revision: session.workspace_revision,
            route: "/".to_string(),
            viewport: VisualViewport {
                width: 1440,
                height: 900,
                device_scale_factor: 1.0,
            },
            dom_path: "main > section:nth-child(1) > h1".to_string(),
            data_slot: Some("hero-title".to_string()),
            accessible_name: Some("Build better".to_string()),
            visible_text_hash: Some("a".repeat(64)),
            bounding_box: ElementBoundingBox {
                x: 40.0,
                y: 80.0,
                width: 640.0,
                height: 80.0,
            },
            source_candidates: vec![ElementSourceCandidate {
                path: "app/page.tsx".to_string(),
                line: Some(20),
                column: Some(5),
                export_name: Some("Page".to_string()),
                confidence: 0.92,
            }],
            screenshot_crop_artifact_id: "artifact-crop-1".to_string(),
        }
    }

    #[test]
    fn signed_observation_survives_restart_and_becomes_stale_after_revision_change() {
        let root = root("observation");
        let (drafts, session) = session(&root);
        let store = EditGuardStore::open(&root).unwrap();
        let observation = store
            .create_observation(&drafts, observation_request(&session))
            .unwrap();
        assert_eq!(observation.confidence, 0.92);
        drop(store);
        let store = EditGuardStore::open(&root).unwrap();
        store
            .get_observation(&drafts, &observation.observation_id)
            .unwrap();
        drafts
            .commit_revision(
                &session.session_id,
                &session.writer_lease_id,
                session.session_epoch,
                0,
            )
            .unwrap();
        assert!(matches!(
            store.get_observation(&drafts, &observation.observation_id),
            Err(EditGuardError::Conflict {
                kind: "edit.base_stale",
                ..
            })
        ));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn high_impact_approval_is_bound_to_plan_hash_and_consumed_once() {
        let root = root("approval");
        let (drafts, session) = session(&root);
        let store = EditGuardStore::open(&root).unwrap();
        let plan = store
            .create_plan(
                &drafts,
                CreateEditImpactPlan {
                    observation_id: None,
                    scope: EditImpactScope::Global,
                    targets: vec!["app/layout.tsx".to_string()],
                    operations: vec![EditImpactOperation::Navigation],
                    risk: EditImpactRisk::Medium,
                    edit_base: EditBase::Draft {
                        snapshot_id: session.durable_snapshot_id.clone(),
                        session_id: session.session_id.clone(),
                        expected_session_epoch: session.session_epoch,
                        expected_workspace_revision: session.workspace_revision,
                        writer_lease_id: session.writer_lease_id.clone(),
                    },
                },
            )
            .unwrap();
        assert!(plan.requires_confirmation);
        assert!(matches!(
            store.consume(&drafts, &plan.plan_hash),
            Err(EditGuardError::Conflict {
                kind: "edit.confirmation_required",
                ..
            })
        ));
        store.confirm(&drafts, &plan.plan_hash).unwrap();
        store.consume(&drafts, &plan.plan_hash).unwrap();
        assert!(matches!(
            store.consume(&drafts, &plan.plan_hash),
            Err(EditGuardError::Conflict {
                kind: "edit.plan_stale",
                ..
            })
        ));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn replan_predecessor_binding_survives_restart_and_cannot_be_rebound() {
        let root = root("replan-binding");
        let (drafts, session) = session(&root);
        let store = EditGuardStore::open(&root).unwrap();
        let plan = store
            .create_plan(
                &drafts,
                CreateEditImpactPlan {
                    observation_id: None,
                    scope: EditImpactScope::Local,
                    targets: vec!["app/page.tsx".to_string()],
                    operations: vec![EditImpactOperation::Copy],
                    risk: EditImpactRisk::Low,
                    edit_base: EditBase::Draft {
                        snapshot_id: session.durable_snapshot_id.clone(),
                        session_id: session.session_id.clone(),
                        expected_session_epoch: session.session_epoch,
                        expected_workspace_revision: session.workspace_revision,
                        writer_lease_id: session.writer_lease_id.clone(),
                    },
                },
            )
            .unwrap();
        store
            .bind_replan_predecessor(&plan.plan_hash, "project-1", "run-predecessor-1")
            .unwrap();
        drop(store);

        let store = EditGuardStore::open(&root).unwrap();
        assert_eq!(
            store.replan_predecessor(&plan.plan_hash).as_deref(),
            Some("run-predecessor-1")
        );
        store
            .bind_replan_predecessor(&plan.plan_hash, "project-1", "run-predecessor-1")
            .unwrap();
        assert!(matches!(
            store.bind_replan_predecessor(&plan.plan_hash, "project-1", "run-predecessor-2"),
            Err(EditGuardError::Conflict {
                kind: "edit.replan_predecessor_conflict",
                ..
            })
        ));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn concurrent_store_open_uses_the_single_winning_signing_key() {
        let root = root("concurrent-key");
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let root = root.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    EditGuardStore::open(root).unwrap().signing_key
                })
            })
            .collect::<Vec<_>>();
        let keys = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert!(keys.iter().all(|key| key == &keys[0]));
        assert_eq!(keys[0].len(), 32);
        fs::remove_dir_all(root).ok();
    }
}
