use super::{
    model::validate_digest, packaging_idempotency_key, PackagingScanEvidence,
    ReleasePackagingInput, ReleasePackagingRecord, ReleasePackagingStatus, WorkRelease,
    WorkReleaseStatus,
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

const JOURNAL_FILE: &str = "release-events.jsonl";
const CHECKPOINT_FILE: &str = "release-checkpoint.json";

#[derive(Debug)]
pub enum ReleaseStoreError {
    InvalidInput(String),
    NotFound(String),
    InvalidTransition(String),
    IntegrityConflict(String),
    Storage(String),
}

impl fmt::Display for ReleaseStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid release input: {message}"),
            Self::NotFound(message) => write!(formatter, "release record not found: {message}"),
            Self::InvalidTransition(message) => {
                write!(formatter, "invalid release transition: {message}")
            }
            Self::IntegrityConflict(message) => {
                write!(formatter, "release integrity conflict: {message}")
            }
            Self::Storage(message) => write!(formatter, "release storage failure: {message}"),
        }
    }
}

impl Error for ReleaseStoreError {}

impl From<std::io::Error> for ReleaseStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<serde_json::Error> for ReleaseStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseState {
    sequence: u64,
    releases: BTreeMap<String, WorkRelease>,
    packagings: BTreeMap<String, ReleasePackagingRecord>,
    packaging_by_key: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReleaseEvent {
    sequence: u64,
    release: WorkRelease,
    packaging: ReleasePackagingRecord,
}

#[derive(Debug)]
pub struct ReleaseStore {
    root: PathBuf,
    state: Mutex<ReleaseState>,
}

impl ReleaseStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ReleaseStoreError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        let mut state = read_checkpoint(&root).unwrap_or_default();
        for event in read_journal(&root)? {
            if event.sequence > state.sequence {
                apply_event(&mut state, event)?;
            }
        }
        validate_state(&state)?;
        Ok(Self {
            root,
            state: Mutex::new(state),
        })
    }

    pub fn prepare(
        &self,
        input: &ReleasePackagingInput,
    ) -> Result<(WorkRelease, ReleasePackagingRecord), ReleaseStoreError> {
        input.validate().map_err(ReleaseStoreError::InvalidInput)?;
        let key = packaging_idempotency_key(input);
        let mut state = self.state.lock().unwrap();
        if let Some(packaging_id) = state.packaging_by_key.get(&key) {
            let packaging = state
                .packagings
                .get(packaging_id)
                .ok_or_else(|| {
                    ReleaseStoreError::Storage("idempotency index is corrupt".to_string())
                })?
                .clone();
            let release = state
                .releases
                .get(&packaging.release_id)
                .ok_or_else(|| {
                    ReleaseStoreError::Storage("packaging release link is corrupt".to_string())
                })?
                .clone();
            if release.project_id != input.project_id
                || release.version_id != input.version_id
                || release.run_id != input.run_id
                || release.template_id != input.template_id
                || release.template_version != input.template_version
                || release.source_snapshot_uri != input.source_snapshot_uri
                || release.runtime_profile_id != input.runtime_profile_id
                || packaging.registry_repository != input.registry_repository
            {
                return Err(ReleaseStoreError::IntegrityConflict(
                    "idempotency key is already bound to different release provenance".to_string(),
                ));
            }
            return Ok((release, packaging));
        }

        let now = Utc::now();
        let release = WorkRelease::packaging(input, &key, now);
        let packaging = ReleasePackagingRecord::prepared(input, &release.id, &key, now);
        self.persist_pair(&mut state, release.clone(), packaging.clone())?;
        Ok((release, packaging))
    }

    pub fn begin_build(
        &self,
        packaging_id: &str,
    ) -> Result<ReleasePackagingRecord, ReleaseStoreError> {
        self.update(packaging_id, |release, packaging| {
            match packaging.status {
                ReleasePackagingStatus::Prepared | ReleasePackagingStatus::Failed => {
                    packaging.status = ReleasePackagingStatus::Building;
                    packaging.attempts = packaging.attempts.saturating_add(1);
                    packaging.last_error = None;
                    release.status = WorkReleaseStatus::Packaging;
                }
                ReleasePackagingStatus::ReconcileRequired => {
                    packaging.status = if packaging
                        .scan_evidence
                        .as_ref()
                        .is_some_and(|evidence| evidence.passed)
                        && packaging.provenance_digest.is_some()
                    {
                        ReleasePackagingStatus::Signing
                    } else if packaging.pushed_image_digest.is_some() {
                        ReleasePackagingStatus::Pushed
                    } else {
                        packaging.attempts = packaging.attempts.saturating_add(1);
                        ReleasePackagingStatus::Building
                    };
                    packaging.last_error = None;
                    release.status = if packaging.pushed_image_digest.is_some() {
                        WorkReleaseStatus::Packaged
                    } else {
                        WorkReleaseStatus::Packaging
                    };
                }
                ReleasePackagingStatus::Building => {}
                _ => {
                    return Err(ReleaseStoreError::InvalidTransition(format!(
                        "cannot begin build from {:?}",
                        packaging.status
                    )))
                }
            }
            Ok(())
        })
        .map(|(_, packaging)| packaging)
    }

    pub fn record_built(
        &self,
        packaging_id: &str,
        image_digest: &str,
    ) -> Result<ReleasePackagingRecord, ReleaseStoreError> {
        validate_digest(image_digest, "built image digest")
            .map_err(ReleaseStoreError::InvalidInput)?;
        self.update(packaging_id, |_, packaging| {
            if packaging.status != ReleasePackagingStatus::Building {
                return Err(ReleaseStoreError::InvalidTransition(format!(
                    "cannot record build from {:?}",
                    packaging.status
                )));
            }
            set_once(
                &mut packaging.built_image_digest,
                image_digest,
                "built image digest",
            )
        })
        .map(|(_, packaging)| packaging)
    }

    pub fn record_pushed(
        &self,
        packaging_id: &str,
        registry_digest: &str,
    ) -> Result<(WorkRelease, ReleasePackagingRecord), ReleaseStoreError> {
        validate_digest(registry_digest, "registry image digest")
            .map_err(ReleaseStoreError::InvalidInput)?;
        self.update(packaging_id, |release, packaging| {
            if packaging.status != ReleasePackagingStatus::Building
                && packaging.status != ReleasePackagingStatus::Pushed
            {
                return Err(ReleaseStoreError::InvalidTransition(format!(
                    "cannot record push from {:?}",
                    packaging.status
                )));
            }
            if packaging.built_image_digest.as_deref() != Some(registry_digest) {
                return Err(ReleaseStoreError::IntegrityConflict(
                    "registry digest differs from the deterministic built digest".to_string(),
                ));
            }
            set_once(
                &mut packaging.pushed_image_digest,
                registry_digest,
                "pushed image digest",
            )?;
            packaging.status = ReleasePackagingStatus::Pushed;
            release.status = WorkReleaseStatus::Packaged;
            release.runtime_image_digest = Some(registry_digest.to_string());
            release.runtime_image_ref = Some(format!(
                "{}/{}@{}",
                packaging.registry_repository, release.id, registry_digest
            ));
            Ok(())
        })
    }

    pub fn begin_scan(
        &self,
        packaging_id: &str,
    ) -> Result<ReleasePackagingRecord, ReleaseStoreError> {
        self.update(packaging_id, |_, packaging| {
            if !matches!(
                packaging.status,
                ReleasePackagingStatus::Pushed | ReleasePackagingStatus::Scanning
            ) {
                return Err(ReleaseStoreError::InvalidTransition(format!(
                    "cannot begin scan from {:?}",
                    packaging.status
                )));
            }
            packaging.status = ReleasePackagingStatus::Scanning;
            Ok(())
        })
        .map(|(_, packaging)| packaging)
    }

    pub fn record_scan(
        &self,
        packaging_id: &str,
        sbom_digest: &str,
        provenance_digest: &str,
        evidence: PackagingScanEvidence,
    ) -> Result<(WorkRelease, ReleasePackagingRecord), ReleaseStoreError> {
        validate_digest(sbom_digest, "SBOM digest").map_err(ReleaseStoreError::InvalidInput)?;
        validate_digest(provenance_digest, "provenance digest")
            .map_err(ReleaseStoreError::InvalidInput)?;
        validate_digest(&evidence.report_digest, "scan report digest")
            .map_err(ReleaseStoreError::InvalidInput)?;
        self.update(packaging_id, move |release, packaging| {
            if packaging.status != ReleasePackagingStatus::Scanning {
                return Err(ReleaseStoreError::InvalidTransition(format!(
                    "cannot record scan from {:?}",
                    packaging.status
                )));
            }
            if evidence.policy_version != packaging.scan_policy_version {
                return Err(ReleaseStoreError::IntegrityConflict(
                    "scan evidence policy version differs from packaging policy".to_string(),
                ));
            }
            packaging.sbom_digest = Some(sbom_digest.to_string());
            packaging.provenance_digest = Some(provenance_digest.to_string());
            packaging.scan_evidence = Some(evidence.clone());
            if evidence.passed {
                packaging.status = ReleasePackagingStatus::Signing;
                packaging.last_error = None;
            } else {
                packaging.status = ReleasePackagingStatus::Failed;
                packaging.last_error = Some("release scan policy rejected image".to_string());
                release.status = WorkReleaseStatus::Failed;
            }
            Ok(())
        })
    }

    pub fn record_signature(
        &self,
        packaging_id: &str,
        signature_identity: &str,
        signature_digest: &str,
    ) -> Result<(WorkRelease, ReleasePackagingRecord), ReleaseStoreError> {
        if signature_identity.trim().is_empty() {
            return Err(ReleaseStoreError::InvalidInput(
                "signature identity must not be empty".to_string(),
            ));
        }
        validate_digest(signature_digest, "signature digest")
            .map_err(ReleaseStoreError::InvalidInput)?;
        self.update(packaging_id, |release, packaging| {
            if packaging.status == ReleasePackagingStatus::Validated {
                if packaging.signature_identity.as_deref() == Some(signature_identity)
                    && packaging.signature_digest.as_deref() == Some(signature_digest)
                {
                    return Ok(());
                }
                return Err(ReleaseStoreError::IntegrityConflict(
                    "validated release signature cannot change".to_string(),
                ));
            }
            if packaging.status != ReleasePackagingStatus::Signing
                || !packaging
                    .scan_evidence
                    .as_ref()
                    .is_some_and(|evidence| evidence.passed)
                || packaging.pushed_image_digest.is_none()
                || packaging.sbom_digest.is_none()
                || packaging.provenance_digest.is_none()
            {
                return Err(ReleaseStoreError::InvalidTransition(
                    "scan, SBOM, provenance, and push evidence are required before signing"
                        .to_string(),
                ));
            }
            packaging.signature_identity = Some(signature_identity.to_string());
            packaging.signature_digest = Some(signature_digest.to_string());
            packaging.status = ReleasePackagingStatus::Validated;
            packaging.last_error = None;
            release.status = WorkReleaseStatus::Validated;
            Ok(())
        })
    }

    pub fn mark_reconcile_required(
        &self,
        packaging_id: &str,
        error: impl Into<String>,
    ) -> Result<ReleasePackagingRecord, ReleaseStoreError> {
        let error = error.into();
        self.update(packaging_id, |_, packaging| {
            if packaging.status == ReleasePackagingStatus::Validated {
                return Err(ReleaseStoreError::InvalidTransition(
                    "validated packaging cannot require reconciliation".to_string(),
                ));
            }
            packaging.status = ReleasePackagingStatus::ReconcileRequired;
            packaging.last_error = Some(error.clone());
            Ok(())
        })
        .map(|(_, packaging)| packaging)
    }

    pub fn mark_failed_garbage_collectable(
        &self,
        packaging_id: &str,
        expected_image_digest: &str,
    ) -> Result<(WorkRelease, ReleasePackagingRecord), ReleaseStoreError> {
        validate_digest(expected_image_digest, "garbage collection image digest")
            .map_err(ReleaseStoreError::InvalidInput)?;
        self.update(packaging_id, |release, packaging| {
            if release.status == WorkReleaseStatus::GarbageCollectable
                && packaging.pushed_image_digest.as_deref() == Some(expected_image_digest)
            {
                return Ok(());
            }
            if release.status != WorkReleaseStatus::Failed
                || packaging.status != ReleasePackagingStatus::Failed
                || packaging.pushed_image_digest.as_deref() != Some(expected_image_digest)
            {
                return Err(ReleaseStoreError::InvalidTransition(
                    "only a failed, digest-matched release may become garbage collectable"
                        .to_string(),
                ));
            }
            release.status = WorkReleaseStatus::GarbageCollectable;
            Ok(())
        })
    }

    pub fn record_garbage_collected(
        &self,
        packaging_id: &str,
        expected_image_digest: &str,
    ) -> Result<(WorkRelease, ReleasePackagingRecord), ReleaseStoreError> {
        validate_digest(expected_image_digest, "garbage collection image digest")
            .map_err(ReleaseStoreError::InvalidInput)?;
        self.update(packaging_id, |release, packaging| {
            if release.status == WorkReleaseStatus::GarbageCollected
                && packaging.pushed_image_digest.as_deref() == Some(expected_image_digest)
            {
                return Ok(());
            }
            if release.status != WorkReleaseStatus::GarbageCollectable
                || packaging.status != ReleasePackagingStatus::Failed
                || packaging.pushed_image_digest.as_deref() != Some(expected_image_digest)
            {
                return Err(ReleaseStoreError::InvalidTransition(
                    "garbage collection completion requires a failed authorized digest".to_string(),
                ));
            }
            release.status = WorkReleaseStatus::GarbageCollected;
            Ok(())
        })
    }

    pub fn release(&self, release_id: &str) -> Option<WorkRelease> {
        self.state.lock().unwrap().releases.get(release_id).cloned()
    }

    pub fn releases_for_project(&self, project_id: &str) -> Vec<WorkRelease> {
        self.state
            .lock()
            .unwrap()
            .releases
            .values()
            .filter(|release| release.project_id == project_id)
            .cloned()
            .collect()
    }

    pub fn packaging(&self, packaging_id: &str) -> Option<ReleasePackagingRecord> {
        self.state
            .lock()
            .unwrap()
            .packagings
            .get(packaging_id)
            .cloned()
    }

    pub fn packaging_for_release(&self, release_id: &str) -> Option<ReleasePackagingRecord> {
        self.state
            .lock()
            .unwrap()
            .packagings
            .values()
            .find(|packaging| packaging.release_id == release_id)
            .cloned()
    }

    pub fn recoverable_packagings(&self) -> Vec<ReleasePackagingRecord> {
        self.state
            .lock()
            .unwrap()
            .packagings
            .values()
            .filter(|packaging| {
                !matches!(
                    packaging.status,
                    ReleasePackagingStatus::Validated | ReleasePackagingStatus::Failed
                )
            })
            .cloned()
            .collect()
    }

    fn update<F>(
        &self,
        packaging_id: &str,
        update: F,
    ) -> Result<(WorkRelease, ReleasePackagingRecord), ReleaseStoreError>
    where
        F: FnOnce(&mut WorkRelease, &mut ReleasePackagingRecord) -> Result<(), ReleaseStoreError>,
    {
        let mut state = self.state.lock().unwrap();
        let mut packaging = state
            .packagings
            .get(packaging_id)
            .cloned()
            .ok_or_else(|| ReleaseStoreError::NotFound(packaging_id.to_string()))?;
        let mut release = state
            .releases
            .get(&packaging.release_id)
            .cloned()
            .ok_or_else(|| ReleaseStoreError::NotFound(packaging.release_id.clone()))?;
        update(&mut release, &mut packaging)?;
        let now = Utc::now();
        release.updated_at = now;
        packaging.updated_at = now;
        self.persist_pair(&mut state, release.clone(), packaging.clone())?;
        Ok((release, packaging))
    }

    fn persist_pair(
        &self,
        state: &mut ReleaseState,
        release: WorkRelease,
        packaging: ReleasePackagingRecord,
    ) -> Result<(), ReleaseStoreError> {
        let event = ReleaseEvent {
            sequence: state.sequence.saturating_add(1),
            release,
            packaging,
        };
        append_event(&self.root, &event)?;
        apply_event(state, event)?;
        write_checkpoint(&self.root, state)?;
        Ok(())
    }
}

