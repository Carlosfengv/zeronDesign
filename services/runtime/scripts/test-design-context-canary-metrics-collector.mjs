#!/usr/bin/env node
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { collectDesignContextCanaryMetrics } from "./collect-design-context-canary-metrics.mjs";

const root = await mkdtemp(join(tmpdir(), "design-context-canary-metrics-"));
const sessionPath = join(root, "session.json");
const exportPath = join(root, "export.json");
const metricsPath = join(root, "metrics.json");
const samplesPath = join(root, "samples.json");
const session = {
  schemaVersion: "design-context-canary-session-config@1",
  recordedAt: "2026-07-08T00:00:00Z",
  sourceUri: "evidence://canary/session",
  provider: { mode: "approved-real", name: "provider", model: "model", approvalReference: "APP-1", credentialPresent: true },
  cohort: {
    projectId: "project-1",
    designProfileId: "profile-1",
    designProfileVersion: 1,
    observePolicyRevision: 1,
    policyRevision: 2,
    policyUpdatedBy: "operator-1",
    thresholdVersion: "website-dcp-canary-thresholds@1",
  },
  images: {
    runtime: { ref: "runtime", manifestDigest: `sha256:${"1".repeat(64)}`, configDigest: `sha256:${"2".repeat(64)}` },
    bff: { ref: "bff", manifestDigest: `sha256:${"3".repeat(64)}`, configDigest: `sha256:${"4".repeat(64)}` },
  },
  window: {
    baselineStartedAt: "2026-07-01T00:00:00Z",
    baselineEndedAt: "2026-07-08T00:00:00Z",
    observationStartedAt: "2026-07-08T00:00:00Z",
  },
};
await writeFile(sessionPath, `${JSON.stringify(session)}\n`);

