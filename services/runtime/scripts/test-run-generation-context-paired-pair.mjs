#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  initializePairedCohortLedger,
  verifyPairedCohortLedger,
} from "./generation-context-paired-cohort-ledger.mjs";

const HASH_A = "a".repeat(64);
const HASH_B = "b".repeat(64);
const HASH_C = "c".repeat(64);
const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../../..");
const casesFile = path.join(root, "infra/generation-reliability/real-provider-cases.json");
const temp = fs.mkdtempSync(path.join(os.tmpdir(), "generation-context-pair-runner-"));
const sessionDir = path.join(temp, "session");
fs.mkdirSync(sessionDir, { recursive: true });

const session = {
  schemaVersion: "generation-context-paired-cohort-session@1",
  sessionId: "offline-pair-runner-test",
  createdAt: "2026-07-20T00:00:00.000Z",
  calculatorVersion: "generation-context-rollout-calculator@1",
  bootstrap: { iterations: 100, seed: 42 },
  sourcePolicy: "hashes_only",
  fixtureManifestSha256: crypto
    .createHash("sha256")
    .update(fs.readFileSync(casesFile))
    .digest("hex"),
  providers: [{
    gatewayMode: "internal_gateway",
    modelResourceId: "deepseek-v4-pro",
    resourceRevision: 4,
    modelVersion: "deepseek-v4-pro",
    providerParametersHash: HASH_B,
    visionCapable: false,
    supportedImageMediaTypes: [],
    maxImageCount: 0,
  }],
  runtimes: {
    control: {
      generationContextMode: "off",
      deploymentRevision: "control-revision",
      allowedModelResourceIds: ["deepseek-v4-pro"],
    },
    candidate: {
      generationContextMode: "enabled",
      deploymentRevision: "candidate-revision",
      allowedModelResourceIds: ["deepseek-v4-pro"],
    },
  },
};
fs.writeFileSync(
  path.join(sessionDir, "session.json"),
  `${JSON.stringify(session, null, 2)}\n`,
);
fs.writeFileSync(
  path.join(sessionDir, "session-meta.json"),
  `${JSON.stringify({
    schemaVersion: "generation-context-cohort-session-meta@1",
    sessionId: session.sessionId,
    context: "k3d-offline-test",
    workspaceNamespace: "ws-offline-test",
    deployments: [
      { side: "control", deployment: "runtime-control" },
      { side: "candidate", deployment: "runtime-candidate" },
    ],
  }, null, 2)}\n`,
);
const ledger = path.join(sessionDir, "cohort.ndjson");
initializePairedCohortLedger(ledger, session);

