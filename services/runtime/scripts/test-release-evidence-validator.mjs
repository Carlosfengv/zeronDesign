#!/usr/bin/env node

import assert from "node:assert/strict";
import crypto from "node:crypto";
import {
  validateProviderCacheRawBinding,
  validateReleaseEvidence,
} from "./validate-release-evidence.mjs";
import { canonicalJson } from "../../../infra/generation-reliability/runtime-budget-evidence.mjs";
import { evaluateRuntimeEfficiencyBenchmark } from "../../../infra/generation-reliability/runtime-efficiency-benchmark.mjs";

const sha = "a".repeat(64);

function efficiencyBenchmarkEvidence() {
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
      templateId: workload === "greenfield_build" ? "next-app" : "next-app",
      templateVersion: "runtime-p7",
      modelResourceId: "deepseek-v4-pro",
      providerResourceRevision: 7,
      modelVersion: "deepseek-v4-pro@7",
      providerParametersHash: sha,
      cacheUsageCapability: "reported",
      attempts,
    };
  });
  const cohort = {
    schemaVersion: "runtime-efficiency-benchmark-cohort@1",
    calculatorVersion: "runtime-efficiency-benchmark-calculator@1",
    source: { commit: "abc123", dirty: false },
    bootstrap: { iterations: 100, seed: 42 },
    promptSet: {
      id: "release-benchmark-prompts",
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
      source: { commit: "abc123", dirty: false },
      pairedLedgerSha256: "1".repeat(64),
      pairedLedgerHeadRecordHash: "2".repeat(64),
      mappingSha256: "3".repeat(64),
      attemptCount: sequence - 1,
    },
    evaluation: evaluateRuntimeEfficiencyBenchmark(cohort),
  };
}
const tokenLimits = {
  maxTurns: 20,
  maxToolCalls: 60,
  maxInputTokens: 300000,
  maxGrossInputTokens: 300000,
  maxUncachedInputTokens: 180000,
  maxPromptTokensPerTurn: 64000,
  maxOutputTokens: 40000,
};

function budgetProfile(phase) {
  const profile = {
    schemaVersion: "run-budget-profile@1",
    profileId: `phase-budget-v1-${phase}`,
    phase,
    rolloutMode: "enforced",
    tokenBudgetMode: "split_enforced",
    operationBudgetMode: "enforced",
    enforcedLimits: tokenLimits,
    phaseTargetLimits: tokenLimits,
    operationLimits: {
      maxGrossInputTokens: 450000,
      maxUncachedInputTokens: 270000,
      maxOutputTokens: 80000,
      maxTurns: 30,
      maxToolCalls: 100,
    },
  };
  profile.profileHash = crypto.createHash("sha256").update(canonicalJson(profile)).digest("hex");
  return profile;
}

const approvedProfiles = Object.fromEntries(
  ["build", "edit", "review", "repair"].map((phase) => [phase, budgetProfile(phase)]),
);
const releaseBudgetPolicy = {
  schemaVersion: "runtime-release-budget-policy@1",
  releaseStage: "production",
  requiredModes: {
    rolloutMode: "enforced",
    tokenBudgetMode: "split_enforced",
    operationBudgetMode: "enforced",
  },
  phaseProfiles: Object.fromEntries(Object.entries(approvedProfiles).map(([phase, profile]) => [
    phase,
    {
      allowedProfileHashes: [profile.profileHash],
      maximumLimits: tokenLimits,
      maximumOperationLimits: profile.operationLimits,
    },
  ])),
};

