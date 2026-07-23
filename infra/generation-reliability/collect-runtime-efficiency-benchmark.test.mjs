#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  appendPairedCohortPair,
  initializePairedCohortLedger,
} from "../../services/runtime/scripts/generation-context-paired-cohort-ledger.mjs";
import {
  appendBenchmarkAttempts,
  getBenchmarkLedgerAppendContext,
  initializeBenchmarkLedger,
  verifyBenchmarkLedger,
} from "./runtime-efficiency-benchmark-ledger.mjs";
import {
  collectRuntimeEfficiencyBenchmarkAttempts,
  synchronizeRuntimeEfficiencyBenchmarkAttempts,
  validateRuntimeEfficiencyBenchmarkSourceBinding,
} from "./collect-runtime-efficiency-benchmark.mjs";

const sha = character => character.repeat(64);

function pairedSession() {
  return {
    schemaVersion: "generation-context-paired-cohort-session@1",
    sessionId: "paired-session-1",
    createdAt: "2026-07-23T10:00:00.000Z",
    calculatorVersion: "generation-context-rollout-calculator@1",
    bootstrap: { iterations: 100, seed: 42 },
    source: { commit: "abc123", dirty: false },
    sourcePolicy: "hashes_only",
    fixtureManifestSha256: sha("a"),
    providers: [{
      gatewayMode: "internal_gateway",
      modelResourceId: "deepseek-v4-pro",
      resourceRevision: 7,
      modelVersion: "deepseek-v4-pro-2026-07",
      providerParametersHash: sha("b"),
    }],
    runtimes: {
      control: {
        generationContextMode: "off",
        deploymentRevision: "control-1",
        allowedModelResourceIds: ["deepseek-v4-pro"],
      },
      candidate: {
        generationContextMode: "enabled",
        deploymentRevision: "candidate-1",
        allowedModelResourceIds: ["deepseek-v4-pro"],
      },
    },
  };
}

function sample(side, overrides = {}) {
  return {
    schemaVersion: "generation-context-paired-cohort-sample@1",
    pairId: "pair-1",
    batchId: "batch-1",
    bucket: "greenfield",
    side,
    flag: side === "control" ? "legacy" : "generation_context",
    status: "completed",
    recordedAt: "2026-07-23T10:01:00.000Z",
    identity: {
      fixtureId: "fixture-1",
      modelResource: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelVersion: "deepseek-v4-pro-2026-07",
      providerParametersHash: sha("b"),
      templateVersion: "next-app@2",
      capabilitySnapshotHash: sha("c"),
      designProfileHash: sha("4"),
      phase: "build",
    },
    execution: {
      gatewayMode: "internal_gateway",
      modelResourceId: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelExecutionEvidenceSha256: side === "control" ? sha("d") : sha("e"),
    },
    source: {
      storageRef: `evidence://pair-1/${side}`,
      contentSha256: side === "control" ? sha("f") : sha("1"),
    },
    acceptanceEvidenceSha256: sha("2"),
    firstBuildSucceeded: true,
    requiredFidelityPassed: true,
    coverage: [],
    metrics: {
      modelTurnAtFirstSourceMutation: side === "control" ? 4 : 2,
      duplicateFullReadDeliveries: side === "control" ? 1 : 0,
      outOfScopeMutationCount: 0,
      modelTurns: side === "control" ? 10 : 6,
      grossInputTokens: side === "control" ? 170_000 : 100_000,
      uncachedInputTokens: side === "control" ? 110_000 : 60_000,
      maxTurnInputTokens: side === "control" ? 18_000 : 12_000,
      cacheHitRateBasisPoints: side === "control" ? 3_000 : 7_000,
      generationContextBytes: side === "control" ? 0 : 12_000,
      caseAttemptCount: 1,
    },
    ...overrides,
  };
}

