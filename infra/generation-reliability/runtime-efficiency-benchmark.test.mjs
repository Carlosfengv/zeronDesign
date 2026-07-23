#!/usr/bin/env node

import assert from "node:assert/strict";
import test from "node:test";
import { evaluateRuntimeEfficiencyBenchmark } from "./runtime-efficiency-benchmark.mjs";

function metrics(variant, overrides = {}) {
  const candidate = variant === "candidate";
  return {
    modelTurns: candidate ? 6 : 10,
    grossInputTokens: candidate ? 100_000 : 170_000,
    uncachedInputTokens: candidate ? 60_000 : 110_000,
    maxPromptTokensPerTurn: candidate ? 12_000 : 18_000,
    cacheHitRateBasisPoints: candidate ? 7_000 : 3_500,
    firstSourceMutationTurn: candidate ? 2 : 4,
    generationContextBytes: 12_000,
    duplicateFullContextReads: candidate ? 0 : 1,
    outOfScopeMutations: 0,
    requiredFidelityPassed: true,
    ...overrides,
  };
}

function fixture() {
  const attempts = [];
  let sequence = 1;
  for (const variant of ["baseline", "candidate"]) {
    for (let index = 0; index < 30; index += 1) {
      attempts.push({
        sequence,
        attemptId: `${variant}-${index}`,
        variant,
        promptId: `prompt-${index % 10}`,
        status: "accepted",
        terminalEvidenceSha256: (variant === "baseline" ? "a" : "b").repeat(64),
        metrics: metrics(variant),
      });
      sequence += 1;
    }
  }
  return {
    schemaVersion: "runtime-efficiency-benchmark-cohort@1",
    calculatorVersion: "runtime-efficiency-benchmark-calculator@1",
    source: { commit: "abc123", dirty: false },
    bootstrap: { iterations: 200, seed: 42 },
    promptSet: {
      id: "design-system-generation",
      version: "2026-07-23",
      sha256: "c".repeat(64),
      promptIds: Array.from({ length: 10 }, (_, index) => `prompt-${index}`),
    },
    ledger: {
      schemaVersion: "runtime-efficiency-benchmark-ledger@1",
      sha256: "d".repeat(64),
      firstSequence: 1,
      lastSequence: attempts.length,
      recordCount: attempts.length,
    },
    profiles: [{
      profileId: "next-app-deepseek-v4-pro",
      workload: "greenfield_build",
      designProfileHash: "e".repeat(64),
      templateId: "next-app",
      templateVersion: "runtime-p7",
      modelResourceId: "deepseek-v4-pro",
      providerResourceRevision: 4,
      modelVersion: "deepseek-v4-pro@4",
      providerParametersHash: "f".repeat(64),
      cacheUsageCapability: "reported",
      attempts,
    }],
  };
}

function repairLedger(cohort) {
  const attempts = cohort.profiles.flatMap(profile => profile.attempts);
  cohort.ledger.recordCount = attempts.length;
  cohort.ledger.firstSequence = Math.min(...attempts.map(attempt => attempt.sequence));
  cohort.ledger.lastSequence = Math.max(...attempts.map(attempt => attempt.sequence));
}

test("a complete fixed-identity cohort reports distributions, confidence intervals, effects, and gates", () => {
  const result = evaluateRuntimeEfficiencyBenchmark(fixture());
  assert.deepEqual(result.errors, []);
  assert.equal(result.result, "pass");
  const profile = result.profiles["next-app-deepseek-v4-pro"];
  assert.equal(profile.status, "pass");
  assert.equal(profile.variants.candidate.acceptedCount, 30);
  assert.equal(profile.variants.candidate.acceptedPromptCount, 10);
  assert.equal(profile.variants.candidate.distributions.grossInputTokens.p95.status, "ready");
  assert.equal(profile.variants.candidate.distributions.grossInputTokens.p95.value, 100_000);
  assert.equal(profile.variants.candidate.distributions.grossInputTokens.p95.interval95.length, 2);
  assert(profile.effectSizes.grossInputTokens.value > 0);
  assert(profile.gates.every(gate => ["pass", "not_applicable"].includes(gate.status)));
});

