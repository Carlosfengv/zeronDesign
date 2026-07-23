import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { replayEvidence } from "./replay-evidence.mjs";
import { createRuntimeRestartEvidence } from "../../services/runtime/scripts/generation-context-runtime-restart-evidence.mjs";
import {
  loadTerminalBundleIndex,
  terminalBundleSetPassed,
} from "../../services/runtime/scripts/aggregate-release-evidence.mjs";

const directory = path.dirname(fileURLToPath(import.meta.url));
const assembler = path.join(directory, "assemble-runtime-terminal-evidence.mjs");
const root = fs.mkdtempSync(path.join(os.tmpdir(), "runtime-terminal-evidence-"));
const suite = path.join(root, "suite-fixture-accepted");
const output = path.join(root, "bundle");
fs.mkdirSync(suite);
const hash = (value) => crypto.createHash("sha256").update(value).digest("hex");
const sha = (character) => character.repeat(64);

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) =>
      `${JSON.stringify(key)}:${canonical(value[key])}`
    ).join(",")}}`;
  }
  return JSON.stringify(value);
}

function fixtureBudgetProfile(phase) {
  const tokenLimits = {
    maxTurns: 60,
    maxToolCalls: 180,
    maxInputTokens: 600000,
    maxGrossInputTokens: 600000,
    maxUncachedInputTokens: 600000,
    maxPromptTokensPerTurn: 600000,
    maxOutputTokens: 80000,
  };
  const profile = {
    schemaVersion: "run-budget-profile@1",
    profileId: `phase-default-${phase}`,
    phase,
    rolloutMode: "shadow",
    tokenBudgetMode: "legacy",
    operationBudgetMode: "shadow",
    enforcedLimits: tokenLimits,
    phaseTargetLimits: { ...tokenLimits },
    operationLimits: {
      maxGrossInputTokens: 5000000,
      maxUncachedInputTokens: 5000000,
      maxOutputTokens: 500000,
      maxTurns: 600,
      maxToolCalls: 1800,
    },
  };
  profile.profileHash = hash(canonical(profile));
  return profile;
}

function withBudgetProfile(run) {
  const profile = fixtureBudgetProfile(run.phase);
  const context = run.generationContextStatus;
  assert.ok(context?.operationId, `${run.runId} fixture requires an Operation identity`);
  return {
    ...run,
    generationContextStatus: {
      ...context,
      operationAttempt: context.operationAttempt ?? 1,
      budgetProfileId: profile.profileId,
      budgetProfileHash: profile.profileHash,
      budgetProfileRolloutMode: profile.rolloutMode,
    },
    budgetProfile: profile,
  };
}
const runId = "run-build-1";
const events = [
  JSON.stringify({
    type: "model.usage",
    runId,
    turn: 1,
    inputTokens: 1000,
    cachedInputTokens: 400,
    outputTokens: 100,
    estimated: false,
  }),
  JSON.stringify({
    type: "run.completed",
    runId,
    status: "completed",
    summarySha256: sha("a"),
    summaryBytes: 9,
  }),
].join("\n") + "\n";
fs.writeFileSync(path.join(suite, "run-build.events.ndjson"), events);
fs.writeFileSync(path.join(suite, "real-provider-examples-summary.json"), JSON.stringify({
  schemaVersion: "generation-real-provider-suite-evidence@2",
  suiteId: "suite-fixture",
  status: "accepted",
  provenance: {
    gitCommit: "commit-fixture",
    gitDirty: false,
    providerConfigSha256: sha("b"),
    providerResourceRevision: 7,
  },
  provider: {
    modelResourceId: "deepseek-v4-pro",
    realProviderVerified: true,
  },
}));
fs.writeFileSync(path.join(suite, "real-provider-case-fixture.json"), JSON.stringify({
  schemaVersion: "generation-real-provider-case-evidence@2",
  id: "fixture",
  kind: "website",
  projectId: "project-fixture",
  status: "accepted",
  expectedRoute: "/",
  expectedText: "Fixture marker",
  promptSha256: sha("c"),
  acceptance: { sha256: sha("d") },
  contentPlan: { intentSha256: sha("e") },
  attempts: [{ attempt: 1, status: "accepted", runIds: [runId] }],
  runs: [withBudgetProfile({
    phase: "build",
    runId,
    status: "completed",
    usage: { inputTokens: 1000, cachedInputTokens: 400, outputTokens: 100, totalTokens: 1100 },
    eventStream: {
      schemaVersion: "generation-run-event-stream@1",
      path: "run-build.events.ndjson",
      format: "ndjson",
      eventCount: 2,
      sha256: hash(events),
    },
    efficiency: { template: "next-app@2" },
    generationContextStatus: {
      schemaVersion: "generation-context-status@1",
      runContractVersion: "generation-context@1",
      status: "compiled",
      runId,
      operationId: "operation-fixture",
      contextContentHash: sha("f"),
      runContextBindingHash: sha("1"),
      budgetProfileId: "phase-default",
      budgetProfileHash: sha("2"),
      budgetProfileRolloutMode: "shadow",
    },
    buildEvidence: {
      buildId: "build-fixture",
      sourceSnapshotUri: "runtime://source-snapshots/project/build-fixture",
      sourceFingerprint: sha("3"),
      candidateManifestHash: sha("4"),
      artifactRouteManifestPath: ".anydesign-artifact-routes.json",
      artifactRouteManifestHash: sha("5"),
    },
    modelExecutions: [{
      modelResourceId: "deepseek-v4-pro",
      modelResourceRevision: 7,
      providerRequestIdPresent: true,
    }],
  })],
  artifact: {
    httpStatus: 200,
    expectedTextFound: true,
    bodySha256: sha("6"),
    bodyBytes: 100,
  },
  sandboxRelease: {
    required: true,
    released: true,
    requiredSuccessfulResponses: 2,
  },
}));
const cacheFile = path.join(root, "cache-audit.json");
fs.writeFileSync(cacheFile, JSON.stringify({
  schemaVersion: "provider-cache-smoke-audit@1",
  toolSetHashVersion: "tool-definition-set@1",
  status: "passed",
  releaseEligible: true,
  sourceCommit: "commit-fixture",
  modelResourceId: "deepseek-v4-pro",
  providerResourceRevision: 7,
  providerConfigSha256: sha("b"),
  auditedRunCount: 1,
  stableRunCount: 1,
  grossInputTokens: 1000,
  cachedInputTokens: 400,
  runs: [{
    runId,
    redactionValid: true,
    compositionValid: true,
    metricsValid: true,
    repeatedStableTurns: 2,
    providerIdentityValid: true,
    buildIdentityValid: true,
    generationContextIdentityValid: true,
  }],
}));

const assembled = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--case", "fixture",
  "--cache", cacheFile,
  "--out", output,
], { encoding: "utf8" });
assert.equal(assembled.status, 0, assembled.stderr || assembled.stdout);
const replay = await replayEvidence(output);
assert.equal(replay.status, "passed");
assert.equal(replay.replay.usage.inputTokens, 1000);
assert.equal(replay.replay.providerCachePassed, true);

const editDirectory = path.join(suite, "published-edit-accepted");
fs.mkdirSync(editDirectory);
const editRunId = "run-edit-1";
const editEvents = [
  JSON.stringify({
    type: "model.usage",
    runId: editRunId,
    turn: 1,
    inputTokens: 700,
    cachedInputTokens: 300,
    outputTokens: 80,
    estimated: false,
  }),
  JSON.stringify({
    type: "run.completed",
    runId: editRunId,
    status: "completed",
    summarySha256: sha("7"),
    summaryBytes: 8,
  }),
].join("\n") + "\n";
fs.writeFileSync(path.join(editDirectory, "run-edit.events.ndjson"), editEvents);
const editSummaryFile = path.join(editDirectory, "real-provider-edit-summary.json");
const editEvidence = {
  schemaVersion: "generation-real-provider-edit-evidence@2",
  status: "accepted",
  projectId: "project-fixture",
  promptSha256: sha("8"),
  baseVersionId: "version-before",
  versionId: "version-after",
  editImpactPlanHash: sha("9"),
  providerVerified: true,
  secretMaterialPersisted: false,
  run: withBudgetProfile({
    phase: "edit",
    runId: editRunId,
    status: "completed",
    usage: { inputTokens: 700, cachedInputTokens: 300, outputTokens: 80, totalTokens: 780 },
    eventStream: {
      schemaVersion: "generation-run-event-stream@1",
      path: "run-edit.events.ndjson",
      format: "ndjson",
      eventCount: 2,
      sha256: hash(editEvents),
    },
    efficiency: { template: "next-app@2" },
    generationContextStatus: {
      schemaVersion: "generation-context-status@1",
      runContractVersion: "generation-context@1",
      status: "compiled",
      runId: editRunId,
      operationId: "operation-edit-fixture",
      contextContentHash: sha("a"),
      runContextBindingHash: sha("b"),
      budgetProfileId: "phase-default",
      budgetProfileHash: sha("c"),
      budgetProfileRolloutMode: "shadow",
    },
    buildEvidence: {
      buildId: "build-edit-fixture",
      sourceSnapshotUri: "runtime://source-snapshots/project/build-edit-fixture",
      sourceFingerprint: sha("d"),
      candidateManifestHash: sha("e"),
      artifactRouteManifestPath: ".anydesign-artifact-routes.json",
      artifactRouteManifestHash: sha("f"),
    },
    modelExecutions: [{
      modelResourceId: "deepseek-v4-pro",
      modelResourceRevision: 7,
      providerRequestIdPresent: true,
    }],
  }),
  artifact: {
    versionId: "version-after",
    route: "/",
    httpStatus: 200,
    semanticNavFound: true,
    originalHeadlineFound: true,
    declaredIconHttpStatus: 200,
    bodySha256: sha("1"),
    bodyBytes: 120,
  },
  releaseEvidence: {
    available: true,
    versionId: "version-after",
    releaseId: "release-after",
    artifactManifestHash: sha("2"),
    sourceFingerprint: sha("d"),
  },
  sandboxRelease: {
    required: true,
    released: true,
    requiredSuccessfulResponses: 2,
    attempts: [
      { attempt: 1, ok: true, status: 204, error: null },
      { attempt: 2, ok: true, status: 204, error: null },
    ],
  },
};
fs.writeFileSync(editSummaryFile, JSON.stringify(editEvidence));
const editOutput = path.join(root, "bundle-edit");
const editAssembled = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--edit", path.relative(suite, editDirectory),
  "--cache", cacheFile,
  "--out", editOutput,
], { encoding: "utf8" });
assert.equal(editAssembled.status, 0, editAssembled.stderr || editAssembled.stdout);
const editReplay = await replayEvidence(editOutput);
assert.equal(editReplay.status, "passed");
assert.equal(editReplay.replay.usage.inputTokens, 700);
assert.equal(JSON.parse(fs.readFileSync(path.join(editOutput, "case-summary.json"), "utf8")).kind, "edit");

editEvidence.sandboxRelease.released = false;
fs.writeFileSync(editSummaryFile, JSON.stringify(editEvidence));
const editWithoutCleanup = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--edit", path.relative(suite, editDirectory),
  "--cache", cacheFile,
  "--out", path.join(root, "bundle-edit-without-cleanup"),
], { encoding: "utf8" });
assert.notEqual(editWithoutCleanup.status, 0);
assert.match(editWithoutCleanup.stderr, /Edit Sandbox release is incomplete/);

function terminalEvents(fixtureRunId, inputTokens, cachedInputTokens, outputTokens, digest) {
  return [
    JSON.stringify({
      type: "model.usage",
      runId: fixtureRunId,
      turn: 1,
      inputTokens,
      cachedInputTokens,
      outputTokens,
      estimated: false,
    }),
    JSON.stringify({
      type: "run.completed",
      runId: fixtureRunId,
      status: "completed",
      summarySha256: sha(digest),
      summaryBytes: 8,
    }),
  ].join("\n") + "\n";
}

function fixtureContext(fixtureRunId, digest) {
  return {
    schemaVersion: "generation-context-status@1",
    runContractVersion: "generation-context@1",
    status: "compiled",
    runId: fixtureRunId,
    operationId: `operation-${fixtureRunId}`,
    contextContentHash: sha(digest),
    runContextBindingHash: sha(digest === "3" ? "4" : "5"),
    budgetProfileId: "phase-default",
    budgetProfileHash: sha("6"),
    budgetProfileRolloutMode: "shadow",
  };
}

function fixtureBuild(fixtureRunId, digest) {
  return {
    buildId: `build-${fixtureRunId}`,
    sourceSnapshotUri: `runtime://source-snapshots/project/build-${fixtureRunId}`,
    sourceFingerprint: sha(digest),
    candidateManifestHash: sha(digest === "7" ? "8" : "9"),
    artifactRouteManifestPath: ".anydesign-artifact-routes.json",
    artifactRouteManifestHash: sha("a"),
  };
}