function benchmarkSession() {
  return {
    schemaVersion: "runtime-efficiency-benchmark-session@1",
    sessionId: "benchmark-session-1",
    createdAt: "2026-07-23T10:00:00.000Z",
    calculatorVersion: "runtime-efficiency-benchmark-calculator@1",
    source: { commit: "abc123", dirty: false },
    bootstrap: { iterations: 100, seed: 7 },
    promptSet: {
      id: "design-system-generation",
      version: "2026-07-23",
      sha256: sha("a"),
      promptIds: Array.from({ length: 10 }, (_, index) => `prompt-${index}`),
    },
    profiles: [{
      profileId: "greenfield-profile",
      workload: "greenfield_build",
      designProfileHash: sha("4"),
      templateId: "next-app",
      templateVersion: "2",
      modelResourceId: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelVersion: "deepseek-v4-pro-2026-07",
      providerParametersHash: sha("b"),
      cacheUsageCapability: "reported",
    }, {
      profileId: "edit-profile",
      workload: "style_token_edit",
      designProfileHash: sha("5"),
      templateId: "next-app",
      templateVersion: "2",
      modelResourceId: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelVersion: "deepseek-v4-pro-2026-07",
      providerParametersHash: sha("b"),
      cacheUsageCapability: "reported",
    }],
  };
}

const mapping = {
  schemaVersion: "runtime-efficiency-benchmark-import@1",
  buckets: {
    greenfield: "greenfield-profile",
    warm_copy_css: "edit-profile",
  },
  promptIdByFixtureId: { "fixture-1": "prompt-0" },
};

function fixture() {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "runtime-efficiency-import-"));
  const pairedLedger = path.join(directory, "paired.ndjson");
  const benchmarkLedger = path.join(directory, "benchmark.ndjson");
  initializePairedCohortLedger(pairedLedger, pairedSession());
  initializeBenchmarkLedger(benchmarkLedger, benchmarkSession());
  return { directory, pairedLedger, benchmarkLedger };
}

