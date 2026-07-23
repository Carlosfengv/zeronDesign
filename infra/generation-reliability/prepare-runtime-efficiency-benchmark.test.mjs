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
import { collectRuntimeEfficiencyBenchmarkAttempts } from "./collect-runtime-efficiency-benchmark.mjs";
import { verifyBenchmarkLedger } from "./runtime-efficiency-benchmark-ledger.mjs";
import { prepareRuntimeEfficiencyBenchmark } from "./prepare-runtime-efficiency-benchmark.mjs";

const sha = character => character.repeat(64);

function session() {
  return {
    schemaVersion: "generation-context-paired-cohort-session@1",
    sessionId: "prepared-source-session",
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

function sample(side, bucket, fixtureId, designProfileHash = sha("c")) {
  const phase = bucket === "greenfield" ? "build" : "edit";
  const candidate = side === "candidate";
  return {
    schemaVersion: "generation-context-paired-cohort-sample@1",
    pairId: `${bucket}-${fixtureId}`,
    batchId: "benchmark-batch",
    bucket,
    side,
    flag: side === "control" ? "legacy" : "generation_context",
    status: "completed",
    recordedAt: "2026-07-23T10:01:00.000Z",
    identity: {
      fixtureId,
      modelResource: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelVersion: "deepseek-v4-pro-2026-07",
      providerParametersHash: sha("b"),
      templateVersion: "next-app@2",
      capabilitySnapshotHash: sha("d"),
      designProfileHash,
      phase,
    },
    execution: {
      gatewayMode: "internal_gateway",
      modelResourceId: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelExecutionEvidenceSha256: candidate ? sha("e") : sha("f"),
    },
    source: {
      storageRef: `evidence://${bucket}/${fixtureId}/${side}`,
      contentSha256: candidate ? sha("1") : sha("2"),
    },
    acceptanceEvidenceSha256: sha("3"),
    firstBuildSucceeded: phase === "build",
    requiredFidelityPassed: true,
    coverage: [],
    metrics: {
      modelTurnAtFirstSourceMutation: candidate ? 2 : 4,
      duplicateFullReadDeliveries: candidate ? 0 : 1,
      outOfScopeMutationCount: 0,
      modelTurns: candidate ? 6 : 10,
      grossInputTokens: candidate ? 100_000 : 170_000,
      uncachedInputTokens: candidate ? 60_000 : 110_000,
      maxTurnInputTokens: candidate ? 12_000 : 18_000,
      cacheHitRateBasisPoints: candidate ? 7_000 : 3_000,
      generationContextBytes: candidate ? 12_000 : 0,
      caseAttemptCount: 1,
    },
  };
}

function sourceFixture(promptCount = 10, driftProfile = false) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "prepare-runtime-efficiency-"));
  const ledger = path.join(directory, "paired.ndjson");
  initializePairedCohortLedger(ledger, session());
  for (const bucket of ["greenfield", "warm_copy_css"]) {
    for (let index = 0; index < promptCount; index += 1) {
      const fixtureId = `design-system-prompt-${index}`;
      const profileHash = driftProfile && bucket === "greenfield" && index === 1
        ? sha("9")
        : sha("c");
      appendPairedCohortPair(
        ledger,
        sample("control", bucket, fixtureId, profileHash),
        sample("candidate", bucket, fixtureId, profileHash),
      );
    }
  }
  return { directory, ledger };
}

test("preparation derives frozen Profiles, Prompt mapping, and an importable Ledger", () => {
  const current = sourceFixture();
  const output = path.join(current.directory, "benchmark");
  try {
    const result = prepareRuntimeEfficiencyBenchmark(current.ledger, output);
    assert.equal(result.promptCount, 10);
    assert.equal(result.profiles.length, 2);
    const mapping = JSON.parse(fs.readFileSync(path.join(output, "import-mapping.json"), "utf8"));
    const benchmarkLedger = path.join(output, "benchmark.ndjson");
    const imported = collectRuntimeEfficiencyBenchmarkAttempts(
      current.ledger,
      benchmarkLedger,
      mapping,
    );
    assert.equal(imported.importedAttemptCount, 40);
    assert.equal(verifyBenchmarkLedger(benchmarkLedger).attemptCount, 40);
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});

test("preparation fails before ten unique Prompt Fixtures exist", () => {
  const current = sourceFixture(9);
  try {
    assert.throws(
      () => prepareRuntimeEfficiencyBenchmark(current.ledger, path.join(current.directory, "benchmark")),
      /at least 10 unique Prompt Fixtures/,
    );
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});

test("preparation rejects mixed Design Profile identity inside one workload", () => {
  const current = sourceFixture(10, true);
  try {
    assert.throws(
      () => prepareRuntimeEfficiencyBenchmark(current.ledger, path.join(current.directory, "benchmark")),
      /exactly one frozen Design Profile\/Template\/Provider identity/,
    );
  } finally {
    fs.rmSync(current.directory, { recursive: true, force: true });
  }
});
