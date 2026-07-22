#!/usr/bin/env node

import assert from "node:assert/strict";
import { collectPairedSamples } from "./collect-generation-context-paired-sample.mjs";
import { createRuntimeRestartEvidence } from "./generation-context-runtime-restart-evidence.mjs";

const HASH_A = "a".repeat(64);
const HASH_B = "b".repeat(64);
const HASH_C = "c".repeat(64);

const session = {
  providers: [{
    modelResourceId: "deepseek-v4-pro",
    modelVersion: "deepseek-v4-pro",
    resourceRevision: 4,
    providerParametersHash: HASH_A,
    visionCapable: false,
    supportedImageMediaTypes: [],
    maxImageCount: 0,
  }],
  runtimes: {
    control: { deploymentRevision: "control-revision" },
    candidate: { deploymentRevision: "candidate-revision" },
  },
};
const spec = {
  schemaVersion: "generation-context-real-provider-pair-spec@1",
  pairId: "batch-1-ai-governance-greenfield",
  batchId: "batch-1",
  bucket: "greenfield",
  control: { phase: "build" },
  candidate: { phase: "build" },
  coverage: ["nextTemplate"],
};

function evidence(side) {
  const projectId = `${side}-project`;
  const runId = `${side}-run`;
  return {
    schemaVersion: "generation-real-provider-case-evidence@2",
    id: "ai-governance-console",
    projectId,
    status: "accepted",
    finishedAt: "2026-07-20T00:00:00.000Z",
    contentPlan: {
      fixtureId: "ai-governance-console",
      intentSha256: HASH_B,
    },
    acceptance: { sha256: HASH_C },
    artifact: null,
    draftPreview: { expectedTextFound: true },
    runs: [{
      runId,
      phase: "build",
      status: "completed",
      summary: "done",
      eventStream: { sha256: side === "control" ? HASH_A : HASH_B },
      modelExecutions: [{
        modelResourceId: "deepseek-v4-pro",
        modelResourceRevision: 4,
        physicalModel: "deepseek-v4-pro",
        capabilitySnapshotHash: HASH_C,
      }],
      efficiency: {
        schemaVersion: "run-efficiency-metrics@1",
        calculatorVersion: "run-efficiency-calculator@1",
        runId,
        projectId,
        phase: "build",
        model: `resource:deepseek-v4-pro`,
        template: "next-app@2",
        status: "completed",
        inputTokens: side === "control" ? 1000 : 400,
        duplicateReadEstimatedTokens: side === "control" ? 100 : 10,
        timeToFirstGreenfieldStaticBuildMs: side === "control" ? 100_000 : 40_000,
        modelTurnAtFirstSourceMutation: 1,
        prebuildFsListCount: 1,
        prebuildFsSearchCount: 0,
        duplicateFullReadRateBasisPoints: 0,
        outOfScopeMutationCount: 0,
        firstBuildSucceeded: true,
        requiredFidelityPassed: true,
      },
    }],
  };
}

const samples = collectPairedSamples(session, spec, evidence("control"), evidence("candidate"));
assert.equal(samples.control.flag, "legacy");
assert.equal(samples.candidate.flag, "generation_context");
assert.deepEqual(samples.control.identity, samples.candidate.identity);
assert.deepEqual(samples.control.coverage, []);
assert.deepEqual(samples.candidate.coverage, ["nextTemplate"]);
assert.equal(samples.candidate.metrics.timeToFirstGreenfieldBuildMs, 40_000);
assert.equal(samples.candidate.requiredFidelityPassed, true);

const retriedControl = evidence("control");
const failedBuild = structuredClone(retriedControl.runs[0]);
failedBuild.runId = "control-failed-build";
failedBuild.status = "partial";
failedBuild.summary = "Run stopped for no_progress";
failedBuild.efficiency.runId = failedBuild.runId;
failedBuild.efficiency.status = "partial";
failedBuild.efficiency.inputTokens = 9_999;
retriedControl.runs.unshift(failedBuild);
retriedControl.attempts = [
  {
    attempt: 1,
    status: "failed",
    runIds: [failedBuild.runId],
  },
  {
    attempt: 2,
    status: "accepted",
    runIds: [retriedControl.runs[1].runId],
  },
];
const retriedSamples = collectPairedSamples(
  session,
  spec,
  retriedControl,
  evidence("candidate"),
);
assert.equal(retriedSamples.control.metrics.inputTokens, 1000);
assert.equal(retriedSamples.control.requiredFidelityPassed, true);

