use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::{fs, path::Path};

use crate::{
    GatewayErrorEnvelope, GatewayTurnResponse, ModelExecutionSummary, ModelResource,
    ModelSelectionPolicy,
};

#[path = "postgres_store.rs"]
mod postgres_store;
use postgres_store::PostgresStore;

#[derive(Debug)]
pub enum StoredTurn {
    Reserved,
    InProgress,
    Completed(String),
    Expired,
    Failed {
        status: u16,
        error: GatewayErrorEnvelope,
    },
    Conflict,
}

#[derive(Debug)]
pub enum StoredAdminOperation {
    Reserved,
    Completed(String),
    InProgress,
    Conflict,
}

#[derive(Debug)]
pub struct SqliteStore {
    connection: Connection,
}

pub enum PersistentStore {
    Sqlite(SqliteStore),
    Postgres(Box<PostgresStore>),
}

macro_rules! delegate_store {
    ($store:expr, $method:ident($($arg:expr),*)) => {
        match $store {
            Self::Sqlite(store) => store.$method($($arg),*),
            Self::Postgres(store) => store.$method($($arg),*),
        }
    };
}

impl PersistentStore {
    pub fn open(database_url: &str) -> Result<Self> {
        if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
            Ok(Self::Postgres(Box::new(PostgresStore::open(database_url)?)))
        } else {
            Ok(Self::Sqlite(SqliteStore::open(database_url)?))
        }
    }

    pub fn reserve_turn(&self, key: &str, request_hash: &str) -> Result<StoredTurn> {
        delegate_store!(self, reserve_turn(key, request_hash))
    }

    pub fn reserve_admin_operation(
        &self,
        key: &str,
        request_hash: &str,
    ) -> Result<StoredAdminOperation> {
        delegate_store!(self, reserve_admin_operation(key, request_hash))
    }

    pub fn complete_admin_operation(&self, key: &str, response_json: &str) -> Result<()> {
        delegate_store!(self, complete_admin_operation(key, response_json))
    }

    pub fn discard_admin_operation(&self, key: &str) -> Result<()> {
        delegate_store!(self, discard_admin_operation(key))
    }

    pub fn resource_circuit_allows(
        &self,
        resource_id: &str,
        revision: u64,
        now_epoch_seconds: i64,
    ) -> Result<bool> {
        delegate_store!(
            self,
            resource_circuit_allows(resource_id, revision, now_epoch_seconds)
        )
    }

    pub fn record_resource_success(&self, resource_id: &str, revision: u64) -> Result<()> {
        delegate_store!(self, record_resource_success(resource_id, revision))
    }

    pub fn record_resource_retryable_failure(
        &self,
        resource_id: &str,
        revision: u64,
        now_epoch_seconds: i64,
    ) -> Result<bool> {
        delegate_store!(
            self,
            record_resource_retryable_failure(resource_id, revision, now_epoch_seconds)
        )
    }

    pub fn resource_health_states(&self) -> Result<Vec<ResourceHealthState>> {
        delegate_store!(self, resource_health_states())
    }

    pub fn reserve_project_daily_input_tokens(
        &self,
        period_utc: &str,
        organization_id: &str,
        project_id: &str,
        requested_tokens: u64,
        limit: u64,
    ) -> Result<bool> {
        delegate_store!(
            self,
            reserve_project_daily_input_tokens(
                period_utc,
                organization_id,
                project_id,
                requested_tokens,
                limit
            )
        )
    }

    pub fn acquire_project_concurrency_lease(
        &self,
        scope_key: &str,
        lease_id: &str,
        limit: usize,
        expires_at_epoch_seconds: i64,
    ) -> Result<bool> {
        delegate_store!(
            self,
            acquire_project_concurrency_lease(scope_key, lease_id, limit, expires_at_epoch_seconds)
        )
    }

    pub fn release_project_concurrency_lease(&self, lease_id: &str) -> Result<()> {
        delegate_store!(self, release_project_concurrency_lease(lease_id))
    }

    pub fn settle_project_daily_usage(
        &self,
        period_utc: &str,
        organization_id: &str,
        project_id: &str,
        reserved_input_tokens: u64,
        actual_input_tokens: u64,
    ) -> Result<()> {
        delegate_store!(
            self,
            settle_project_daily_usage(
                period_utc,
                organization_id,
                project_id,
                reserved_input_tokens,
                actual_input_tokens
            )
        )
    }

    pub fn complete_turn(
        &self,
        key: &str,
        encrypted_response: &str,
        response: &GatewayTurnResponse,
        run_id: &str,
        turn: u32,
    ) -> Result<()> {
        delegate_store!(
            self,
            complete_turn(key, encrypted_response, response, run_id, turn)
        )
    }

    pub fn fail_turn(&self, key: &str, status: u16, error: &GatewayErrorEnvelope) -> Result<()> {
        delegate_store!(self, fail_turn(key, status, error))
    }

    pub fn execution_snapshot(&self, id: &str) -> Result<Option<ModelExecutionSummary>> {
        delegate_store!(self, execution_snapshot(id))
    }

    pub fn execution_snapshots_for_run(&self, run_id: &str) -> Result<Vec<ModelExecutionSummary>> {
        delegate_store!(self, execution_snapshots_for_run(run_id))
    }

    pub fn initialize_configuration(
        &self,
        configured_resources: &[ModelResource],
        configured_policies: &[ModelSelectionPolicy],
    ) -> Result<(Vec<ModelResource>, Vec<ModelSelectionPolicy>)> {
        delegate_store!(
            self,
            initialize_configuration(configured_resources, configured_policies)
        )
    }

    pub fn current_model_resources(&self) -> Result<Vec<ModelResource>> {
        delegate_store!(self, current_model_resources())
    }

    pub fn model_resource(&self, id: &str, revision: Option<u64>) -> Result<Option<ModelResource>> {
        delegate_store!(self, model_resource(id, revision))
    }

    pub fn current_model_selection_policies(&self) -> Result<Vec<ModelSelectionPolicy>> {
        delegate_store!(self, current_model_selection_policies())
    }

    pub fn model_selection_policy(
        &self,
        id: &str,
        revision: Option<u64>,
    ) -> Result<Option<ModelSelectionPolicy>> {
        delegate_store!(self, model_selection_policy(id, revision))
    }

    pub fn save_model_resource(
        &self,
        resource: ModelResource,
        expected_revision: Option<u64>,
    ) -> Result<ModelResource> {
        delegate_store!(self, save_model_resource(resource, expected_revision))
    }

    pub fn set_model_resource_enabled(
        &self,
        id: &str,
        enabled: bool,
        expected_revision: u64,
    ) -> Result<ModelResource> {
        delegate_store!(
            self,
            set_model_resource_enabled(id, enabled, expected_revision)
        )
    }

    pub fn save_model_selection_policy(
        &self,
        policy: ModelSelectionPolicy,
        expected_revision: Option<u64>,
    ) -> Result<ModelSelectionPolicy> {
        delegate_store!(self, save_model_selection_policy(policy, expected_revision))
    }

    pub fn activate_model_selection_policy(
        &self,
        id: &str,
        revision_to_activate: u64,
        expected_current_revision: u64,
    ) -> Result<ModelSelectionPolicy> {
        delegate_store!(
            self,
            activate_model_selection_policy(id, revision_to_activate, expected_current_revision)
        )
    }

    pub fn audit_admin_operation(
        &self,
        event_type: &str,
        subject_id: &str,
        operator_id: &str,
        reason: &str,
        change_reference: &str,
    ) -> Result<()> {
        delegate_store!(
            self,
            audit_admin_operation(
                event_type,
                subject_id,
                operator_id,
                reason,
                change_reference
            )
        )
    }

    pub fn audit_event(
        &self,
        event_type: &str,
        subject_id: &str,
        metadata: &impl serde::Serialize,
    ) -> Result<()> {
        match self {
            Self::Sqlite(store) => store.audit_event(event_type, subject_id, metadata),
            Self::Postgres(store) => store.audit_event(event_type, subject_id, metadata),
        }
    }

    pub fn audit_events(
        &self,
        event_type: Option<&str>,
        subject_id: Option<&str>,
        before_id: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AuditEventRecord>> {
        delegate_store!(self, audit_events(event_type, subject_id, before_id, limit))
    }

    pub fn save_encrypted_secret(&self, name: &str, ciphertext: &str) -> Result<()> {
        delegate_store!(self, save_encrypted_secret(name, ciphertext))
    }

    pub fn encrypted_secret(&self, name: &str) -> Result<Option<String>> {
        delegate_store!(self, encrypted_secret(name))
    }
}

