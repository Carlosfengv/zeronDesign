#!/usr/bin/env node

import assert from "node:assert/strict";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "audit-content-plan-approval-migration.mjs");

const legacy = run([
  { briefId: "brief-1", projectId: "project-1", runId: "run-1", status: "draft" },
  { briefId: "brief-1", projectId: "project-1", runId: "run-1", status: "confirmed" },
  { briefId: "brief-2", projectId: "project-2", runId: "run-2", status: "draft" },
]);
assert.equal(legacy.status, 0, legacy.stderr);
assert.equal(legacy.evidence.inventory.rawRecordCount, 3);
assert.equal(legacy.evidence.inventory.uniqueBriefCount, 2);
assert.equal(legacy.evidence.inventory.confirmedCandidateCount, 1);
assert.equal(legacy.evidence.inventory.mappableCount, 0);
assert.equal(legacy.evidence.inventory.unmappableCount, 1);
assert.equal(legacy.evidence.migration.verifiedApprovalsCreated, 0);
assert.equal(legacy.evidence.gate.enabledOrEnforceAllowed, true);
assert.deepEqual(legacy.evidence.candidates[0].reasons, [
  "missing_plan_id",
  "missing_revision",
  "missing_content_hash",
  "missing_confirmation_event_id",
]);

const mappable = run([{
  briefId: "brief-3",
  projectId: "project-3",
  runId: "run-3",
  status: "confirmed",
  contentPlan: { planId: "plan-3", revision: 2, contentHash: "a".repeat(64) },
  confirmationEventId: "confirmation-3",
}]);
assert.equal(mappable.status, 4);
assert.equal(mappable.evidence.inventory.mappableCount, 1);
assert.equal(mappable.evidence.migration.requiresProducerMigration, true);
assert.equal(mappable.evidence.migration.verifiedApprovalsCreated, 0);
assert.equal(mappable.evidence.gate.enabledOrEnforceAllowed, false);

const mismatch = run([{ briefId: "brief-4", projectId: "project-4", runId: "run-4", status: "confirmed" }], {
  sourceSha256: "0".repeat(64),
});
assert.equal(mismatch.status, 2);
assert.match(mismatch.stderr, /source hash mismatch/);

console.log("content plan approval migration audit tests passed");

function run(records, overrides = {}) {
  const input = Buffer.from(`${records.map(record => JSON.stringify(record)).join("\n")}\n`);
  const sourceSha256 = overrides.sourceSha256 ?? crypto.createHash("sha256").update(input).digest("hex");
  const result = spawnSync(process.execPath, [
    script,
    "--authority=test-runtime-control-plane",
    "--source-file=briefs.jsonl",
    "--source-revision=3",
    "--source-updated-at=2026-07-21T07:45:13.776Z",
    "--recorded-at=2026-07-21T10:00:00.000Z",
    `--source-sha256=${sourceSha256}`,
    "--source-complete=true",
  ], { input, encoding: "utf8" });
  return {
    status: result.status,
    stderr: result.stderr,
    evidence: result.stdout ? JSON.parse(result.stdout) : null,
  };
}