const setupRunId = "run-repair-setup";
const reviewRunId = "run-repair-review";
const repairRunId = "run-repair-final";
const setupEvents = terminalEvents(setupRunId, 200, 50, 20, "b");
const reviewEvents = terminalEvents(reviewRunId, 300, 100, 30, "c");
const repairEvents = terminalEvents(repairRunId, 500, 200, 50, "d");
const setupDirectory = path.join(suite, "repair-setup-edit-accepted");
fs.mkdirSync(setupDirectory);
fs.writeFileSync(path.join(setupDirectory, "run-edit.events.ndjson"), setupEvents);
fs.writeFileSync(path.join(suite, "run-review.events.ndjson"), reviewEvents);
fs.writeFileSync(path.join(suite, "run-repair.events.ndjson"), repairEvents);
const setupRun = withBudgetProfile({
  phase: "edit",
  runId: setupRunId,
  status: "completed",
  usage: { inputTokens: 200, cachedInputTokens: 50, outputTokens: 20, totalTokens: 220 },
  eventStream: {
    schemaVersion: "generation-run-event-stream@1",
    path: "run-edit.events.ndjson",
    format: "ndjson",
    eventCount: 2,
    sha256: hash(setupEvents),
  },
  efficiency: { template: "fumadocs-docs@2" },
  generationContextStatus: fixtureContext(setupRunId, "3"),
  buildEvidence: fixtureBuild(setupRunId, "7"),
  modelExecutions: [{
    modelResourceId: "deepseek-v4-pro",
    modelResourceRevision: 7,
    providerRequestIdPresent: true,
  }],
});
const setupEvidence = {
  schemaVersion: "generation-real-provider-edit-evidence@2",
  status: "accepted",
  projectId: "project-fixture",
  promptSha256: sha("e"),
  baseVersionId: "repair-base-original",
  versionId: "repair-base-defect",
  providerVerified: true,
  secretMaterialPersisted: false,
  run: setupRun,
  sandboxRelease: {
    required: false,
    released: false,
    reason: "keep-sandbox-requested",
    attempts: [],
  },
};
const setupEvidenceFile = path.join(setupDirectory, "real-provider-edit-summary.json");
fs.writeFileSync(setupEvidenceFile, JSON.stringify(setupEvidence));
const reviewRun = withBudgetProfile({
  phase: "review",
  runId: reviewRunId,
  status: "completed",
  usage: { inputTokens: 300, cachedInputTokens: 100, outputTokens: 30, totalTokens: 330 },
  eventStream: {
    schemaVersion: "generation-run-event-stream@1",
    path: "run-review.events.ndjson",
    format: "ndjson",
    eventCount: 2,
    sha256: hash(reviewEvents),
  },
  efficiency: { template: "fumadocs-docs@2" },
  generationContextStatus: fixtureContext(reviewRunId, "e"),
  modelExecutions: [{
    modelResourceId: "deepseek-v4-pro",
    modelResourceRevision: 7,
    providerRequestIdPresent: true,
  }],
});
const repairContext = fixtureContext(repairRunId, "f");
const repairRun = withBudgetProfile({
  phase: "repair",
  runId: repairRunId,
  status: "completed",
  usage: { inputTokens: 500, cachedInputTokens: 200, outputTokens: 50, totalTokens: 550 },
  eventStream: {
    schemaVersion: "generation-run-event-stream@1",
    path: "run-repair.events.ndjson",
    format: "ndjson",
    eventCount: 2,
    sha256: hash(repairEvents),
  },
  efficiency: { template: "fumadocs-docs@2" },
  generationContextStatus: repairContext,
  buildEvidence: fixtureBuild(repairRunId, "1"),
  modelExecutions: [{
    modelResourceId: "deepseek-v4-pro",
    modelResourceRevision: 7,
    providerRequestIdPresent: true,
  }],
});
const repairCaseFile = path.join(suite, "real-provider-case-fixture.json");
const repairCase = JSON.parse(fs.readFileSync(repairCaseFile, "utf8"));
repairCase.repair = {
  schemaVersion: "generation-real-provider-repair-evidence@2",
  status: "accepted",
  projectId: "project-fixture",
  promptSha256: sha("2"),
  baseVersionId: "repair-base-defect",
  repairedVersionId: "repair-version-fixed",
  providerVerified: true,
  secretMaterialPersisted: false,
  setupEdit: {
    ...setupEvidence,
    evidencePath: path.relative(suite, setupEvidenceFile),
  },
  reviewRun,
  reviewFinding: { findingId: "finding-contrast" },
  generationContextStatus: repairContext,
  run: repairRun,
  repairVerification: {
    reviewFindingRecorded: true,
    findingFixedByCompletedRepair: true,
    freshVersionCreated: true,
    sourceMutationRecorded: true,
    previewPublishRecorded: true,
    markerPreserved: true,
    artifactRoute: "/docs/",
    artifactHttpStatus: 200,
    artifactBodySha256: sha("3"),
    artifactBodyBytes: 321,
  },
};
fs.writeFileSync(repairCaseFile, JSON.stringify(repairCase));
const repairOutput = path.join(root, "bundle-repair");
const repairAssembled = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--repair", "fixture",
  "--cache", cacheFile,
  "--out", repairOutput,
], { encoding: "utf8" });
assert.equal(repairAssembled.status, 0, repairAssembled.stderr || repairAssembled.stdout);
const repairReplay = await replayEvidence(repairOutput);
assert.equal(repairReplay.status, "passed");
assert.equal(repairReplay.replay.usage.inputTokens, 1000);
const repairRunModelUsage = JSON.parse(
  fs.readFileSync(path.join(repairOutput, "run-model-usage.json"), "utf8"),
);
assert.equal(repairRunModelUsage.schemaVersion, "runtime-evidence-run-model-usage@1");
assert.deepEqual(
  new Set(repairRunModelUsage.runs.map((runUsage) => runUsage.runId)),
  new Set([setupRunId, reviewRunId, repairRunId]),
);
assert.deepEqual(
  new Set(JSON.parse(fs.readFileSync(path.join(repairOutput, "usage.json"), "utf8")).turns.map((turn) => turn.runId)),
  new Set([setupRunId, reviewRunId, repairRunId]),
);
repairCase.sandboxRelease.released = false;
fs.writeFileSync(repairCaseFile, JSON.stringify(repairCase));
const repairWithoutCleanup = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--repair", "fixture",
  "--cache", cacheFile,
  "--out", path.join(root, "bundle-repair-without-cleanup"),
], { encoding: "utf8" });
assert.notEqual(repairWithoutCleanup.status, 0);
assert.match(repairWithoutCleanup.stderr, /Repair Sandbox release is incomplete/);
repairCase.sandboxRelease.released = true;
fs.writeFileSync(repairCaseFile, JSON.stringify(repairCase));

