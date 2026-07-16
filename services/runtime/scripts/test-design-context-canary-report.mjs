#!/usr/bin/env node
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  renderDesignContextCanaryReport,
  renderReportFile,
} from "./render-design-context-canary-report.mjs";

function exportDocument(overrides = {}) {
  return {
    schemaVersion: "design-context-canary-operational-export@1",
    generatedAt: "2026-07-15T00:01:00Z",
    source: { kind: "runtime-durable-store", projectRunCount: 62, cohortRunCount: 60, metricEventCount: 20 },
    cohort: {
      projectId: "project-1",
      designProfileId: "profile-1",
      designProfileVersion: 1,
      observePolicyRevision: 1,
      policyRevision: 2,
    },
    window: {
      baselineStartedAt: "2026-07-01T00:00:00Z",
      baselineEndedAt: "2026-07-08T00:00:00Z",
      observationStartedAt: "2026-07-08T00:00:00Z",
      observationEndedAt: "2026-07-15T00:00:00Z",
    },
    publish: {
      samples: [],
      baselinePublishCount: 30,
      enforcedPublishCount: 30,
      baselineFailureRate: 0,
      enforcedFailureRate: 0,
      publishFailureRateDeltaPp: 0,
    },
    metrics: {
      totals: {},
      verifierUnavailableCount: 0,
      verifierRuntimeLostCount: 0,
      unexpectedReadGateBlockCount: 0,
      recoveryRequiredOver24hCount: 0,
      requiredFindingCount: 1,
      repairedRequiredFindingCount: 1,
      requiredFindingRepairRate: 1,
    },
    alerts: [
      { code: "verifier_unavailable", severity: "ok", triggered: false, actual: 0, threshold: "must_equal_0", action: "page" },
    ],
    alertsTriggered: false,
    conclusionRecordedBy: "operator-1",
    ...overrides,
  };
}

const ready = renderDesignContextCanaryReport(exportDocument());
assert.match(ready, /READY FOR REQUIRED ROLLBACK DRILL/);
assert.match(ready, /never grants `canary-verified`/);
assert.doesNotMatch(ready, /Decision: \*\*canary-verified/);

const stopped = renderDesignContextCanaryReport(exportDocument({
  alertsTriggered: true,
  alerts: [
    { code: "verifier_unavailable", severity: "page", triggered: true, actual: 1, threshold: "must_equal_0", action: "disable_exact_policy" },
  ],
  metrics: {
    ...exportDocument().metrics,
    verifierUnavailableCount: 1,
  },
}));
assert.match(stopped, /STOP \/ ROLLBACK REQUIRED/);
assert.match(stopped, /verifier_unavailable \| TRIGGERED/);

const root = await mkdtemp(join(tmpdir(), "design-context-canary-report-"));
try {
  const input = join(root, "export.json");
  const output = join(root, "report.md");
  await writeFile(input, `${JSON.stringify(exportDocument())}\n`);
  await renderReportFile({ inputPath: input, outputPath: output });
  await assert.rejects(renderReportFile({ inputPath: input, outputPath: output }), /EEXIST/);

  const secretInput = join(root, "secret-export.json");
  await writeFile(secretInput, `${JSON.stringify({ ...exportDocument(), authorization: "Bearer forbidden-value-123456" })}\n`);
  await assert.rejects(
    renderReportFile({ inputPath: secretInput, outputPath: join(root, "secret-report.md") }),
    /credential-like/,
  );
} finally {
  await rm(root, { recursive: true, force: true });
}

process.stdout.write("design-context canary report tests passed\n");
