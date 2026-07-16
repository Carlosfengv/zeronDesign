use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use postgres::{Client, GenericClient, NoTls, Row};
use std::sync::Mutex;

use crate::storage::{AuditEventRecord, ResourceHealthState, StoredAdminOperation, StoredTurn};
use crate::{
    GatewayErrorEnvelope, GatewayTurnResponse, ModelExecutionSummary, ModelResource,
    ModelSelectionPolicy,
};

pub struct PostgresStore {
    database_url: String,
    connection: Mutex<Option<Client>>,
}

impl PostgresStore {
    pub fn open(database_url: &str) -> Result<Self> {
        let database_url_owned = database_url.to_string();
        let connection = std::thread::spawn({
            let database_url = database_url_owned.clone();
            move || connect(&database_url)
        })
        .join()
        .map_err(|_| anyhow!("Provider Gateway PostgreSQL startup thread panicked"))??;
        Ok(Self {
            database_url: database_url_owned,
            connection: Mutex::new(Some(connection)),
        })
    }

    fn with_client<T: Send>(
        &self,
        operation: impl FnOnce(&mut Client) -> Result<T> + Send,
    ) -> Result<T> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let mut connection = self.connection.lock().map_err(|_| {
                        anyhow!("Provider Gateway PostgreSQL connection lock poisoned")
                    })?;
                    if connection.is_none() {
                        *connection = Some(connect(&self.database_url)?);
                    }
                    let result = operation(
                        connection
                            .as_mut()
                            .ok_or_else(|| anyhow!("PostgreSQL connection is unavailable"))?,
                    );
                    if result.is_err()
                        && connection.as_ref().is_some_and(postgres::Client::is_closed)
                    {
                        *connection = None;
                    }
                    result
                })
                .join()
                .map_err(|_| anyhow!("Provider Gateway PostgreSQL operation thread panicked"))?
        })
    }

    pub fn reserve_turn(&self, key: &str, request_hash: &str) -> Result<StoredTurn> {
        self.with_client(|client| {
            let inserted = client.execute(
                "INSERT INTO turn_idempotency_records
                    (idempotency_key, request_hash, state)
                 VALUES ($1, $2, 'in_progress')
                 ON CONFLICT (idempotency_key) DO NOTHING",
                &[&key, &request_hash],
            )?;
            if inserted == 1 {
                return Ok(StoredTurn::Reserved);
            }
            let row = client
                .query_opt(
                    "SELECT request_hash, state, response_json, error_json, error_status,
                            response_expires_at
                     FROM turn_idempotency_records
                     WHERE idempotency_key = $1",
                    &[&key],
                )?
                .ok_or_else(|| anyhow!("idempotency record disappeared during reservation"))?;
            if row.get::<_, String>(0) != request_hash {
                return Ok(StoredTurn::Conflict);
            }
            match row.get::<_, String>(1).as_str() {
                "in_progress" => Ok(StoredTurn::InProgress),
                "completed"
                    if row
                        .get::<_, Option<i64>>(5)
                        .is_some_and(|expires_at| expires_at <= Utc::now().timestamp()) =>
                {
                    client.execute(
                        "UPDATE turn_idempotency_records
                         SET state = 'expired', response_json = NULL,
                             updated_at = CURRENT_TIMESTAMP
                         WHERE idempotency_key = $1 AND state = 'completed'",
                        &[&key],
                    )?;
                    Ok(StoredTurn::Expired)
                }
                "completed" => Ok(StoredTurn::Completed(
                    row.get::<_, Option<String>>(2)
                        .ok_or_else(|| anyhow!("completed idempotency record has no response"))?,
                )),
                "expired" => Ok(StoredTurn::Expired),
                "failed" => Ok(StoredTurn::Failed {
                    status: row
                        .get::<_, Option<i32>>(4)
                        .and_then(|status| u16::try_from(status).ok())
                        .ok_or_else(|| anyhow!("failed idempotency record has no HTTP status"))?,
                    error: serde_json::from_str(
                        row.get::<_, Option<String>>(3)
                            .as_deref()
                            .ok_or_else(|| anyhow!("failed idempotency record has no error"))?,
                    )?,
                }),
                state => Err(anyhow!("unknown idempotency state {state}")),
            }
        })
    }

    pub fn reserve_admin_operation(
        &self,
        key: &str,
        request_hash: &str,
    ) -> Result<StoredAdminOperation> {
        self.with_client(|client| {
            let inserted = client.execute(
                "INSERT INTO admin_idempotency_records
                    (idempotency_key, request_hash, state)
                 VALUES ($1, $2, 'in_progress')
                 ON CONFLICT (idempotency_key) DO NOTHING",
                &[&key, &request_hash],
            )?;
            if inserted == 1 {
                return Ok(StoredAdminOperation::Reserved);
            }
            let row = client
                .query_opt(
                    "SELECT request_hash, state, response_json
                     FROM admin_idempotency_records WHERE idempotency_key = $1",
                    &[&key],
                )?
                .ok_or_else(|| anyhow!("admin idempotency record disappeared"))?;
            if row.get::<_, String>(0) != request_hash {
                return Ok(StoredAdminOperation::Conflict);
            }
            match row.get::<_, String>(1).as_str() {
                "in_progress" => Ok(StoredAdminOperation::InProgress),
                "completed" => Ok(StoredAdminOperation::Completed(
                    row.get::<_, Option<String>>(2).ok_or_else(|| {
                        anyhow!("completed admin idempotency record has no response")
                    })?,
                )),
                state => Err(anyhow!("unknown admin idempotency state {state}")),
            }
        })
    }

    pub fn complete_admin_operation(&self, key: &str, response_json: &str) -> Result<()> {
        self.with_client(|client| {
            let updated = client.execute(
                "UPDATE admin_idempotency_records
                 SET state = 'completed', response_json = $2, updated_at = CURRENT_TIMESTAMP
                 WHERE idempotency_key = $1 AND state = 'in_progress'",
                &[&key, &response_json],
            )?;
            if updated != 1 {
                return Err(anyhow!("admin idempotency reservation is no longer active"));
            }
            Ok(())
        })
    }

    pub fn discard_admin_operation(&self, key: &str) -> Result<()> {
        self.with_client(|client| {
            client.execute(
                "DELETE FROM admin_idempotency_records
                 WHERE idempotency_key = $1 AND state = 'in_progress'",
                &[&key],
            )?;
            Ok(())
        })
    }

    pub fn resource_circuit_allows(
        &self,
        resource_id: &str,
        revision: u64,
        now_epoch_seconds: i64,
    ) -> Result<bool> {
        self.with_client(|client| {
            let revision = to_i64(revision, "model resource revision")?;
            let mut transaction = client.transaction()?;
            transaction.execute(
                "INSERT INTO model_resource_health
                    (model_resource_id, model_resource_revision)
                 VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
                &[&resource_id, &revision],
            )?;
            let row = transaction.query_one(
                "SELECT circuit_state, opened_until_epoch_seconds
                 FROM model_resource_health
                 WHERE model_resource_id = $1 AND model_resource_revision = $2
                 FOR UPDATE",
                &[&resource_id, &revision],
            )?;
            let state = row.get::<_, String>(0);
            let until = row.get::<_, i64>(1);
            if state == "open" && until > now_epoch_seconds {
                transaction.commit()?;
                return Ok(false);
            }
            if state == "open" {
                transaction.execute(
                    "UPDATE model_resource_health
                     SET circuit_state = 'half_open', updated_at = CURRENT_TIMESTAMP
                     WHERE model_resource_id = $1 AND model_resource_revision = $2",
                    &[&resource_id, &revision],
                )?;
            }
            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn record_resource_success(&self, resource_id: &str, revision: u64) -> Result<()> {
        self.with_client(|client| {
            let revision = to_i64(revision, "model resource revision")?;
            client.execute(
                "INSERT INTO model_resource_health
                    (model_resource_id, model_resource_revision, consecutive_failures,
                     circuit_state, opened_until_epoch_seconds)
                 VALUES ($1, $2, 0, 'closed', 0)
                 ON CONFLICT (model_resource_id, model_resource_revision) DO UPDATE SET
                    consecutive_failures = 0,
                    circuit_state = 'closed',
                    opened_until_epoch_seconds = 0,
                    updated_at = CURRENT_TIMESTAMP",
                &[&resource_id, &revision],
            )?;
            Ok(())
        })
    }

    pub fn record_resource_retryable_failure(
        &self,
        resource_id: &str,
        revision: u64,
        now_epoch_seconds: i64,
    ) -> Result<bool> {
        self.with_client(|client| {
            let revision = to_i64(revision, "model resource revision")?;
            let mut transaction = client.transaction()?;
            transaction.execute(
                "INSERT INTO model_resource_health
                    (model_resource_id, model_resource_revision)
                 VALUES ($1, $2)
                 ON CONFLICT DO NOTHING",
                &[&resource_id, &revision],
            )?;
            let row = transaction.query_one(
                "SELECT consecutive_failures
                 FROM model_resource_health
                 WHERE model_resource_id = $1 AND model_resource_revision = $2
                 FOR UPDATE",
                &[&resource_id, &revision],
            )?;
            let failures = row.get::<_, i32>(0).saturating_add(1);
            let open = failures >= 3;
            let opened_until = if open {
                now_epoch_seconds.saturating_add(30)
            } else {
                0
            };
            transaction.execute(
                "UPDATE model_resource_health
                 SET consecutive_failures = $3,
                     circuit_state = $4,
                     opened_until_epoch_seconds = $5,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE model_resource_id = $1 AND model_resource_revision = $2",
                &[
                    &resource_id,
                    &revision,
                    &failures,
                    &if open { "open" } else { "closed" },
                    &opened_until,
                ],
            )?;
            transaction.commit()?;
            Ok(open)
        })
    }

    pub fn resource_health_states(&self) -> Result<Vec<ResourceHealthState>> {
        self.with_client(|client| {
            client
                .query(
                    "SELECT model_resource_id, model_resource_revision, circuit_state
                     FROM model_resource_health",
                    &[],
                )?
                .into_iter()
                .map(|row| {
                    Ok(ResourceHealthState {
                        model_resource_id: row.get(0),
                        model_resource_revision: to_u64(row.get::<_, i64>(1), "revision")?,
                        circuit_state: row.get(2),
                    })
                })
                .collect()
        })
    }

    pub fn reserve_project_daily_input_tokens(
        &self,
        period_utc: &str,
        organization_id: &str,
        project_id: &str,
        requested_tokens: u64,
        limit: u64,
    ) -> Result<bool> {
        self.with_client(|client| {
            let requested = to_i64(requested_tokens, "requested token reservation")?;
            let limit = to_i64(limit, "daily token limit")?;
            let affected = client.execute(
                "INSERT INTO project_daily_quota_usage
                    (period_utc, organization_id, project_id, input_tokens)
                 SELECT $1::text, $2::text, $3::text, $4::bigint
                 WHERE $4::bigint <= $5::bigint
                 ON CONFLICT (period_utc, organization_id, project_id) DO UPDATE SET
                    input_tokens = project_daily_quota_usage.input_tokens + EXCLUDED.input_tokens,
                    updated_at = CURRENT_TIMESTAMP
                 WHERE project_daily_quota_usage.input_tokens + EXCLUDED.input_tokens
                       <= $5::bigint",
                &[
                    &period_utc,
                    &organization_id,
                    &project_id,
                    &requested,
                    &limit,
                ],
            )?;
            Ok(affected == 1)
        })
    }

    pub fn acquire_project_concurrency_lease(
        &self,
        scope_key: &str,
        lease_id: &str,
        limit: usize,
        expires_at_epoch_seconds: i64,
    ) -> Result<bool> {
        self.with_client(|client| {
            let mut transaction = client.transaction()?;
            transaction.query_one(
                "SELECT pg_advisory_xact_lock(hashtext($1)::bigint)",
                &[&scope_key],
            )?;
            transaction.execute(
                "DELETE FROM project_concurrency_leases
                 WHERE scope_key = $1 AND expires_at_epoch_seconds <= $2",
                &[&scope_key, &Utc::now().timestamp()],
            )?;
            let active = transaction
                .query_one(
                    "SELECT COUNT(*) FROM project_concurrency_leases WHERE scope_key = $1",
                    &[&scope_key],
                )?
                .get::<_, i64>(0);
            if active >= i64::try_from(limit).unwrap_or(i64::MAX) {
                transaction.commit()?;
                return Ok(false);
            }
            transaction.execute(
                "INSERT INTO project_concurrency_leases
                    (lease_id, scope_key, expires_at_epoch_seconds)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (lease_id) DO UPDATE SET
                    expires_at_epoch_seconds = EXCLUDED.expires_at_epoch_seconds",
                &[&lease_id, &scope_key, &expires_at_epoch_seconds],
            )?;
            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn release_project_concurrency_lease(&self, lease_id: &str) -> Result<()> {
        self.with_client(|client| {
            client.execute(
                "DELETE FROM project_concurrency_leases WHERE lease_id = $1",
                &[&lease_id],
            )?;
            Ok(())
        })
    }

    pub fn settle_project_daily_usage(
        &self,
        period_utc: &str,
        organization_id: &str,
        project_id: &str,
        reserved_input_tokens: u64,
        actual_input_tokens: u64,
    ) -> Result<()> {
        self.with_client(|client| {
            let reserved = to_i64(reserved_input_tokens, "reserved token value")?;
            let actual = to_i64(actual_input_tokens, "actual input token value")?;
            client.execute(
                "UPDATE project_daily_quota_usage
                 SET input_tokens = GREATEST(0, input_tokens + $4 - $5),
                     updated_at = CURRENT_TIMESTAMP
                 WHERE period_utc = $1 AND organization_id = $2 AND project_id = $3",
                &[
                    &period_utc,
                    &organization_id,
                    &project_id,
                    &actual,
                    &reserved,
                ],
            )?;
            Ok(())
        })
    }

    pub fn complete_turn(
        &self,
        key: &str,
        encrypted_response: &str,
        response: &GatewayTurnResponse,
        run_id: &str,
        turn: u32,
    ) -> Result<()> {
        self.with_client(|client| {
            let turn = i32::try_from(turn).context("turn value is too large")?;
            let resource_revision = to_i64(
                response.model_execution.model_resource_revision,
                "resource revision",
            )?;
            let snapshot_json = serde_json::to_string(&response.model_execution)?;
            let mut transaction = client.transaction()?;
            let updated = transaction.execute(
                "UPDATE turn_idempotency_records
                 SET state = 'completed', response_json = $2,
                     response_expires_at = $3, error_json = NULL, error_status = NULL,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE idempotency_key = $1 AND state = 'in_progress'",
                &[
                    &key,
                    &encrypted_response,
                    &Utc::now().timestamp().saturating_add(24 * 60 * 60),
                ],
            )?;
            if updated != 1 {
                return Err(anyhow!(
                    "idempotency reservation disappeared during completion"
                ));
            }
            transaction.execute(
                "INSERT INTO model_execution_snapshots
                    (id, run_id, turn, model_resource_id, model_resource_revision, snapshot_json)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (id) DO NOTHING",
                &[
                    &response.model_execution.id,
                    &run_id,
                    &turn,
                    &response.model_execution.model_resource_id,
                    &resource_revision,
                    &snapshot_json,
                ],
            )?;
            insert_audit(
                &mut transaction,
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
                insert_audit(
                    &mut transaction,
                    "model_execution.automatic_switch",
                    &response.model_execution.id,
                    &response.model_execution.automatic_switch,
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn fail_turn(&self, key: &str, status: u16, error: &GatewayErrorEnvelope) -> Result<()> {
        self.with_client(|client| {
            let status = i32::from(status);
            client.execute(
                "UPDATE turn_idempotency_records
                 SET state = 'failed', response_json = NULL, response_expires_at = NULL,
                     error_json = $2, error_status = $3, updated_at = CURRENT_TIMESTAMP
                 WHERE idempotency_key = $1 AND state = 'in_progress'",
                &[&key, &serde_json::to_string(error)?, &status],
            )?;
            Ok(())
        })
    }

    pub fn execution_snapshot(&self, id: &str) -> Result<Option<ModelExecutionSummary>> {
        self.with_client(|client| {
            client
                .query_opt(
                    "SELECT snapshot_json FROM model_execution_snapshots WHERE id = $1",
                    &[&id],
                )?
                .map(|row| {
                    serde_json::from_str(row.get::<_, String>(0).as_str()).map_err(Into::into)
                })
                .transpose()
        })
    }

    pub fn execution_snapshots_for_run(&self, run_id: &str) -> Result<Vec<ModelExecutionSummary>> {
        self.with_client(|client| {
            client
                .query(
                    "SELECT snapshot_json FROM model_execution_snapshots
                     WHERE run_id = $1 ORDER BY turn, created_at",
                    &[&run_id],
                )?
                .into_iter()
                .map(|row| {
                    serde_json::from_str(row.get::<_, String>(0).as_str()).map_err(Into::into)
                })
                .collect()
        })
    }

    pub fn initialize_configuration(
        &self,
        configured_resources: &[ModelResource],
        configured_policies: &[ModelSelectionPolicy],
    ) -> Result<(Vec<ModelResource>, Vec<ModelSelectionPolicy>)> {
        self.with_client(|client| {
            let mut transaction = client.transaction()?;
            transaction.query_one("SELECT pg_advisory_xact_lock(8976211105)", &[])?;
            let resource_count = transaction
                .query_one(
                    "SELECT COUNT(*) FROM model_resource_revisions WHERE is_current = TRUE",
                    &[],
                )?
                .get::<_, i64>(0);
            let policy_count = transaction
                .query_one(
                    "SELECT COUNT(*) FROM model_selection_policy_revisions WHERE is_current = TRUE",
                    &[],
                )?
                .get::<_, i64>(0);
            if resource_count == 0 && policy_count == 0 {
                for resource in configured_resources {
                    insert_resource_revision(&mut transaction, resource, true)?;
                }
                for policy in configured_policies {
                    insert_policy_revision(&mut transaction, policy, true)?;
                }
            }
            transaction.commit()?;
            Ok(())
        })?;
        Ok((
            self.current_model_resources()?,
            self.current_model_selection_policies()?,
        ))
    }

    pub fn current_model_resources(&self) -> Result<Vec<ModelResource>> {
        self.with_client(|client| {
            client
                .query(
                    "SELECT resource_json FROM model_resource_revisions
                     WHERE is_current = TRUE ORDER BY id",
                    &[],
                )?
                .into_iter()
                .map(parse_resource)
                .collect()
        })
    }

    pub fn model_resource(&self, id: &str, revision: Option<u64>) -> Result<Option<ModelResource>> {
        self.with_client(|client| {
            let row = if let Some(revision) = revision {
                let revision = to_i64(revision, "resource revision")?;
                client.query_opt(
                    "SELECT resource_json FROM model_resource_revisions
                     WHERE id = $1 AND revision = $2",
                    &[&id, &revision],
                )?
            } else {
                client.query_opt(
                    "SELECT resource_json FROM model_resource_revisions
                     WHERE id = $1 AND is_current = TRUE",
                    &[&id],
                )?
            };
            row.map(parse_resource).transpose()
        })
    }

    pub fn current_model_selection_policies(&self) -> Result<Vec<ModelSelectionPolicy>> {
        self.with_client(|client| {
            client
                .query(
                    "SELECT policy_json FROM model_selection_policy_revisions
                     WHERE is_current = TRUE ORDER BY id",
                    &[],
                )?
                .into_iter()
                .map(parse_policy)
                .collect()
        })
    }

    pub fn model_selection_policy(
        &self,
        id: &str,
        revision: Option<u64>,
    ) -> Result<Option<ModelSelectionPolicy>> {
        self.with_client(|client| {
            let row = if let Some(revision) = revision {
                let revision = to_i64(revision, "policy revision")?;
                client.query_opt(
                    "SELECT policy_json FROM model_selection_policy_revisions
                     WHERE id = $1 AND revision = $2",
                    &[&id, &revision],
                )?
            } else {
                client.query_opt(
                    "SELECT policy_json FROM model_selection_policy_revisions
                     WHERE id = $1 AND is_current = TRUE",
                    &[&id],
                )?
            };
            row.map(parse_policy).transpose()
        })
    }

    pub fn save_model_resource(
        &self,
        mut resource: ModelResource,
        expected_revision: Option<u64>,
    ) -> Result<ModelResource> {
        self.with_client(|client| {
            let mut transaction = client.transaction()?;
            transaction.query_one(
                "SELECT pg_advisory_xact_lock(hashtext($1)::bigint)",
                &[&resource.id],
            )?;
            let current =
                current_revision(&mut transaction, "model_resource_revisions", &resource.id)?;
            check_revision("resource", &resource.id, current, expected_revision)?;
            resource.revision = current.unwrap_or(0).saturating_add(1);
            transaction.execute(
                "UPDATE model_resource_revisions SET is_current = FALSE
                 WHERE id = $1 AND is_current = TRUE",
                &[&resource.id],
            )?;
            insert_resource_revision(&mut transaction, &resource, true)?;
            insert_audit(
                &mut transaction,
                "model_resource.saved",
                &resource.id,
                &serde_json::json!({
                    "revision": resource.revision,
                    "enabled": resource.enabled,
                    "kind": resource.kind,
                    "physicalModel": resource.physical_model,
                    "authType": resource.auth.auth_type,
                    "secretConfigured": !resource.auth.secret_ref.trim().is_empty(),
                }),
            )?;
            transaction.commit()?;
            Ok(resource)
        })
    }

    pub fn set_model_resource_enabled(
        &self,
        id: &str,
        enabled: bool,
        expected_revision: u64,
    ) -> Result<ModelResource> {
        let mut resource = self
            .model_resource(id, None)?
            .ok_or_else(|| anyhow!("model resource {id} does not exist"))?;
        resource.enabled = enabled;
        self.save_model_resource(resource, Some(expected_revision))
    }

    pub fn save_model_selection_policy(
        &self,
        mut policy: ModelSelectionPolicy,
        expected_revision: Option<u64>,
    ) -> Result<ModelSelectionPolicy> {
        self.with_client(|client| {
            let mut transaction = client.transaction()?;
            transaction.query_one(
                "SELECT pg_advisory_xact_lock(hashtext($1)::bigint)",
                &[&policy.id],
            )?;
            let current = current_revision(
                &mut transaction,
                "model_selection_policy_revisions",
                &policy.id,
            )?;
            check_revision("policy", &policy.id, current, expected_revision)?;
            let max_revision = transaction
                .query_one(
                    "SELECT MAX(revision) FROM model_selection_policy_revisions WHERE id = $1",
                    &[&policy.id],
                )?
                .get::<_, Option<i64>>(0)
                .map(|value| to_u64(value, "policy revision"))
                .transpose()?
                .unwrap_or(0);
            policy.revision = max_revision.saturating_add(1);
            transaction.execute(
                "UPDATE model_selection_policy_revisions SET is_current = FALSE
                 WHERE id = $1 AND is_current = TRUE",
                &[&policy.id],
            )?;
            insert_policy_revision(&mut transaction, &policy, true)?;
            insert_audit(
                &mut transaction,
                "model_selection_policy.saved",
                &policy.id,
                &policy,
            )?;
            transaction.commit()?;
            Ok(policy)
        })
    }

    pub fn activate_model_selection_policy(
        &self,
        id: &str,
        revision_to_activate: u64,
        expected_current_revision: u64,
    ) -> Result<ModelSelectionPolicy> {
        self.with_client(|client| {
            let target_revision = to_i64(revision_to_activate, "policy revision")?;
            let expected_revision = to_i64(expected_current_revision, "policy revision")?;
            let mut transaction = client.transaction()?;
            transaction.query_one(
                "SELECT pg_advisory_xact_lock(hashtext($1)::bigint)",
                &[&id],
            )?;
            let policy = transaction
                .query_opt(
                    "SELECT policy_json FROM model_selection_policy_revisions
                     WHERE id = $1 AND revision = $2",
                    &[&id, &target_revision],
                )?
                .map(parse_policy)
                .transpose()?
                .ok_or_else(|| {
                    anyhow!("policy {id} revision {revision_to_activate} does not exist")
                })?;
            let updated = transaction.execute(
                "UPDATE model_selection_policy_revisions SET is_current = FALSE
                 WHERE id = $1 AND revision = $2 AND is_current = TRUE",
                &[&id, &expected_revision],
            )?;
            if updated != 1 {
                return Err(anyhow!(
                    "policy revision conflict for {id}: expected current {expected_current_revision}"
                ));
            }
            transaction.execute(
                "UPDATE model_selection_policy_revisions SET is_current = TRUE
                 WHERE id = $1 AND revision = $2",
                &[&id, &target_revision],
            )?;
            insert_audit(
                &mut transaction,
                "model_selection_policy.activated",
                id,
                &serde_json::json!({
                    "revision": revision_to_activate,
                    "previousRevision": expected_current_revision,
                }),
            )?;
            transaction.commit()?;
            Ok(policy)
        })
    }

    pub fn audit_admin_operation(
        &self,
        event_type: &str,
        subject_id: &str,
        operator_id: &str,
        reason: &str,
        change_reference: &str,
    ) -> Result<()> {
        self.audit_event(
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
        let metadata = serde_json::to_value(metadata)?;
        self.with_client(|client| insert_audit(client, event_type, subject_id, &metadata))
    }

    pub fn audit_events(
        &self,
        event_type: Option<&str>,
        subject_id: Option<&str>,
        before_id: Option<i64>,
        limit: u16,
    ) -> Result<Vec<AuditEventRecord>> {
        self.with_client(|client| {
            client
                .query(
                    "SELECT id, event_type, subject_id, metadata_json, created_at::text
                     FROM provider_gateway_audit_events
                     WHERE ($1::text IS NULL OR event_type = $1)
                       AND ($2::text IS NULL OR subject_id = $2)
                       AND ($3::bigint IS NULL OR id < $3)
                     ORDER BY id DESC LIMIT $4",
                    &[&event_type, &subject_id, &before_id, &i64::from(limit)],
                )?
                .into_iter()
                .map(|row| {
                    Ok(AuditEventRecord {
                        id: row.get(0),
                        event_type: row.get(1),
                        subject_id: row.get(2),
                        metadata: serde_json::from_str(row.get::<_, String>(3).as_str())?,
                        created_at: row.get(4),
                    })
                })
                .collect()
        })
    }

    pub fn save_encrypted_secret(&self, name: &str, ciphertext: &str) -> Result<()> {
        self.with_client(|client| {
            client.execute(
                "INSERT INTO provider_gateway_encrypted_secrets (name, ciphertext)
                 VALUES ($1, $2)
                 ON CONFLICT (name) DO UPDATE SET
                    ciphertext = EXCLUDED.ciphertext,
                    updated_at = CURRENT_TIMESTAMP",
                &[&name, &ciphertext],
            )?;
            Ok(())
        })
    }

    pub fn encrypted_secret(&self, name: &str) -> Result<Option<String>> {
        self.with_client(|client| {
            Ok(client
                .query_opt(
                    "SELECT ciphertext FROM provider_gateway_encrypted_secrets WHERE name = $1",
                    &[&name],
                )?
                .map(|row| row.get(0)))
        })
    }
}

fn connect(database_url: &str) -> Result<Client> {
    let mut connection = Client::connect(database_url, NoTls)
        .context("opening Provider Gateway PostgreSQL database")?;
    connection
        .batch_execute(include_str!("../migrations/0001_postgres_shared_state.sql"))
        .context("applying Provider Gateway PostgreSQL migrations")?;
    Ok(connection)
}

fn insert_resource_revision(
    client: &mut impl GenericClient,
    resource: &ModelResource,
    current: bool,
) -> Result<()> {
    client.execute(
        "INSERT INTO model_resource_revisions
            (id, revision, enabled, is_current, resource_json)
         VALUES ($1, $2, $3, $4, $5)",
        &[
            &resource.id,
            &to_i64(resource.revision, "resource revision")?,
            &resource.enabled,
            &current,
            &serde_json::to_string(resource)?,
        ],
    )?;
    Ok(())
}

fn insert_policy_revision(
    client: &mut impl GenericClient,
    policy: &ModelSelectionPolicy,
    current: bool,
) -> Result<()> {
    client.execute(
        "INSERT INTO model_selection_policy_revisions
            (id, revision, is_current, policy_json)
         VALUES ($1, $2, $3, $4)",
        &[
            &policy.id,
            &to_i64(policy.revision, "policy revision")?,
            &current,
            &serde_json::to_string(policy)?,
        ],
    )?;
    Ok(())
}

fn insert_audit(
    client: &mut impl GenericClient,
    event_type: &str,
    subject_id: &str,
    metadata: &impl serde::Serialize,
) -> Result<()> {
    client.execute(
        "INSERT INTO provider_gateway_audit_events
            (event_type, subject_id, metadata_json)
         VALUES ($1, $2, $3)",
        &[&event_type, &subject_id, &serde_json::to_string(metadata)?],
    )?;
    Ok(())
}

fn current_revision(client: &mut impl GenericClient, table: &str, id: &str) -> Result<Option<u64>> {
    let statement = format!("SELECT revision FROM {table} WHERE id = $1 AND is_current = TRUE");
    client
        .query_opt(&statement, &[&id])?
        .map(|row| to_u64(row.get::<_, i64>(0), "revision"))
        .transpose()
}

fn check_revision(kind: &str, id: &str, current: Option<u64>, expected: Option<u64>) -> Result<()> {
    match (current, expected) {
        (Some(current), Some(expected)) if current == expected => Ok(()),
        (Some(current), _) => Err(anyhow!(
            "{kind} revision conflict for {id}: expected {expected:?}, current {current}"
        )),
        (None, None | Some(0)) => Ok(()),
        (None, Some(expected)) => Err(anyhow!(
            "{kind} {id} does not exist at expected revision {expected}"
        )),
    }
}

fn parse_resource(row: Row) -> Result<ModelResource> {
    Ok(serde_json::from_str(row.get::<_, String>(0).as_str())?)
}

fn parse_policy(row: Row) -> Result<ModelSelectionPolicy> {
    Ok(serde_json::from_str(row.get::<_, String>(0).as_str())?)
}

fn to_i64(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value).with_context(|| format!("{label} is too large"))
}

fn to_u64(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value).with_context(|| format!("{label} is negative"))
}