const restartBudgetProfile = fixtureBudgetProfile("build");
const restartSnapshot = {
  schemaVersion: "generation-context-runtime-restart-snapshot@1",
  recordedAt: "2026-07-23T00:00:00.000Z",
  projectId: "project-fixture",
  runId,
  healthReady: true,
  generationContextStatus: {
    schemaVersion: "generation-context-status@1",
    runId,
    runContractVersion: "generation-context@1",
    status: "compiled",
    runtimeMode: "enabled",
    compilerVersion: "generation-context-compiler@1",
    contextContentHash: sha("f"),
    runContextBindingHash: sha("1"),
    runtimeAttestationHash: sha("2"),
    executionProfile: "greenfield_static",
    budgetProfileId: restartBudgetProfile.profileId,
    budgetProfileHash: restartBudgetProfile.profileHash,
    budgetProfileRolloutMode: restartBudgetProfile.rolloutMode,
    workflowState: "completed",
    contextWindowEpoch: 0,
  },
  budgetProfile: restartBudgetProfile,
  efficiency: {
    schemaVersion: "run-efficiency-metrics@1",
    calculatorVersion: "run-efficiency-calculator@1",
    runId,
    projectId: "project-fixture",
    phase: "build",
    model: "resource:deepseek-v4-pro",
    template: "next-app@2",
    status: "completed",
    inputTokens: 1000,
    firstBuildSucceeded: true,
  },
  projectState: {
    stateKind: "published_version",
    currentVersionId: "version-fixture",
    sandboxBindingId: "sandbox-fixture",
    sourceSnapshotRefSha256: hash("runtime://source-snapshots/project/build-fixture"),
    templateKey: "next-app",
    styleContractSha256: sha("3"),
    latestBuildSha256: sha("4"),
    dependencyStateSha256: null,
    previewSha256: sha("5"),
  },
  history: { itemCount: 1, sha256: sha("6") },
  releaseEvidence: {
    httpStatus: 200,
    available: true,
    canonicalResponseSha256: sha("7"),
    stableStateSha256: sha("8"),
  },
  artifact: {
    source: "current_version",
    httpStatus: 200,
    markerFound: true,
    markerSha256: hash("Fixture marker"),
    bodySha256: sha("6"),
    bodyBytes: 100,
  },
};
const restartAfter = {
  ...structuredClone(restartSnapshot),
  recordedAt: "2026-07-23T00:01:00.000Z",
  sandboxRelease: {
    required: true,
    released: true,
    requiredSuccessfulResponses: 2,
    attempts: [
      { attempt: 1, ok: true, status: 204, error: null },
      { attempt: 2, ok: true, status: 204, error: null },
    ],
  },
};
const restartEvidence = createRuntimeRestartEvidence({
  recordedAt: "2026-07-23T00:01:00.000Z",
  side: "candidate",
  deployment: "runtime-candidate",
  runtimeDeploymentRevision: "runtime-candidate-revision",
  deploymentUid: "deployment-uid",
  deploymentGeneration: 7,
  deploymentTemplateSha256: sha("9"),
  podBefore: { name: "pod-before", uid: "pod-uid-before" },
  deploymentUidAfter: "deployment-uid",
  deploymentGenerationAfter: 7,
  deploymentTemplateSha256After: sha("9"),
  podAfter: { name: "pod-after", uid: "pod-uid-after" },
  restartDurationMs: 2500,
}, restartSnapshot, restartAfter);
const restartFile = path.join(suite, "runtime-restart-evidence.json");
fs.writeFileSync(restartFile, JSON.stringify(restartEvidence));
const restartOutput = path.join(root, "bundle-runtime-restart");
const restartAssembled = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--restart", path.basename(restartFile),
  "--restart-case", "fixture",
  "--cache", cacheFile,
  "--out", restartOutput,
], { encoding: "utf8" });
assert.equal(restartAssembled.status, 0, restartAssembled.stderr || restartAssembled.stdout);
const restartReplay = await replayEvidence(restartOutput);
assert.equal(restartReplay.status, "passed");
assert.equal(restartReplay.replay.podUidChanged, true);
assert.equal(restartReplay.replay.sandboxReleased, true);