test("verified paired cohort imports baseline and candidate Attempts atomically", () => {
  const current = fixture();
  try {
    appendPairedCohortPair(current.pairedLedger, sample("control"), sample("candidate"));
    const result = collectRuntimeEfficiencyBenchmarkAttempts(
      current.pairedLedger,
      current.benchmarkLedger,
      mapping,
    );
    assert.equal(result.importedAttemptCount, 2);
    assert.equal(result.firstSequence, 1);
    assert.equal(result.lastSequence, 2);
    assert.equal(verifyBenchmarkLedger(current.benchmarkLedger).attemptCount, 2);
    const binding = validateRuntimeEfficiencyBenchmarkSourceBinding(
      current.pairedLedger,
      current.benchmarkLedger,
      mapping,
    );
    assert.equal(binding.status, "passed");
    assert.equal(binding.attemptCount, 2);
    assert.throws(
      () => collectRuntimeEfficiencyBenchmarkAttempts(
        current.pairedLedger,
        current.benchmarkLedger,
        mapping,
      ),
      /no new eligible/,
    );
    const synchronized = synchronizeRuntimeEfficiencyBenchmarkAttempts(
      current.pairedLedger,
      current.benchmarkLedger,
      mapping,
    );
    assert.equal(synchronized.status, "passed");
    assert.equal(synchronized.importedAttemptCount, 0);
    assert.equal(synchronized.sourceBinding.attemptCount, 2);
    appendPairedCohortPair(
      current.pairedLedger,
      sample("control", { pairId: "pair-2" }),
      sample("candidate", { pairId: "pair-2" }),
    );
    const incremental = collectRuntimeEfficiencyBenchmarkAttempts(
      current.pairedLedger,
      current.benchmarkLedger,
      mapping,
    );
    assert.equal(incremental.firstSequence, 3);
    assert.equal(incremental.lastSequence, 4);
    const resynchronized = synchronizeRuntimeEfficiencyBenchmarkAttempts(
      current.pairedLedger,
      current.benchmarkLedger,
      mapping,
    );
    assert.equal(resynchronized.importedAttemptCount, 0);
    assert.equal(resynchronized.benchmarkLedger.attemptCount, 4);
    assert.equal(validateRuntimeEfficiencyBenchmarkSourceBinding(
      current.pairedLedger,
      current.benchmarkLedger,
      mapping,
    ).attemptCount, 4);
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});

test("source binding rejects a structurally valid manually appended Attempt", () => {
  const current = fixture();
  const manualLedger = path.join(current.directory, "manual-benchmark.ndjson");
  try {
    appendPairedCohortPair(current.pairedLedger, sample("control"), sample("candidate"));
    collectRuntimeEfficiencyBenchmarkAttempts(
      current.pairedLedger,
      current.benchmarkLedger,
      mapping,
    );
    initializeBenchmarkLedger(manualLedger, benchmarkSession());
    const records = getBenchmarkLedgerAppendContext(current.benchmarkLedger).attempts;
    records[0].attempt.terminalEvidenceSha256 = sha("9");
    appendBenchmarkAttempts(manualLedger, records);
    assert.throws(
      () => validateRuntimeEfficiencyBenchmarkSourceBinding(
        current.pairedLedger,
        manualLedger,
        mapping,
      ),
      /do not exactly match/,
    );
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});

test("accepted source with a missing required metric does not partially append", () => {
  const current = fixture();
  try {
    const candidate = sample("candidate");
    delete candidate.metrics.generationContextBytes;
    appendPairedCohortPair(current.pairedLedger, sample("control"), candidate);
    assert.throws(
      () => collectRuntimeEfficiencyBenchmarkAttempts(
        current.pairedLedger,
        current.benchmarkLedger,
        mapping,
      ),
      /generationContextBytes is missing/,
    );
    assert.equal(verifyBenchmarkLedger(current.benchmarkLedger).attemptCount, 0);
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});

test("Provider or template identity drift is rejected", () => {
  const current = fixture();
  try {
    const candidate = sample("candidate");
    candidate.identity.templateVersion = "fumadocs-docs@2";
    assert.throws(
      () => appendPairedCohortPair(current.pairedLedger, sample("control"), candidate),
      /identity mismatch/,
    );
    const wrongProfileControl = sample("control");
    const wrongProfileCandidate = sample("candidate");
    wrongProfileControl.identity.designProfileHash = sha("8");
    wrongProfileCandidate.identity.designProfileHash = sha("8");
    appendPairedCohortPair(
      current.pairedLedger,
      wrongProfileControl,
      wrongProfileCandidate,
    );
    assert.throws(
      () => collectRuntimeEfficiencyBenchmarkAttempts(
        current.pairedLedger,
        current.benchmarkLedger,
        mapping,
      ),
      /does not match frozen Benchmark Profile/,
    );
    assert.equal(verifyBenchmarkLedger(current.benchmarkLedger).attemptCount, 0);
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});

test("a retried case cannot hide failed Attempts inside an accepted sample", () => {
  const current = fixture();
  try {
    const candidate = sample("candidate");
    candidate.metrics.caseAttemptCount = 2;
    appendPairedCohortPair(current.pairedLedger, sample("control"), candidate);
    assert.throws(
      () => collectRuntimeEfficiencyBenchmarkAttempts(
        current.pairedLedger,
        current.benchmarkLedger,
        mapping,
      ),
      /exactly one case Attempt/,
    );
    assert.equal(verifyBenchmarkLedger(current.benchmarkLedger).attemptCount, 0);
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});

test("Benchmark source commit and Prompt corpus must match the Paired Session", () => {
  const current = fixture();
  const wrongSourceLedger = path.join(current.directory, "wrong-source.ndjson");
  const wrongPromptLedger = path.join(current.directory, "wrong-prompt.ndjson");
  try {
    appendPairedCohortPair(current.pairedLedger, sample("control"), sample("candidate"));
    const wrongSource = benchmarkSession();
    wrongSource.source.commit = "other-commit";
    initializeBenchmarkLedger(wrongSourceLedger, wrongSource);
    assert.throws(
      () => collectRuntimeEfficiencyBenchmarkAttempts(
        current.pairedLedger,
        wrongSourceLedger,
        mapping,
      ),
      /source does not match/,
    );
    const wrongPrompt = benchmarkSession();
    wrongPrompt.promptSet.sha256 = sha("7");
    initializeBenchmarkLedger(wrongPromptLedger, wrongPrompt);
    assert.throws(
      () => collectRuntimeEfficiencyBenchmarkAttempts(
        current.pairedLedger,
        wrongPromptLedger,
        mapping,
      ),
      /Prompt Set SHA does not match/,
    );
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});