const fakeRunnerNode = path.join(temp, "fake-runner.mjs");
const fakeRunnerShell = path.join(temp, "fake-runner.sh");
const fakeRestartNode = path.join(temp, "fake-restart.mjs");
const fakeRestartShell = path.join(temp, "fake-restart.sh");
fs.writeFileSync(fakeRunnerShell, '#!/usr/bin/env bash\nexec node "$GENERATION_COHORT_FAKE_RUNNER_NODE"\n', { mode: 0o700 });
fs.writeFileSync(fakeRunnerNode, String.raw`
import fs from "node:fs";
import path from "node:path";
const side = process.env.GENERATION_REAL_RUNTIME_ROLE.endsWith("control") ? "control" : "candidate";
const caseId = process.env.GENERATION_REAL_CASE_IDS;
const docs = caseId === "agent-cloud-quickstart";
const expectedDraftAcceptance = docs ? "0" : "1";
if (process.env.GENERATION_REAL_DRAFT_PREVIEW_ACCEPTANCE !== expectedDraftAcceptance) {
  throw new Error("paired acceptance mode does not match the fixture template lifecycle");
}
if (
  process.env.GENERATION_COHORT_EXPECT_KEEP_SANDBOX === "1" &&
  process.env.GENERATION_REAL_KEEP_SANDBOX !== "true"
) {
  throw new Error("Runtime Restart pair must preserve the Sandbox until restart evidence completes");
}
const projectId = side + "-project";
const run = (phase, suffix) => ({
  runId: side + "-" + suffix,
  phase,
  status: "completed",
  summary: "done",
  usage: { inputTokens: 100, outputTokens: 10, cachedInputTokens: 0, totalTokens: 110 },
  eventStream: { sha256: side === "control" ? "${HASH_A}" : "${HASH_C}" },
  modelExecutions: [{
    modelResourceId: "deepseek-v4-pro",
    modelResourceRevision: 4,
    physicalModel: "deepseek-v4-pro",
    capabilitySnapshotHash: "${HASH_C}",
  }],
  efficiency: {
    schemaVersion: "run-efficiency-metrics@1",
    calculatorVersion: "run-efficiency-calculator@1",
    runId: side + "-" + suffix,
    projectId,
    phase,
    model: "resource:deepseek-v4-pro",
    template: docs ? "fumadocs-docs@runtime-p6" : "next-app@2",
    status: "completed",
    inputTokens: side === "control" ? 1000 : 500,
    duplicateReadEstimatedTokens: side === "control" ? 100 : 20,
    timeToFirstGreenfieldStaticBuildMs: phase === "build" ? (side === "control" ? 100000 : 50000) : null,
    timeToIframeAppliedMs: phase === "edit" ? (side === "control" ? 30000 : 12000) : null,
    modelTurnAtFirstSourceMutation: 1,
    prebuildFsListCount: 0,
    prebuildFsSearchCount: 0,
    duplicateFullReadRateBasisPoints: 0,
    outOfScopeMutationCount: 0,
    firstBuildSucceeded: true,
    requiredFidelityPassed: true,
  },
});
const evidence = {
  schemaVersion: "generation-real-provider-case-evidence@2",
  id: caseId,
  kind: docs ? "docs" : "website",
  projectId,
  expectedRoute: docs ? "/docs/" : "/",
  expectedText: "frozen acceptance marker",
  status: "accepted",
  finishedAt: "2026-07-20T00:03:00.000Z",
  contentPlan: { fixtureId: caseId, intentSha256: "${HASH_A}" },
  acceptance: { sha256: "${HASH_B}" },
  artifact: { bodySha256: "${HASH_C}" },
  draftPreview: null,
  runs: [run("build", "build")],
  warmEdit: null,
  coldDevEdit: null,
  repair: null,
};
if (process.env.GENERATION_REAL_DRAFT_WARM_EDIT_CANARY === "1") {
  const marker = process.env.GENERATION_REAL_WARM_EDIT_MARKER;
  const kind = process.env.GENERATION_REAL_DRAFT_WARM_EDIT_KIND;
  evidence.warmEdit = {
    schemaVersion: "generation-real-provider-edit-evidence@1",
    startedAt: "2026-07-20T00:01:00.000Z",
    finishedAt: "2026-07-20T00:02:00.000Z",
    status: "accepted",
    projectId,
    prompt: kind + " exact paired prompt " + marker,
    warmEditKind: kind,
    editImpactPlanHash: "${HASH_B}",
    run: run("edit", "edit"),
    draftPreview: { expectedText: marker, expectedTextFound: true },
  };
  if (process.env.GENERATION_REAL_MULTIMODAL_REFERENCE === "true") {
    if (
      side === "candidate" &&
      process.env.GENERATION_REAL_EXPECT_MULTIMODAL_DELIVERED !== "true"
    ) {
      throw new Error("candidate multimodal canary must require delivered evidence");
    }
    if (
      side === "control" &&
      process.env.GENERATION_REAL_EXPECT_MULTIMODAL_DELIVERED === "true"
    ) {
      throw new Error("control must not assert candidate multimodal coverage");
    }
    evidence.warmEdit.visualDelivery = {
      state: side === "candidate" ? "delivered" : "not_applicable",
      visualBindingsVerified: side === "candidate",
      visualBindingSetHash: side === "candidate" ? "${HASH_A}" : null,
      runtimeAttestationHash: side === "candidate" ? "${HASH_B}" : null,
      bindingVerificationSource:
        "frozen-runtime-attestation-plus-gateway-visual-input-attestation",
      unavailableMetricRecorded: false,
      gatewayVisualInputAttested: side === "candidate",
      gatewayAcceptedImageCount: side === "candidate" ? 1 : 0,
      mainTaskCompleted: true,
      providerModelResourceId: "deepseek-v4-pro",
      providerVisionCapable: true,
      referenceArtifactId: side + "-visual-reference",
      referenceArtifactSha256: "${HASH_C}",
      referenceMediaType: "image/png",
    };
    if (side === "candidate") {
      evidence.warmEdit.run.modelExecutions[0].visualInput = {
        state: "verified_and_provider_accepted",
        imageCount: 1,
        artifactSha256s: ["${HASH_C}"],
        mediaTypes: ["image/png"],
      };
    }
  }
}
if (process.env.GENERATION_REAL_DRAFT_COLD_DEV_EDIT_CANARY === "1") {
  const marker = process.env.GENERATION_REAL_WARM_EDIT_MARKER;
  const editRun = run("edit", "cold-dev-edit");
  editRun.efficiency.timeToIframeAppliedMs = null;
  editRun.efficiency.coldDevReadyMs = side === "control" ? 25000 : 14000;
  editRun.efficiency.timeToDurableSnapshotMs = side === "control" ? 20000 : 10000;
  evidence.coldDevEdit = {
    schemaVersion: "generation-real-provider-edit-evidence@1",
    startedAt: "2026-07-20T00:01:00.000Z",
    finishedAt: "2026-07-20T00:02:00.000Z",
    status: "accepted",
    projectId,
    prompt: "cold_dev exact paired prompt " + marker,
    lifecycleProfile: "cold_dev",
    editImpactPlanHash: "${HASH_B}",
    run: editRun,
    draftPreview: { expectedText: marker, expectedTextFound: true },
  };
}
if (process.env.GENERATION_REAL_REPAIR_CANARY === "1") {
  const marker = process.env.GENERATION_REAL_REPAIR_MARKER;
  const repairRun = run("repair", "repair");
  repairRun.efficiency.timeToIframeAppliedMs = null;
  repairRun.efficiency.timeToFirstSourceMutationMs = side === "control" ? 30000 : 18000;
  evidence.repair = {
    schemaVersion: "generation-real-provider-repair-evidence@1",
    startedAt: "2026-07-20T00:01:00.000Z",
    finishedAt: "2026-07-20T00:02:00.000Z",
    status: "accepted",
    projectId,
    prompt: "repair exact paired prompt " + marker,
    lifecycleProfile: "repair_warm",
    repairMarker: marker,
    run: repairRun,
    repairVerification: {
      freshVersionCreated: true,
      sourceMutationRecorded: true,
      previewPublishRecorded: true,
      markerPreserved: true,
    },
  };
}
const directory = path.join(process.env.GENERATION_REAL_EVIDENCE_DIR, "suite-offline-accepted");
fs.mkdirSync(directory, { recursive: true });
fs.writeFileSync(path.join(directory, "real-provider-case-" + caseId + ".json"), JSON.stringify(evidence));
`);
fs.writeFileSync(fakeRestartShell, '#!/usr/bin/env bash\nexec node "$GENERATION_COHORT_FAKE_RESTART_NODE" "$@"\n', { mode: 0o700 });
fs.writeFileSync(fakeRestartNode, String.raw`
import fs from "node:fs";
import { pathToFileURL } from "node:url";
const [sessionDir, side, deployment, caseFile, outputFile] = process.argv.slice(2);
const { createRuntimeRestartEvidence } = await import(pathToFileURL(process.env.GENERATION_COHORT_RESTART_EVIDENCE_MODULE));
const session = JSON.parse(fs.readFileSync(sessionDir + "/session.json", "utf8"));
const evidence = JSON.parse(fs.readFileSync(caseFile, "utf8"));
const run = evidence.runs.find(item => item.phase === "build");
const generationContextStatus = side === "candidate" ? {
  schemaVersion: "generation-context-status@1",
  runId: run.runId,
  runContractVersion: "generation-context@1",
  status: "compiled",
  runtimeMode: "enabled",
  contextContentHash: "${HASH_A}",
  runContextBindingHash: "${HASH_B}",
  runtimeAttestationHash: "${HASH_C}",
  workflowState: "completed",
} : {
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
  projectId: evidence.projectId,
  runId: run.runId,
  healthReady: true,
  generationContextStatus,
  efficiency: run.efficiency,
  projectState: {
    currentVersionId: side + "-version",
    sandboxBindingId: side + "-sandbox",
    sourceSnapshotRefSha256: "${HASH_A}",
    templateKey: "fumadocs-docs@runtime-p6",
    styleContractSha256: "${HASH_B}",
    latestBuildSha256: "${HASH_C}",
    dependencyStateSha256: null,
    previewSha256: "${HASH_A}",
  },
  history: { itemCount: 1, sha256: "${HASH_B}" },
  releaseEvidence: {
    httpStatus: 409,
    available: false,
    canonicalResponseSha256: "${HASH_C}",
    stableStateSha256: "${HASH_C}",
  },
  artifact: {
    httpStatus: 200,
    markerFound: true,
    markerSha256: "${HASH_A}",
    bodySha256: "${HASH_B}",
    bodyBytes: 1000,
  },
};
const revision = session.runtimes[side].deploymentRevision;
const result = createRuntimeRestartEvidence({
  recordedAt: "2026-07-22T00:01:00.000Z",
  side,
  deployment,
  runtimeDeploymentRevision: revision,
  deploymentUid: side + "-deployment-uid",
  deploymentGeneration: 1,
  deploymentTemplateSha256: "${HASH_A}",
  podBefore: { name: side + "-before", uid: side + "-before-uid" },
  deploymentUidAfter: side + "-deployment-uid",
  deploymentGenerationAfter: 1,
  deploymentTemplateSha256After: "${HASH_A}",
  podAfter: { name: side + "-after", uid: side + "-after-uid" },
  restartDurationMs: 1000,
}, snapshot, { ...structuredClone(snapshot), recordedAt: "2026-07-22T00:01:00.000Z" });
fs.writeFileSync(outputFile, JSON.stringify(result));
`);