const docsOutput = path.join(root, "bundle-docs");
fs.cpSync(output, docsOutput, { recursive: true });
const docsManifestFile = path.join(docsOutput, "manifest.json");
const docsManifest = JSON.parse(fs.readFileSync(docsManifestFile, "utf8"));
docsManifest.evidenceId = `${docsManifest.evidenceId}-duplicate`;
fs.writeFileSync(docsManifestFile, `${JSON.stringify(docsManifest, null, 2)}\n`);
const docsChecksumsFile = path.join(docsOutput, "checksums.sha256");
const docsChecksums = fs.readFileSync(docsChecksumsFile, "utf8")
  .trim().split("\n")
  .map((line) => {
    if (line.endsWith("  manifest.json")) {
      return `${hash(fs.readFileSync(docsManifestFile))}  manifest.json`;
    }
    return line;
  })
  .join("\n") + "\n";
fs.writeFileSync(docsChecksumsFile, docsChecksums);
assert.equal((await replayEvidence(docsOutput)).status, "passed");
const tamperedRunModelOutput = path.join(root, "bundle-tampered-run-model-usage");
fs.cpSync(output, tamperedRunModelOutput, { recursive: true });
const tamperedRunModelFile = path.join(tamperedRunModelOutput, "run-model-usage.json");
const tamperedRunModelUsage = JSON.parse(fs.readFileSync(tamperedRunModelFile, "utf8"));
tamperedRunModelUsage.runs[0].inputTokens += 1;
tamperedRunModelUsage.runs[0].totalTokens += 1;
fs.writeFileSync(tamperedRunModelFile, `${JSON.stringify(tamperedRunModelUsage, null, 2)}\n`);
const tamperedRunModelSha256 = hash(fs.readFileSync(tamperedRunModelFile));
const tamperedRunModelManifestFile = path.join(tamperedRunModelOutput, "manifest.json");
const tamperedRunModelManifest = JSON.parse(
  fs.readFileSync(tamperedRunModelManifestFile, "utf8"),
);
tamperedRunModelManifest.runModelUsageSha256 = tamperedRunModelSha256;
tamperedRunModelManifest.replayExpectations.runModelUsageSha256 = tamperedRunModelSha256;
fs.writeFileSync(
  tamperedRunModelManifestFile,
  `${JSON.stringify(tamperedRunModelManifest, null, 2)}\n`,
);
const tamperedRunModelCaseFile = path.join(tamperedRunModelOutput, "case-summary.json");
const tamperedRunModelCase = JSON.parse(fs.readFileSync(tamperedRunModelCaseFile, "utf8"));
tamperedRunModelCase.runModelUsageSha256 = tamperedRunModelSha256;
fs.writeFileSync(
  tamperedRunModelCaseFile,
  `${JSON.stringify(tamperedRunModelCase, null, 2)}\n`,
);
const tamperedRunModelChecksumsFile = path.join(tamperedRunModelOutput, "checksums.sha256");
const tamperedRunModelChecksums = fs.readFileSync(tamperedRunModelChecksumsFile, "utf8")
  .trim().split("\n")
  .map((line) => {
    if (line.endsWith("  run-model-usage.json")) {
      return `${tamperedRunModelSha256}  run-model-usage.json`;
    }
    if (line.endsWith("  manifest.json")) {
      return `${hash(fs.readFileSync(tamperedRunModelManifestFile))}  manifest.json`;
    }
    if (line.endsWith("  case-summary.json")) {
      return `${hash(fs.readFileSync(tamperedRunModelCaseFile))}  case-summary.json`;
    }
    return line;
  })
  .join("\n") + "\n";