#[derive(Debug, Clone)]
pub struct AuditEventRecord {
    pub id: i64,
    pub event_type: String,
    pub subject_id: String,
    pub metadata: Value,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ResourceHealthState {
    pub model_resource_id: String,
    pub model_resource_revision: u64,
    pub circuit_state: String,
}

impl SqliteStore {
    pub fn open(database_url: &str) -> Result<Self> {
        let path = database_url
            .strip_prefix("sqlite://")
            .or_else(|| database_url.strip_prefix("sqlite:"))
            .unwrap_or(database_url);
        if path != ":memory:" {
            let path = Path::new(path);
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating SQLite directory {}", parent.display()))?;
            }
        }
        let connection = Connection::open(path)
            .with_context(|| format!("opening Provider Gateway SQLite database {database_url}"))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS turn_idempotency_records (
                idempotency_key TEXT PRIMARY KEY,
                request_hash TEXT NOT NULL,
                state TEXT NOT NULL,
                response_json TEXT,
                error_json TEXT,
                error_status INTEGER,
                response_expires_at INTEGER,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS model_execution_snapshots (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                turn INTEGER NOT NULL,
                model_resource_id TEXT NOT NULL,
                model_resource_revision INTEGER NOT NULL,
                snapshot_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS model_execution_snapshots_run_turn
                ON model_execution_snapshots(run_id, turn);
            CREATE TABLE IF NOT EXISTS model_resource_revisions (
                id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                enabled INTEGER NOT NULL,
                is_current INTEGER NOT NULL,
                resource_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY(id, revision)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS model_resource_current
                ON model_resource_revisions(id) WHERE is_current = 1;
            CREATE TABLE IF NOT EXISTS model_selection_policy_revisions (
                id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                is_current INTEGER NOT NULL,
                policy_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY(id, revision)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS model_selection_policy_current
                ON model_selection_policy_revisions(id) WHERE is_current = 1;
            CREATE TABLE IF NOT EXISTS provider_gateway_audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                subject_id TEXT NOT NULL,
                metadata_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS admin_idempotency_records (
                idempotency_key TEXT PRIMARY KEY,
                request_hash TEXT NOT NULL,
                state TEXT NOT NULL,
                response_json TEXT,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS model_resource_health (
                model_resource_id TEXT NOT NULL,
                model_resource_revision INTEGER NOT NULL,
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                circuit_state TEXT NOT NULL DEFAULT 'closed',
                opened_until_epoch_seconds INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY(model_resource_id, model_resource_revision)
            );
            CREATE TABLE IF NOT EXISTS project_daily_quota_usage (
                period_utc TEXT NOT NULL,
                organization_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY(period_utc, organization_id, project_id)
            );
            CREATE TABLE IF NOT EXISTS project_concurrency_leases (
                lease_id TEXT PRIMARY KEY,
                scope_key TEXT NOT NULL,
                expires_at_epoch_seconds INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS project_concurrency_leases_scope
                ON project_concurrency_leases(scope_key, expires_at_epoch_seconds);
            CREATE TABLE IF NOT EXISTS provider_gateway_encrypted_secrets (
                name TEXT PRIMARY KEY,
                ciphertext TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            ",
        )?;
        let _ = connection.execute(
            "ALTER TABLE turn_idempotency_records ADD COLUMN response_expires_at INTEGER",
            [],
        );
        Ok(Self { connection })
    }

    pub fn reserve_turn(&self, key: &str, request_hash: &str) -> Result<StoredTurn> {
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO turn_idempotency_records (idempotency_key, request_hash, state)
             VALUES (?1, ?2, 'in_progress')",
            params![key, request_hash],
        )?;
        if inserted == 1 {
            return Ok(StoredTurn::Reserved);
        }
        let row = self
            .connection
            .query_row(
                "SELECT request_hash, state, response_json, error_json, error_status,
                        response_expires_at
                 FROM turn_idempotency_records WHERE idempotency_key = ?1",
                params![key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<u16>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("idempotency record disappeared during reservation"))?;
        if row.0 != request_hash {
            return Ok(StoredTurn::Conflict);
        }
        match row.1.as_str() {
            "in_progress" => Ok(StoredTurn::InProgress),
            "completed"
                if row
                    .5
                    .is_some_and(|expires_at| expires_at <= chrono::Utc::now().timestamp()) =>
            {
                self.connection.execute(
                    "UPDATE turn_idempotency_records
                     SET state = 'expired', response_json = NULL, updated_at = CURRENT_TIMESTAMP
                     WHERE idempotency_key = ?1 AND state = 'completed'",
                    params![key],
                )?;
                Ok(StoredTurn::Expired)
            }
            "completed" => {
                Ok(StoredTurn::Completed(row.2.ok_or_else(|| {
                    anyhow!("completed idempotency record has no response")
                })?))
            }
            "expired" => Ok(StoredTurn::Expired),
            "failed" => Ok(StoredTurn::Failed {
                status: row
                    .4
                    .ok_or_else(|| anyhow!("failed idempotency record has no HTTP status"))?,
                error: serde_json::from_str(
                    row.3
                        .as_deref()
                        .ok_or_else(|| anyhow!("failed idempotency record has no error"))?,
                )?,
            }),
            state => Err(anyhow!("unknown idempotency state {state}")),
        }
    }

    pub fn reserve_admin_operation(
        &self,
        key: &str,
        request_hash: &str,
    ) -> Result<StoredAdminOperation> {
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO admin_idempotency_records (idempotency_key, request_hash, state)
             VALUES (?1, ?2, 'in_progress')",
            params![key, request_hash],
        )?;
        if inserted == 1 {
            return Ok(StoredAdminOperation::Reserved);
        }
        let row = self
            .connection
            .query_row(
                "SELECT request_hash, state, response_json
                 FROM admin_idempotency_records WHERE idempotency_key = ?1",
                params![key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("admin idempotency record disappeared during reservation"))?;
        if row.0 != request_hash {
            return Ok(StoredAdminOperation::Conflict);
        }
        match row.1.as_str() {
            "in_progress" => Ok(StoredAdminOperation::InProgress),
            "completed" => Ok(StoredAdminOperation::Completed(row.2.ok_or_else(|| {
                anyhow!("completed admin idempotency record has no response")
            })?)),
            state => Err(anyhow!("unknown admin idempotency state {state}")),
        }
    }

    pub fn complete_admin_operation(&self, key: &str, response_json: &str) -> Result<()> {
        let updated = self.connection.execute(
            "UPDATE admin_idempotency_records
             SET state = 'completed', response_json = ?2, updated_at = CURRENT_TIMESTAMP
             WHERE idempotency_key = ?1 AND state = 'in_progress'",
            params![key, response_json],
        )?;
        if updated != 1 {
            return Err(anyhow!("admin idempotency reservation is no longer active"));
        }
        Ok(())
    }

    pub fn discard_admin_operation(&self, key: &str) -> Result<()> {
        self.connection.execute(
            "DELETE FROM admin_idempotency_records
             WHERE idempotency_key = ?1 AND state = 'in_progress'",
            params![key],
        )?;
        Ok(())
    }

    pub fn resource_circuit_allows(
        &self,
        resource_id: &str,
        revision: u64,
        now_epoch_seconds: i64,
    ) -> Result<bool> {
        let row = self
            .connection
            .query_row(
                "SELECT circuit_state, opened_until_epoch_seconds
                 FROM model_resource_health
                 WHERE model_resource_id = ?1 AND model_resource_revision = ?2",
                params![resource_id, revision],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        match row {
            Some((state, until)) if state == "open" && until > now_epoch_seconds => Ok(false),
            Some((state, _)) if state == "open" => {
                self.connection.execute(
                    "UPDATE model_resource_health
                     SET circuit_state = 'half_open', updated_at = CURRENT_TIMESTAMP
                     WHERE model_resource_id = ?1 AND model_resource_revision = ?2",
                    params![resource_id, revision],
                )?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    pub fn record_resource_success(&self, resource_id: &str, revision: u64) -> Result<()> {
        self.connection.execute(
            "INSERT INTO model_resource_health
                (model_resource_id, model_resource_revision, consecutive_failures, circuit_state, opened_until_epoch_seconds)
             VALUES (?1, ?2, 0, 'closed', 0)
             ON CONFLICT(model_resource_id, model_resource_revision) DO UPDATE SET
                consecutive_failures = 0,
                circuit_state = 'closed',
                opened_until_epoch_seconds = 0,
                updated_at = CURRENT_TIMESTAMP",
            params![resource_id, revision],
        )?;
        Ok(())
    }

    pub fn record_resource_retryable_failure(
        &self,
        resource_id: &str,
        revision: u64,
        now_epoch_seconds: i64,
    ) -> Result<bool> {
        let failures = self
            .connection
            .query_row(
                "SELECT consecutive_failures FROM model_resource_health
                 WHERE model_resource_id = ?1 AND model_resource_revision = ?2",
                params![resource_id, revision],
                |row| row.get::<_, u32>(0),
            )
            .optional()?
            .unwrap_or_default()
            .saturating_add(1);
        let open = failures >= 3;
        let opened_until = if open { now_epoch_seconds + 30 } else { 0 };
        self.connection.execute(
            "INSERT INTO model_resource_health
                (model_resource_id, model_resource_revision, consecutive_failures, circuit_state, opened_until_epoch_seconds)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(model_resource_id, model_resource_revision) DO UPDATE SET
                consecutive_failures = excluded.consecutive_failures,
                circuit_state = excluded.circuit_state,
                opened_until_epoch_seconds = excluded.opened_until_epoch_seconds,
                updated_at = CURRENT_TIMESTAMP",
            params![
                resource_id,
                revision,
                failures,
                if open { "open" } else { "closed" },
                opened_until,
            ],
        )?;
        Ok(open)
    }

    pub fn resource_health_states(&self) -> Result<Vec<ResourceHealthState>> {
        let mut statement = self.connection.prepare(
            "SELECT model_resource_id, model_resource_revision, circuit_state
             FROM model_resource_health",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(ResourceHealthState {
                model_resource_id: row.get(0)?,
                model_resource_revision: row.get(1)?,
                circuit_state: row.get(2)?,
            })
        })?;
        let states = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        Ok(states)
    }

    pub fn reserve_project_daily_input_tokens(
        &self,
        period_utc: &str,
        organization_id: &str,
        project_id: &str,
        requested_tokens: u64,
        limit: u64,
    ) -> Result<bool> {
        let requested_tokens = i64::try_from(requested_tokens)
            .map_err(|_| anyhow!("requested token reservation is too large"))?;
        let limit = i64::try_from(limit).map_err(|_| anyhow!("daily token limit is too large"))?;
        let affected = self.connection.execute(
            "INSERT INTO project_daily_quota_usage
                (period_utc, organization_id, project_id, input_tokens)
             SELECT ?1, ?2, ?3, ?4 WHERE ?4 <= ?5
             ON CONFLICT(period_utc, organization_id, project_id) DO UPDATE SET
                input_tokens = input_tokens + excluded.input_tokens,
                updated_at = CURRENT_TIMESTAMP
             WHERE input_tokens + excluded.input_tokens <= ?5",
            params![
                period_utc,
                organization_id,
                project_id,
                requested_tokens,
                limit
            ],
        )?;
        Ok(affected == 1)
    }

    pub fn settle_project_daily_usage(
        &self,
        period_utc: &str,
        organization_id: &str,
        project_id: &str,
        reserved_input_tokens: u64,
        actual_input_tokens: u64,
    ) -> Result<()> {
        let reserved = i64::try_from(reserved_input_tokens)
            .map_err(|_| anyhow!("reserved token value is too large"))?;
        let actual = i64::try_from(actual_input_tokens)
            .map_err(|_| anyhow!("actual input token value is too large"))?;
        self.connection.execute(
            "UPDATE project_daily_quota_usage
             SET input_tokens = MAX(0, input_tokens + ?4 - ?5),
                 updated_at = CURRENT_TIMESTAMP
             WHERE period_utc = ?1 AND organization_id = ?2 AND project_id = ?3",
            params![period_utc, organization_id, project_id, actual, reserved],
        )?;
        Ok(())
    }

    pub fn acquire_project_concurrency_lease(
        &self,
        scope_key: &str,
        lease_id: &str,
        limit: usize,
        expires_at_epoch_seconds: i64,
    ) -> Result<bool> {
        self.connection.execute(
            "DELETE FROM project_concurrency_leases
             WHERE scope_key = ?1 AND expires_at_epoch_seconds <= ?2",
            params![scope_key, chrono::Utc::now().timestamp()],
        )?;
        let active: u64 = self.connection.query_row(
            "SELECT COUNT(*) FROM project_concurrency_leases WHERE scope_key = ?1",
            params![scope_key],
            |row| row.get(0),
        )?;
        if active >= limit as u64 {
            return Ok(false);
        }
        self.connection.execute(
            "INSERT OR IGNORE INTO project_concurrency_leases
                (lease_id, scope_key, expires_at_epoch_seconds)
             VALUES (?1, ?2, ?3)",
            params![lease_id, scope_key, expires_at_epoch_seconds],
        )?;
        Ok(true)
    }

    pub fn release_project_concurrency_lease(&self, lease_id: &str) -> Result<()> {
        self.connection.execute(
            "DELETE FROM project_concurrency_leases WHERE lease_id = ?1",
            params![lease_id],
        )?;
        Ok(())
    }

    pub fn complete_turn(
        &self,
        key: &str,
        encrypted_response: &str,
        response: &GatewayTurnResponse,
        run_id: &str,
        turn: u32,
    ) -> Result<()> {
        let snapshot_json = serde_json::to_string(&response.model_execution)?;
        self.connection.execute(
            "UPDATE turn_idempotency_records
             SET state = 'completed', response_json = ?2, error_json = NULL, error_status = NULL,
                 response_expires_at = ?3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE idempotency_key = ?1",
            params![
                key,
                encrypted_response,
                chrono::Utc::now().timestamp().saturating_add(24 * 60 * 60)
            ],
        )?;
        self.connection.execute(
            "INSERT OR IGNORE INTO model_execution_snapshots
                (id, run_id, turn, model_resource_id, model_resource_revision, snapshot_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                response.model_execution.id,
                run_id,
                turn,
                response.model_execution.model_resource_id,
                response.model_execution.model_resource_revision,
                snapshot_json,
            ],
        )?;
        self.audit(
            "model_execution.completed",
            &response.model_execution.id,
            &serde_json::json!({
                "runId": run_id,
                "turn": turn,
                "modelResourceId": response.model_execution.model_resource_id,
                "modelResourceRevision": response.model_execution.model_resource_revision,
                "selectionPolicyId": response.model_execution.selection_policy_id,
                "selectionPolicyRevision": response.model_execution.selection_policy_revision,
                "selectionReason": response.model_execution.selection_reason,
                "providerRequestId": response.provider.request_id,
                "attemptCount": response.provider.attempt_count,
                "usage": response.usage,
            }),
        )?;
        if response.model_execution.automatic_switch.used {
            self.audit(
                "model_execution.automatic_switch",
                &response.model_execution.id,
                &response.model_execution.automatic_switch,
            )?;
        }
        Ok(())
    }

    pub fn fail_turn(&self, key: &str, status: u16, error: &GatewayErrorEnvelope) -> Result<()> {
        self.connection.execute(
            "UPDATE turn_idempotency_records
             SET state = 'failed', response_json = NULL, error_json = ?2, error_status = ?3,
                 updated_at = CURRENT_TIMESTAMP
             WHERE idempotency_key = ?1",
            params![key, serde_json::to_string(error)?, status],
        )?;
        Ok(())
    }

    pub fn execution_snapshot(&self, id: &str) -> Result<Option<ModelExecutionSummary>> {
        self.connection
            .query_row(
                "SELECT snapshot_json FROM model_execution_snapshots WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|snapshot| serde_json::from_str(&snapshot).map_err(Into::into))
            .transpose()
    }

    pub fn execution_snapshots_for_run(&self, run_id: &str) -> Result<Vec<ModelExecutionSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT snapshot_json FROM model_execution_snapshots WHERE run_id = ?1 ORDER BY turn, created_at",
        )?;
        let snapshots = statement
            .query_map(params![run_id], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str::<ModelExecutionSummary>(&row?)?))
            .collect::<Result<Vec<_>>>();
        snapshots
    }

    pub fn initialize_configuration(
        &self,
        configured_resources: &[ModelResource],
        configured_policies: &[ModelSelectionPolicy],
    ) -> Result<(Vec<ModelResource>, Vec<ModelSelectionPolicy>)> {
        let resource_count: u64 = self.connection.query_row(
            "SELECT COUNT(*) FROM model_resource_revisions WHERE is_current = 1",
            [],
            |row| row.get(0),
        )?;
        let policy_count: u64 = self.connection.query_row(
            "SELECT COUNT(*) FROM model_selection_policy_revisions WHERE is_current = 1",
            [],
            |row| row.get(0),
        )?;
        if resource_count == 0 && policy_count == 0 {
            for resource in configured_resources {
                self.insert_resource_revision(resource, true)?;
            }
            for policy in configured_policies {
                self.insert_policy_revision(policy, true)?;
            }
        }
        Ok((
            self.current_model_resources()?,
            self.current_model_selection_policies()?,
        ))
    }

    pub fn current_model_resources(&self) -> Result<Vec<ModelResource>> {
        let mut statement = self.connection.prepare(
            "SELECT resource_json FROM model_resource_revisions WHERE is_current = 1 ORDER BY id",
        )?;
        let resources = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str::<ModelResource>(&row?)?))
            .collect::<Result<Vec<_>>>();
        resources
    }

    pub fn model_resource(&self, id: &str, revision: Option<u64>) -> Result<Option<ModelResource>> {
        let sql = if revision.is_some() {
            "SELECT resource_json FROM model_resource_revisions WHERE id = ?1 AND revision = ?2"
        } else {
            "SELECT resource_json FROM model_resource_revisions WHERE id = ?1 AND is_current = 1"
        };
        let resource = if let Some(revision) = revision {
            self.connection
                .query_row(sql, params![id, revision], |row| row.get::<_, String>(0))
                .optional()?
        } else {
            self.connection
                .query_row(sql, params![id], |row| row.get::<_, String>(0))
                .optional()?
        };
        resource
            .map(|resource| serde_json::from_str(&resource).map_err(Into::into))
            .transpose()
    }

    pub fn current_model_selection_policies(&self) -> Result<Vec<ModelSelectionPolicy>> {
        let mut statement = self.connection.prepare(
            "SELECT policy_json FROM model_selection_policy_revisions WHERE is_current = 1 ORDER BY id",
        )?;
        let policies = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str::<ModelSelectionPolicy>(&row?)?))
            .collect::<Result<Vec<_>>>();
        policies
    }