const drifted = evidence("candidate");
drifted.contentPlan.intentSha256 = "d".repeat(64);
assert.throws(
  () => collectPairedSamples(session, spec, evidence("control"), drifted),
  /intent hash mismatch/,
);

function warmEvidence(side) {
  const value = evidence(side);
  const run = structuredClone(value.runs[0]);
  run.phase = "edit";
  run.efficiency.phase = "edit";
  run.efficiency.timeToIframeAppliedMs = side === "control" ? 30_000 : 12_000;
  value.warmEdit = {
    schemaVersion: "generation-real-provider-edit-evidence@1",
    startedAt: "2026-07-20T00:01:00.000Z",
    finishedAt: "2026-07-20T00:02:00.000Z",
    status: "accepted",
    projectId: value.projectId,
    prompt: "Add the exact marker PAIRED_WARM_MARKER and make a small CSS change.",
    warmEditKind: "copy_css",
    editImpactPlanHash: HASH_A,
    run,
    draftPreview: {
      expectedText: "PAIRED_WARM_MARKER",
      expectedTextFound: true,
    },
  };
  return value;
}

const warmSpec = {
  ...spec,
  pairId: "batch-1-ai-governance-warm-copy-css",
  bucket: "warm_copy_css",
  control: { source: "warmEdit", phase: "edit" },
  candidate: { source: "warmEdit", phase: "edit" },
};
const warmSamples = collectPairedSamples(
  session,
  warmSpec,
  warmEvidence("control"),
  warmEvidence("candidate"),
);
assert.equal(warmSamples.control.identity.phase, "edit");
assert.equal(warmSamples.candidate.metrics.timeToIframeAppliedMs, 12_000);
assert.equal(warmSamples.candidate.requiredFidelityPassed, true);

const nonVisualSpec = {
  ...warmSpec,
  pairId: "batch-1-ai-governance-nonvisual-fallback",
  coverage: ["nextTemplate", "nonVisualUnavailableMainTaskPassed"],
};
const nonVisualControl = warmEvidence("control");
nonVisualControl.warmEdit.visualDelivery = {
  state: "not_applicable",
  visualBindingsVerified: true,
  visualBindingSetHash: HASH_A,
  runtimeAttestationHash: HASH_B,
  bindingVerificationSource:
    "frozen-runtime-attestation-plus-unavailable-delivery-metric",
  unavailableMetricRecorded: false,
  mainTaskCompleted: true,
  providerModelResourceId: "deepseek-v4-pro",
  providerVisionCapable: false,
  referenceArtifactId: "visual-control-reference",
};
const nonVisualCandidate = warmEvidence("candidate");
nonVisualCandidate.warmEdit.visualDelivery = {
  state: "unavailable",
  visualBindingsVerified: true,
  visualBindingSetHash: HASH_A,
  runtimeAttestationHash: HASH_B,
  bindingVerificationSource:
    "frozen-runtime-attestation-plus-unavailable-delivery-metric",
  unavailableMetricRecorded: true,
  mainTaskCompleted: true,
  providerModelResourceId: "deepseek-v4-pro",
  providerVisionCapable: false,
  referenceArtifactId: "visual-candidate-reference",
};
const nonVisualSamples = collectPairedSamples(
  session,
  nonVisualSpec,
  nonVisualControl,
  nonVisualCandidate,
);
assert.deepEqual(nonVisualSamples.candidate.coverage, [
  "nextTemplate",
  "nonVisualUnavailableMainTaskPassed",
]);

for (const [field, value] of [
  ["state", "not_applicable"],
  ["visualBindingsVerified", false],
  ["visualBindingSetHash", "invalid"],
  ["runtimeAttestationHash", "invalid"],
  ["bindingVerificationSource", "asserted"],
  ["unavailableMetricRecorded", false],
  ["mainTaskCompleted", false],
  ["providerVisionCapable", true],
  ["referenceArtifactId", ""],
]) {
  const incomplete = structuredClone(nonVisualCandidate);
  incomplete.warmEdit.visualDelivery[field] = value;
  assert.throws(
    () => collectPairedSamples(session, nonVisualSpec, nonVisualControl, incomplete),
    /nonVisualUnavailableMainTaskPassed coverage is not proven/,
  );
}

