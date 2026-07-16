import { DatabaseSync } from "node:sqlite";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";

export type ProductProject = {
  id: string;
  ownerId: string;
  name: string;
  kind: "website" | "docs";
  runtimeProjectId: string;
  status: string;
  createdAt: string;
  updatedAt: string;
  latestRunId?: string;
};

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

const globalDatabase = globalThis as typeof globalThis & {
  zeronDesignProductDatabase?: DatabaseSync;
};

function productDatabase(): DatabaseSync {
  if (globalDatabase.zeronDesignProductDatabase) {
    return globalDatabase.zeronDesignProductDatabase;
  }
  const databasePath = process.env.ZERONDESIGN_PRODUCT_DB_PATH?.trim()
    || join(process.cwd(), ".data", "product.sqlite");
  mkdirSync(dirname(databasePath), { recursive: true });
  const database = new DatabaseSync(databasePath);
  database.exec("PRAGMA busy_timeout = 5000;");
  database.exec(`
    CREATE TABLE IF NOT EXISTS projects (
      id TEXT PRIMARY KEY,
      owner_id TEXT NOT NULL,
      name TEXT NOT NULL,
      kind TEXT NOT NULL CHECK (kind IN ('website', 'docs')),
      runtime_project_id TEXT NOT NULL UNIQUE,
      status TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS projects_owner_updated
      ON projects(owner_id, updated_at DESC);
    CREATE TABLE IF NOT EXISTS project_runs (
      run_id TEXT PRIMARY KEY,
      project_id TEXT NOT NULL,
      phase TEXT NOT NULL,
      created_at TEXT NOT NULL,
      FOREIGN KEY(project_id) REFERENCES projects(id)
    );
    CREATE INDEX IF NOT EXISTS project_runs_project_created
      ON project_runs(project_id, created_at DESC);
    CREATE TABLE IF NOT EXISTS project_versions (
      project_id TEXT NOT NULL,
      version_id TEXT NOT NULL,
      status TEXT NOT NULL,
      first_seen_at TEXT NOT NULL,
      PRIMARY KEY(project_id, version_id),
      FOREIGN KEY(project_id) REFERENCES projects(id)
    );
    CREATE TABLE IF NOT EXISTS project_publication_jobs (
      project_id TEXT PRIMARY KEY,
      version_id TEXT,
      release_id TEXT,
      packaging_id TEXT,
      operation_id TEXT,
      idempotency_key TEXT,
      expected_generation INTEGER,
      expected_current_release_id TEXT,
      action TEXT NOT NULL,
      phase TEXT NOT NULL,
      status TEXT NOT NULL,
      last_error TEXT,
      updated_at TEXT NOT NULL,
      FOREIGN KEY(project_id) REFERENCES projects(id)
    );
  `);
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
  globalDatabase.zeronDesignProductDatabase = database;
  return database;
}

export function recordReleasePackaging(input: {
  projectId: string;
  versionId: string;
  releaseId: string;
  packagingId: string;
  status: string;
  lastError?: string | null;
}): void {
  productDatabase()
    .prepare(`INSERT INTO project_publication_jobs
      (project_id, version_id, release_id, packaging_id, operation_id, action, phase, status, last_error, updated_at)
      VALUES (?, ?, ?, ?, NULL, 'publish', 'packaging', ?, ?, ?)
      ON CONFLICT(project_id) DO UPDATE SET
        version_id = excluded.version_id,
        release_id = excluded.release_id,
        packaging_id = excluded.packaging_id,
        operation_id = NULL,
        action = excluded.action,
        phase = excluded.phase,
        status = excluded.status,
        last_error = excluded.last_error,
        updated_at = excluded.updated_at`)
    .run(
      input.projectId,
      input.versionId,
      input.releaseId,
      input.packagingId,
      input.status,
      input.lastError ?? null,
      new Date().toISOString(),
    );
}

export function recordPublicationOperation(input: {
  projectId: string;
  action: "publish" | "rollback" | "unpublish";
  releaseId?: string;
  operationId: string;
  status: string;
  lastError?: string | null;
}): void {
  productDatabase()
    .prepare(`INSERT INTO project_publication_jobs
      (project_id, release_id, operation_id, action, phase, status, last_error, updated_at)
      VALUES (?, ?, ?, ?, 'publication', ?, ?, ?)
      ON CONFLICT(project_id) DO UPDATE SET
        release_id = COALESCE(excluded.release_id, project_publication_jobs.release_id),
        operation_id = excluded.operation_id,
        action = excluded.action,
        phase = excluded.phase,
        status = excluded.status,
        last_error = excluded.last_error,
        updated_at = excluded.updated_at`)
    .run(
      input.projectId,
      input.releaseId ?? null,
      input.operationId,
      input.action,
      input.status,
      input.lastError ?? null,
      new Date().toISOString(),
    );
}

