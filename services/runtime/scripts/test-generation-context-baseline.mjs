#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { calculateBaselineSummary } from "./calculate-generation-context-baseline.mjs";

const manifestUrl = new URL("../evidence/baselines/generation-context-2026-07-19.json", import.meta.url);
const manifest = JSON.parse(await readFile(manifestUrl, "utf8"));
const result = calculateBaselineSummary(manifest);
assert.deepEqual(result.errors, []);
assert.equal(result.summary.sampleCount, 2);
assert.equal(result.summary.eligibleSampleCount, 0);
assert.equal(result.summary.excludedSampleCount, 2);
assert.equal(result.summary.evidenceState, "insufficient_evidence");
assert.equal(result.summary.metrics.nextActionViolationRateBasisPoints, null);
assert.equal(result.summary.metrics.firstBuildSuccessRateBasisPoints, null);

const invalid = structuredClone(manifest);
invalid.samples[0].inclusion.includedInImprovementDenominator = true;
assert.match(calculateBaselineSummary(invalid).errors.join("\n"), /unavailable evidence cannot enter/);

const eligible = structuredClone(manifest);
eligible.samples = [{
  id: "eligible-build",
  source: { availability: "available", contentSha256: "a".repeat(64) },
  metrics: {
    inputTokens: 100,
    timeToFirstBuildMs: 200,
    timeToFirstSourceMutationMs: 50,
    modelTurnAtFirstSourceMutation: 3,
    prebuildObservationCallCount: 7,
    duplicateReadTokens: 0,
    nextActionMatchCount: 2,
    nextActionViolationCount: 3,
    completionMissingDurableSnapshot: 1,
    noProgressFailure: 0,
    firstBuildSucceeded: 1,
    artifactAccepted: 0,
  },
  inclusion: { includedInImprovementDenominator: true, exclusionReasons: [] },
}];
const eligibleSummary = calculateBaselineSummary(eligible);
assert.deepEqual(eligibleSummary.errors, []);
assert.equal(eligibleSummary.summary.metrics.timeToFirstSourceMutationMsMedian, 50);
assert.equal(eligibleSummary.summary.metrics.prebuildObservationCallCountMedian, 7);
assert.equal(eligibleSummary.summary.metrics.nextActionViolationRateBasisPoints, 6000);
assert.equal(eligibleSummary.summary.metrics.completionMissingDurableSnapshotCount, 1);
assert.equal(eligibleSummary.summary.metrics.firstBuildSuccessRateBasisPoints, 10000);
assert.equal(eligibleSummary.summary.metrics.artifactAcceptanceRateBasisPoints, 0);
console.log("generation-context baseline calculator tests passed");