const script = path.join(root, "infra/generation-reliability/run-generation-context-paired-pair.sh");
const coldDevScript = path.join(
  root,
  "infra/generation-reliability/run-generation-context-cold-dev-pair.sh",
);
const repairScript = path.join(
  root,
  "infra/generation-reliability/run-generation-context-repair-pair.sh",
);
const runtimeRestartScript = path.join(
  root,
  "infra/generation-reliability/run-generation-context-runtime-restart-pair.sh",
);
const runPair = (batch, bucket) => execFileSync(
  "bash",
  [script, sessionDir, batch, "zenova-agent-cloud", bucket],
  {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_COHORT_REAL_PROVIDER_RUNNER: fakeRunnerShell,
      GENERATION_COHORT_FAKE_RUNNER_NODE: fakeRunnerNode,
    },
    encoding: "utf8",
  },
);

const runPairInSession = (targetSessionDir, batch, bucket, extraEnv = {}) => execFileSync(
  "bash",
  [script, targetSessionDir, batch, "zenova-agent-cloud", bucket],
  {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_COHORT_REAL_PROVIDER_RUNNER: fakeRunnerShell,
      GENERATION_COHORT_FAKE_RUNNER_NODE: fakeRunnerNode,
      ...extraEnv,
    },
    encoding: "utf8",
  },
);

