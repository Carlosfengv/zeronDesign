use crate::{
    types::sha256_hex,
    visual_contracts::{
        DraftPreviewEvent, DraftPreviewSession, DraftPreviewSessionStatus,
        DRAFT_PREVIEW_SESSION_SCHEMA,
    },
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::Mutex,
};
use tokio::sync::broadcast;

const EVENT_CHANNEL_CAPACITY: usize = 256;
const MAX_RESTARTS: u32 = 2;

#[derive(Debug)]
pub enum DraftPreviewStoreError {
    InvalidInput(String),
    NotFound(String),
    Conflict(String),
    InvalidTransition(String),
    Storage(String),
}

impl std::fmt::Display for DraftPreviewStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid draft preview: {message}"),
            Self::NotFound(message) => write!(formatter, "draft preview not found: {message}"),
            Self::Conflict(message) => write!(formatter, "draft preview conflict: {message}"),
            Self::InvalidTransition(message) => {
                write!(formatter, "invalid draft preview transition: {message}")
            }
            Self::Storage(message) => write!(formatter, "draft preview storage failure: {message}"),
        }
    }
}

impl std::error::Error for DraftPreviewStoreError {}

impl From<std::io::Error> for DraftPreviewStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<serde_json::Error> for DraftPreviewStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct StartDraftPreview {
    pub project_id: String,
    pub sandbox_binding_id: String,
    pub template_id: String,
    pub base_snapshot_id: String,
    pub base_version_id: Option<String>,
    pub proxy_url: String,
    pub writer_ttl_seconds: u64,
}

#[derive(Debug, Default)]
struct State {
    sessions: BTreeMap<String, DraftPreviewSession>,
    events: BTreeMap<String, Vec<DraftPreviewEvent>>,
    broadcasters: BTreeMap<String, broadcast::Sender<DraftPreviewEvent>>,
}

#[derive(Debug)]
pub struct DraftPreviewStore {
    session_log_path: PathBuf,
    event_log_path: PathBuf,
    state: Mutex<State>,
}

impl DraftPreviewStore {
    pub fn open(runtime_storage_dir: impl Into<PathBuf>) -> Result<Self, DraftPreviewStoreError> {
        let root = runtime_storage_dir.into().join("draft-previews");
        let session_log_path = root.join("sessions.jsonl");
        let event_log_path = root.join("events.jsonl");
        let mut state = State::default();
        read_jsonl::<DraftPreviewSession>(&session_log_path, |session| {
            session
                .validate()
                .map_err(DraftPreviewStoreError::Storage)?;
            state.sessions.insert(session.session_id.clone(), session);
            Ok(())
        })?;
        read_jsonl::<DraftPreviewEvent>(&event_log_path, |event| {
            state
                .events
                .entry(event_session_id(&event).to_string())
                .or_default()
                .push(event);
            Ok(())
        })?;
        Ok(Self {
            session_log_path,
            event_log_path,
            state: Mutex::new(state),
        })
    }

    pub fn start(
        &self,
        request: StartDraftPreview,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        validate_start(&request)?;
        let mut state = self.state.lock().unwrap();
        let now = Utc::now();
        if let Some(active) = state.sessions.values().find(|session| {
            session.project_id == request.project_id
                && session.status != DraftPreviewSessionStatus::Stopped
                && session.status != DraftPreviewSessionStatus::Failed
                && session.writer_lease_expires_at > now
        }) {
            return Err(DraftPreviewStoreError::Conflict(format!(
                "project already has an active Draft writer lease: {}",
                active.session_id
            )));
        }
        let session_id = random_id("draft-session");
        let writer_lease_id = random_id("writer-lease");
        let session = DraftPreviewSession {
            schema_version: DRAFT_PREVIEW_SESSION_SCHEMA.to_string(),
            session_id: session_id.clone(),
            project_id: request.project_id.clone(),
            sandbox_binding_id: request.sandbox_binding_id,
            template_id: request.template_id,
            base_snapshot_id: request.base_snapshot_id.clone(),
            base_version_id: request.base_version_id,
            writer_lease_id,
            writer_lease_expires_at: now
                + Duration::seconds(request.writer_ttl_seconds.clamp(30, 3600) as i64),
            workspace_revision: 0,
            last_ready_revision: 0,
            durable_revision: 0,
            durable_snapshot_id: request.base_snapshot_id,
            publish_revision: None,
            session_epoch: 1,
            status: DraftPreviewSessionStatus::Starting,
            proxy_url: request.proxy_url,
            started_at: now,
            last_activity_at: now,
            restart_count: 0,
            last_error: None,
        };
        session
            .validate()
            .map_err(DraftPreviewStoreError::InvalidInput)?;
        self.persist_session(&session)?;
        state.sessions.insert(session_id, session.clone());
        self.record_event_locked(
            &mut state,
            DraftPreviewEvent::DevStarting {
                project_id: session.project_id.clone(),
                session_id: session.session_id.clone(),
                session_epoch: session.session_epoch,
                workspace_revision: session.workspace_revision,
                timestamp: now,
            },
        )?;
        Ok(session)
    }

