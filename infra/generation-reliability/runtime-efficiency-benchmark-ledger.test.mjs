#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { evaluateRuntimeEfficiencyBenchmark } from "./runtime-efficiency-benchmark.mjs";
import {
  appendBenchmarkAttempt,
  appendBenchmarkAttempts,
  assembleBenchmarkCohort,
  evaluateBenchmarkLedger,
  initializeBenchmarkLedger,
  verifyBenchmarkLedger,
} from "./runtime-efficiency-benchmark-ledger.mjs";

function session() {
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
      sha256: "a".repeat(64),
      promptIds: Array.from({ length: 10 }, (_, index) => `prompt-${index}`),
    },
    profiles: [{
      profileId: "next-app-profile",
      workload: "greenfield_build",
      designProfileHash: "b".repeat(64),
      templateId: "next-app",
      templateVersion: "runtime-p7",
      modelResourceId: "deepseek-v4-pro",
      providerResourceRevision: 4,
      modelVersion: "deepseek-v4-pro@4",
      providerParametersHash: "c".repeat(64),
      cacheUsageCapability: "reported",
    }],
  };
}

function acceptedAttempt(sequence, attemptId, variant) {
  return {
    profileId: "next-app-profile",
    attempt: {
      sequence,
      attemptId,
      variant,
      promptId: `prompt-${(sequence - 1) % 10}`,
      status: "accepted",
      terminalEvidenceSha256: "d".repeat(64),
      metrics: {
        modelTurns: variant === "candidate" ? 6 : 10,
        grossInputTokens: variant === "candidate" ? 100_000 : 170_000,
        uncachedInputTokens: variant === "candidate" ? 60_000 : 110_000,
        maxPromptTokensPerTurn: variant === "candidate" ? 12_000 : 18_000,
        cacheHitRateBasisPoints: variant === "candidate" ? 7_000 : 3_000,
        firstSourceMutationTurn: variant === "candidate" ? 2 : 4,
        generationContextBytes: 12_000,
        duplicateFullContextReads: variant === "candidate" ? 0 : 1,
        outOfScopeMutations: 0,
        requiredFidelityPassed: true,
      },
    },
  };
}

function failedAttempt(sequence, attemptId) {
  const record = acceptedAttempt(sequence, attemptId, "candidate");
  record.attempt.status = "failed";
  for (const key of Object.keys(record.attempt.metrics)) record.attempt.metrics[key] = null;
  return record;
}

function temporaryLedger() {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "runtime-efficiency-ledger-"));
  return { directory, ledger: path.join(directory, "benchmark.jsonl") };
}

test("Hash-chain Ledger assembles a cohort with a computed raw Ledger SHA", () => {
  const { directory, ledger } = temporaryLedger();
  try {
    const initialized = initializeBenchmarkLedger(ledger, session());
    assert.equal(initialized.attemptCount, 0);
    appendBenchmarkAttempt(ledger, acceptedAttempt(1, "baseline-1", "baseline"));
    appendBenchmarkAttempt(ledger, failedAttempt(2, "candidate-failed-1"));
    const verification = verifyBenchmarkLedger(ledger);
    assert.equal(verification.status, "passed");
    assert.equal(verification.attemptCount, 2);
    const cohort = assembleBenchmarkCohort(ledger);
    assert.equal(cohort.ledger.sha256, verification.ledgerSha256);
    assert.equal(cohort.ledger.recordCount, 2);
    assert.equal(cohort.profiles[0].attempts[1].status, "failed");
    const evaluation = evaluateRuntimeEfficiencyBenchmark(cohort);
    assert.equal(evaluation.result, "insufficient_sample");
    assert.equal(evaluation.profiles["next-app-profile"].variants.candidate.failedCount, 1);
    const authoritative = evaluateBenchmarkLedger(ledger);
    assert.equal(authoritative.sourceLedger.ledgerSha256, verification.ledgerSha256);
    assert.equal(authoritative.evaluation.result, "insufficient_sample");
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("append rejects sequence gaps and duplicate Attempt IDs", () => {
  const { directory, ledger } = temporaryLedger();
  try {
    initializeBenchmarkLedger(ledger, session());
    appendBenchmarkAttempt(ledger, acceptedAttempt(1, "baseline-1", "baseline"));
    assert.throws(
      () => appendBenchmarkAttempt(ledger, acceptedAttempt(3, "candidate-3", "candidate")),
      /attempt\.sequence must be 2/,
    );
    assert.throws(
      () => appendBenchmarkAttempt(ledger, acceptedAttempt(2, "baseline-1", "candidate")),
      /duplicate Attempt ID/,
    );
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("batch append validates every Attempt before writing any record", () => {
  const { directory, ledger } = temporaryLedger();
  try {
    initializeBenchmarkLedger(ledger, session());
    assert.throws(
      () => appendBenchmarkAttempts(ledger, [
        acceptedAttempt(1, "baseline-1", "baseline"),
        acceptedAttempt(3, "candidate-1", "candidate"),
      ]),
      /attempt\.sequence must be 2/,
    );
    assert.equal(verifyBenchmarkLedger(ledger).attemptCount, 0);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("record tampering breaks verification and cannot be assembled", () => {
  const { directory, ledger } = temporaryLedger();
  try {
    initializeBenchmarkLedger(ledger, session());
    appendBenchmarkAttempt(ledger, acceptedAttempt(1, "baseline-1", "baseline"));
    const records = fs.readFileSync(ledger, "utf8").trimEnd().split("\n").map(JSON.parse);
    records[1].payload.attempt.metrics.grossInputTokens = 1;
    fs.writeFileSync(ledger, `${records.map(JSON.stringify).join("\n")}\n`);
    assert.throws(() => verifyBenchmarkLedger(ledger), /record hash mismatch/);
    assert.throws(() => assembleBenchmarkCohort(ledger), /record hash mismatch/);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("Session initialization rejects prompt text and Provider credentials", () => {
  const first = temporaryLedger();
  const second = temporaryLedger();
  try {
    const promptLeak = session();
    promptLeak.promptText = "raw benchmark prompt";
    assert.throws(() => initializeBenchmarkLedger(first.ledger, promptLeak), /forbidden/);

    const credentialLeak = session();
    credentialLeak.providerApiKey = "sk-not-a-real-key";
    assert.throws(() => initializeBenchmarkLedger(second.ledger, credentialLeak), /forbidden/);
  } finally {
    fs.rmSync(first.directory, { recursive: true, force: true });
    fs.rmSync(second.directory, { recursive: true, force: true });
  }
});