function terminalBudgetEvidence(kind, index) {
  const phases = kind === "repair" ? ["edit", "review", "repair"] : [kind === "edit" ? "edit" : "build"];
  return {
    schemaVersion: "runtime-evidence-budget-profiles@1",
    profiles: phases.map((phase, phaseIndex) => ({
      runId: phaseIndex === phases.length - 1 ? `terminal-run-${index}` : `terminal-run-${index}-${phase}`,
      operationId: `terminal-operation-${index}-${phaseIndex}`,
      operationAttempt: 1,
      phase,
      profile: approvedProfiles[phase],
    })),
  };
}
const fixture = {
  schemaVersion: "release-evidence@1",
  releaseEligible: true,
  result: "pass",
  repository: { commit: "abc123", dirty: false, lockHash: sha },
  cluster: { name: "fresh", kubeContext: "k3d-fresh", createdAt: "2026-07-11T00:00:00Z", nodeUid: "node-1" },
  images: {
    runtime: { ref: "runtime:test", configDigest: `sha256:${sha}`, manifestDigest: `sha256:${sha}`, reportedCommit: "abc123" },
    sandbox: { ref: "sandbox:test", configDigest: `sha256:${sha}` },
    controller: `controller@sha256:${sha}`,
    npmProxy: `proxy@sha256:${sha}`,
    dockerfileFrontend: `frontend@sha256:${sha}`,
  },
  transport: { mode: "mtls", mtlsVerified: true, rotationWindowVerified: true, runtimeSanHash: sha, sandboxSanHash: sha, runtimeCertSerialHash: sha, sandboxCertSerialHash: sha, runtimeCertExpiresAt: "2026-07-13T00:00:00Z", sandboxCertExpiresAt: "2026-07-13T00:00:00Z" },
  auth: { principalMode: "required", projectOwnershipVerified: true, channelJwtVerified: true },
  provider: {
    mode: "real",
    model: "deepseek-v4-pro",
    modelResourceId: "deepseek-v4-pro",
    providerResourceRevision: 7,
    providerConfigSha256: sha,
    credentialPresent: true,
  },
  providerCacheSmoke: {
    schemaVersion: "provider-cache-smoke-audit@1",
    toolSetHashVersion: "tool-definition-set@1",
    status: "passed",
    releaseEligible: true,
    stableRunCount: 2,
    auditedRunCount: 2,
    invalidRunCount: 0,
    grossInputTokens: 2000,
    cachedInputTokens: 1000,
    sourceCommit: "abc123",
    sourceDirty: false,
    modelResourceId: "deepseek-v4-pro",
    providerResourceRevision: 7,
    providerConfigSha256: sha,
    runs: [0, 1].map(index => ({
      runId: `cache-run-${index + 1}`,
      compositionValid: true,
      metricsValid: true,
      providerIdentityValid: true,
      buildIdentityValid: true,
      generationContextIdentityValid: true,
      redactionValid: true,
      repeatedStableTurns: 2,
      grossInputTokens: 1000,
      cachedInputTokens: 500,
    })),
  },
  providerCacheSmokeSha256: sha,
  releaseBudgetPolicySha256: sha,
  releaseBudgetPolicy,
  efficiencyBenchmark: efficiencyBenchmarkEvidence(),
  terminalBundles: {
    schemaVersion: "runtime-terminal-bundle-index@1",
    status: "passed",
    setSha256: sha,
    budgetPolicyStatus: "passed",
    budgetPolicySha256: sha,
    filesScanned: 40,
    replayedCount: 5,
    entries: ["website", "docs", "edit", "repair", "runtime_restart"].map((kind, index) => {
      const budgetProfiles = terminalBudgetEvidence(kind, index);
      const primary = budgetProfiles.profiles.at(-1);
      return {
        evidenceId: `terminal-${kind}`,
        bundleKind: kind === "runtime_restart" ? "runtime_restart_terminal" : "real_provider_terminal",
        kind,
        entryRoute: kind === "docs" || kind === "repair" ? "/docs/" : "/",
        projectId: `terminal-project-${kind}`,
        runId: `terminal-run-${index}`,
        operationId: primary.operationId,
        gitSha: "abc123",
        modelResourceId: "deepseek-v4-pro",
        modelResourceRevision: 7,
        providerConfigSha256: sha,
        providerCacheSourceAuditSha256: sha,
        budgetProfileId: primary.profile.profileId,
        budgetProfileHash: primary.profile.profileHash,
        budgetProfilesSha256: sha,
        runModelUsageSha256: sha,
        budgetProfiles,
        budgetProfileCount: budgetProfiles.profiles.length,
        budgetConformanceStatus: "passed",
        releaseBudgetPolicyStatus: "passed",
        maxTurnInputTokens: 1000,
        checksumsSha256: sha,
        streamSha256: sha,
        resultStatus: "accepted",
        replayStatus: "passed",
      };
    }),
  },
  preflight: {
    schemaVersion: "runtime-rc-preflight@1", passed: true, prefetchImages: true, lockHash: sha,
    entries: [{ name: "runtime", canonicalRef: "registry.example/runtime:v1", lockedDigest: `sha256:${sha}`, lockedDigestVerified: true, mutableTagMatchesLock: true, pulled: true }],
  },
  projects: ["website", "docs"].map(kind => ({
    kind, projectId: kind, buildRunId: `${kind}-build`, editRunId: `${kind}-edit`, bindingId: `${kind}-binding`, podUid: `${kind}-pod`,
    buildId: `${kind}-build-id`, candidateManifestHash: sha, sourceSnapshotUri: `runtime://snapshot/${kind}`,
    previewLeaseId: `${kind}-lease`, screenshotId: `${kind}-shot`, nonblankPixelRatio: 0.1,
    versionBeforeCas: `${kind}-v1`, versionAfterCas: `${kind}-v2`, artifactManifestHash: sha,
    artifactUrl: `/artifacts/${kind}/current/`, events: { previewUpdated: "1", runCompleted: "2", sequenceValid: true },
    artifactAssertions: {
      route: kind === "docs" ? "/docs/" : "/",
      content: { expectedTextSha256: sha, documentSha256: sha, matched: true },
      computedStyle: { selector: "body", display: "block", color: "rgb(0, 0, 0)", fontFamily: "sans-serif", passed: true },
    },
    cancelCleanup: { runId: `${kind}-cancel`, runStatus: "cancelled", previewHttpStatusAfterCancel: 404, passed: true },
    dependencyEvidence: { podUid: `${kind}-pod`, pod: `${kind}-pod-name`, podIp: `10.0.0.${kind === "website" ? 1 : 2}`, nodeModulesPresent: true, lockfileSha256: kind === "website" ? "d".repeat(64) : "e".repeat(64), tarballRequestCount: 2, passed: true },
    recoverableToolFailureCount: 0, terminalToolFailureCount: 0,
    sandboxReleasedAt: "2026-07-11T00:00:00Z", artifactHttpStatusAfterRelease: 200,
    artifactAccessAfterRelease: { authentication: "project-principal", projectId: kind, httpStatus: 200, authenticated: true },
  })),
  recoveryScenarios: [
    ["runtime-restart", "active-preview-lease"],
    ["port-forward-kill", "active-preview-lease"],
    ["sandbox-pod-replacement", "ready-warm-pod"],
    ["channel-lease-pod-uid-change", "ready-channel-lease"],
    ["checkpoint-runtime-restart", "persisted-partial-run"],
    ["artifact-staged-before-cas", "promotion-wal"],
    ["cas-before-event", "promotion-outbox"],
    ["run-cancel", "active-preview-process"],
  ].map(([scenario, injectionPoint]) => ({ scenario, injectionPoint, result: "pass", orphanCount: 0 })),
  networkChecks: { directRegistryDenied: true, npmProxyInstallPassed: true },
  secretScan: { patternSet: "runtime-credentials@1", filesScanned: 5, matches: [] },
};
fixture.fixture = {
  deployed: true,
  concurrent: true,
  projects: structuredClone(fixture.projects),
};
fixture.fixture.projects[0].podUid = "fixture-website-pod";
fixture.fixture.projects[1].podUid = "fixture-docs-pod";
fixture.fixture.projects[0].artifactManifestHash = "b".repeat(64);
fixture.fixture.projects[1].artifactManifestHash = "c".repeat(64);
const dcpStage = runId => ({
  runId,
  gate: "ready",
  missingRequiredReads: [],
  materialization: { hash: sha, ready: true },
  package: { contentHash: sha, effectiveCompatibilityMode: "enforced" },
  styleContract: { verified: true },
  verification: { capabilities: {
    "computed-style": { available: true }, a11y: { available: true }, viewport: { available: true },
  } },
});
const generationDcpStage = (runId, index) => {
  const stage = dcpStage(runId);
  delete stage.package;
  delete stage.missingRequiredReads;
  stage.templateVersion = "next-app@2";
  stage.generationContext = {
    schemaVersion: "generation-context-status@1",
    runContractVersion: "generation-context@1",
    status: "compiled",
    contextContentHash: String(index + 1).repeat(64),
    runContextBindingHash: String(index + 4).repeat(64),
    runtimeAttestationHash: String(index + 7).repeat(64),
  };
  stage.attestation = {
    state: "verified",
    runtimeAttestationHash: stage.generationContext.runtimeAttestationHash,
  };
  stage.efficiency = {
    schemaVersion: "run-efficiency-metrics@1",
    uniqueContextReads: 0,
    uniqueSourceReads: 3,
    duplicateReads: 0,
    duplicateReadTokens: 0,
    unchangedReadStubs: 0,
    postCompactSourceRestores: 0,
    prebuildLists: 1,
  };
  return stage;
};
fixture.enforcedDcpFixture = {
  reviewRepair: {
    editRunId: "dcp-edit",
    reviewRunId: "dcp-review",
    repairRunId: "dcp-repair",
    findings: [{ findingId: "dcp-finding", versionId: "dcp-v2", severity: "blocking", repairable: true, status: "fixed" }],
  },
  designContextEnforced: {
    lifecycle: {
      buildRunId: "dcp-build", editRunId: "dcp-edit", reviewRunId: "dcp-review", repairRunId: "dcp-repair",
      findingId: "dcp-finding", candidateVersionId: "dcp-v2", findingStatus: "fixed",
    },
    build: dcpStage("dcp-build"), edit: dcpStage("dcp-edit"), repair: dcpStage("dcp-repair"),
  },
};
fixture.providerDcpProject = {
  ...structuredClone(fixture.projects[0]),
  projectId: "provider-dcp-website",
  buildRunId: "provider-dcp-build",
  editRunId: "provider-dcp-edit",
  bindingId: "provider-dcp-binding",
  podUid: "provider-dcp-pod",
  buildId: "provider-dcp-build-id",
  sourceSnapshotUri: "runtime://snapshot/provider-dcp",
  previewLeaseId: "provider-dcp-lease",
  screenshotId: "provider-dcp-shot",
  artifactUrl: "/artifacts/provider-dcp-website/current/",
  artifactAccessAfterRelease: { authentication: "project-principal", projectId: "provider-dcp-website", httpStatus: 200, authenticated: true },
  dependencyEvidence: { ...structuredClone(fixture.projects[0].dependencyEvidence), podUid: "provider-dcp-pod", pod: "provider-dcp-pod-name" },
  reviewRepair: {
    editRunId: "provider-dcp-edit",
    reviewRunId: "provider-dcp-review",
    repairRunId: "provider-dcp-repair",
    findings: [{ findingId: "provider-dcp-finding", versionId: "provider-dcp-v2", severity: "blocking", repairable: true, status: "fixed" }],
  },
  designContextEnforced: {
    lifecycle: {
      buildRunId: "provider-dcp-build", editRunId: "provider-dcp-edit", reviewRunId: "provider-dcp-review", repairRunId: "provider-dcp-repair",
      findingId: "provider-dcp-finding", candidateVersionId: "provider-dcp-v2", findingStatus: "fixed",
    },
    build: dcpStage("provider-dcp-build"), edit: dcpStage("provider-dcp-edit"), repair: dcpStage("provider-dcp-repair"),
    reviewProvenance: {
      source: "fixture-seeded", mutationProvider: "deepseek", reviewProvider: "deterministic-tool-sequence", repairProvider: "deepseek",
    },
  },
};