    pub fn model_selection_policy(
        &self,
        id: &str,
        revision: Option<u64>,
    ) -> Result<Option<ModelSelectionPolicy>> {
        let sql = if revision.is_some() {
            "SELECT policy_json FROM model_selection_policy_revisions WHERE id = ?1 AND revision = ?2"
        } else {
            "SELECT policy_json FROM model_selection_policy_revisions WHERE id = ?1 AND is_current = 1"
        };
        let policy = if let Some(revision) = revision {
            self.connection
                .query_row(sql, params![id, revision], |row| row.get::<_, String>(0))
                .optional()?
        } else {
            self.connection
                .query_row(sql, params![id], |row| row.get::<_, String>(0))
                .optional()?
        };
        policy
            .map(|policy| serde_json::from_str(&policy).map_err(Into::into))
            .transpose()
    }

    pub fn save_model_resource(
        &self,
        mut resource: ModelResource,
        expected_revision: Option<u64>,
    ) -> Result<ModelResource> {
        let current_revision = self.current_resource_revision(&resource.id)?;
        match (current_revision, expected_revision) {
            (Some(current), Some(expected)) if current == expected => {}
            (Some(current), _) => {
                return Err(anyhow!(
                    "resource revision conflict for {}: expected {:?}, current {}",
                    resource.id,
                    expected_revision,
                    current
                ))
            }
            (None, None | Some(0)) => {}
            (None, Some(expected)) => {
                return Err(anyhow!(
                    "resource {} does not exist at expected revision {}",
                    resource.id,
                    expected
                ))
            }
        }
        resource.revision = current_revision.unwrap_or(0) + 1;
        self.connection.execute(
            "UPDATE model_resource_revisions SET is_current = 0 WHERE id = ?1 AND is_current = 1",
            params![resource.id],
        )?;
        self.insert_resource_revision(&resource, true)?;
        self.audit(
            "model_resource.saved",
            &resource.id,
            &serde_json::json!({
                "revision": resource.revision,
                "enabled": resource.enabled,
                "kind": &resource.kind,
                "physicalModel": &resource.physical_model,
                "authType": &resource.auth.auth_type,
                "secretConfigured": !resource.auth.secret_ref.trim().is_empty(),
            }),
        )?;
        Ok(resource)
    }