    pub fn get(&self, session_id: &str) -> Option<DraftPreviewSession> {
        self.state.lock().unwrap().sessions.get(session_id).cloned()
    }

    pub fn active_for_project(&self, project_id: &str) -> Option<DraftPreviewSession> {
        let now = Utc::now();
        self.state
            .lock()
            .unwrap()
            .sessions
            .values()
            .filter(|session| {
                session.project_id == project_id
                    && session.status != DraftPreviewSessionStatus::Stopped
                    && session.status != DraftPreviewSessionStatus::Failed
                    && session.writer_lease_expires_at > now
            })
            .max_by_key(|session| session.started_at)
            .cloned()
    }

    pub fn events(&self, session_id: &str) -> Vec<DraftPreviewEvent> {
        self.state
            .lock()
            .unwrap()
            .events
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn subscribe(&self, session_id: &str) -> broadcast::Receiver<DraftPreviewEvent> {
        let mut state = self.state.lock().unwrap();
        state
            .broadcasters
            .entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(EVENT_CHANNEL_CAPACITY).0)
            .subscribe()
    }

    pub fn heartbeat(
        &self,
        session_id: &str,
        writer_lease_id: &str,
        session_epoch: u64,
        ttl_seconds: u64,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        self.update_session(session_id, |session, now| {
            validate_writer(session, writer_lease_id, session_epoch, now)?;
            session.writer_lease_expires_at =
                now + Duration::seconds(ttl_seconds.clamp(30, 3600) as i64);
            Ok(None)
        })
    }

    pub fn takeover(
        &self,
        session_id: &str,
        expected_session_epoch: u64,
        ttl_seconds: u64,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        self.update_session(session_id, |session, now| {
            if session.session_epoch != expected_session_epoch {
                return Err(DraftPreviewStoreError::Conflict(
                    "expectedSessionEpoch is stale".to_string(),
                ));
            }
            if session.status == DraftPreviewSessionStatus::Stopped {
                return Err(DraftPreviewStoreError::InvalidTransition(
                    "stopped session cannot be taken over".to_string(),
                ));
            }
            if session.writer_lease_expires_at > now {
                return Err(DraftPreviewStoreError::Conflict(
                    "active Draft writer lease cannot be taken over".to_string(),
                ));
            }
            session.session_epoch = session.session_epoch.saturating_add(1);
            session.writer_lease_id = random_id("writer-lease");
            session.writer_lease_expires_at =
                now + Duration::seconds(ttl_seconds.clamp(30, 3600) as i64);
            Ok(None)
        })
    }

    pub fn commit_revision(
        &self,
        session_id: &str,
        writer_lease_id: &str,
        expected_session_epoch: u64,
        expected_workspace_revision: u64,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        self.update_session(session_id, |session, now| {
            validate_writer(session, writer_lease_id, expected_session_epoch, now)?;
            if session.workspace_revision != expected_workspace_revision {
                return Err(DraftPreviewStoreError::Conflict(format!(
                    "expectedWorkspaceRevision is stale: expected {expected_workspace_revision}, actual {}",
                    session.workspace_revision
                )));
            }
            session.workspace_revision = session.workspace_revision.saturating_add(1);
            session.status = DraftPreviewSessionStatus::Updating;
            session.last_error = None;
            Ok(Some(DraftPreviewEvent::SourceRevisionCommitted {
                project_id: session.project_id.clone(),
                session_id: session.session_id.clone(),
                session_epoch: session.session_epoch,
                workspace_revision: session.workspace_revision,
                timestamp: now,
            }))
        })
    }

    pub fn mark_ready(
        &self,
        session_id: &str,
        session_epoch: u64,
        ready_revision: u64,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        self.update_session(session_id, |session, now| {
            validate_process_epoch(session, session_epoch)?;
            if ready_revision > session.workspace_revision {
                return Err(DraftPreviewStoreError::Conflict(
                    "ready revision exceeds workspace revision".to_string(),
                ));
            }
            if ready_revision < session.last_ready_revision {
                return Err(DraftPreviewStoreError::Conflict(
                    "ready revision cannot move backwards".to_string(),
                ));
            }
            session.last_ready_revision = ready_revision;
            session.status = DraftPreviewSessionStatus::Ready;
            session.last_error = None;
            Ok(Some(DraftPreviewEvent::DevReady {
                project_id: session.project_id.clone(),
                session_id: session.session_id.clone(),
                session_epoch: session.session_epoch,
                workspace_revision: session.workspace_revision,
                proxy_url: session.proxy_url.clone(),
                ready_revision,
                timestamp: now,
            }))
        })
    }