assert.deepEqual(validateReleaseEvidence(fixture), []);

const rawBoundFixture = structuredClone(fixture);
const providerCacheRaw = `${JSON.stringify(rawBoundFixture.providerCacheSmoke, null, 2)}\n`;
rawBoundFixture.providerCacheSmokeSha256 = crypto
  .createHash("sha256")
  .update(providerCacheRaw)
  .digest("hex");
assert.deepEqual(
  validateProviderCacheRawBinding(rawBoundFixture, providerCacheRaw),
  rawBoundFixture.providerCacheSmoke,
);
assert.throws(
  () => validateProviderCacheRawBinding(
    rawBoundFixture,
    providerCacheRaw.replace('"stableRunCount": 2', '"stableRunCount": 1'),
  ),
  /raw SHA-256 mismatch/,
);
const rawWithoutToolHashVersion = JSON.parse(providerCacheRaw);
delete rawWithoutToolHashVersion.toolSetHashVersion;
const rawWithoutVersionText = JSON.stringify(rawWithoutToolHashVersion);
const aggregateWithoutVersion = structuredClone(rawBoundFixture);
aggregateWithoutVersion.providerCacheSmoke = rawWithoutToolHashVersion;
aggregateWithoutVersion.providerCacheSmokeSha256 = crypto
  .createHash("sha256")
  .update(rawWithoutVersionText)
  .digest("hex");
