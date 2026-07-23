#!/usr/bin/env node

import assert from "node:assert/strict";
import {
  efficiencyBenchmarkPassed,
  enforcedDcpLifecyclePassed,
  normalizeProviderEvidence,
  providerCacheSmokePassed,
  terminalBundleSetPassed,
} from "./aggregate-release-evidence.mjs";
import { evaluateRuntimeEfficiencyBenchmark } from "../../../infra/generation-reliability/runtime-efficiency-benchmark.mjs";

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

const cacheSmoke = {
  schemaVersion: "provider-cache-smoke-audit@1",
  toolSetHashVersion: "tool-definition-set@1",
  status: "passed",
  releaseEligible: true,
  stableRunCount: 1,
  auditedRunCount: 1,
  invalidRunCount: 0,
  grossInputTokens: 200,
  cachedInputTokens: 100,
  sourceCommit: "commit-fixture",
  sourceDirty: false,
  modelResourceId: "deepseek-v4-pro",
  providerResourceRevision: 7,
  providerConfigSha256: sha,
  runs: [{
    compositionValid: true,
    metricsValid: true,
    providerIdentityValid: true,
    buildIdentityValid: true,
    generationContextIdentityValid: true,
    redactionValid: true,
    repeatedStableTurns: 2,
    grossInputTokens: 200,
    cachedInputTokens: 100,
  }],
};
const provider = {
  modelResourceId: "deepseek-v4-pro",
  providerResourceRevision: 7,
  providerConfigSha256: sha,
};

function efficiencyBenchmark() {
  let sequence = 1;
  const profiles = [
    ["greenfield-profile", "greenfield_build"],
    ["edit-profile", "style_token_edit"],
  ].map(([profileId, workload]) => {
    const attempts = [];
    for (const variant of ["baseline", "candidate"]) {
      for (let index = 0; index < 30; index += 1) {
        const candidate = variant === "candidate";
        attempts.push({
          sequence,
          attemptId: `${profileId}-${variant}-${index}`,
          variant,
          promptId: `prompt-${index % 10}`,
          status: "accepted",
          terminalEvidenceSha256: candidate ? "b".repeat(64) : "c".repeat(64),
          metrics: {
            modelTurns: candidate ? (workload === "greenfield_build" ? 6 : 5) : 12,
            grossInputTokens: candidate ? (workload === "greenfield_build" ? 100000 : 80000) : 180000,
            uncachedInputTokens: candidate ? (workload === "greenfield_build" ? 60000 : 50000) : 120000,
            maxPromptTokensPerTurn: candidate ? 12000 : 18000,
            cacheHitRateBasisPoints: candidate ? 7000 : 3000,
            firstSourceMutationTurn: candidate ? 2 : 4,
            generationContextBytes: 12000,
            duplicateFullContextReads: candidate ? 0 : 1,
            outOfScopeMutations: 0,
            requiredFidelityPassed: true,
          },
        });
        sequence += 1;
      }
    }
    return {
      profileId,
      workload,
      designProfileHash: profileId === "greenfield-profile" ? "d".repeat(64) : "e".repeat(64),
      templateId: "next-app",
      templateVersion: "runtime-p7",
      modelResourceId: provider.modelResourceId,
      providerResourceRevision: provider.providerResourceRevision,
      modelVersion: "deepseek-v4-pro@7",
      providerParametersHash: provider.providerConfigSha256,
      cacheUsageCapability: "reported",
      attempts,
    };
  });
  const cohort = {
    schemaVersion: "runtime-efficiency-benchmark-cohort@1",
    calculatorVersion: "runtime-efficiency-benchmark-calculator@1",
    source: { commit: "commit-fixture", dirty: false },
    bootstrap: { iterations: 100, seed: 42 },
    promptSet: {
      id: "release-prompts",
      version: "v1",
      sha256: "f".repeat(64),
      promptIds: Array.from({ length: 10 }, (_, index) => `prompt-${index}`),
    },
    ledger: {
      schemaVersion: "runtime-efficiency-benchmark-ledger@1",
      sha256: sha,
      firstSequence: 1,
      lastSequence: sequence - 1,
      recordCount: sequence - 1,
    },
    profiles,
  };
  return {
    cohort,
    sourceLedger: {
      schemaVersion: "runtime-efficiency-benchmark-ledger-verification@1",
      sessionId: "release-benchmark-session",
      recordCount: sequence,
      attemptCount: sequence - 1,
      headRecordHash: sha,
      ledgerSha256: sha,
      status: "passed",
    },
    sourceBinding: {
      schemaVersion: "runtime-efficiency-benchmark-source-binding@1",
      status: "passed",
      pairedSessionId: "paired-session",
      source: { commit: "commit-fixture", dirty: false },
      pairedLedgerSha256: "1".repeat(64),
      pairedLedgerHeadRecordHash: "2".repeat(64),
      mappingSha256: "3".repeat(64),
      attemptCount: sequence - 1,
    },
    evaluation: evaluateRuntimeEfficiencyBenchmark(cohort),
  };
}

const benchmark = efficiencyBenchmark();
assert.equal(efficiencyBenchmarkPassed(benchmark, provider, "commit-fixture"), true);
assert.equal(efficiencyBenchmarkPassed(benchmark, provider, "other-commit"), false);
const benchmarkWithoutEdit = structuredClone(benchmark);
benchmarkWithoutEdit.cohort.profiles = benchmarkWithoutEdit.cohort.profiles
  .filter(profile => profile.workload !== "style_token_edit");
