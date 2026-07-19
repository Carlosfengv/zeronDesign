import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { DatabaseSync, type SQLInputValue } from "node:sqlite";
import { Pool, type PoolClient, type QueryResultRow } from "pg";

export type ProductProject = {
  id: string;
  ownerId: string;
  name: string;
  kind: "website" | "docs";
  runtimeProjectId: string;
  workspaceNamespace: string;
  status: string;
  createdAt: string;
  updatedAt: string;
  latestRunId?: string;
};

export type ProductWorkspace = {
  namespace: string;
  name: string;
  ownerPrincipalId: string;
  status: "active" | "disabled";
  createdAt: string;
  updatedAt: string;
};

export class WorkspaceUnavailableError extends Error {}

export type ProductPublicationJob = {
  projectId: string;
  versionId?: string;
  releaseId?: string;
  packagingId?: string;
  operationId?: string;
  idempotencyKey?: string;
  expectedGeneration?: number;
  expectedCurrentReleaseId?: string;
  action: "publish" | "rollback" | "unpublish";
  phase: "packaging" | "publication";
  status: string;
  lastError?: string;
  updatedAt: string;
};

type ProductDatabaseGlobals = typeof globalThis & {
  zeronDesignProductSqlite?: DatabaseSync;
  zeronDesignProductPostgres?: Pool;
  zeronDesignProductPostgresSchema?: Promise<void>;
};

const globalDatabase = globalThis as ProductDatabaseGlobals;

const productSchema = `
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
`;

function postgresUrl(): string | null {
  const value = process.env.ZERONDESIGN_PRODUCT_DATABASE_URL?.trim();
  if (!value) {
    if (process.env.NODE_ENV === "production") {
      throw new Error("ZERONDESIGN_PRODUCT_DATABASE_URL is required in production");
    }
    return null;
  }
  if (!/^postgres(?:ql)?:\/\//.test(value)) {
    throw new Error("ZERONDESIGN_PRODUCT_DATABASE_URL must use PostgreSQL");
  }
  return value;
}

async function postgresPool(): Promise<Pool | null> {
  const connectionString = postgresUrl();
  if (!connectionString) return null;
  if (!globalDatabase.zeronDesignProductPostgres) {
    const configuredMaximum = Number(process.env.ZERONDESIGN_PRODUCT_DATABASE_POOL_MAX ?? "10");
    const max = Number.isSafeInteger(configuredMaximum) && configuredMaximum >= 1
      && configuredMaximum <= 50 ? configuredMaximum : 10;
    globalDatabase.zeronDesignProductPostgres = new Pool({
      connectionString,
      max,
      application_name: "zerondesign-web-product-catalog",
      connectionTimeoutMillis: 5_000,
      idleTimeoutMillis: 30_000,
    });
  }
  if (!globalDatabase.zeronDesignProductPostgresSchema) {
    const pool = globalDatabase.zeronDesignProductPostgres;
    globalDatabase.zeronDesignProductPostgresSchema = (async () => {
      if (process.env.ZERONDESIGN_PRODUCT_DATABASE_AUTO_MIGRATE === "true") {
        if (process.env.NODE_ENV === "production") {
          throw new Error("product catalog auto-migration is forbidden in production");
        }
        await pool.query(productSchema);
        await pool.query(
          `INSERT INTO product_catalog_metadata (key, value, updated_at)
           VALUES ('schema_version', 'product-catalog@1', $1)
           ON CONFLICT(key) DO UPDATE SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at`,
          [now()],
        );
      }
      const metadata = await pool.query<{ value: string }>(
        "SELECT value FROM product_catalog_metadata WHERE key = 'schema_version'",
      );
      if (metadata.rows[0]?.value !== "product-catalog@1") {
        throw new Error("product catalog migration product-catalog@1 is required");
      }
    })();
  }
  await globalDatabase.zeronDesignProductPostgresSchema;
  return globalDatabase.zeronDesignProductPostgres;
}

function sqliteDatabase(): DatabaseSync {
  if (globalDatabase.zeronDesignProductSqlite) return globalDatabase.zeronDesignProductSqlite;
  const databasePath = process.env.ZERONDESIGN_PRODUCT_DB_PATH?.trim()
    || join(process.cwd(), ".data", "product.sqlite");
  mkdirSync(dirname(databasePath), { recursive: true });
  const database = new DatabaseSync(databasePath);
  database.exec("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;");
  database.exec(productSchema);
  for (const migration of [
    "ALTER TABLE project_publication_jobs ADD COLUMN idempotency_key TEXT",
    "ALTER TABLE project_publication_jobs ADD COLUMN expected_generation INTEGER",
    "ALTER TABLE project_publication_jobs ADD COLUMN expected_current_release_id TEXT",
  ]) {
    try {
      database.exec(migration);
    } catch (error) {
      if (!(error instanceof Error) || !error.message.includes("duplicate column name")) throw error;
    }
  }
  database.prepare(
    `INSERT INTO product_catalog_metadata (key, value, updated_at)
     VALUES ('schema_version', 'product-catalog@1', ?)
     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at`,
  ).run(now());
  globalDatabase.zeronDesignProductSqlite = database;
  return database;
}