const multimodalSpec = {
  ...warmSpec,
  pairId: "batch-1-ai-governance-multimodal-delivery",
  coverage: ["nextTemplate", "multimodalVisualDelivered"],
};
const multimodalSession = structuredClone(session);
Object.assign(multimodalSession.providers[0], {
  modelResourceId: "vision-model",
  modelVersion: "vision-model-v1",
  visionCapable: true,
  supportedImageMediaTypes: ["image/png", "image/jpeg"],
  maxImageCount: 4,
});
const multimodalEvidence = (side) => {
  const value = warmEvidence(side);
  const run = value.warmEdit.run;
  run.efficiency.model = "resource:vision-model";
  for (const execution of run.modelExecutions) {
    execution.modelResourceId = "vision-model";
    execution.physicalModel = "vision-model-v1";
  }
  return value;
};
const multimodalControl = multimodalEvidence("control");
const multimodalCandidate = multimodalEvidence("candidate");
multimodalCandidate.warmEdit.visualDelivery = {
  state: "delivered",
  visualBindingsVerified: true,
  visualBindingSetHash: HASH_A,
  runtimeAttestationHash: HASH_B,
  bindingVerificationSource:
    "frozen-runtime-attestation-plus-gateway-visual-input-attestation",
  unavailableMetricRecorded: false,
  gatewayVisualInputAttested: true,
  gatewayAcceptedImageCount: 1,
  mainTaskCompleted: true,
  providerModelResourceId: "vision-model",
  providerVisionCapable: true,
  referenceArtifactId: "visual-candidate-reference",
  referenceArtifactSha256: HASH_C,
  referenceMediaType: "image/png",
};
multimodalCandidate.warmEdit.run.modelExecutions[0].visualInput = {
  state: "verified_and_provider_accepted",
  imageCount: 1,
  artifactSha256s: [HASH_C],
  mediaTypes: ["image/png"],
};
const multimodalSamples = collectPairedSamples(
  multimodalSession,
  multimodalSpec,
  multimodalControl,
  multimodalCandidate,
);
assert.deepEqual(multimodalSamples.candidate.coverage, [
  "nextTemplate",
  "multimodalVisualDelivered",
]);

for (const [field, value] of [
  ["state", "unavailable"],
  ["visualBindingsVerified", false],
  ["visualBindingSetHash", "invalid"],
  ["runtimeAttestationHash", "invalid"],
  ["bindingVerificationSource", "asserted"],
  ["unavailableMetricRecorded", true],
  ["gatewayVisualInputAttested", false],
  ["gatewayAcceptedImageCount", 0],
  ["mainTaskCompleted", false],
  ["providerVisionCapable", false],
  ["providerModelResourceId", "wrong-model"],
  ["referenceArtifactId", ""],
  ["referenceArtifactSha256", "invalid"],
  ["referenceMediaType", ""],
]) {
  const incomplete = structuredClone(multimodalCandidate);
  incomplete.warmEdit.visualDelivery[field] = value;
  assert.throws(
    () => collectPairedSamples(
      multimodalSession,
      multimodalSpec,
      multimodalControl,
      incomplete,
    ),
    /multimodalVisualDelivered coverage is not proven/,
  );
}

const unattestedMultimodal = structuredClone(multimodalCandidate);
unattestedMultimodal.warmEdit.run.modelExecutions[0].visualInput.artifactSha256s = [HASH_A];
assert.throws(
  () => collectPairedSamples(
    multimodalSession,
    multimodalSpec,
    multimodalControl,
    unattestedMultimodal,
  ),
  /multimodalVisualDelivered coverage is not proven/,
);

const falselyVisionCapableSession = structuredClone(multimodalSession);
falselyVisionCapableSession.providers[0].visionCapable = false;
assert.throws(
  () => collectPairedSamples(
    falselyVisionCapableSession,
    multimodalSpec,
    multimodalControl,
    multimodalCandidate,
  ),
  /multimodalVisualDelivered coverage is not proven/,
);

const warmPromptDrift = warmEvidence("candidate");
warmPromptDrift.warmEdit.prompt += " drift";
assert.throws(
  () => collectPairedSamples(session, warmSpec, warmEvidence("control"), warmPromptDrift),
  /prompt hash mismatch/,
);

const missingControlWarmEdit = warmEvidence("control");
missingControlWarmEdit.warmEdit = null;
assert.throws(
  () => collectPairedSamples(session, warmSpec, missingControlWarmEdit, warmEvidence("candidate")),
  /lifecycle evidence missing: control/,
);