benchmarkWithoutEdit.evaluation = evaluateRuntimeEfficiencyBenchmark(benchmarkWithoutEdit.cohort);
assert.equal(efficiencyBenchmarkPassed(benchmarkWithoutEdit, provider, "commit-fixture"), false);
const benchmarkTamperedEvaluation = structuredClone(benchmark);
benchmarkTamperedEvaluation.evaluation.profiles["greenfield-profile"]
  .effectSizes.grossInputTokens.value = 0.99;
assert.equal(efficiencyBenchmarkPassed(benchmarkTamperedEvaluation, provider, "commit-fixture"), false);
const benchmarkMissingSourceBinding = structuredClone(benchmark);
delete benchmarkMissingSourceBinding.sourceBinding;
assert.equal(efficiencyBenchmarkPassed(benchmarkMissingSourceBinding, provider, "commit-fixture"), false);
const benchmarkDirtyPairedSource = structuredClone(benchmark);
benchmarkDirtyPairedSource.sourceBinding.source.dirty = true;
assert.equal(efficiencyBenchmarkPassed(benchmarkDirtyPairedSource, provider, "commit-fixture"), false);
assert.equal(providerCacheSmokePassed(cacheSmoke, provider, "commit-fixture"), true);
assert.equal(providerCacheSmokePassed({
  ...cacheSmoke,
  toolSetHashVersion: undefined,
}, provider, "commit-fixture"), false);
assert.equal(providerCacheSmokePassed({
  ...cacheSmoke,
  status: "provider_not_reporting_cached_usage",
  releaseEligible: false,
  cachedInputTokens: 0,
}, provider, "commit-fixture"), false);

const terminalBundleIndex = {
  schemaVersion: "runtime-terminal-bundle-index@1",
  status: "passed",
  setSha256: sha,
  budgetPolicyStatus: "passed",
  budgetPolicySha256: sha,
  filesScanned: 40,
  replayedCount: 5,
  entries: ["website", "docs", "edit", "repair", "runtime_restart"].map((kind, index) => ({
    evidenceId: `evidence-${kind}`,
    bundleKind: kind === "runtime_restart" ? "runtime_restart_terminal" : "real_provider_terminal",
    kind,
    entryRoute: kind === "docs" || kind === "repair" ? "/docs/" : "/",
    projectId: `project-${kind}`,
    runId: `run-${index}`,
    operationId: `operation-${index}`,
    gitSha: "commit-fixture",
    modelResourceId: "deepseek-v4-pro",
    modelResourceRevision: 7,
    providerConfigSha256: sha,
    providerCacheSourceAuditSha256: sha,
    budgetProfileId: "phase-default",
    budgetProfileHash: sha,
    budgetProfilesSha256: sha,
    runModelUsageSha256: sha,
    budgetProfileCount: 1,
    budgetConformanceStatus: "passed",
    releaseBudgetPolicyStatus: "passed",
    maxTurnInputTokens: 1000,
    checksumsSha256: sha,
    streamSha256: sha,
    resultStatus: "accepted",
    replayStatus: "passed",
  })),
};
assert.equal(terminalBundleSetPassed(terminalBundleIndex, provider, "commit-fixture", sha, sha), true);
assert.equal(terminalBundleSetPassed(
  terminalBundleIndex,
  provider,
  "commit-fixture",
  "b".repeat(64),
  sha,
), false);
assert.equal(terminalBundleSetPassed({
  ...terminalBundleIndex,
  entries: terminalBundleIndex.entries.filter((entry) => entry.kind !== "repair"),
  replayedCount: 4,
}, provider, "commit-fixture", sha, sha), false);
assert.equal(terminalBundleSetPassed({
  ...terminalBundleIndex,
  entries: terminalBundleIndex.entries.map((entry, index) =>
    index === 0 ? { ...entry, runModelUsageSha256: undefined } : entry),
}, provider, "commit-fixture", sha, sha), false);
assert.equal(terminalBundleSetPassed({
  ...terminalBundleIndex,
  entries: terminalBundleIndex.entries.map((entry, index) =>
    index === 0 ? { ...entry, gitSha: "other-commit" } : entry),
}, provider, "commit-fixture", sha, sha), false);
assert.equal(providerCacheSmokePassed(cacheSmoke, {
  ...provider,
  providerResourceRevision: 8,
}, "commit-fixture"), false);
assert.equal(providerCacheSmokePassed({
  ...cacheSmoke,
  stableRunCount: 2,
}, provider, "commit-fixture"), false);

assert.deepEqual(normalizeProviderEvidence({
  provider: {
    mode: "real",
    model: "deepseek-v4-pro",
    modelResourceId: "deepseek-v4-pro",
    credentialPresent: true,
  },
  provenance: {
    providerResourceRevision: 7,
    providerConfigSha256: sha,
  },
}), {
  mode: "real",
  model: "deepseek-v4-pro",
  modelResourceId: "deepseek-v4-pro",
  providerResourceRevision: 7,
  providerConfigSha256: sha,
  credentialPresent: true,
});

assert.deepEqual(normalizeProviderEvidence({
  schemaVersion: "generation-real-provider-suite-evidence@2",
  status: "accepted",
  provider: {
    gatewayMode: "internal_gateway",
    modelResourceId: "deepseek-v4-pro",
    realProviderVerified: true,
  },
  provenance: {
    providerResourceRevision: 7,
    providerConfigSha256: sha,
  },
}), {
  mode: "real",
  model: "deepseek-v4-pro",
  modelResourceId: "deepseek-v4-pro",
  providerResourceRevision: 7,
  providerConfigSha256: sha,
  credentialPresent: true,
});

process.stdout.write("aggregate release evidence tests passed\n");