assert.throws(
  () => validateProviderCacheRawBinding(aggregateWithoutVersion, rawWithoutVersionText),
  /raw evidence mismatch/,
);

const missingProviderCache = structuredClone(fixture);
delete missingProviderCache.providerCacheSmoke;
assert(validateReleaseEvidence(missingProviderCache).some(error =>
  error.includes("stable-prefix cache smoke")
));

const legacyToolSetHash = structuredClone(fixture);
delete legacyToolSetHash.providerCacheSmoke.toolSetHashVersion;
assert(validateReleaseEvidence(legacyToolSetHash).some(error =>
  error.includes("stable-prefix cache smoke")
));

const missingEfficiencyBenchmark = structuredClone(fixture);
delete missingEfficiencyBenchmark.efficiencyBenchmark;
assert(validateReleaseEvidence(missingEfficiencyBenchmark).some(error =>
  error.includes("efficiency Benchmark")
));

const missingEfficiencySourceBinding = structuredClone(fixture);
delete missingEfficiencySourceBinding.efficiencyBenchmark.sourceBinding;
assert(validateReleaseEvidence(missingEfficiencySourceBinding).some(error =>
  error.includes("efficiency Benchmark")
));

const dirtyEfficiencySourceBinding = structuredClone(fixture);
dirtyEfficiencySourceBinding.efficiencyBenchmark.sourceBinding.source.dirty = true;
assert(validateReleaseEvidence(dirtyEfficiencySourceBinding).some(error =>
  error.includes("efficiency Benchmark")
));