function sqliteSql(sql: string): string {
  return sql.replace(/\$\d+/g, "?");
}

async function rows<T extends QueryResultRow = QueryResultRow>(
  sql: string,
  parameters: unknown[] = [],
): Promise<T[]> {
  const pool = await postgresPool();
  if (pool) return (await pool.query<T>(sql, parameters)).rows;
  return sqliteDatabase().prepare(sqliteSql(sql)).all(
    ...(parameters as SQLInputValue[]),
  ) as T[];
}

async function row<T extends QueryResultRow = QueryResultRow>(
  sql: string,
  parameters: unknown[] = [],
): Promise<T | null> {
  return (await rows<T>(sql, parameters))[0] ?? null;
}

async function execute(sql: string, parameters: unknown[] = []): Promise<number> {
  const pool = await postgresPool();
  if (pool) return (await pool.query(sql, parameters)).rowCount ?? 0;
  return Number(sqliteDatabase().prepare(sqliteSql(sql)).run(
    ...(parameters as SQLInputValue[]),
  ).changes);
}

export async function productDatabaseHealth(): Promise<{
  ok: true;
  backend: "postgresql" | "sqlite";
  schemaVersion: string;
}> {
  const metadata = await row<{ value: string }>(
    "SELECT value FROM product_catalog_metadata WHERE key = 'schema_version'",
  );
  if (!metadata) throw new Error("product catalog schema metadata is missing");
  return {
    ok: true,
    backend: (await postgresPool()) ? "postgresql" : "sqlite",
    schemaVersion: metadata.value,
  };
}

export async function recordReleasePackaging(input: {
  projectId: string;
  versionId: string;
  releaseId: string;
  packagingId: string;
  status: string;
  lastError?: string | null;
}): Promise<void> {
  await execute(
    `INSERT INTO project_publication_jobs
      (project_id, version_id, release_id, packaging_id, operation_id, action, phase, status, last_error, updated_at)
      VALUES ($1, $2, $3, $4, NULL, 'publish', 'packaging', $5, $6, $7)
      ON CONFLICT(project_id) DO UPDATE SET
        version_id = EXCLUDED.version_id, release_id = EXCLUDED.release_id,
        packaging_id = EXCLUDED.packaging_id, operation_id = NULL,
        action = EXCLUDED.action, phase = EXCLUDED.phase, status = EXCLUDED.status,
        last_error = EXCLUDED.last_error, updated_at = EXCLUDED.updated_at`,
    [input.projectId, input.versionId, input.releaseId, input.packagingId,
      input.status, input.lastError ?? null, now()],
  );
}

export async function recordPublicationOperation(input: {
  projectId: string;
  action: "publish" | "rollback" | "unpublish";
  releaseId?: string;
  operationId: string;
  status: string;
  lastError?: string | null;
}): Promise<void> {
  await execute(
    `INSERT INTO project_publication_jobs
      (project_id, release_id, operation_id, action, phase, status, last_error, updated_at)
      VALUES ($1, $2, $3, $4, 'publication', $5, $6, $7)
      ON CONFLICT(project_id) DO UPDATE SET
        release_id = COALESCE(EXCLUDED.release_id, project_publication_jobs.release_id),
        operation_id = EXCLUDED.operation_id, action = EXCLUDED.action,
        phase = EXCLUDED.phase, status = EXCLUDED.status,
        last_error = EXCLUDED.last_error, updated_at = EXCLUDED.updated_at`,
    [input.projectId, input.releaseId ?? null, input.operationId, input.action,
      input.status, input.lastError ?? null, now()],
  );
}

