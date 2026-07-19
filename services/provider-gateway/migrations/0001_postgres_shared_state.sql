CREATE TABLE IF NOT EXISTS provider_gateway_schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS turn_idempotency_records (
    idempotency_key TEXT PRIMARY KEY,
    request_hash TEXT NOT NULL,
    state TEXT NOT NULL,
    response_json TEXT,
    response_expires_at BIGINT,
    error_json TEXT,
    error_status INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS turn_idempotency_response_expiry
    ON turn_idempotency_records(response_expires_at)
    WHERE state = 'completed';

CREATE TABLE IF NOT EXISTS admin_idempotency_records (
    idempotency_key TEXT PRIMARY KEY,
    request_hash TEXT NOT NULL,
    state TEXT NOT NULL,
    response_json TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS model_execution_snapshots (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    turn INTEGER NOT NULL,
    model_resource_id TEXT NOT NULL,
    model_resource_revision BIGINT NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS model_execution_snapshots_run_turn
    ON model_execution_snapshots(run_id, turn, created_at);

CREATE TABLE IF NOT EXISTS model_resource_revisions (
    id TEXT NOT NULL,
    revision BIGINT NOT NULL,
    enabled BOOLEAN NOT NULL,
    is_current BOOLEAN NOT NULL,
    resource_json TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (id, revision)
);

CREATE UNIQUE INDEX IF NOT EXISTS model_resource_current
    ON model_resource_revisions(id) WHERE is_current = TRUE;

CREATE TABLE IF NOT EXISTS model_selection_policy_revisions (
    id TEXT NOT NULL,
    revision BIGINT NOT NULL,
    is_current BOOLEAN NOT NULL,
    policy_json TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (id, revision)
);

CREATE UNIQUE INDEX IF NOT EXISTS model_selection_policy_current
    ON model_selection_policy_revisions(id) WHERE is_current = TRUE;

CREATE TABLE IF NOT EXISTS model_resource_health (
    model_resource_id TEXT NOT NULL,
    model_resource_revision BIGINT NOT NULL,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    circuit_state TEXT NOT NULL DEFAULT 'closed',
    opened_until_epoch_seconds BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (model_resource_id, model_resource_revision)
);

CREATE TABLE IF NOT EXISTS project_daily_quota_usage (
    period_utc TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (period_utc, workspace_id, project_id)
);

CREATE TABLE IF NOT EXISTS project_concurrency_leases (
    lease_id TEXT PRIMARY KEY,
    scope_key TEXT NOT NULL,
    expires_at_epoch_seconds BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS project_concurrency_leases_scope
    ON project_concurrency_leases(scope_key, expires_at_epoch_seconds);

CREATE TABLE IF NOT EXISTS provider_gateway_encrypted_secrets (
    name TEXT PRIMARY KEY,
    ciphertext TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS provider_gateway_audit_events (
    id BIGSERIAL PRIMARY KEY,
    event_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO provider_gateway_schema_migrations (version)
VALUES (2)
ON CONFLICT (version) DO NOTHING;