const tamperedEfficiencyEvaluation = structuredClone(fixture);
tamperedEfficiencyEvaluation.efficiencyBenchmark.evaluation.result = "pass";
tamperedEfficiencyEvaluation.efficiencyBenchmark.evaluation
  .profiles["greenfield-profile"].effectSizes.grossInputTokens.value = 0.99;
assert(validateReleaseEvidence(tamperedEfficiencyEvaluation).some(error =>
  error.includes("efficiency Benchmark")
));

const mismatchedEfficiencyProvider = structuredClone(fixture);
mismatchedEfficiencyProvider.efficiencyBenchmark.cohort
  .profiles[0].providerResourceRevision = 8;
assert(validateReleaseEvidence(mismatchedEfficiencyProvider).some(error =>
  error.includes("efficiency Benchmark")
));

const insufficientEfficiencyBenchmark = structuredClone(fixture);
insufficientEfficiencyBenchmark.efficiencyBenchmark.cohort.profiles[1].attempts.pop();
insufficientEfficiencyBenchmark.efficiencyBenchmark.cohort.ledger.lastSequence -= 1;
insufficientEfficiencyBenchmark.efficiencyBenchmark.cohort.ledger.recordCount -= 1;
assert(validateReleaseEvidence(insufficientEfficiencyBenchmark).some(error =>
  error.includes("efficiency Benchmark")
));

const providerCacheNotReported = structuredClone(fixture);
providerCacheNotReported.providerCacheSmoke.status = "provider_not_reporting_cached_usage";
providerCacheNotReported.providerCacheSmoke.releaseEligible = false;
providerCacheNotReported.providerCacheSmoke.cachedInputTokens = 0;
assert(validateReleaseEvidence(providerCacheNotReported).some(error =>
  error.includes("stable-prefix cache smoke")
));

const mismatchedProviderCache = structuredClone(fixture);
mismatchedProviderCache.providerCacheSmoke.providerResourceRevision = 8;
assert(validateReleaseEvidence(mismatchedProviderCache).some(error =>
  error.includes("stable-prefix cache smoke")
));

const missingTerminalBundles = structuredClone(fixture);
delete missingTerminalBundles.terminalBundles;
assert(validateReleaseEvidence(missingTerminalBundles).some(error =>
  error.includes("replayed terminal Bundles")
));

const missingRestartBundle = structuredClone(fixture);
missingRestartBundle.terminalBundles.entries = missingRestartBundle.terminalBundles.entries
  .filter((entry) => entry.kind !== "runtime_restart");
missingRestartBundle.terminalBundles.replayedCount = 4;
assert(validateReleaseEvidence(missingRestartBundle).some(error =>
  error.includes("replayed terminal Bundles")
));