export async function recordPublicationIntent(input: {
  projectId: string;
  action: "publish" | "rollback" | "unpublish";
  releaseId?: string;
  idempotencyKey: string;
  expectedGeneration: number;
  expectedCurrentReleaseId?: string;
}): Promise<void> {
  await execute(
    `INSERT INTO project_publication_jobs
      (project_id, release_id, action, phase, status, idempotency_key,
       expected_generation, expected_current_release_id, updated_at)
      VALUES ($1, $2, $3, 'publication', 'requesting', $4, $5, $6, $7)
      ON CONFLICT(project_id) DO UPDATE SET
        release_id = EXCLUDED.release_id, operation_id = NULL, action = EXCLUDED.action,
        phase = EXCLUDED.phase, status = EXCLUDED.status, last_error = NULL,
        idempotency_key = EXCLUDED.idempotency_key,
        expected_generation = EXCLUDED.expected_generation,
        expected_current_release_id = EXCLUDED.expected_current_release_id,
        updated_at = EXCLUDED.updated_at`,
    [input.projectId, input.releaseId ?? null, input.action, input.idempotencyKey,
      input.expectedGeneration, input.expectedCurrentReleaseId ?? null, now()],
  );
}

export async function updatePublicationJob(input: {
  projectId: string;
  status: string;
  lastError?: string | null;
}): Promise<void> {
  await execute(
    `UPDATE project_publication_jobs SET status = $1, last_error = $2, updated_at = $3
     WHERE project_id = $4`,
    [input.status, input.lastError ?? null, now(), input.projectId],
  );
}

export async function getPublicationJob(projectId: string): Promise<ProductPublicationJob | null> {
  const value = await row(
    `SELECT project_id, version_id, release_id, packaging_id, operation_id,
            idempotency_key, expected_generation, expected_current_release_id,
            action, phase, status, last_error, updated_at
     FROM project_publication_jobs WHERE project_id = $1`,
    [projectId],
  );
  if (!value) return null;
  return publicationJobFromRow(value);
}

export async function recordProjectVersion(input: {
  projectId: string;
  versionId: string;
  status: string;
}): Promise<void> {
  await execute(
    `INSERT INTO project_versions (project_id, version_id, status, first_seen_at)
     VALUES ($1, $2, $3, $4)
     ON CONFLICT(project_id, version_id) DO UPDATE SET status = EXCLUDED.status`,
    [input.projectId, input.versionId, input.status, now()],
  );
}

export async function listProjectVersionIds(projectId: string): Promise<string[]> {
  return (await rows<{ version_id: string }>(
    `SELECT version_id FROM project_versions
     WHERE project_id = $1 ORDER BY first_seen_at DESC`,
    [projectId],
  )).map((value) => String(value.version_id));
}

export async function recordProjectRun(input: {
  runId: string;
  projectId: string;
  phase: string;
}): Promise<void> {
  await execute(
    `INSERT INTO project_runs (run_id, project_id, phase, created_at)
     VALUES ($1, $2, $3, $4) ON CONFLICT(run_id) DO NOTHING`,
    [input.runId, input.projectId, input.phase, now()],
  );
}

export async function ownsProjectRun(input: {
  runId: string;
  projectId: string;
  ownerId: string;
}): Promise<boolean> {
  return Boolean(await row(
    `SELECT 1 FROM project_runs r JOIN projects p ON p.id = r.project_id
     WHERE r.run_id = $1 AND r.project_id = $2 AND p.owner_id = $3`,
    [input.runId, input.projectId, input.ownerId],
  ));
}

export async function listProjects(ownerId: string): Promise<ProductProject[]> {
  return (await rows(
    `SELECT p.id, p.owner_id, p.name, p.kind, p.runtime_project_id,
            p.workspace_namespace, p.status, p.created_at, p.updated_at,
            (SELECT r.run_id FROM project_runs r WHERE r.project_id = p.id
             ORDER BY r.created_at DESC LIMIT 1) AS latest_run_id
     FROM projects p WHERE p.owner_id = $1 ORDER BY p.updated_at DESC`,
    [ownerId],
  )).map(projectFromRow);
}

export async function listWorkspaces(ownerPrincipalId: string): Promise<ProductWorkspace[]> {
  return (await rows(
    `SELECT namespace, name, owner_principal_id, status, created_at, updated_at
     FROM workspaces WHERE owner_principal_id = $1 AND status = 'active'
     ORDER BY name, namespace`,
    [ownerPrincipalId],
  )).map(workspaceFromRow);
}

export async function listAllWorkspaces(): Promise<ProductWorkspace[]> {
  return (await rows(
    `SELECT namespace, name, owner_principal_id, status, created_at, updated_at
     FROM workspaces ORDER BY updated_at DESC`,
  )).map(workspaceFromRow);
}