const invalidatedSessionDir = path.join(temp, "invalidated-session");
fs.mkdirSync(invalidatedSessionDir, { recursive: true });
for (const file of ["session.json", "session-meta.json", "cohort.ndjson"]) {
  fs.copyFileSync(path.join(sessionDir, file), path.join(invalidatedSessionDir, file));
}
fs.writeFileSync(
  path.join(invalidatedSessionDir, "session-invalidation.json"),
  `${JSON.stringify({
    schemaVersion: "generation-context-cohort-session-invalidation@1",
    sessionId: session.sessionId,
    invalidatedAt: "2026-07-22T00:00:00.000Z",
    reason: "collector_fidelity_mapping_invalid",
  }, null, 2)}\n`,
);
const rejectedInvalidatedSession = spawnSync(
  "bash",
  [script, invalidatedSessionDir, "batch-after-invalidation", "zenova-agent-cloud", "greenfield"],
  {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_COHORT_REAL_PROVIDER_RUNNER: fakeRunnerShell,
      GENERATION_COHORT_FAKE_RUNNER_NODE: fakeRunnerNode,
    },
    encoding: "utf8",
  },
);
assert.equal(rejectedInvalidatedSession.status, 2);
assert.match(rejectedInvalidatedSession.stderr, /session_invalidated/);
assert.equal(
  fs.existsSync(path.join(invalidatedSessionDir, "pairs", "batch-after-invalidation-zenova-agent-cloud-greenfield")),
  false,
);

const runDocsPair = (batch, bucket) => execFileSync(
  "bash",
  [script, sessionDir, batch, "agent-cloud-quickstart", bucket],
  {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_COHORT_REAL_PROVIDER_RUNNER: fakeRunnerShell,
      GENERATION_COHORT_FAKE_RUNNER_NODE: fakeRunnerNode,
    },
    encoding: "utf8",
  },
);

const runColdDevPair = (batch) => execFileSync(
  "bash",
  [coldDevScript, sessionDir, batch, "zenova-agent-cloud"],
  {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_COHORT_REAL_PROVIDER_RUNNER: fakeRunnerShell,
      GENERATION_COHORT_FAKE_RUNNER_NODE: fakeRunnerNode,
    },
    encoding: "utf8",
  },
);