let responseDocument = {
  schemaVersion: "design-context-canary-operational-export@1",
  generatedAt: "2026-07-10T00:01:00Z",
  source: { kind: "runtime-durable-store", projectRunCount: 3, cohortRunCount: 2, metricEventCount: 4 },
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
    observationEndedAt: "2026-07-10T00:00:00Z",
  },
  publish: {
    samples: [
      {
        sampleId: "sample-baseline",
        runId: "run-baseline",
        observedAt: "2026-07-07T00:00:00Z",
        mode: "baseline",
        publishVerdict: "pass",
        dcpCausedFailure: false,
        projectId: "project-1",
        designProfileId: "profile-1",
        designProfileVersion: 1,
        policyRevision: 1,
      },
      {
        sampleId: "sample-enforced",
        runId: "run-enforced",
        observedAt: "2026-07-09T00:00:00Z",
        mode: "enforced",
        publishVerdict: "fail",
        dcpCausedFailure: true,
        projectId: "project-1",
        designProfileId: "profile-1",
        designProfileVersion: 1,
        policyRevision: 2,
      },
    ],
    baselinePublishCount: 1,
    enforcedPublishCount: 1,
    baselineFailureRate: 0,
    enforcedFailureRate: 1,
    publishFailureRateDeltaPp: 100,
  },
  metrics: {
    totals: {},
    verifierUnavailableCount: 1,
    verifierRuntimeLostCount: 0,
    unexpectedReadGateBlockCount: 0,
    recoveryRequiredOver24hCount: 0,
    requiredFindingCount: 1,
    repairedRequiredFindingCount: 0,
    requiredFindingRepairRate: 0,
  },
  alerts: [{ code: "verifier_unavailable", severity: "page", triggered: true }],
  alertsTriggered: true,
  conclusionRecordedBy: "operator-1",
};
let requestCount = 0;
const server = createServer((request, response) => {
  requestCount += 1;
  assert.equal(request.headers["x-anydesign-internal"], "true");
  assert.equal(request.headers["x-runtime-admin-token"], "admin-token");
  const url = new URL(request.url, "http://127.0.0.1");
  assert.equal(url.pathname, "/internal/projects/project-1/design-context-canary-metrics");
  assert.equal(url.searchParams.get("designProfileId"), "profile-1");
  assert.equal(url.searchParams.get("observationEndedAt"), "2026-07-10T00:00:00Z");
  response.writeHead(200, { "content-type": "application/json" });
  response.end(JSON.stringify(responseDocument));
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const address = server.address();

try {
  const result = await collectDesignContextCanaryMetrics({
    baseUrl: `http://127.0.0.1:${address.port}`,
    sessionPath,
    observationEndedAt: "2026-07-10T00:00:00Z",
    conclusionRecordedBy: "operator-1",
    exportOutput: exportPath,
    metricsOutput: metricsPath,
    metricsSourceUri: "evidence://canary/metrics-2026-07-10",
    samplesOutput: samplesPath,
    samplesSourceUri: "evidence://canary/samples-final",
    adminToken: "admin-token",
  });
  assert.equal(result.metricsEvent.payload.alertsTriggered, true, "failing operational snapshots must remain collectable evidence");
  assert.equal(result.samplesEvent.payload.samples.length, 2);
  assert.equal(JSON.parse(await readFile(exportPath, "utf8")).source.kind, "runtime-durable-store");
  assert.equal(JSON.parse(await readFile(metricsPath, "utf8")).type, "metrics.snapshot");
  assert.equal(JSON.parse(await readFile(samplesPath, "utf8")).type, "publish.samples");
  await assert.rejects(
    collectDesignContextCanaryMetrics({
      baseUrl: `http://127.0.0.1:${address.port}`,
      sessionPath,
      observationEndedAt: "2026-07-10T00:00:00Z",
      conclusionRecordedBy: "operator-1",
      exportOutput: exportPath,
      metricsOutput: join(root, "metrics-duplicate.json"),
      metricsSourceUri: "evidence://canary/metrics-duplicate",
      adminToken: "admin-token",
    }),
    /EEXIST/,
    "collector must not overwrite an immutable operational export",
  );

  const partialExport = join(root, "partial-export.json");
  const partialMetrics = join(root, "partial-metrics.json");
  const occupiedSamples = join(root, "occupied-samples.json");
  await writeFile(occupiedSamples, "occupied\n");
  await assert.rejects(
    collectDesignContextCanaryMetrics({
      baseUrl: `http://127.0.0.1:${address.port}`,
      sessionPath,
      observationEndedAt: "2026-07-10T00:00:00Z",
      conclusionRecordedBy: "operator-1",
      exportOutput: partialExport,
      metricsOutput: partialMetrics,
      metricsSourceUri: "evidence://canary/partial-metrics",
      samplesOutput: occupiedSamples,
      samplesSourceUri: "evidence://canary/occupied-samples",
      adminToken: "admin-token",
    }),
    /EEXIST/,
  );
  await assert.rejects(access(partialExport), /ENOENT/, "failed output set must remove a partial export");
  await assert.rejects(access(partialMetrics), /ENOENT/, "failed output set must remove a partial metrics fragment");

  responseDocument = { ...responseDocument, authorization: "Bearer should-never-appear" };
  await assert.rejects(
    collectDesignContextCanaryMetrics({
      baseUrl: `http://127.0.0.1:${address.port}`,
      sessionPath,
      observationEndedAt: "2026-07-10T00:00:00Z",
      conclusionRecordedBy: "operator-1",
      exportOutput: join(root, "secret-export.json"),
      metricsOutput: join(root, "secret-metrics.json"),
      metricsSourceUri: "evidence://canary/secret",
      adminToken: "admin-token",
    }),
    /credential-like/,
  );
  assert.equal(requestCount, 4);
} finally {
  await new Promise(resolve => server.close(resolve));
  await rm(root, { recursive: true, force: true });
}

process.stdout.write("design-context canary metrics collector tests passed\n");