const mismatchedTerminalProvider = structuredClone(fixture);
mismatchedTerminalProvider.terminalBundles.entries[0].modelResourceRevision = 8;
assert(validateReleaseEvidence(mismatchedTerminalProvider).some(error =>
  error.includes("replayed terminal Bundles")
));

const mismatchedTerminalCacheAudit = structuredClone(fixture);
mismatchedTerminalCacheAudit.terminalBundles.entries[0].providerCacheSourceAuditSha256 =
  "b".repeat(64);
assert(validateReleaseEvidence(mismatchedTerminalCacheAudit).some(error =>
  error.includes("replayed terminal Bundles")
));

const tamperedTerminalBudgetProfile = structuredClone(fixture);
tamperedTerminalBudgetProfile.terminalBundles.entries[0]
  .budgetProfiles.profiles[0].profile.phaseTargetLimits.maxPromptTokensPerTurn += 1;
assert(validateReleaseEvidence(tamperedTerminalBudgetProfile).some(error =>
  error.includes("replayed terminal Bundles")
));

const missingTerminalRunModelUsage = structuredClone(fixture);
delete missingTerminalRunModelUsage.terminalBundles.entries[0].runModelUsageSha256;
assert(validateReleaseEvidence(missingTerminalRunModelUsage).some(error =>
  error.includes("replayed terminal Bundles")
));

const shadowTerminalBudgetProfile = structuredClone(fixture);
shadowTerminalBudgetProfile.terminalBundles.entries[0]
  .budgetProfiles.profiles[0].profile.rolloutMode = "shadow";
assert(validateReleaseEvidence(shadowTerminalBudgetProfile).some(error =>
  error.includes("replayed terminal Bundles")
));

const publishWorkflowFixture = structuredClone(fixture);
const website = publishWorkflowFixture.projects.find(project => project.kind === "website");
delete website.screenshotId;
delete website.nonblankPixelRatio;
website.visualReview = { mode: "advisory", status: "not_requested" };
website.publication = {
  workflowId: "publish-workflow-website",
  status: "completed",
  checkpoint: "completed",
  versionId: website.versionAfterCas,
  releaseId: "release-website",
  publicationOperationId: "operation-website",
  publicUrl: "https://website.works.example.test",
  externalProbePassed: true,
};
website.events.sequenceValid = false;
assert.deepEqual(validateReleaseEvidence(publishWorkflowFixture), []);

publishWorkflowFixture.projects[0].publication.externalProbePassed = false;
assert.ok(
  validateReleaseEvidence(publishWorkflowFixture).some(error =>
    error.includes("nonblank screenshot or completed advisory PublishWorkflow")),
);
const generationContextFixture = structuredClone(fixture);
for (const target of [generationContextFixture.enforcedDcpFixture, generationContextFixture.providerDcpProject]) {
  const lifecycle = target.designContextEnforced.lifecycle;
  target.designContextEnforced.build = generationDcpStage(lifecycle.buildRunId, 0);
  target.designContextEnforced.edit = generationDcpStage(lifecycle.editRunId, 1);
  target.designContextEnforced.repair = generationDcpStage(lifecycle.repairRunId, 2);
  target.designContextEnforced.edit.efficiency = {
    schemaVersion: "run-efficiency-metrics@1",
    calculatorVersion: "run-efficiency-calculator@1",
    prebuildFsReadCount: 1,
    prebuildFsListCount: 0,
    prebuildFsSearchCount: 0,
    contextReadDeliveries: 0,
    sourceReadDeliveries: 2,
    fullReadDeliveries: 2,
    duplicateFullReadDeliveries: 0,
    duplicateFullReadRateBasisPoints: 0,
    duplicateReadEstimatedTokens: 0,
  };
}
assert.deepEqual(validateReleaseEvidence(generationContextFixture), []);
const reusedGenerationBinding = structuredClone(generationContextFixture);
reusedGenerationBinding.enforcedDcpFixture.designContextEnforced.edit.generationContext.runContextBindingHash =
  reusedGenerationBinding.enforcedDcpFixture.designContextEnforced.build.generationContext.runContextBindingHash;