const runRepairPair = (batch) => execFileSync(
  "bash",
  [repairScript, sessionDir, batch, "agent-cloud-quickstart"],
  {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_COHORT_REAL_PROVIDER_RUNNER: fakeRunnerShell,
      GENERATION_COHORT_FAKE_RUNNER_NODE: fakeRunnerNode,
    },
    encoding: "utf8",
  },
);

const runRuntimeRestartPair = (batch, caseId = "agent-cloud-quickstart") => execFileSync(
  "bash",
  [runtimeRestartScript, sessionDir, batch, caseId],
  {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_COHORT_REAL_PROVIDER_RUNNER: fakeRunnerShell,
      GENERATION_COHORT_FAKE_RUNNER_NODE: fakeRunnerNode,
      GENERATION_COHORT_RUNTIME_RESTART_RUNNER: fakeRestartShell,
      GENERATION_COHORT_EXPECT_KEEP_SANDBOX: "1",
      GENERATION_COHORT_FAKE_RESTART_NODE: fakeRestartNode,
      GENERATION_COHORT_RESTART_EVIDENCE_MODULE: path.join(
        root,
        "services/runtime/scripts/generation-context-runtime-restart-evidence.mjs",
      ),
    },
    encoding: "utf8",
  },
);

assert.match(runPair("batch-greenfield", "greenfield"), /paired sample appended/);
let verification = verifyPairedCohortLedger(ledger);
assert.equal(verification.completePairCount, 1);
assert.equal(verification.sampleCount, 2);

assert.match(runPair("batch-warm", "warm_copy_css"), /paired sample appended/);
verification = verifyPairedCohortLedger(ledger);
assert.equal(verification.completePairCount, 2);
assert.equal(verification.sampleCount, 4);
assert.deepEqual(verification.pendingPairs, []);

const warmSpec = JSON.parse(fs.readFileSync(
  path.join(
    sessionDir,
    "pairs",
    "batch-warm-zenova-agent-cloud-warm-copy-css",
    "pair-spec.json",
  ),
  "utf8",
));
assert.equal(warmSpec.control.source, "warmEdit");
assert.equal(warmSpec.candidate.phase, "edit");

assert.match(runColdDevPair("batch-cold-dev"), /paired sample appended/);
verification = verifyPairedCohortLedger(ledger);
assert.equal(verification.completePairCount, 3);
assert.equal(verification.sampleCount, 6);
const coldDevSpec = JSON.parse(fs.readFileSync(
  path.join(
    sessionDir,
    "pairs",
    "batch-cold-dev-zenova-agent-cloud-cold-dev",
    "pair-spec.json",
  ),
  "utf8",
));
assert.equal(coldDevSpec.bucket, "cold_dev");
assert.equal(coldDevSpec.control.source, "coldDevEdit");
assert.equal(coldDevSpec.candidate.phase, "edit");

assert.match(runDocsPair("batch-docs", "greenfield"), /paired sample appended/);
verification = verifyPairedCohortLedger(ledger);
assert.equal(verification.completePairCount, 4);
assert.equal(verification.sampleCount, 8);
const docsSpec = JSON.parse(fs.readFileSync(
  path.join(
    sessionDir,
    "pairs",
    "batch-docs-agent-cloud-quickstart-greenfield",
    "pair-spec.json",
  ),
  "utf8",
));
assert.deepEqual(docsSpec.coverage, ["fumadocsTemplate"]);

assert.match(runRepairPair("batch-repair"), /paired sample appended/);
verification = verifyPairedCohortLedger(ledger);
assert.equal(verification.completePairCount, 5);
assert.equal(verification.sampleCount, 10);
const repairSpec = JSON.parse(fs.readFileSync(
  path.join(
    sessionDir,
    "pairs",
    "batch-repair-agent-cloud-quickstart-repair",
    "pair-spec.json",
  ),
  "utf8",
));
assert.equal(repairSpec.bucket, "repair");
assert.equal(repairSpec.control.source, "repair");
assert.equal(repairSpec.candidate.phase, "repair");
assert.deepEqual(repairSpec.coverage, ["fumadocsTemplate"]);