    pub fn set_model_resource_enabled(
        &self,
        id: &str,
        enabled: bool,
        expected_revision: u64,
    ) -> Result<ModelResource> {
        let mut resource = self
            .current_model_resources()?
            .into_iter()
            .find(|resource| resource.id == id)
            .ok_or_else(|| anyhow!("model resource {id} does not exist"))?;
        resource.enabled = enabled;
        self.save_model_resource(resource, Some(expected_revision))
    }

    pub fn save_model_selection_policy(
        &self,
        mut policy: ModelSelectionPolicy,
        expected_revision: Option<u64>,
    ) -> Result<ModelSelectionPolicy> {
        let current_revision = self.current_policy_revision(&policy.id)?;
        let max_revision = self.max_policy_revision(&policy.id)?;
        match (current_revision, expected_revision) {
            (Some(current), Some(expected)) if current == expected => {}
            (Some(current), _) => {
                return Err(anyhow!(
                    "policy revision conflict for {}: expected {:?}, current {}",
                    policy.id,
                    expected_revision,
                    current
                ))
            }
            (None, None | Some(0)) => {}
            (None, Some(expected)) => {
                return Err(anyhow!(
                    "policy {} does not exist at expected revision {}",
                    policy.id,
                    expected
                ))
            }
        }
        policy.revision = max_revision.unwrap_or(0) + 1;
        self.connection.execute(
            "UPDATE model_selection_policy_revisions SET is_current = 0 WHERE id = ?1 AND is_current = 1",
            params![policy.id],
        )?;
        self.insert_policy_revision(&policy, true)?;
        self.audit("model_selection_policy.saved", &policy.id, &policy)?;
        Ok(policy)
    }