const failedControlWarmEdit = warmEvidence("control");
failedControlWarmEdit.warmEdit.status = "failed";
failedControlWarmEdit.warmEdit.draftPreview = null;
failedControlWarmEdit.warmEdit.run.status = "completed";
failedControlWarmEdit.warmEdit.run.efficiency.requiredFidelityPassed = null;
const failureSamples = collectPairedSamples(
  session,
  warmSpec,
  failedControlWarmEdit,
  warmEvidence("candidate"),
);
assert.equal(failureSamples.control.status, "completed");
assert.equal(failureSamples.control.requiredFidelityPassed, false);
assert.equal(failureSamples.candidate.requiredFidelityPassed, true);

const rejectedBaseControl = warmEvidence("control");
rejectedBaseControl.status = "rejected";
rejectedBaseControl.error = {
  classification: "acceptance_rejected",
  message: "base Draft Preview omitted the required fixture text",
};
const rejectedBaseSamples = collectPairedSamples(
  session,
  warmSpec,
  rejectedBaseControl,
  warmEvidence("candidate"),
);
assert.equal(rejectedBaseSamples.control.status, "completed");
assert.equal(rejectedBaseSamples.control.requiredFidelityPassed, false);
assert.equal(rejectedBaseSamples.candidate.requiredFidelityPassed, true);
assert.notEqual(
  rejectedBaseSamples.control.acceptanceEvidenceSha256,
  failureSamples.control.acceptanceEvidenceSha256,
  "lifecycle acceptance evidence must bind the outer case acceptance result",
);

const incompleteAcceptedWarmEdit = warmEvidence("control");
incompleteAcceptedWarmEdit.warmEdit.draftPreview = null;
assert.throws(
  () => collectPairedSamples(session, warmSpec, incompleteAcceptedWarmEdit, warmEvidence("candidate")),
  /accepted evidence is missing expected text: control/,
);

function coldDevEvidence(side) {
  const value = warmEvidence(side);
  value.coldDevEdit = value.warmEdit;
  value.warmEdit = null;
  value.coldDevEdit.lifecycleProfile = "cold_dev";
  value.coldDevEdit.warmEditKind = null;
  value.coldDevEdit.prompt =
    "Add exact marker PAIRED_WARM_MARKER, restore dependencies, and restart Dev.";
  value.coldDevEdit.run.efficiency.timeToIframeAppliedMs = null;
  value.coldDevEdit.run.efficiency.coldDevReadyMs =
    side === "control" ? 25_000 : 14_000;
  value.coldDevEdit.run.efficiency.timeToDurableSnapshotMs =
    side === "control" ? 20_000 : 10_000;
  return value;
}

const coldSpec = {
  ...spec,
  pairId: "batch-1-ai-governance-cold-dev",
  bucket: "cold_dev",
  control: { source: "coldDevEdit", phase: "edit" },
  candidate: { source: "coldDevEdit", phase: "edit" },
};
const coldSamples = collectPairedSamples(
  session,
  coldSpec,
  coldDevEvidence("control"),
  coldDevEvidence("candidate"),
);
assert.equal(coldSamples.control.identity.phase, "edit");
assert.equal(coldSamples.candidate.metrics.coldDevReadyMs, 14_000);
assert.equal(coldSamples.candidate.metrics.timeToDurableSnapshotMs, 10_000);

const coldProfileDrift = coldDevEvidence("candidate");
coldProfileDrift.coldDevEdit.lifecycleProfile = "warm_hmr";
assert.throws(
  () => collectPairedSamples(session, coldSpec, coldDevEvidence("control"), coldProfileDrift),
  /Cold Dev Edit lifecycle profile mismatch/,
);

function repairEvidence(side) {
  const value = evidence(side);
  const run = structuredClone(value.runs[0]);
  run.phase = "repair";
  run.efficiency.phase = "repair";
  run.efficiency.timeToFirstSourceMutationMs = side === "control" ? 30_000 : 18_000;
  value.repair = {
    schemaVersion: "generation-real-provider-repair-evidence@1",
    startedAt: "2026-07-20T00:03:00.000Z",
    finishedAt: "2026-07-20T00:04:00.000Z",
    status: "accepted",
    projectId: value.projectId,
    prompt: "Repair the scoped inaccessible contrast finding and preserve the exact marker.",
    lifecycleProfile: "repair_warm",
    repairMarker: "PAIR_REPAIR_MARKER",
    run,
    repairVerification: {
      freshVersionCreated: true,
      sourceMutationRecorded: true,
      previewPublishRecorded: true,
      markerPreserved: true,
    },
  };
  return value;
}

