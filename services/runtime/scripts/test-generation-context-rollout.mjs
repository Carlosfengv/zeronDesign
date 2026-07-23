#!/usr/bin/env node

import assert from "node:assert/strict";
import { evaluateRolloutEvidence } from "./evaluate-generation-context-rollout.mjs";

const bucketCounts = { greenfield: 20, warm_copy_css: 20, warm_structural: 20, cold_dev: 10, repair: 10 };
const pairs = [];
for (const [bucket, count] of Object.entries(bucketCounts)) {
  for (let index = 0; index < count; index += 1) {
    const batchId = `batch-${index % 3}`;
    const controlBuild = 100_000 + index;
    const candidateBuild = 50_000 + index;
    const iframe = bucket === "warm_copy_css" ? 700 : 1_500;
    pairs.push({
      id: `${bucket}-${index}`,
      batchId,
      bucket,
      identity: {
        fixtureId: `fixture-${bucket}-${index}`,
        modelResource: "provider-model",
        providerResourceRevision: 3,
        modelVersion: "v1",
        providerParametersHash: "a".repeat(64),
        templateVersion: bucket === "repair" ? "fumadocs-docs@runtime-p6" : "next-app@2",
        capabilitySnapshotHash: "b".repeat(64),
        designProfileHash: "c".repeat(64),
        phase: bucket,
      },
      control: {
        flag: "legacy",
        status: index === 0 ? "failed" : "completed",
        execution: {
          gatewayMode: "internal_gateway",
          modelResourceId: "provider-model",
          providerResourceRevision: 3,
          modelExecutionEvidenceSha256: "e".repeat(64),
        },
        source: { storageRef: `evidence://control/${bucket}/${index}`, contentSha256: "c".repeat(64) },
        acceptanceEvidenceSha256: "9".repeat(64),
        firstBuildSucceeded: index !== 0,
        requiredFidelityPassed: index !== 0,
        metrics: {
          duplicateReadTokens: 1_000,
          inputTokens: 100_000,
          timeToFirstGreenfieldBuildMs: controlBuild,
        },
      },
      candidate: {
        flag: "generation_context",
        status: index === 0 ? "failed" : "completed",
        execution: {
          gatewayMode: "internal_gateway",
          modelResourceId: "provider-model",
          providerResourceRevision: 3,
          modelExecutionEvidenceSha256: "f".repeat(64),
        },
        source: { storageRef: `evidence://candidate/${bucket}/${index}`, contentSha256: "d".repeat(64) },
        acceptanceEvidenceSha256: "8".repeat(64),
        firstBuildSucceeded: index !== 0,
        requiredFidelityPassed: index !== 0,
        metrics: {
          duplicateReadTokens: 100,
          inputTokens: 40_000,
          timeToFirstGreenfieldBuildMs: candidateBuild,
          coldDevReadyMs: 10_000,
          timeToIframeAppliedMs: iframe,
          timeToDurableSnapshotMs: 2_000,
          modelTurnAtFirstSourceMutation: 2,
          prebuildFsListCount: 1,
          prebuildFsSearchCount: 1,
          duplicateFullReadRateBasisPoints: 100,
          outOfScopeMutationCount: 0,
        },
      },
    });
  }
}

const fixture = {
  schemaVersion: "generation-context-rollout-evidence@1",
  calculatorVersion: "generation-context-rollout-calculator@1",
  bootstrap: { iterations: 200, seed: 42 },
  coverage: {
    nextTemplate: true,
    fumadocsTemplate: true,
    multimodalVisualDelivered: true,
    nonVisualUnavailableMainTaskPassed: true,
    runtimeRestart: true,
  },
  pairs,
};

const passed = evaluateRolloutEvidence(fixture);
assert.deepEqual(passed.errors, []);
assert.equal(passed.result, "pass");
assert(Object.values(passed.buckets).every(bucket => bucket.status === "pass"));
assert.equal(passed.buckets.greenfield.distributions.inputTokens.candidate.median.value, 40_000);
assert(finiteReport(passed.buckets.greenfield.distributions.timeToFirstGreenfieldBuildMs.candidate.p95.value));
assert.equal(
  passed.buckets.greenfield.distributions.inputTokens.candidate.median.interval95.length,
  2,
);

const undersized = structuredClone(fixture);
undersized.pairs = undersized.pairs.filter(pair => pair.bucket !== "cold_dev" || !pair.id.endsWith("-9"));
assert.equal(evaluateRolloutEvidence(undersized).result, "insufficient_evidence");

const regression = structuredClone(fixture);
for (const pair of regression.pairs.filter(pair => pair.bucket === "warm_copy_css")) {
  pair.candidate.metrics.timeToIframeAppliedMs = 4_000;
}
assert.equal(evaluateRolloutEvidence(regression).buckets.warm_copy_css.status, "fail");

const missingFailure = structuredClone(fixture);
missingFailure.pairs[0].control.status = "discarded";
assert.equal(evaluateRolloutEvidence(missingFailure).result, "invalid");

const directCredential = structuredClone(fixture);
directCredential.providerApiKey = "not-a-real-value";
assert.equal(evaluateRolloutEvidence(directCredential).result, "invalid");

const wrongRevision = structuredClone(fixture);
wrongRevision.pairs[0].candidate.execution.providerResourceRevision = 4;
assert.equal(evaluateRolloutEvidence(wrongRevision).result, "invalid");

process.stdout.write("generation-context rollout evaluator tests passed\n");

function finiteReport(value) {
  return typeof value === "number" && Number.isFinite(value);
}