    pub fn activate_model_selection_policy(
        &self,
        id: &str,
        revision_to_activate: u64,
        expected_current_revision: u64,
    ) -> Result<ModelSelectionPolicy> {
        let policy = self
            .model_selection_policy(id, Some(revision_to_activate))?
            .ok_or_else(|| anyhow!("policy {id} revision {revision_to_activate} does not exist"))?;
        let updated = self.connection.execute(
            "UPDATE model_selection_policy_revisions
             SET is_current = 0
             WHERE id = ?1 AND revision = ?2 AND is_current = 1",
            params![id, expected_current_revision],
        )?;
        if updated != 1 {
            return Err(anyhow!(
                "policy revision conflict for {id}: expected current {expected_current_revision}"
            ));
        }
        let activated = self.connection.execute(
            "UPDATE model_selection_policy_revisions
             SET is_current = 1
             WHERE id = ?1 AND revision = ?2",
            params![id, revision_to_activate],
        )?;
        if activated != 1 {
            return Err(anyhow!(
                "policy {id} revision {revision_to_activate} disappeared during activation"
            ));
        }
        self.audit(
            "model_selection_policy.activated",
            id,
            &serde_json::json!({
                "revision": revision_to_activate,
                "previousRevision": expected_current_revision,
            }),
        )?;
        Ok(policy)
    }