test("an undersized candidate cohort is explicitly insufficient_sample and suppresses P50/P95 claims", () => {
  const cohort = fixture();
  cohort.profiles[0].attempts.pop();
  repairLedger(cohort);
  const result = evaluateRuntimeEfficiencyBenchmark(cohort);
  assert.equal(result.result, "insufficient_sample");
  const profile = result.profiles["next-app-deepseek-v4-pro"];
  assert.equal(profile.status, "insufficient_sample");
  assert.equal(profile.variants.candidate.sampleStatus, "insufficient_sample");
  assert.equal(profile.variants.candidate.distributions.grossInputTokens.p50.status, "insufficient_sample");
  assert.equal(profile.variants.candidate.distributions.grossInputTokens.p50.value, null);
  assert.equal(profile.variants.candidate.distributions.grossInputTokens.p95.value, null);
  assert.deepEqual(profile.gates, []);
});

test("failed Attempts remain visible without reducing the accepted sample count", () => {
  const cohort = fixture();
  cohort.profiles[0].attempts.push({
    sequence: 61,
    attemptId: "candidate-failed-30",
    variant: "candidate",
    promptId: "prompt-0",
    status: "failed",
    terminalEvidenceSha256: "9".repeat(64),
    metrics: Object.fromEntries(Object.keys(metrics("candidate")).map(key => [key, null])),
  });
  repairLedger(cohort);
  const result = evaluateRuntimeEfficiencyBenchmark(cohort);
  assert.equal(result.result, "pass");
  const candidate = result.profiles["next-app-deepseek-v4-pro"].variants.candidate;
  assert.equal(candidate.attemptCount, 31);
  assert.equal(candidate.acceptedCount, 30);
  assert.equal(candidate.failedCount, 1);
  assert.equal(candidate.failureStatusCounts.failed, 1);
});

test("a continuous Ledger gap fails closed instead of silently dropping an Attempt", () => {
  const cohort = fixture();
  cohort.profiles[0].attempts.splice(9, 1);
  cohort.ledger.recordCount -= 1;
  const result = evaluateRuntimeEfficiencyBenchmark(cohort);
  assert.equal(result.result, "invalid");
  assert(result.errors.some(error => error.includes("complete, continuous")));
});

test("a statistically ready cohort still fails deterministic workload thresholds", () => {
  const cohort = fixture();
  for (const attempt of cohort.profiles[0].attempts.filter(attempt => attempt.variant === "candidate")) {
    attempt.metrics.modelTurns = 20;
  }
  const result = evaluateRuntimeEfficiencyBenchmark(cohort);
  assert.equal(result.result, "fail");
  assert.equal(result.profiles["next-app-deepseek-v4-pro"].status, "fail");
  assert(result.profiles["next-app-deepseek-v4-pro"].gates.some(gate => gate.name === "modelTurns.p95" && gate.status === "fail"));
});

test("providers without cached usage omit only the cache gate and never fabricate cache numbers", () => {
  const cohort = fixture();
  cohort.profiles[0].cacheUsageCapability = "unsupported";
  for (const attempt of cohort.profiles[0].attempts) attempt.metrics.cacheHitRateBasisPoints = null;
  const result = evaluateRuntimeEfficiencyBenchmark(cohort);
  assert.equal(result.result, "pass");
  const profile = result.profiles["next-app-deepseek-v4-pro"];
  assert.equal(profile.gates.find(gate => gate.name === "cacheHitRateBasisPoints.p50").status, "not_applicable");
  assert.equal(profile.variants.candidate.distributions.cacheHitRateBasisPoints.p50.status, "not_applicable");
  assert.equal(profile.effectSizes.cacheHitRateBasisPoints.status, "not_applicable");
});

test("accepted Attempts reject impossible zero work and cache rates above 100 percent", () => {
  const zeroTurns = fixture();
  zeroTurns.profiles[0].attempts[0].metrics.modelTurns = 0;
  assert.equal(evaluateRuntimeEfficiencyBenchmark(zeroTurns).result, "invalid");

  const impossibleCache = fixture();
  impossibleCache.profiles[0].attempts[0].metrics.cacheHitRateBasisPoints = 10_001;
  assert.equal(evaluateRuntimeEfficiencyBenchmark(impossibleCache).result, "invalid");
});

test("unknown Prompt IDs and sensitive fields make the cohort invalid", () => {
  const unknownPrompt = fixture();
  unknownPrompt.profiles[0].attempts[0].promptId = "prompt-not-frozen";
  assert.equal(evaluateRuntimeEfficiencyBenchmark(unknownPrompt).result, "invalid");

  const sensitive = fixture();
  sensitive.profiles[0].attempts[0].promptText = "do not persist prompt text";
  const result = evaluateRuntimeEfficiencyBenchmark(sensitive);
  assert.equal(result.result, "invalid");
  assert(result.errors.some(error => error.includes("forbidden in benchmark evidence")));
});
