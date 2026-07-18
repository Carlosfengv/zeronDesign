#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const auditor = path.join(scriptDirectory, "audit-real-provider-stability.mjs");
const root = fs.mkdtempSync(path.join(os.tmpdir(), "real-provider-stability-"));

try {
  writeSuite(root, "001", { finishedAt: "2026-01-01T00:00:01Z" });
  writeSuite(root, "002", { finishedAt: "2026-01-01T00:00:02Z" });
  let result = runAudit(root, false);
  assert.equal(result.execution.status, 1);
  assert.equal(result.audit.status, "incomplete");
  assert.equal(result.audit.currentConsecutiveFullPasses, 2);

  result = runAudit(root, true);
  assert.equal(result.execution.status, 0);
  assert.equal(result.audit.currentConsecutiveFullPasses, 2);

  writeSuite(root, "003", { finishedAt: "2026-01-01T00:00:03Z" });
  result = runAudit(root, false);
  assert.equal(result.execution.status, 0);
  assert.equal(result.audit.status, "passed");
  assert.deepEqual(result.audit.currentPassingSuiteIds, ["001", "002", "003"]);

  writeSuite(root, "004-targeted", {
    finishedAt: "2026-01-01T00:00:04Z",
    executedCaseCount: 1,
    partial: true,
    caseCount: 1,
  });
  result = runAudit(root, false);
  assert.equal(result.audit.currentConsecutiveFullPasses, 3);

  writeSuite(root, "005-rejected", {
    finishedAt: "2026-01-01T00:00:05Z",
    status: "rejected",
    accepted: false,
  });
  result = runAudit(root, true);
  assert.equal(result.audit.status, "incomplete");
  assert.equal(result.audit.currentConsecutiveFullPasses, 0);

  const untrustedRoot = fs.mkdtempSync(path.join(root, "untrusted-"));
  writeSuite(untrustedRoot, "006", { providerVerified: false });
  result = runAudit(untrustedRoot, true);
  assert.equal(result.execution.status, 1);
  assert.equal(result.audit.status, "failed_evidence");
  assert.equal(result.audit.counts.falseSuccesses, 1);

  process.stdout.write("real-provider stability audit tests passed\n");
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}

function runAudit(evidenceRoot, allowIncomplete) {
  const out = path.join(evidenceRoot, "audit.json");
  const execution = spawnSync(
    process.execPath,
    [
      auditor,
      "--evidence-root",
      evidenceRoot,
      "--out",
      out,
      "--required-consecutive",
      "3",
      ...(allowIncomplete ? ["--allow-incomplete"] : []),
    ],
    { encoding: "utf8" },
  );
  return { execution, audit: JSON.parse(fs.readFileSync(out, "utf8")) };
}

function writeSuite(
  evidenceRoot,
  suiteId,
  {
    finishedAt = "2026-01-01T00:00:00Z",
    status = "accepted",
    providerVerified = true,
    executedCaseCount = 5,
    partial = false,
    caseCount = 5,
    accepted = true,
  } = {},
) {
  const directory = path.join(evidenceRoot, `suite-${suiteId}-${status}`);
  fs.mkdirSync(directory, { recursive: true });
  const cases = Array.from({ length: caseCount }, (_, index) => ({
    id: `case-${index + 1}`,
    status: accepted ? "accepted" : "rejected",
    artifact: accepted
      ? { httpStatus: 200, expectedTextFound: true }
      : null,
  }));
  fs.writeFileSync(
    path.join(directory, "real-provider-examples-summary.json"),
    `${JSON.stringify({
      schemaVersion: "generation-real-provider-suite-evidence@2",
      suiteId,
      finishedAt,
      status,
      execution: { generatedCaseCount: 5, executedCaseCount, partial },
      provider: { realProviderVerified: providerVerified },
      budget: { exceeded: false },
      cases,
    })}\n`,
  );
}