export async function registerWorkspace(input: {
  namespace: string;
  name: string;
  ownerPrincipalId: string;
}): Promise<ProductWorkspace> {
  const pool = await postgresPool();
  if (pool) return registerWorkspacePostgres(pool, input);
  const database = sqliteDatabase();
  database.exec("BEGIN IMMEDIATE");
  try {
    const existing = database.prepare(
      "SELECT owner_principal_id FROM workspaces WHERE namespace = ?",
    ).get(input.namespace) as { owner_principal_id?: string } | undefined;
    assertWorkspaceOwner(existing?.owner_principal_id, input.ownerPrincipalId);
    const timestamp = now();
    database.prepare(
      `INSERT INTO workspaces (namespace, name, owner_principal_id, status, created_at, updated_at)
       VALUES (?, ?, ?, 'active', ?, ?)
       ON CONFLICT(namespace) DO UPDATE SET name = excluded.name, status = 'active',
         updated_at = excluded.updated_at`,
    ).run(input.namespace, input.name, input.ownerPrincipalId, timestamp, timestamp);
    const result = database.prepare(
      `SELECT namespace, name, owner_principal_id, status, created_at, updated_at
       FROM workspaces WHERE namespace = ?`,
    ).get(input.namespace);
    if (!result) throw new Error("workspace registration did not return a row");
    database.exec("COMMIT");
    return workspaceFromRow(result);
  } catch (error) {
    database.exec("ROLLBACK");
    throw error;
  }
}

async function registerWorkspacePostgres(
  pool: Pool,
  input: { namespace: string; name: string; ownerPrincipalId: string },
): Promise<ProductWorkspace> {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    const existing = await client.query<{ owner_principal_id: string }>(
      "SELECT owner_principal_id FROM workspaces WHERE namespace = $1 FOR UPDATE",
      [input.namespace],
    );
    assertWorkspaceOwner(existing.rows[0]?.owner_principal_id, input.ownerPrincipalId);
    const timestamp = now();
    const result = await client.query(
      `INSERT INTO workspaces (namespace, name, owner_principal_id, status, created_at, updated_at)
       VALUES ($1, $2, $3, 'active', $4, $5)
       ON CONFLICT(namespace) DO UPDATE SET name = EXCLUDED.name, status = 'active',
         updated_at = EXCLUDED.updated_at
       RETURNING namespace, name, owner_principal_id, status, created_at, updated_at`,
      [input.namespace, input.name, input.ownerPrincipalId, timestamp, timestamp],
    );
    await client.query("COMMIT");
    return workspaceFromRow(result.rows[0]);
  } catch (error) {
    await rollback(client);
    throw error;
  } finally {
    client.release();
  }
}

export async function setWorkspaceStatus(
  namespace: string,
  status: "active" | "disabled",
): Promise<ProductWorkspace | null> {
  await execute(
    "UPDATE workspaces SET status = $1, updated_at = $2 WHERE namespace = $3",
    [status, now(), namespace],
  );
  const value = await row(
    `SELECT namespace, name, owner_principal_id, status, created_at, updated_at
     FROM workspaces WHERE namespace = $1`,
    [namespace],
  );
  return value ? workspaceFromRow(value) : null;
}

export async function getProject(id: string, ownerId: string): Promise<ProductProject | null> {
  const value = await row(
    `SELECT id, owner_id, name, kind, runtime_project_id, workspace_namespace,
            status, created_at, updated_at
     FROM projects WHERE id = $1 AND owner_id = $2`,
    [id, ownerId],
  );
  return value ? projectFromRow(value) : null;
}

