BEGIN;

CREATE TABLE IF NOT EXISTS product_catalog_metadata (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workspaces (
  namespace TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  owner_principal_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS workspaces_owner_status
  ON workspaces(owner_principal_id, status, updated_at DESC);

CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  owner_id TEXT NOT NULL,
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('website', 'docs')),
  runtime_project_id TEXT NOT NULL UNIQUE,
  workspace_namespace TEXT NOT NULL REFERENCES workspaces(namespace),
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS projects_owner_updated
  ON projects(owner_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS project_runs (
  run_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  phase TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS project_runs_project_created
  ON project_runs(project_id, created_at DESC);

CREATE TABLE IF NOT EXISTS project_versions (
  project_id TEXT NOT NULL REFERENCES projects(id),
  version_id TEXT NOT NULL,
  status TEXT NOT NULL,
  first_seen_at TEXT NOT NULL,
  PRIMARY KEY(project_id, version_id)
);

CREATE TABLE IF NOT EXISTS project_publication_jobs (
  project_id TEXT PRIMARY KEY REFERENCES projects(id),
  version_id TEXT,
  release_id TEXT,
  packaging_id TEXT,
  operation_id TEXT,
  idempotency_key TEXT,
  expected_generation BIGINT,
  expected_current_release_id TEXT,
  action TEXT NOT NULL CHECK (action IN ('publish', 'rollback', 'unpublish')),
  phase TEXT NOT NULL CHECK (phase IN ('packaging', 'publication')),
  status TEXT NOT NULL,
  last_error TEXT,
  updated_at TEXT NOT NULL
);

INSERT INTO product_catalog_metadata (key, value, updated_at)
VALUES ('schema_version', 'product-catalog@1', CURRENT_TIMESTAMP::TEXT)
ON CONFLICT(key) DO UPDATE
SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at;

COMMIT;