fs.writeFileSync(tamperedRunModelChecksumsFile, tamperedRunModelChecksums);
await assert.rejects(
  () => replayEvidence(tamperedRunModelOutput),
  /RunModelUsage evidence does not match the event replay/,
);
const bundleSetFile = path.join(root, "terminal-bundle-set.json");
fs.writeFileSync(bundleSetFile, JSON.stringify({
  schemaVersion: "runtime-terminal-bundle-set@1",
  bundles: [output, docsOutput, editOutput, repairOutput, restartOutput]
    .map((directoryPath) => path.relative(root, directoryPath)),
}));
const terminalIndex = await loadTerminalBundleIndex(bundleSetFile);
assert.equal(terminalIndex.replayedCount, 5);
assert.equal(terminalBundleSetPassed(terminalIndex, {
  modelResourceId: "deepseek-v4-pro",
  providerResourceRevision: 7,
  providerConfigSha256: sha("b"),
}, "commit-fixture", hash(fs.readFileSync(cacheFile))), false,
"a duplicated Website bundle must not satisfy Docs coverage");

const restartWithoutCleanup = structuredClone(restartEvidence);
restartWithoutCleanup.cleanup.released = false;
fs.writeFileSync(restartFile, JSON.stringify(restartWithoutCleanup));
const uncleanRestart = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--restart", path.basename(restartFile),
  "--restart-case", "fixture",
  "--cache", cacheFile,
  "--out", path.join(root, "bundle-runtime-restart-without-cleanup"),
], { encoding: "utf8" });
assert.notEqual(uncleanRestart.status, 0);
assert.match(uncleanRestart.stderr, /confirmed Sandbox release/);