    pub fn mark_compile_error(
        &self,
        session_id: &str,
        session_epoch: u64,
        error: String,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        if error.trim().is_empty() {
            return Err(DraftPreviewStoreError::InvalidInput(
                "compile error must not be empty".to_string(),
            ));
        }
        self.update_session(session_id, |session, now| {
            validate_process_epoch(session, session_epoch)?;
            session.status = DraftPreviewSessionStatus::CompileError;
            session.last_error = Some(error.clone());
            Ok(Some(DraftPreviewEvent::DevCompileError {
                project_id: session.project_id.clone(),
                session_id: session.session_id.clone(),
                session_epoch: session.session_epoch,
                workspace_revision: session.workspace_revision,
                error,
                timestamp: now,
            }))
        })
    }

    pub fn mark_durable(
        &self,
        session_id: &str,
        writer_lease_id: &str,
        session_epoch: u64,
        revision: u64,
        snapshot_id: String,
        source_hash: String,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        if snapshot_id.trim().is_empty()
            || source_hash.len() != 64
            || !source_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DraftPreviewStoreError::InvalidInput(
                "durable snapshot identity is invalid".to_string(),
            ));
        }
        self.update_session(session_id, |session, now| {
            validate_writer(session, writer_lease_id, session_epoch, now)?;
            if revision > session.workspace_revision || revision < session.durable_revision {
                return Err(DraftPreviewStoreError::Conflict(
                    "durable revision is outside the committed revision range".to_string(),
                ));
            }
            session.durable_revision = revision;
            session.durable_snapshot_id = snapshot_id.clone();
            Ok(Some(DraftPreviewEvent::SourceRevisionDurable {
                project_id: session.project_id.clone(),
                session_id: session.session_id.clone(),
                session_epoch: session.session_epoch,
                workspace_revision: session.workspace_revision,
                snapshot_id,
                source_hash,
                timestamp: now,
            }))
        })
    }

    pub fn mark_publish_revision(
        &self,
        session_id: &str,
        session_epoch: u64,
        revision: u64,
        snapshot_id: &str,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        self.update_session(session_id, |session, _now| {
            if session.session_epoch != session_epoch {
                return Err(DraftPreviewStoreError::Conflict(
                    "publish source session epoch is stale".to_string(),
                ));
            }
            if revision != session.durable_revision
                || snapshot_id != session.durable_snapshot_id
                || revision > session.last_ready_revision
            {
                return Err(DraftPreviewStoreError::Conflict(
                    "publish revision is not the last ready durable snapshot".to_string(),
                ));
            }
            session.publish_revision = Some(revision);
            Ok(None)
        })
    }

    pub fn begin_restart(
        &self,
        session_id: &str,
        error: String,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        self.update_session(session_id, |session, now| {
            if session.restart_count >= MAX_RESTARTS {
                session.status = DraftPreviewSessionStatus::Failed;
                session.last_error = Some(error.clone());
                return Ok(Some(DraftPreviewEvent::DevFailed {
                    project_id: session.project_id.clone(),
                    session_id: session.session_id.clone(),
                    session_epoch: session.session_epoch,
                    workspace_revision: session.workspace_revision,
                    error,
                    timestamp: now,
                }));
            }
            session.restart_count += 1;
            session.session_epoch += 1;
            session.status = DraftPreviewSessionStatus::Restarting;
            session.last_error = Some(error);
            Ok(Some(DraftPreviewEvent::DevRestarting {
                project_id: session.project_id.clone(),
                session_id: session.session_id.clone(),
                session_epoch: session.session_epoch,
                workspace_revision: session.workspace_revision,
                restart_count: session.restart_count,
                timestamp: now,
            }))
        })
    }

    pub fn stop(
        &self,
        session_id: &str,
        reason: String,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError> {
        if reason.trim().is_empty() {
            return Err(DraftPreviewStoreError::InvalidInput(
                "stop reason must not be empty".to_string(),
            ));
        }
        self.update_session(session_id, |session, now| {
            session.status = DraftPreviewSessionStatus::Stopped;
            session.writer_lease_expires_at = now;
            Ok(Some(DraftPreviewEvent::DevStopped {
                project_id: session.project_id.clone(),
                session_id: session.session_id.clone(),
                session_epoch: session.session_epoch,
                workspace_revision: session.workspace_revision,
                reason,
                timestamp: now,
            }))
        })
    }

    fn update_session<F>(
        &self,
        session_id: &str,
        update: F,
    ) -> Result<DraftPreviewSession, DraftPreviewStoreError>
    where
        F: FnOnce(
            &mut DraftPreviewSession,
            chrono::DateTime<Utc>,
        ) -> Result<Option<DraftPreviewEvent>, DraftPreviewStoreError>,
    {
        let mut state = self.state.lock().unwrap();
        let mut session = state
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| DraftPreviewStoreError::NotFound(session_id.to_string()))?;
        let now = Utc::now();
        let event = update(&mut session, now)?;
        session.last_activity_at = now;
        session
            .validate()
            .map_err(DraftPreviewStoreError::InvalidTransition)?;
        self.persist_session(&session)?;
        state
            .sessions
            .insert(session_id.to_string(), session.clone());
        if let Some(event) = event {
            self.record_event_locked(&mut state, event)?;
        }
        Ok(session)
    }

    fn persist_session(&self, session: &DraftPreviewSession) -> Result<(), DraftPreviewStoreError> {
        append_jsonl(&self.session_log_path, session)
    }

    #[cfg(test)]
    fn expire_writer_for_test(&self, session_id: &str) {
        let mut state = self.state.lock().unwrap();
        let session = state.sessions.get_mut(session_id).unwrap();
        session.writer_lease_expires_at = Utc::now() - Duration::seconds(1);
    }

    fn record_event_locked(
        &self,
        state: &mut State,
        event: DraftPreviewEvent,
    ) -> Result<(), DraftPreviewStoreError> {
        append_jsonl(&self.event_log_path, &event)?;
        let session_id = event_session_id(&event).to_string();
        state
            .events
            .entry(session_id.clone())
            .or_default()
            .push(event.clone());
        if let Some(sender) = state.broadcasters.get(&session_id) {
            let _ = sender.send(event);
        }
        Ok(())
    }
}