export function recordPublicationIntent(input: {
  projectId: string;
  action: "publish" | "rollback" | "unpublish";
  releaseId?: string;
  idempotencyKey: string;
  expectedGeneration: number;
  expectedCurrentReleaseId?: string;
}): void {
  productDatabase()
    .prepare(`INSERT INTO project_publication_jobs
      (project_id, release_id, action, phase, status, idempotency_key,
       expected_generation, expected_current_release_id, updated_at)
      VALUES (?, ?, ?, 'publication', 'requesting', ?, ?, ?, ?)
      ON CONFLICT(project_id) DO UPDATE SET
        release_id = excluded.release_id,
        operation_id = NULL,
        action = excluded.action,
        phase = excluded.phase,
        status = excluded.status,
        last_error = NULL,
        idempotency_key = excluded.idempotency_key,
        expected_generation = excluded.expected_generation,
        expected_current_release_id = excluded.expected_current_release_id,
        updated_at = excluded.updated_at`)
    .run(
      input.projectId,
      input.releaseId ?? null,
      input.action,
      input.idempotencyKey,
      input.expectedGeneration,
      input.expectedCurrentReleaseId ?? null,
      new Date().toISOString(),
    );
}

export function updatePublicationJob(input: {
  projectId: string;
  status: string;
  lastError?: string | null;
}): void {
  productDatabase()
    .prepare(`UPDATE project_publication_jobs
              SET status = ?, last_error = ?, updated_at = ? WHERE project_id = ?`)
    .run(input.status, input.lastError ?? null, new Date().toISOString(), input.projectId);
}

export function getPublicationJob(projectId: string): ProductPublicationJob | null {
  const row = productDatabase()
    .prepare(`SELECT project_id, version_id, release_id, packaging_id, operation_id,
                     idempotency_key, expected_generation, expected_current_release_id,
                     action, phase, status, last_error, updated_at
              FROM project_publication_jobs WHERE project_id = ?`)
    .get(projectId);
  if (!row) return null;
  const value = row as Record<string, unknown>;
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

export function recordProjectVersion(input: {
  projectId: string;
  versionId: string;
  status: string;
}): void {
  productDatabase()
    .prepare(`INSERT INTO project_versions (project_id, version_id, status, first_seen_at)
              VALUES (?, ?, ?, ?)
              ON CONFLICT(project_id, version_id) DO UPDATE SET status = excluded.status`)
    .run(input.projectId, input.versionId, input.status, new Date().toISOString());
}

export function listProjectVersionIds(projectId: string): string[] {
  return productDatabase()
    .prepare(`SELECT version_id FROM project_versions
              WHERE project_id = ? ORDER BY first_seen_at DESC`)
    .all(projectId)
    .map((row) => String((row as Record<string, unknown>).version_id));
}

export function recordProjectRun(input: {
  runId: string;
  projectId: string;
  phase: string;
}): void {
  productDatabase()
    .prepare(`INSERT INTO project_runs (run_id, project_id, phase, created_at)
              VALUES (?, ?, ?, ?)
              ON CONFLICT(run_id) DO NOTHING`)
    .run(input.runId, input.projectId, input.phase, new Date().toISOString());
}

export function ownsProjectRun(input: {
  runId: string;
  projectId: string;
  ownerId: string;
}): boolean {
  return Boolean(
    productDatabase()
      .prepare(`SELECT 1 FROM project_runs r
                JOIN projects p ON p.id = r.project_id
                WHERE r.run_id = ? AND r.project_id = ? AND p.owner_id = ?`)
      .get(input.runId, input.projectId, input.ownerId),
  );
}

export function listProjects(ownerId: string): ProductProject[] {
  return productDatabase()
    .prepare(`SELECT p.id, p.owner_id, p.name, p.kind, p.runtime_project_id, p.status,
                     p.created_at, p.updated_at,
                     (SELECT r.run_id FROM project_runs r WHERE r.project_id = p.id
                      ORDER BY r.created_at DESC LIMIT 1) AS latest_run_id
              FROM projects p WHERE p.owner_id = ? ORDER BY p.updated_at DESC`)
    .all(ownerId)
    .map(projectFromRow);
}

export function getProject(id: string, ownerId: string): ProductProject | null {
  const row = productDatabase()
    .prepare(`SELECT id, owner_id, name, kind, runtime_project_id, status, created_at, updated_at
              FROM projects WHERE id = ? AND owner_id = ?`)
    .get(id, ownerId);
  return row ? projectFromRow(row) : null;
}

export function createProject(input: {
  id: string;
  ownerId: string;
  name: string;
  kind: "website" | "docs";
}): ProductProject {
  const now = new Date().toISOString();
  const project: ProductProject = {
    ...input,
    runtimeProjectId: input.id,
    status: "draft",
    createdAt: now,
    updatedAt: now,
  };
  productDatabase()
    .prepare(`INSERT INTO projects
      (id, owner_id, name, kind, runtime_project_id, status, created_at, updated_at)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(project.id, project.ownerId, project.name, project.kind, project.runtimeProjectId,
      project.status, project.createdAt, project.updatedAt);
  return project;
}

function projectFromRow(row: unknown): ProductProject {
  const value = row as Record<string, unknown>;
  return {
    id: String(value.id),
    ownerId: String(value.owner_id),
    name: String(value.name),
    kind: value.kind === "docs" ? "docs" : "website",
    runtimeProjectId: String(value.runtime_project_id),
    status: String(value.status),
    createdAt: String(value.created_at),
    updatedAt: String(value.updated_at),
    ...(value.latest_run_id ? { latestRunId: String(value.latest_run_id) } : {}),
  };
}