assert(validateReleaseEvidence(reusedGenerationBinding).some(error => error.includes("binding hashes")));
const dirty = structuredClone(fixture);
dirty.repository.dirty = true;
assert(validateReleaseEvidence(dirty).some(error => error.includes("clean")));
const missingEdit = structuredClone(fixture);
missingEdit.projects[0].editRunId = missingEdit.projects[0].buildRunId;
assert(validateReleaseEvidence(missingEdit).some(error => error.includes("must differ")));
const leaked = structuredClone(fixture);
leaked.secretScan.matches.push({ file: "log", pattern: "token" });
assert(validateReleaseEvidence(leaked).some(error => error.includes("credential-like")));
const missingComputedStyle = structuredClone(fixture);
missingComputedStyle.projects[0].artifactAssertions.computedStyle.passed = false;
assert(validateReleaseEvidence(missingComputedStyle).some(error => error.includes("computed-style")));
const wrongDocsRoute = structuredClone(fixture);
wrongDocsRoute.projects.find(project => project.kind === "docs").artifactAssertions.route = "/";
assert(validateReleaseEvidence(wrongDocsRoute).some(error => error.includes("was not asserted at /docs/")));
const missingRuntimeManifest = structuredClone(fixture);
delete missingRuntimeManifest.images.runtime.manifestDigest;
assert(validateReleaseEvidence(missingRuntimeManifest).some(error => error.includes("manifest digest")));
const missingPreflightPull = structuredClone(fixture);
missingPreflightPull.preflight.entries[0].pulled = false;
assert(validateReleaseEvidence(missingPreflightPull).some(error => error.includes("preflight image")));
const mismatchedPreflightLock = structuredClone(fixture);
mismatchedPreflightLock.preflight.lockHash = "b".repeat(64);
assert(validateReleaseEvidence(mismatchedPreflightLock).some(error => error.includes("preflight lockHash")));
const leakedPreview = structuredClone(fixture);
leakedPreview.projects[0].cancelCleanup.previewHttpStatusAfterCancel = 200;
assert(validateReleaseEvidence(leakedPreview).some(error => error.includes("cancellation")));
const snapshotOnlyCancellation = structuredClone(fixture);
snapshotOnlyCancellation.projects[0].cancelCleanup = {
  runId: "website-cancel",
  runStatus: "cancelled",
  previewLeaseStatus: "not_applicable",
  durableDraftCreated: false,
  passed: true,
};
assert(!validateReleaseEvidence(snapshotOnlyCancellation).some(error => error.includes("cancellation")));
snapshotOnlyCancellation.projects[0].cancelCleanup.durableDraftCreated = true;
assert(validateReleaseEvidence(snapshotOnlyCancellation).some(error => error.includes("cancellation")));
const anonymousArtifactProbe = structuredClone(fixture);
anonymousArtifactProbe.projects[0].artifactAccessAfterRelease.authentication = "anonymous";
assert(validateReleaseEvidence(anonymousArtifactProbe).some(error => error.includes("project principal")));
const sequentialFixture = structuredClone(fixture);
sequentialFixture.fixture.concurrent = false;
assert(validateReleaseEvidence(sequentialFixture).some(error => error.includes("concurrent fixture")));
const missingRepair = structuredClone(fixture);
delete missingRepair.enforcedDcpFixture.designContextEnforced.repair;
assert(validateReleaseEvidence(missingRepair).some(error => error.includes("repair diagnostics")));
const unfixedFinding = structuredClone(fixture);
unfixedFinding.enforcedDcpFixture.reviewRepair.findings[0].status = "repairing";
assert(validateReleaseEvidence(unfixedFinding).some(error => error.includes("inconsistent")));
const missingProviderDcp = structuredClone(fixture);
delete missingProviderDcp.providerDcpProject;
assert(validateReleaseEvidence(missingProviderDcp).some(error => error.includes("real-provider enforced DCP Website")));
const fixtureRepairProvider = structuredClone(fixture);
fixtureRepairProvider.providerDcpProject.designContextEnforced.reviewProvenance.repairProvider = "deterministic-tool-sequence";
assert(validateReleaseEvidence(fixtureRepairProvider).some(error => error.includes("Review provenance")));
const failedProviderDcpRepair = structuredClone(fixture);
failedProviderDcpRepair.providerDcpProject.reviewRepair.findings[0].status = "repairing";
assert(validateReleaseEvidence(failedProviderDcpRepair).some(error => error.includes("real-provider enforced DCP Review finding")));
process.stdout.write("release evidence validator tests passed\n");