fn validate_start(request: &StartDraftPreview) -> Result<(), DraftPreviewStoreError> {
    if request.project_id.trim().is_empty()
        || request.sandbox_binding_id.trim().is_empty()
        || request.template_id.trim().is_empty()
        || request.base_snapshot_id.trim().is_empty()
        || !(request.proxy_url.starts_with("http://") || request.proxy_url.starts_with("https://"))
    {
        return Err(DraftPreviewStoreError::InvalidInput(
            "draft preview start identity is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_writer(
    session: &DraftPreviewSession,
    writer_lease_id: &str,
    session_epoch: u64,
    now: chrono::DateTime<Utc>,
) -> Result<(), DraftPreviewStoreError> {
    validate_process_epoch(session, session_epoch)?;
    if session.writer_lease_id != writer_lease_id {
        return Err(DraftPreviewStoreError::Conflict(
            "writerLeaseId does not own this session".to_string(),
        ));
    }
    if session.writer_lease_expires_at <= now {
        return Err(DraftPreviewStoreError::Conflict(
            "draft writer lease expired".to_string(),
        ));
    }
    Ok(())
}

fn validate_process_epoch(
    session: &DraftPreviewSession,
    session_epoch: u64,
) -> Result<(), DraftPreviewStoreError> {
    if session.session_epoch != session_epoch {
        return Err(DraftPreviewStoreError::Conflict(format!(
            "session epoch is stale: expected {}, actual {session_epoch}",
            session.session_epoch
        )));
    }
    if session.status == DraftPreviewSessionStatus::Stopped {
        return Err(DraftPreviewStoreError::InvalidTransition(
            "draft preview session is stopped".to_string(),
        ));
    }
    Ok(())
}

fn random_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &sha256_hex(&rand::random::<[u8; 32]>())[..32]
    )
}

fn event_session_id(event: &DraftPreviewEvent) -> &str {
    match event {
        DraftPreviewEvent::DevStarting { session_id, .. }
        | DraftPreviewEvent::DevReady { session_id, .. }
        | DraftPreviewEvent::DevUpdating { session_id, .. }
        | DraftPreviewEvent::DevCompileError { session_id, .. }
        | DraftPreviewEvent::DevRestarting { session_id, .. }
        | DraftPreviewEvent::DevFailed { session_id, .. }
        | DraftPreviewEvent::DevStopped { session_id, .. }
        | DraftPreviewEvent::SourceRevisionCommitted { session_id, .. }
        | DraftPreviewEvent::SourceRevisionDurable { session_id, .. }
        | DraftPreviewEvent::SourceSnapshotCreated { session_id, .. } => session_id,
    }
}

fn append_jsonl<T: Serialize>(path: &PathBuf, value: &T) -> Result<(), DraftPreviewStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(
    path: &PathBuf,
    mut accept: impl FnMut(T) -> Result<(), DraftPreviewStoreError>,
) -> Result<(), DraftPreviewStoreError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "anydesign-draft-preview-{name}-{}",
            rand::random::<u64>()
        ))
    }

    fn start(store: &DraftPreviewStore, project_id: &str) -> DraftPreviewSession {
        store
            .start(StartDraftPreview {
                project_id: project_id.to_string(),
                sandbox_binding_id: "binding-1".to_string(),
                template_id: "next-app".to_string(),
                base_snapshot_id: "snapshot-0".to_string(),
                base_version_id: None,
                proxy_url: "https://runtime.test/previews/lease-1/".to_string(),
                writer_ttl_seconds: 60,
            })
            .unwrap()
    }

    #[test]
    fn enforces_single_writer_and_compare_and_swap_revisions() {
        let root = test_root("writer-cas");
        let store = DraftPreviewStore::open(&root).unwrap();
        let session = start(&store, "project-1");
        assert!(matches!(
            store.start(StartDraftPreview {
                project_id: "project-1".to_string(),
                sandbox_binding_id: "binding-2".to_string(),
                template_id: "next-app".to_string(),
                base_snapshot_id: "snapshot-0".to_string(),
                base_version_id: None,
                proxy_url: "https://runtime.test/previews/lease-2/".to_string(),
                writer_ttl_seconds: 60,
            }),
            Err(DraftPreviewStoreError::Conflict(_))
        ));
        let updated = store
            .commit_revision(
                &session.session_id,
                &session.writer_lease_id,
                session.session_epoch,
                0,
            )
            .unwrap();
        assert_eq!(updated.workspace_revision, 1);
        assert!(matches!(
            store.commit_revision(
                &session.session_id,
                &session.writer_lease_id,
                session.session_epoch,
                0,
            ),
            Err(DraftPreviewStoreError::Conflict(_))
        ));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn takeover_rejects_late_process_events_and_preserves_last_ready_revision() {
        let root = test_root("takeover");
        let store = DraftPreviewStore::open(&root).unwrap();
        let session = start(&store, "project-1");
        store
            .commit_revision(
                &session.session_id,
                &session.writer_lease_id,
                session.session_epoch,
                0,
            )
            .unwrap();
        store.mark_ready(&session.session_id, 1, 1).unwrap();
        store.expire_writer_for_test(&session.session_id);
        let takeover = store.takeover(&session.session_id, 1, 60).unwrap();
        assert_eq!(takeover.session_epoch, 2);
        assert!(matches!(
            store.mark_compile_error(&session.session_id, 1, "late error".to_string()),
            Err(DraftPreviewStoreError::Conflict(_))
        ));
        let failed = store
            .mark_compile_error(&session.session_id, 2, "compile failed".to_string())
            .unwrap();
        assert_eq!(failed.last_ready_revision, 1);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn durable_publish_identity_survives_restart_and_restart_budget_is_bounded() {
        let root = test_root("durable");
        let store = DraftPreviewStore::open(&root).unwrap();
        let session = start(&store, "project-1");
        store
            .commit_revision(
                &session.session_id,
                &session.writer_lease_id,
                session.session_epoch,
                0,
            )
            .unwrap();
        store.mark_ready(&session.session_id, 1, 1).unwrap();
        store
            .mark_durable(
                &session.session_id,
                &session.writer_lease_id,
                1,
                1,
                "snapshot-1".to_string(),
                "a".repeat(64),
            )
            .unwrap();
        drop(store);

        let store = DraftPreviewStore::open(&root).unwrap();
        let restored = store.get(&session.session_id).unwrap();
        assert_eq!(restored.durable_revision, 1);
        store
            .mark_publish_revision(&session.session_id, 1, 1, "snapshot-1")
            .unwrap();
        let first = store
            .begin_restart(&session.session_id, "crash-1".to_string())
            .unwrap();
        let second = store
            .begin_restart(&session.session_id, "crash-2".to_string())
            .unwrap();
        let failed = store
            .begin_restart(&session.session_id, "crash-3".to_string())
            .unwrap();
        assert_eq!(first.restart_count, 1);
        assert_eq!(second.restart_count, 2);
        assert_eq!(failed.status, DraftPreviewSessionStatus::Failed);
        fs::remove_dir_all(root).ok();
    }
}