fs.appendFileSync(path.join(output, "events.ndjson"), "{}\n");
await assert.rejects(() => replayEvidence(output), /checksum mismatch/);

const caseFixtureFile = path.join(suite, "real-provider-case-fixture.json");
const caseFixture = JSON.parse(fs.readFileSync(caseFixtureFile, "utf8"));
caseFixture.runs[0].buildEvidence.sourceSnapshotUri = "";
fs.writeFileSync(caseFixtureFile, JSON.stringify(caseFixture));
const missingSourceIdentity = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--case", "fixture",
  "--cache", cacheFile,
  "--out", path.join(root, "bundle-missing-source"),
], { encoding: "utf8" });
assert.notEqual(missingSourceIdentity.status, 0);
assert.match(missingSourceIdentity.stderr, /Build identity is incomplete/);

caseFixture.runs[0].buildEvidence.sourceSnapshotUri = "runtime://source-snapshots/project/build-fixture";
fs.writeFileSync(caseFixtureFile, JSON.stringify(caseFixture));
const cacheFixture = JSON.parse(fs.readFileSync(cacheFile, "utf8"));
cacheFixture.runs[0].repeatedStableTurns = 1;
fs.writeFileSync(cacheFile, JSON.stringify(cacheFixture));
const unstableCacheRun = spawnSync(process.execPath, [
  assembler,
  "--suite", suite,
  "--case", "fixture",
  "--cache", cacheFile,
  "--out", path.join(root, "bundle-unstable-cache"),
], { encoding: "utf8" });
assert.notEqual(unstableCacheRun.status, 0);
assert.match(unstableCacheRun.stderr, /not covered by the release cache audit/);

process.stdout.write("Runtime terminal evidence bundle tests passed\n");
