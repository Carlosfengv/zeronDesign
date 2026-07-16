#!/usr/bin/env node

import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  buildCanarySourceLedger,
  createCanaryLedgerRecord,
} from "./validate-design-context-canary-evidence.mjs";
import {
  disableCanaryPolicy,
  verifyCanaryRollback,
} from "./run-design-context-canary-rollback.mjs";

const sha = "a".repeat(64);
const cohort = {
  projectId: "project-1",
  designProfileId: "profile-1",
  designProfileVersion: 1,
  observePolicyRevision: 2,
  policyRevision: 3,
  policyUpdatedBy: "operator-1",
  thresholdVersion: "website-dcp-canary-thresholds@1",
};
let runStartedAt = "2026-07-15T01:01:00Z";
const requests = [];
const server = createServer(async (request, response) => {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  requests.push({ method: request.method, url: request.url, headers: request.headers, body: Buffer.concat(chunks).toString("utf8") });
  if (request.method === "PUT" && request.url === "/internal/projects/project-1/design-context-enforcement") {
    assert.equal(request.headers["x-runtime-admin-token"], "admin-test-token");
    assert.deepEqual(JSON.parse(requests.at(-1).body), {
      designProfileId: "profile-1",
      designProfileVersion: 1,
      enabled: false,
      expectedRevision: 3,
      updatedBy: "operator-1",
    });
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({
      policy: {
        projectId: "project-1",
        designProfileId: "profile-1",
        designProfileVersion: 1,
        enabled: false,
        revision: 4,
        updatedBy: "operator-1",
      },
    }));
    return;
  }
  assert.equal(request.headers.authorization, "Bearer principal-test-token");
  if (request.url === "/runs/run-post-rollback/design-context-diagnostics") {
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({
      runId: "run-post-rollback",
      package: {
        designProfileId: "profile-1",
        designProfileVersion: 1,
        effectiveCompatibilityMode: "observe",
        enforcementPolicy: {
          source: "persistent",
          enabled: false,
          policyRevision: 4,
          policyUpdatedBy: "operator-1",
        },
      },
      gate: "ready",
      missingRequiredReads: [],
    }));
    return;
  }
  if (request.url === "/runs/run-post-rollback/events") {
    response.setHeader("content-type", "text/event-stream");
    response.end([
      `data: ${JSON.stringify({ type: "run.started", runId: "run-post-rollback", timestamp: runStartedAt })}`,
      `data: ${JSON.stringify({ type: "run.completed", runId: "run-post-rollback", status: "completed", timestamp: "2026-07-15T01:02:00Z" })}`,
      "",
    ].join("\n"));
    return;
  }
  response.statusCode = 404;
  response.end("not found");
});

await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const address = server.address();
const baseUrl = `http://127.0.0.1:${address.port}`;
const root = await mkdtemp(join(tmpdir(), "design-context-canary-rollback-"));
try {
  const statePath = join(root, "rollback-state.json");
  const state = await disableCanaryPolicy({
    baseUrl,
    projectId: cohort.projectId,
    designProfileId: cohort.designProfileId,
    designProfileVersion: cohort.designProfileVersion,
    expectedRevision: cohort.policyRevision,
    updatedBy: cohort.policyUpdatedBy,
    statePath,
    adminToken: "admin-test-token",
    recordedAt: "2026-07-15T01:00:00Z",
  });
  assert.equal(state.policyRevisionAfterRollback, 4);
  assert.equal(JSON.parse(await readFile(statePath, "utf8")).schemaVersion, "design-context-canary-rollback-state@1");

  const session = {
    provider: { mode: "approved-real", name: "provider", model: "model", approvalReference: "APP-1", credentialPresent: true },
    cohort,
    images: {},
    window: {
      baselineStartedAt: "2026-07-01T00:00:00Z",
      baselineEndedAt: "2026-07-08T00:00:00Z",
      observationStartedAt: "2026-07-08T00:00:00Z",
    },
  };
  const recovery = {
    ...cohort,
    operationId: "operation-3",
    sourceRunId: "run-recovery-source",
    childRunId: "run-recovery-child",
    planHash: sha,
    beforeTokenSnapshotHash: sha,
    afterTokenSnapshotHash: "b".repeat(64),
    status: "applied",
    recoveryRequiredObserved: true,
    reusedChildRun: true,
  };
  const records = [];
  for (const [type, recordedAt, payload] of [
    ["session.started", "2026-07-08T00:00:00Z", session],
    ["profile-sync.recovery", "2026-07-08T01:00:00Z", recovery],
  ]) {
    records.push(createCanaryLedgerRecord({
      sequence: records.length + 1,
      type,
      recordedAt,
      sourceUri: `evidence://rollback-test/${type}`,
      sourceSha256: sha,
      payload,
      previousRecordHash: records.at(-1)?.recordHash ?? null,
    }));
  }
  const ledgerPath = join(root, "canary.ndjson");
  await writeFile(ledgerPath, `${records.map(record => JSON.stringify(record)).join("\n")}\n`);
  assert.equal(buildCanarySourceLedger(records).recordCount, 2);

  const outputPath = join(root, "rollback-event.json");
  const event = await verifyCanaryRollback({
    baseUrl,
    statePath,
    postRunId: "run-post-rollback",
    ledgerPath,
    outputPath,
    sourceUri: "evidence://canary/rollback-event",
    principalToken: "principal-test-token",
    recordedAt: "2026-07-15T01:03:00Z",
  });
  assert.equal(event.type, "rollback");
  assert.equal(event.payload.policyRevisionAfterRollback, 4);
  assert.equal(event.payload.postRollbackMode, "observe");
  assert.equal(event.payload.operationRecoveryPreserved, true);
  assert.equal(JSON.parse(await readFile(outputPath, "utf8")).payload.postRollbackRunId, "run-post-rollback");

  runStartedAt = "2026-07-15T00:59:00Z";
  await assert.rejects(() => verifyCanaryRollback({
    baseUrl,
    statePath,
    postRunId: "run-post-rollback",
    ledgerPath,
    outputPath: join(root, "stale-event.json"),
    sourceUri: "evidence://canary/stale-rollback-event",
    principalToken: "principal-test-token",
    recordedAt: "2026-07-15T01:04:00Z",
  }), /new observe Run/);
} finally {
  server.close();
  await rm(root, { recursive: true, force: true });
}

process.stdout.write("design-context canary rollback tests passed\n");