    pub fn audit_admin_operation(
        &self,
        event_type: &str,
        subject_id: &str,
        operator_id: &str,
        reason: &str,
        change_reference: &str,
    ) -> Result<()> {
        self.audit(
            event_type,
            subject_id,
            &serde_json::json!({
                "operatorId": operator_id,
                "reason": reason,
                "changeReference": change_reference,
            }),
        )
    }

    pub fn audit_event(
        &self,
        event_type: &str,
        subject_id: &str,
        metadata: &impl serde::Serialize,
    ) -> Result<()> {
        self.audit(event_type, subject_id, metadata)
    }

    pub fn audit_events(
        &self,
        event_type: Option<&str>,
        subject_id: Option<&str>,
        before_id: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AuditEventRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT id, event_type, subject_id, metadata_json, created_at
             FROM provider_gateway_audit_events
             WHERE (?1 IS NULL OR event_type = ?1)
               AND (?2 IS NULL OR subject_id = ?2)
               AND (?3 IS NULL OR id < ?3)
             ORDER BY id DESC
             LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![event_type, subject_id, before_id, i64::from(limit)],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?;
        let events = rows
            .map(|row| {
                let (id, event_type, subject_id, metadata, created_at) = row?;
                Ok(AuditEventRecord {
                    id,
                    event_type,
                    subject_id,
                    metadata: serde_json::from_str(&metadata)?,
                    created_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(events)
    }

    pub fn save_encrypted_secret(&self, name: &str, ciphertext: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO provider_gateway_encrypted_secrets (name, ciphertext)
             VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET
                ciphertext = excluded.ciphertext,
                updated_at = CURRENT_TIMESTAMP",
            params![name, ciphertext],
        )?;
        Ok(())
    }

    pub fn encrypted_secret(&self, name: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT ciphertext FROM provider_gateway_encrypted_secrets WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn current_resource_revision(&self, id: &str) -> Result<Option<u64>> {
        self.connection
            .query_row(
                "SELECT revision FROM model_resource_revisions WHERE id = ?1 AND is_current = 1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn current_policy_revision(&self, id: &str) -> Result<Option<u64>> {
        self.connection
            .query_row(
                "SELECT revision FROM model_selection_policy_revisions WHERE id = ?1 AND is_current = 1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    fn max_policy_revision(&self, id: &str) -> Result<Option<u64>> {
        self.connection
            .query_row(
                "SELECT MAX(revision) FROM model_selection_policy_revisions WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn insert_resource_revision(&self, resource: &ModelResource, current: bool) -> Result<()> {
        self.connection.execute(
            "INSERT INTO model_resource_revisions (id, revision, enabled, is_current, resource_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                resource.id,
                resource.revision,
                resource.enabled,
                current,
                serde_json::to_string(resource)?,
            ],
        )?;
        Ok(())
    }

    fn insert_policy_revision(&self, policy: &ModelSelectionPolicy, current: bool) -> Result<()> {
        self.connection.execute(
            "INSERT INTO model_selection_policy_revisions (id, revision, is_current, policy_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                policy.id,
                policy.revision,
                current,
                serde_json::to_string(policy)?,
            ],
        )?;
        Ok(())
    }

    fn audit(
        &self,
        event_type: &str,
        subject_id: &str,
        metadata: &impl serde::Serialize,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO provider_gateway_audit_events (event_type, subject_id, metadata_json)
             VALUES (?1, ?2, ?3)",
            params![event_type, subject_id, serde_json::to_string(metadata)?],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{PersistentStore, SqliteStore, StoredTurn};

    #[test]
    fn resource_circuit_recovers_through_half_open_probe() {
        let store = SqliteStore::open(":memory:").unwrap();
        let now = 1_000_000;
        for _ in 0..3 {
            store
                .record_resource_retryable_failure("resource-a", 1, now)
                .unwrap();
        }
        assert!(!store
            .resource_circuit_allows("resource-a", 1, now + 29)
            .unwrap());
        assert!(store
            .resource_circuit_allows("resource-a", 1, now + 30)
            .unwrap());
        store.record_resource_success("resource-a", 1).unwrap();
        assert!(store
            .resource_circuit_allows("resource-a", 1, now + 31)
            .unwrap());
    }

    #[test]
    #[ignore = "requires PROVIDER_GATEWAY_POSTGRES_TEST_URL"]
    fn postgres_store_shares_idempotency_quota_and_circuit_state_across_connections() {
        let database_url = std::env::var("PROVIDER_GATEWAY_POSTGRES_TEST_URL")
            .expect("PROVIDER_GATEWAY_POSTGRES_TEST_URL must be set");
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let first = PersistentStore::open(&database_url).unwrap();
        let second = PersistentStore::open(&database_url).unwrap();
        let key = format!("postgres-test-turn-{suffix}");
        assert!(matches!(
            first.reserve_turn(&key, "request-hash").unwrap(),
            StoredTurn::Reserved
        ));
        assert!(matches!(
            second.reserve_turn(&key, "request-hash").unwrap(),
            StoredTurn::InProgress
        ));

        let project_id = format!("postgres-test-project-{suffix}");
        assert!(first
            .reserve_project_daily_input_tokens(
                "2099-01-01",
                "postgres-test-org",
                &project_id,
                2,
                3,
            )
            .unwrap());
        assert!(!second
            .reserve_project_daily_input_tokens(
                "2099-01-01",
                "postgres-test-org",
                &project_id,
                2,
                3,
            )
            .unwrap());

        let scope = format!("postgres-test-concurrency-{suffix}");
        let lease_one = format!("postgres-test-lease-one-{suffix}");
        let lease_two = format!("postgres-test-lease-two-{suffix}");
        assert!(first
            .acquire_project_concurrency_lease(&scope, &lease_one, 1, i64::MAX)
            .unwrap());
        assert!(!second
            .acquire_project_concurrency_lease(&scope, &lease_two, 1, i64::MAX)
            .unwrap());
        first.release_project_concurrency_lease(&lease_one).unwrap();
        assert!(second
            .acquire_project_concurrency_lease(&scope, &lease_two, 1, i64::MAX)
            .unwrap());

        let secret_name = format!("postgres-test-secret-{suffix}");
        first
            .save_encrypted_secret(&secret_name, "ciphertext-only")
            .unwrap();
        assert_eq!(
            second.encrypted_secret(&secret_name).unwrap().as_deref(),
            Some("ciphertext-only")
        );

        for _ in 0..3 {
            first
                .record_resource_retryable_failure("postgres-test-resource", 1, 1_000_000)
                .unwrap();
        }
        assert!(!second
            .resource_circuit_allows("postgres-test-resource", 1, 1_000_001)
            .unwrap());
    }
}
