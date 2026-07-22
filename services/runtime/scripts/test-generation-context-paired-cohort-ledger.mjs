#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  appendPairedCohortPair,
  appendPairedCohortSample,
  assemblePairedCohortEvidence,
  initializePairedCohortLedger,
  verifyPairedCohortLedger,
} from "./generation-context-paired-cohort-ledger.mjs";
import { evaluateRolloutEvidence } from "./evaluate-generation-context-rollout.mjs";

const HASH_A = "a".repeat(64);
const HASH_B = "b".repeat(64);
const HASH_C = "c".repeat(64);
const temp = fs.mkdtempSync(path.join(os.tmpdir(), "generation-context-paired-ledger-"));

function session() {
  return {
    schemaVersion: "generation-context-paired-cohort-session@1",
    sessionId: "deepseek-v4-pro-rollout-001",
    createdAt: "2026-07-20T00:00:00.000Z",
    calculatorVersion: "generation-context-rollout-calculator@1",
    bootstrap: { iterations: 100, seed: 42 },
    sourcePolicy: "hashes_only",
    fixtureManifestSha256: HASH_A,
    providers: [{
      gatewayMode: "internal_gateway",
      modelResourceId: "deepseek-v4-pro",
      resourceRevision: 7,
      modelVersion: "deepseek-v4-pro-2026-07",
      providerParametersHash: HASH_B,
    }],
    runtimes: {
      control: {
        generationContextMode: "off",
        deploymentRevision: "runtime-control-abc123",
        allowedModelResourceIds: ["deepseek-v4-pro"],
      },
      candidate: {
        generationContextMode: "enabled",
        deploymentRevision: "runtime-candidate-abc123",
        allowedModelResourceIds: ["deepseek-v4-pro"],
      },
    },
  };
}

function sample(side, pairId = "batch-1-fixture-1") {
  return {
    schemaVersion: "generation-context-paired-cohort-sample@1",
    pairId,
    batchId: "batch-1",
    bucket: "greenfield",
    side,
    flag: side === "control" ? "legacy" : "generation_context",
    status: "completed",
    recordedAt: "2026-07-20T00:01:00.000Z",
    identity: {
      fixtureId: "fixture-1",
      modelResource: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelVersion: "deepseek-v4-pro-2026-07",
      providerParametersHash: HASH_B,
      templateVersion: "next-app@2",
      capabilitySnapshotHash: HASH_C,
      phase: "build",
    },
    execution: {
      gatewayMode: "internal_gateway",
      modelResourceId: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelExecutionEvidenceSha256: HASH_A,
    },
    source: {
      storageRef: `evidence://batch-1/${pairId}/${side}`,
      contentSha256: side === "control" ? HASH_A : HASH_C,
    },
    acceptanceEvidenceSha256: HASH_B,
    firstBuildSucceeded: true,
    requiredFidelityPassed: true,
    coverage: side === "candidate" ? [
      "nextTemplate",
      "fumadocsTemplate",
      "multimodalVisualDelivered",
      "nonVisualUnavailableMainTaskPassed",
      "runtimeRestart",
    ] : [],
    metrics: {
      duplicateReadTokens: side === "control" ? 1000 : 100,
      inputTokens: side === "control" ? 10000 : 4000,
      timeToFirstGreenfieldBuildMs: side === "control" ? 100000 : 40000,
      modelTurnAtFirstSourceMutation: 1,
      prebuildFsListCount: 1,
      prebuildFsSearchCount: 0,
      duplicateFullReadRateBasisPoints: 100,
      outOfScopeMutationCount: 0,
    },
  };
}

const ledger = path.join(temp, "cohort.ndjson");
initializePairedCohortLedger(ledger, session());
appendPairedCohortSample(ledger, sample("control"));
let verification = verifyPairedCohortLedger(ledger);
assert.deepEqual(verification.pendingPairs, ["batch-1-fixture-1"]);
appendPairedCohortSample(ledger, sample("candidate"));
verification = verifyPairedCohortLedger(ledger);
assert.equal(verification.completePairCount, 1);
assert.deepEqual(verification.pendingPairs, []);

const evidence = assemblePairedCohortEvidence(ledger);
assert.equal(evidence.pairs.length, 1);
assert.equal(evidence.pairs[0].control.flag, "legacy");
assert.equal(evidence.pairs[0].candidate.flag, "generation_context");
assert.equal(evidence.provenance.providers[0].modelResourceId, "deepseek-v4-pro");
assert.equal(evidence.coverage.runtimeRestart, true);
assert.equal(evaluateRolloutEvidence(evidence).result, "insufficient_evidence");

assert.throws(
  () => appendPairedCohortSample(ledger, sample("control")),
  /duplicate sample side/,
);

const mismatched = sample("control", "mismatch-pair");
appendPairedCohortSample(ledger, mismatched);
const mismatchedCandidate = sample("candidate", "mismatch-pair");
mismatchedCandidate.identity.templateVersion = "fumadocs-docs@2";
assert.throws(
  () => appendPairedCohortSample(ledger, mismatchedCandidate),
  /identity mismatch/,
);
assert.throws(() => assemblePairedCohortEvidence(ledger), /incomplete pairs/);

const atomicLedger = path.join(temp, "atomic.ndjson");
initializePairedCohortLedger(atomicLedger, session());
appendPairedCohortPair(
  atomicLedger,
  sample("control", "atomic-pair"),
  sample("candidate", "atomic-pair"),
);
const atomicVerification = verifyPairedCohortLedger(atomicLedger);
assert.equal(atomicVerification.sampleCount, 2);
assert.equal(atomicVerification.completePairCount, 1);
assert.deepEqual(atomicVerification.pendingPairs, []);
assert.throws(
  () => appendPairedCohortPair(
    atomicLedger,
    sample("control", "atomic-pair"),
    sample("candidate", "atomic-pair"),
  ),
  /already has recorded samples/,
);

const rejectedAtomicLedger = path.join(temp, "atomic-rejected.ndjson");
initializePairedCohortLedger(rejectedAtomicLedger, session());
const rejectedCandidate = sample("candidate", "atomic-rejected-pair");
rejectedCandidate.identity.templateVersion = "fumadocs-docs@2";
assert.throws(
  () => appendPairedCohortPair(
    rejectedAtomicLedger,
    sample("control", "atomic-rejected-pair"),
    rejectedCandidate,
  ),
  /identity mismatch/,
);
assert.equal(verifyPairedCohortLedger(rejectedAtomicLedger).sampleCount, 0);

const secretLedger = path.join(temp, "secret.ndjson");
initializePairedCohortLedger(secretLedger, session());
const leaked = sample("control");
leaked.apiKey = "not-a-real-secret-value";
assert.throws(
  () => appendPairedCohortSample(secretLedger, leaked),
  /forbidden in hashes-only cohort evidence/,
);

const tamperedLedger = path.join(temp, "tampered.ndjson");
fs.copyFileSync(secretLedger, tamperedLedger);
fs.appendFileSync(tamperedLedger, `${JSON.stringify({ bad: true })}\n`);
assert.throws(() => verifyPairedCohortLedger(tamperedLedger), /unsupported schema/);

fs.rmSync(temp, { recursive: true, force: true });
process.stdout.write("Generation Context paired-cohort ledger tests passed.\n");