const repairSpec = {
  ...spec,
  pairId: "batch-1-ai-governance-repair",
  bucket: "repair",
  control: { source: "repair", phase: "repair" },
  candidate: { source: "repair", phase: "repair" },
  coverage: ["fumadocsTemplate"],
};
const repairSamples = collectPairedSamples(
  session,
  repairSpec,
  repairEvidence("control"),
  repairEvidence("candidate"),
);
assert.equal(repairSamples.control.identity.phase, "repair");
assert.equal(repairSamples.candidate.metrics.timeToFirstSourceMutationMs, 18_000);
assert.equal(repairSamples.candidate.requiredFidelityPassed, true);

const repairMarkerDrift = repairEvidence("candidate");
repairMarkerDrift.repair.repairMarker = "PAIR_REPAIR_DRIFT";
assert.throws(
  () => collectPairedSamples(session, repairSpec, repairEvidence("control"), repairMarkerDrift),
  /Repair lifecycle identity mismatch/,
);

const incompleteRepair = repairEvidence("control");
incompleteRepair.repair.repairVerification.previewPublishRecorded = false;
const incompleteRepairSamples = collectPairedSamples(
  session,
  repairSpec,
  incompleteRepair,
  repairEvidence("candidate"),
);
assert.equal(incompleteRepairSamples.control.requiredFidelityPassed, false);

function restartEvidence(side) {
  const base = evidence(side);
  const run = base.runs[0];
  const generationContextStatus = side === "candidate"
    ? {
        schemaVersion: "generation-context-status@1",
        runId: run.runId,
        runContractVersion: "generation-context@1",
        status: "compiled",
        runtimeMode: "enabled",
        contextContentHash: HASH_A,
        runContextBindingHash: HASH_B,
        runtimeAttestationHash: HASH_C,
        workflowState: "completed",
      }
    : {
        schemaVersion: "generation-context-status@1",
        runId: run.runId,
        runContractVersion: "legacy@1",
        status: "not_compiled",
        runtimeMode: null,
        contextContentHash: null,
        runContextBindingHash: null,
        runtimeAttestationHash: null,
        workflowState: null,
      };
  const snapshot = {
    schemaVersion: "generation-context-runtime-restart-snapshot@1",
    recordedAt: "2026-07-22T00:00:00.000Z",
    projectId: base.projectId,
    runId: run.runId,
    healthReady: true,
    generationContextStatus,
    efficiency: structuredClone(run.efficiency),
    projectState: {
      currentVersionId: `${side}-version`,
      sandboxBindingId: `${side}-sandbox`,
      sourceSnapshotRefSha256: HASH_A,
      templateKey: "next-app@2",
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
      bodyBytes: 1000,
    },
  };
  return createRuntimeRestartEvidence({
    recordedAt: "2026-07-22T00:01:00.000Z",
    side,
    deployment: `runtime-${side}`,
    runtimeDeploymentRevision: `${side}-revision`,
    deploymentUid: `${side}-deployment-uid`,
    deploymentGeneration: 1,
    deploymentTemplateSha256: HASH_A,
    podBefore: { name: `${side}-before`, uid: `${side}-before-uid` },
    deploymentUidAfter: `${side}-deployment-uid`,
    deploymentGenerationAfter: 1,
    deploymentTemplateSha256After: HASH_A,
    podAfter: { name: `${side}-after`, uid: `${side}-after-uid` },
    restartDurationMs: 1000,
  }, snapshot, { ...structuredClone(snapshot), recordedAt: "2026-07-22T00:01:00.000Z" });
}

const runtimeRestartSpec = {
  ...spec,
  pairId: "batch-1-ai-governance-runtime-restart",
  coverage: ["nextTemplate", "runtimeRestart"],
};
const runtimeRestartSamples = collectPairedSamples(
  session,
  runtimeRestartSpec,
  evidence("control"),
  evidence("candidate"),
  {
    control: restartEvidence("control"),
    candidate: restartEvidence("candidate"),
  },
);
assert.deepEqual(runtimeRestartSamples.candidate.coverage, ["nextTemplate", "runtimeRestart"]);

assert.throws(
  () => collectPairedSamples(
    session,
    runtimeRestartSpec,
    evidence("control"),
    evidence("candidate"),
  ),
  /restart evidence schema/,
);

const driftedRestart = restartEvidence("candidate");
driftedRestart.after.projectState.currentVersionId = "drifted-version";
assert.throws(
  () => collectPairedSamples(
    session,
    runtimeRestartSpec,
    evidence("control"),
    evidence("candidate"),
    { control: restartEvidence("control"), candidate: driftedRestart },
  ),
  /state changed|recovery invariant|hash mismatch/,
);

process.stdout.write("Generation Context real-provider paired sample collector tests passed.\n");
