#!/usr/bin/env node

import assert from "node:assert/strict";
import { enforcedDcpLifecyclePassed } from "./aggregate-release-evidence.mjs";

const sha = "a".repeat(64);
const capabilities = {
  "computed-style": { available: true },
  a11y: { available: true },
  viewport: { available: true },
};
const legacyStage = runId => ({
  runId,
  gate: "ready",
  missingRequiredReads: [],
  materialization: { hash: sha, ready: true },
  package: { contentHash: sha, effectiveCompatibilityMode: "enforced" },
  styleContract: { verified: true },
  verification: { capabilities },
});
const generationStage = (runId, index) => ({
  runId,
  gate: "ready",
  templateVersion: "next-app@2",
  materialization: { hash: sha, ready: true },
  styleContract: { verified: true },
  verification: { capabilities },
  generationContext: {
    schemaVersion: "generation-context@1",
    status: "compiled",
    contextContentHash: String(index + 1).repeat(64),
    runContextBindingHash: String(index + 4).repeat(64),
    runtimeAttestationHash: String(index + 7).repeat(64),
  },
  attestation: {
    state: "verified",
    runtimeAttestationHash: String(index + 7).repeat(64),
  },
  efficiency: {
    schemaVersion: "run-efficiency-metrics@1",
    uniqueContextReads: 0,
    uniqueSourceReads: 2,
    duplicateReads: 0,
    duplicateReadTokens: 0,
    unchangedReadStubs: 0,
    postCompactSourceRestores: 0,
    prebuildLists: 0,
  },
});

function fixture(stage) {
  return {
    reviewRepair: {
      reviewRunId: "review",
      repairRunId: "repair",
      findings: [{
        findingId: "finding",
        versionId: "candidate",
        severity: "blocking",
        repairable: true,
        status: "fixed",
      }],
    },
    designContextEnforced: {
      lifecycle: {
        buildRunId: "build",
        editRunId: "edit",
        reviewRunId: "review",
        repairRunId: "repair",
        findingId: "finding",
        candidateVersionId: "candidate",
        findingStatus: "fixed",
      },
      build: stage("build", 0),
      edit: stage("edit", 1),
      repair: stage("repair", 2),
    },
  };
}

assert.equal(enforcedDcpLifecyclePassed(fixture(legacyStage)), true);
assert.equal(enforcedDcpLifecyclePassed(fixture(generationStage)), true);

const missingAttestation = fixture(generationStage);
delete missingAttestation.designContextEnforced.edit.generationContext.runtimeAttestationHash;
assert.equal(enforcedDcpLifecyclePassed(missingAttestation), false);

const mixedProtocols = fixture(generationStage);
mixedProtocols.designContextEnforced.repair = legacyStage("repair");
assert.equal(enforcedDcpLifecyclePassed(mixedProtocols), false);

process.stdout.write("aggregate release evidence tests passed\n");