fn set_once(
    target: &mut Option<String>,
    value: &str,
    field: &str,
) -> Result<(), ReleaseStoreError> {
    if let Some(existing) = target.as_deref() {
        if existing != value {
            return Err(ReleaseStoreError::IntegrityConflict(format!(
                "{field} cannot change"
            )));
        }
    } else {
        *target = Some(value.to_string());
    }
    Ok(())
}

fn apply_event(state: &mut ReleaseState, event: ReleaseEvent) -> Result<(), ReleaseStoreError> {
    if event.release.id != event.packaging.release_id
        || event.release.project_id != event.packaging.project_id
    {
        return Err(ReleaseStoreError::Storage(
            "release event linkage is invalid".to_string(),
        ));
    }
    state.sequence = event.sequence;
    state.packaging_by_key.insert(
        event.packaging.idempotency_key.clone(),
        event.packaging.id.clone(),
    );
    state
        .releases
        .insert(event.release.id.clone(), event.release);
    state
        .packagings
        .insert(event.packaging.id.clone(), event.packaging);
    Ok(())
}

fn validate_state(state: &ReleaseState) -> Result<(), ReleaseStoreError> {
    for (key, packaging_id) in &state.packaging_by_key {
        let packaging = state.packagings.get(packaging_id).ok_or_else(|| {
            ReleaseStoreError::Storage("packaging key index is invalid".to_string())
        })?;
        if &packaging.idempotency_key != key || !state.releases.contains_key(&packaging.release_id)
        {
            return Err(ReleaseStoreError::Storage(
                "persisted release state is inconsistent".to_string(),
            ));
        }
    }
    Ok(())
}

fn append_event(root: &Path, event: &ReleaseEvent) -> Result<(), ReleaseStoreError> {
    let mut bytes = serde_json::to_vec(event)?;
    bytes.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(JOURNAL_FILE))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_journal(root: &Path) -> Result<Vec<ReleaseEvent>, ReleaseStoreError> {
    let path = root.join(JOURNAL_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut events = Vec::new();
    let chunks = bytes
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    for (index, chunk) in chunks.iter().enumerate() {
        let line = chunk.strip_suffix(b"\n").unwrap_or(chunk);
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(event) => events.push(event),
            Err(_) if index + 1 == chunks.len() && !bytes.ends_with(b"\n") => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(events)
}

fn read_checkpoint(root: &Path) -> Result<ReleaseState, ReleaseStoreError> {
    Ok(serde_json::from_slice(&fs::read(
        root.join(CHECKPOINT_FILE),
    )?)?)
}

fn write_checkpoint(root: &Path, state: &ReleaseState) -> Result<(), ReleaseStoreError> {
    let target = root.join(CHECKPOINT_FILE);
    let temporary = root.join(format!(".{CHECKPOINT_FILE}.{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(state)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, &target)?;
    fs::File::open(root)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
