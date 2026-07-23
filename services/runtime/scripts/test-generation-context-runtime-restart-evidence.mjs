#!/usr/bin/env node

import assert from "node:assert/strict";
import crypto from "node:crypto";
import {
  createRuntimeRestartEvidence,
  validateRuntimeRestartEvidence,
} from "./generation-context-runtime-restart-evidence.mjs";

const HASH_A = "a".repeat(64);
const HASH_B = "b".repeat(64);
const HASH_C = "c".repeat(64);

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) =>
      `${JSON.stringify(key)}:${canonical(value[key])}`
    ).join(",")}}`;
  }
  return JSON.stringify(value);
}

function budgetProfile() {
  const tokenLimits = {
    maxTurns: 16,
    maxToolCalls: 100,
    maxInputTokens: 300000,
    maxGrossInputTokens: 300000,
    maxUncachedInputTokens: 180000,
    maxPromptTokensPerTurn: 64000,
    maxOutputTokens: 50000,
  };
  const profile = {
    schemaVersion: "run-budget-profile@1",
    profileId: "phase-budget-v1-build",
    phase: "build",
    rolloutMode: "shadow",
    tokenBudgetMode: "split_shadow",
    operationBudgetMode: "shadow",
    enforcedLimits: tokenLimits,
    phaseTargetLimits: { ...tokenLimits },
    operationLimits: {
      maxGrossInputTokens: 600000,
      maxUncachedInputTokens: 400000,
      maxOutputTokens: 100000,
      maxTurns: 40,
      maxToolCalls: 200,
    },
  };
  profile.profileHash = crypto.createHash("sha256").update(canonical(profile)).digest("hex");
  return profile;
}

function snapshot(side) {
  const projectId = `${side}-project`;
  const runId = `${side}-run`;
  const frozenBudgetProfile = budgetProfile();
  return {
    schemaVersion: "generation-context-runtime-restart-snapshot@1",
    recordedAt: "2026-07-22T00:00:00.000Z",
    projectId,
    runId,
    healthReady: true,
    generationContextStatus: side === "candidate"
      ? {
          schemaVersion: "generation-context-status@1",
          runId,
          runContractVersion: "generation-context@1",
          status: "compiled",
          runtimeMode: "enabled",
          compilerVersion: "generation-context-compiler@1",
          contextContentHash: HASH_A,
          runContextBindingHash: HASH_B,
          runtimeAttestationHash: HASH_C,
          visualBindingSetHash: HASH_A,
          executionProfile: "greenfield_static",
          budgetProfileId: "phase-budget-v1-build",
          budgetProfileHash: frozenBudgetProfile.profileHash,
          budgetProfileRolloutMode: "shadow",
          workflowState: "completed",
          contextWindowEpoch: 0,
        }
      : {
          schemaVersion: "generation-context-status@1",
          runId,
          runContractVersion: "legacy@1",
          status: "not_compiled",
          runtimeMode: null,
          compilerVersion: null,
          contextContentHash: null,
          runContextBindingHash: null,
          runtimeAttestationHash: null,
          visualBindingSetHash: null,
          executionProfile: "greenfield_static",
          budgetProfileId: "phase-budget-v1-build",
          budgetProfileHash: frozenBudgetProfile.profileHash,
          budgetProfileRolloutMode: "shadow",
          workflowState: null,
          contextWindowEpoch: 0,
        },
    budgetProfile: frozenBudgetProfile,
    efficiency: {
      schemaVersion: "run-efficiency-metrics@1",
      calculatorVersion: "run-efficiency-calculator@1",
      runId,
      projectId,
      phase: "build",
      model: "resource:deepseek-v4-pro",
      template: "fumadocs-docs",
      status: "completed",
      inputTokens: 100,
      firstBuildSucceeded: true,
    },
    projectState: {
      currentVersionId: `${side}-version`,
      sandboxBindingId: `${side}-sandbox`,
      sourceSnapshotRefSha256: HASH_A,
      templateKey: "fumadocs-docs",
      styleContractSha256: HASH_B,
      latestBuildSha256: HASH_C,
      dependencyStateSha256: null,
      previewSha256: HASH_A,
    },
    history: { itemCount: 1, sha256: HASH_B },
    releaseEvidence: {
      httpStatus: 409,
      available: false,
      canonicalResponseSha256: HASH_C,
      stableStateSha256: HASH_C,
    },
    artifact: {
      httpStatus: 200,
      markerFound: true,
      markerSha256: HASH_A,
      bodySha256: HASH_B,
      bodyBytes: 1024,
    },
  };
}

function metadata(side) {
  return {
    recordedAt: "2026-07-22T00:01:00.000Z",
    side,
    deployment: `runtime-${side}`,
    runtimeDeploymentRevision: `${side}-revision`,
    deploymentUid: `${side}-deployment-uid`,
    deploymentGeneration: 7,
    deploymentTemplateSha256: HASH_A,
    podBefore: { name: `${side}-pod-before`, uid: `${side}-pod-uid-before` },
    deploymentUidAfter: `${side}-deployment-uid`,
    deploymentGenerationAfter: 7,
    deploymentTemplateSha256After: HASH_A,
    podAfter: { name: `${side}-pod-after`, uid: `${side}-pod-uid-after` },
    restartDurationMs: 2500,
  };
}

for (const side of ["control", "candidate"]) {
  const before = snapshot(side);
  const after = { ...structuredClone(before), recordedAt: "2026-07-22T00:01:00.000Z" };
  const evidence = createRuntimeRestartEvidence(metadata(side), before, after);
  assert.equal(evidence.status, "accepted");
  assert.equal(evidence.verification.podUidChanged, true);
  assert.equal(evidence.verification.generationContextIdentityPreserved, true);
  assert.doesNotThrow(() => validateRuntimeRestartEvidence(evidence, {
    side,
    projectId: before.projectId,
    runId: before.runId,
    runtimeDeploymentRevision: `${side}-revision`,
  }));

  const contextDrift = structuredClone(evidence);
  contextDrift.after.generationContextStatus.workflowState = "diagnostic_required";
  assert.throws(
    () => validateRuntimeRestartEvidence(contextDrift, {
      side,
      projectId: before.projectId,
      runId: before.runId,
      runtimeDeploymentRevision: `${side}-revision`,
    }),
    /Budget identity|state changed|recovery invariant|hash mismatch/,
  );

  const samePod = structuredClone(evidence);
  samePod.podAfter.uid = samePod.podBefore.uid;
  assert.throws(
    () => validateRuntimeRestartEvidence(samePod, {
      side,
      projectId: before.projectId,
      runId: before.runId,
      runtimeDeploymentRevision: `${side}-revision`,
    }),
    /replace the Runtime Pod UID/,
  );

  const budgetDrift = structuredClone(evidence);
  budgetDrift.after.generationContextStatus.budgetProfileHash = HASH_B;
  assert.throws(
    () => validateRuntimeRestartEvidence(budgetDrift, {
      side,
      projectId: before.projectId,
      runId: before.runId,
      runtimeDeploymentRevision: `${side}-revision`,
    }),
    /Budget identity|state changed|recovery invariant|hash mismatch/,
  );
}

{
  const before = snapshot("candidate");
  const after = { ...structuredClone(before), recordedAt: "2026-07-22T00:01:00.000Z" };
  delete before.budgetProfile;
  delete after.budgetProfile;
  const legacy = createRuntimeRestartEvidence(metadata("candidate"), before, after);
  assert.equal(legacy.schemaVersion, "generation-context-runtime-restart-evidence@1");
  assert.doesNotThrow(() => validateRuntimeRestartEvidence(legacy, {
    side: "candidate",
    projectId: before.projectId,
    runId: before.runId,
    runtimeDeploymentRevision: "candidate-revision",
  }));
}

{
  const before = snapshot("candidate");
  before.projectState = {
    stateKind: "durable_draft",
    currentVersionId: null,
    sandboxBindingId: null,
    sourceSnapshotRefSha256: HASH_A,
    templateKey: "next-app",
    draftSnapshotId: "candidate-draft-snapshot",
    previewLeaseId: "candidate-preview-lease",
    styleContractSha256: null,
    latestBuildSha256: null,
    dependencyStateSha256: null,
    previewSha256: HASH_B,
  };
  before.artifact = {
    ...before.artifact,
    source: "draft_preview",
  };
  const after = { ...structuredClone(before), recordedAt: "2026-07-22T00:01:00.000Z" };
  after.artifact.bodySha256 = HASH_C;
  const evidence = createRuntimeRestartEvidence(metadata("candidate"), before, after);
  assert.equal(evidence.status, "accepted");
  assert.equal(evidence.verification.artifactPreserved, true);
  assert.equal(evidence.observations.artifactBodySha256Preserved, false);
  assert.doesNotThrow(() => validateRuntimeRestartEvidence(evidence, {
    side: "candidate",
    projectId: before.projectId,
    runId: before.runId,
    runtimeDeploymentRevision: "candidate-revision",
  }));

  const mixedState = structuredClone(evidence);
  mixedState.after.artifact.source = "current_version";
  assert.throws(
    () => validateRuntimeRestartEvidence(mixedState, {
      side: "candidate",
      projectId: before.projectId,
      runId: before.runId,
      runtimeDeploymentRevision: "candidate-revision",
    }),
    /artifact|state changed|recovery invariant|unsupported|hash mismatch/,
  );
}

{
  const before = snapshot("candidate");
  const after = { ...structuredClone(before), recordedAt: "2026-07-22T00:01:00.000Z" };
  after.sandboxRelease = {
    required: true,
    released: true,
    requiredSuccessfulResponses: 2,
    attempts: [
      { attempt: 1, ok: true, status: 204, error: null },
      { attempt: 2, ok: true, status: 204, error: null },
    ],
  };
  const evidence = createRuntimeRestartEvidence(metadata("candidate"), before, after);
  assert.equal(evidence.schemaVersion, "generation-context-runtime-restart-evidence@2");
  assert.equal(evidence.cleanup.released, true);
  assert.doesNotThrow(() => validateRuntimeRestartEvidence(evidence, {
    side: "candidate",
    projectId: before.projectId,
    runId: before.runId,
    runtimeDeploymentRevision: "candidate-revision",
  }));

  const missingCleanup = structuredClone(evidence);
  missingCleanup.cleanup.released = false;
  assert.throws(
    () => validateRuntimeRestartEvidence(missingCleanup, {
      side: "candidate",
      projectId: before.projectId,
      runId: before.runId,
      runtimeDeploymentRevision: "candidate-revision",
    }),
    /confirmed Sandbox release/,
  );

  const missingProfile = structuredClone(evidence);
  delete missingProfile.after.budgetProfile;
  assert.throws(
    () => validateRuntimeRestartEvidence(missingProfile, {
      side: "candidate",
      projectId: before.projectId,
      runId: before.runId,
      runtimeDeploymentRevision: "candidate-revision",
    }),
    /budgetProfile/,
  );

  const tamperedProfile = structuredClone(evidence);
  tamperedProfile.after.budgetProfile.enforcedLimits.maxPromptTokensPerTurn += 1;
  assert.throws(
    () => validateRuntimeRestartEvidence(tamperedProfile, {
      side: "candidate",
      projectId: before.projectId,
      runId: before.runId,
      runtimeDeploymentRevision: "candidate-revision",
    }),
    /profileHash|state changed/,
  );
}

process.stdout.write("Generation Context Runtime Restart evidence tests passed.\n");