assert.match(runRuntimeRestartPair("batch-restart"), /paired sample appended/);
verification = verifyPairedCohortLedger(ledger);
assert.equal(verification.completePairCount, 6);
assert.equal(verification.sampleCount, 12);
const restartSpec = JSON.parse(fs.readFileSync(
  path.join(
    sessionDir,
    "pairs",
    "batch-restart-agent-cloud-quickstart-greenfield",
    "pair-spec.json",
  ),
  "utf8",
));
assert.deepEqual(restartSpec.coverage, ["fumadocsTemplate", "runtimeRestart"]);
assert.equal(restartSpec.control.restartEvidenceFile, "control/runtime-restart-evidence.json");
assert.equal(restartSpec.candidate.restartEvidenceFile, "candidate/runtime-restart-evidence.json");

assert.match(
  runRuntimeRestartPair("batch-restart-next", "zenova-agent-cloud"),
  /paired sample appended/,
);
verification = verifyPairedCohortLedger(ledger);
assert.equal(verification.completePairCount, 7);
assert.equal(verification.sampleCount, 14);
const nextRestartSpec = JSON.parse(fs.readFileSync(
  path.join(
    sessionDir,
    "pairs",
    "batch-restart-next-zenova-agent-cloud-greenfield",
    "pair-spec.json",
  ),
  "utf8",
));
assert.deepEqual(nextRestartSpec.coverage, ["nextTemplate", "runtimeRestart"]);

const rejectedMultimodal = spawnSync(
  "bash",
  [script, sessionDir, "batch-invalid-multimodal", "zenova-agent-cloud", "warm_copy_css"],
  {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_COHORT_REAL_PROVIDER_RUNNER: fakeRunnerShell,
      GENERATION_COHORT_FAKE_RUNNER_NODE: fakeRunnerNode,
      GENERATION_COHORT_MULTIMODAL_REFERENCE: "1",
    },
    encoding: "utf8",
  },
);
assert.notEqual(rejectedMultimodal.status, 0);
assert.match(
  rejectedMultimodal.stderr,
  /requires a frozen bounded PNG-capable Provider Resource/,
);

const visionSessionDir = path.join(temp, "vision-session");
fs.mkdirSync(visionSessionDir, { recursive: true });
const visionSession = structuredClone(session);
visionSession.sessionId = "offline-multimodal-pair-runner-test";
Object.assign(visionSession.providers[0], {
  visionCapable: true,
  supportedImageMediaTypes: ["image/png"],
  maxImageCount: 4,
});
fs.writeFileSync(
  path.join(visionSessionDir, "session.json"),
  `${JSON.stringify(visionSession, null, 2)}\n`,
);
fs.writeFileSync(
  path.join(visionSessionDir, "session-meta.json"),
  `${JSON.stringify({
    schemaVersion: "generation-context-cohort-session-meta@1",
    sessionId: visionSession.sessionId,
    context: "k3d-offline-test",
    workspaceNamespace: "ws-offline-test",
    deployments: [
      { side: "control", deployment: "runtime-control" },
      { side: "candidate", deployment: "runtime-candidate" },
    ],
  }, null, 2)}\n`,
);
const visionLedger = path.join(visionSessionDir, "cohort.ndjson");
initializePairedCohortLedger(visionLedger, visionSession);
assert.match(
  runPairInSession(
    visionSessionDir,
    "batch-multimodal",
    "warm_copy_css",
    { GENERATION_COHORT_MULTIMODAL_REFERENCE: "1" },
  ),
  /paired sample appended/,
);
const multimodalSpec = JSON.parse(fs.readFileSync(
  path.join(
    visionSessionDir,
    "pairs",
    "batch-multimodal-zenova-agent-cloud-warm-copy-css",
    "pair-spec.json",
  ),
  "utf8",
));
assert.deepEqual(multimodalSpec.coverage, [
  "nextTemplate",
  "multimodalVisualDelivered",
]);
assert.equal(verifyPairedCohortLedger(visionLedger).completePairCount, 1);

assert.throws(
  () => runDocsPair("batch-docs-warm", "warm_copy_css"),
  (error) => {
    assert.match(error.stderr.toString(), /draft_lifecycle_template_unsupported/);
    return true;
  },
);
assert.equal(
  fs.existsSync(path.join(
    sessionDir,
    "pairs",
    "batch-docs-warm-agent-cloud-quickstart-warm-copy-css",
  )),
  false,
);

assert.throws(
  () => runPair("batch-website-repair", "repair"),
  (error) => {
    assert.match(error.stderr.toString(), /repair_template_unsupported/);
    return true;
  },
);

fs.rmSync(temp, { recursive: true, force: true });
process.stdout.write("Generation Context paired-pair runner tests passed.\n");