export async function beginProjectRegistration(input: {
  id: string;
  ownerId: string;
  name: string;
  kind: "website" | "docs";
  workspaceNamespace: string;
}): Promise<ProductProject> {
  const timestamp = now();
  const project: ProductProject = {
    ...input,
    runtimeProjectId: input.id,
    status: "registering",
    createdAt: timestamp,
    updatedAt: timestamp,
  };
  const pool = await postgresPool();
  if (pool) {
    const client = await pool.connect();
    try {
      await client.query("BEGIN");
      await requireAvailableWorkspacePostgres(client, project.workspaceNamespace, project.ownerId);
      await client.query(
        `INSERT INTO projects
          (id, owner_id, name, kind, runtime_project_id, workspace_namespace,
           status, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)`,
        [project.id, project.ownerId, project.name, project.kind, project.runtimeProjectId,
          project.workspaceNamespace, project.status, project.createdAt, project.updatedAt],
      );
      await client.query("COMMIT");
      return project;
    } catch (error) {
      await rollback(client);
      throw error;
    } finally {
      client.release();
    }
  }
  const database = sqliteDatabase();
  database.exec("BEGIN IMMEDIATE");
  try {
    const workspace = database.prepare(
      `SELECT 1 FROM workspaces
       WHERE namespace = ? AND owner_principal_id = ? AND status = 'active'`,
    ).get(project.workspaceNamespace, project.ownerId);
    if (!workspace) throw new WorkspaceUnavailableError(
      "workspace is not registered or not available to this user",
    );
    database.prepare(
      `INSERT INTO projects
        (id, owner_id, name, kind, runtime_project_id, workspace_namespace,
         status, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(project.id, project.ownerId, project.name, project.kind, project.runtimeProjectId,
      project.workspaceNamespace, project.status, project.createdAt, project.updatedAt);
    database.exec("COMMIT");
    return project;
  } catch (error) {
    database.exec("ROLLBACK");
    throw error;
  }
}

export async function finishProjectRegistration(
  id: string,
  ownerId: string,
): Promise<ProductProject> {
  await execute(
    `UPDATE projects SET status = 'draft', updated_at = $1
     WHERE id = $2 AND owner_id = $3 AND status IN ('registering', 'registration_failed')`,
    [now(), id, ownerId],
  );
  const project = await getProject(id, ownerId);
  if (!project) throw new Error("project registration disappeared before completion");
  return project;
}

export async function failProjectRegistration(id: string, ownerId: string): Promise<void> {
  await execute(
    `UPDATE projects SET status = 'registration_failed', updated_at = $1
     WHERE id = $2 AND owner_id = $3 AND status = 'registering'`,
    [now(), id, ownerId],
  );
}

async function requireAvailableWorkspacePostgres(
  client: PoolClient,
  namespace: string,
  ownerId: string,
): Promise<void> {
  const workspace = await client.query(
    `SELECT 1 FROM workspaces
     WHERE namespace = $1 AND owner_principal_id = $2 AND status = 'active' FOR SHARE`,
    [namespace, ownerId],
  );
  if (!workspace.rows[0]) throw new WorkspaceUnavailableError(
    "workspace is not registered or not available to this user",
  );
}

function assertWorkspaceOwner(existingOwner: string | undefined, requestedOwner: string): void {
  if (existingOwner && existingOwner !== requestedOwner) {
    throw new WorkspaceUnavailableError("workspace owner cannot be changed by registration");
  }
}

async function rollback(client: PoolClient): Promise<void> {
  try {
    await client.query("ROLLBACK");
  } catch {
    // Preserve the original transaction failure.
  }
}

function now(): string {
  return new Date().toISOString();
}

function publicationJobFromRow(value: Record<string, unknown>): ProductPublicationJob {
  return {
    projectId: String(value.project_id),
    ...(value.version_id ? { versionId: String(value.version_id) } : {}),
    ...(value.release_id ? { releaseId: String(value.release_id) } : {}),
    ...(value.packaging_id ? { packagingId: String(value.packaging_id) } : {}),
    ...(value.operation_id ? { operationId: String(value.operation_id) } : {}),
    ...(value.idempotency_key ? { idempotencyKey: String(value.idempotency_key) } : {}),
    ...(value.expected_generation !== null && value.expected_generation !== undefined
      ? { expectedGeneration: Number(value.expected_generation) }
      : {}),
    ...(value.expected_current_release_id
      ? { expectedCurrentReleaseId: String(value.expected_current_release_id) }
      : {}),
    action: value.action === "rollback" || value.action === "unpublish" ? value.action : "publish",
    phase: value.phase === "publication" ? "publication" : "packaging",
    status: String(value.status),
    ...(value.last_error ? { lastError: String(value.last_error) } : {}),
    updatedAt: String(value.updated_at),
  };
}

function projectFromRow(value: Record<string, unknown>): ProductProject {
  return {
    id: String(value.id),
    ownerId: String(value.owner_id),
    name: String(value.name),
    kind: value.kind === "docs" ? "docs" : "website",
    runtimeProjectId: String(value.runtime_project_id),
    workspaceNamespace: String(value.workspace_namespace),
    status: String(value.status),
    createdAt: String(value.created_at),
    updatedAt: String(value.updated_at),
    ...(value.latest_run_id ? { latestRunId: String(value.latest_run_id) } : {}),
  };
}

function workspaceFromRow(value: Record<string, unknown>): ProductWorkspace {
  return {
    namespace: String(value.namespace),
    name: String(value.name),
    ownerPrincipalId: String(value.owner_principal_id),
    status: value.status === "disabled" ? "disabled" : "active",
    createdAt: String(value.created_at),
    updatedAt: String(value.updated_at),
  };
}
