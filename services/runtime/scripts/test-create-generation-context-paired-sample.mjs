#!/usr/bin/env node

import assert from "node:assert/strict";
import { createPairedCohortSample } from "./create-generation-context-paired-sample.mjs";

const hash = (value) => value.repeat(64);
const metadata = {
  schemaVersion: "generation-context-paired-cohort-sample-metadata@1",
  pairId: "pair-1",
  batchId: "batch-1",
  bucket: "warm_copy_css",
  side: "candidate",
  status: "completed",
  recordedAt: "2026-07-20T00:00:00.000Z",
  identity: {
    fixtureId: "fixture-1",
    modelResource: "deepseek-v4-pro",
    providerResourceRevision: 4,
    modelVersion: "deepseek-v4-pro",
    providerParametersHash: hash("a"),
    templateVersion: "next-app@2",
    capabilitySnapshotHash: hash("b"),
    phase: "edit",
  },
  execution: {
    gatewayMode: "internal_gateway",
    modelResourceId: "deepseek-v4-pro",
    providerResourceRevision: 4,
    modelExecutionEvidenceSha256: hash("c"),
  },
  source: { storageRef: "evidence://pair-1/candidate", contentSha256: hash("d") },
  acceptanceEvidenceSha256: hash("e"),
  coverage: ["nextTemplate"],
};
const efficiency = {
  schemaVersion: "run-efficiency-metrics@1",
  calculatorVersion: "run-efficiency-calculator@1",
  runId: "omitted-from-output",
  projectId: "omitted-from-output",
  phase: "edit",
  model: "deepseek-v4-pro",
  template: "next-app",
  status: "completed",
  totalDurationMs: 2_000,
  timeToFirstModelTurnMs: 100,
  timeToFirstSourceMutationMs: 300,
  modelTurnAtFirstSourceMutation: 1,
  timeToFirstGreenfieldStaticBuildMs: null,
  coldDevReadyMs: null,
  timeToIframeAppliedMs: 700,
  timeToDurableSnapshotMs: 900,
  timeToDraftReadyMs: 1_000,
  prebuildFsReadCount: 1,
  prebuildFsListCount: 1,
  prebuildFsSearchCount: 0,
  inputTokens: 4_000,
  outputTokens: 500,
  cachedInputTokens: 0,
  contextReadDeliveries: 1,
  sourceReadDeliveries: 2,
  diagnosticReadDeliveries: 0,
  verificationReadDeliveries: 0,
  fullReadDeliveries: 3,
  duplicateFullReadDeliveries: 0,
  duplicateFullReadRateBasisPoints: 0,
  duplicateReadEstimatedTokens: 0,
  outOfScopeMutationCount: 0,
  firstBuildSucceeded: false,
  requiredFidelityPassed: true,
};

const resourceSelectedEfficiency = {
  ...efficiency,
  model: `resource:${metadata.identity.modelResource}`,
};
assert.equal(
  createPairedCohortSample(metadata, resourceSelectedEfficiency).identity.modelResource,
  metadata.identity.modelResource,
);

const sample = createPairedCohortSample(metadata, efficiency);
assert.equal(sample.metrics.timeToIframeAppliedMs, 700);
assert.equal(sample.metrics.timeToDurableSnapshotMs, 900);
assert.equal(sample.metrics.timeToFirstSourceMutationMs, 300);
assert.equal(sample.requiredFidelityPassed, true);
assert.equal(sample.firstBuildSucceeded, false);
assert.equal("runId" in sample, false);
assert.equal("projectId" in sample, false);

assert.throws(
  () => createPairedCohortSample({ ...metadata, status: "completed" }, { ...efficiency, status: "failed" }),
  /cannot mark a non-completed/,
);
assert.throws(
  () => createPairedCohortSample(metadata, { ...efficiency, model: "other-resource" }),
  /model must select/,
);

process.stdout.write("Generation Context paired sample creator tests passed.\n");
