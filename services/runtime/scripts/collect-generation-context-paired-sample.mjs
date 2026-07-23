#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { createPairedCohortSample } from "./create-generation-context-paired-sample.mjs";
import { appendPairedCohortPair } from "./generation-context-paired-cohort-ledger.mjs";
import { validateRuntimeRestartEvidence } from "./generation-context-runtime-restart-evidence.mjs";

const SPEC_SCHEMA = "generation-context-real-provider-pair-spec@1";
const HASH = /^[a-f0-9]{64}$/;

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function unique(values, label) {
  const present = [...new Set(values.filter(hasText))];
  if (present.length !== 1) throw new Error(`${label} must have one frozen value`);
  return present[0];
}

function selectedRun(caseEvidence, selection, side) {
  if (caseEvidence?.schemaVersion !== "generation-real-provider-case-evidence@2") {
    throw new Error(`${side} evidence must be generation-real-provider-case-evidence@2`);
  }
  const source = selection?.source || "runs";
  if (!new Set(["runs", "warmEdit", "coldDevEdit", "repair"]).has(source)) {
    throw new Error(
      `${side} selection.source must be runs, warmEdit, coldDevEdit, or repair`,
    );
  }
  if (source !== "runs") {
    const lifecycle = caseEvidence[source];
    const supportedSchemas = source === "repair"
      ? new Set([
          "generation-real-provider-repair-evidence@1",
          "generation-real-provider-repair-evidence@2",
        ])
      : new Set([
          "generation-real-provider-edit-evidence@1",
          "generation-real-provider-edit-evidence@2",
        ]);
    if (!supportedSchemas.has(lifecycle?.schemaVersion)) {
      throw new Error(`${side} evidence has no ${source} evidence`);
    }
    const run = lifecycle.run;
    if (!run || (selection.runId && run.runId !== selection.runId)) {
      throw new Error(
        `${side} evidence must contain exactly one selected lifecycle Run`,
      );
    }
    validateSelectedRun(caseEvidence, run, selection, side);
    return { run, evidence: lifecycle, source };
  }
  const runs = Array.isArray(caseEvidence.runs) ? caseEvidence.runs : [];
  const finalAttemptRunIds = new Set(
    Array.isArray(caseEvidence.attempts) && caseEvidence.attempts.length > 0
      ? caseEvidence.attempts.at(-1)?.runIds || []
      : [],
  );
  const eligibleRuns = selection.runId || finalAttemptRunIds.size === 0
    ? runs
    : runs.filter((run) => finalAttemptRunIds.has(run.runId));
  const matches = eligibleRuns.filter((run) =>
    selection.runId ? run.runId === selection.runId : run.phase === selection.phase,
  );
  if (matches.length !== 1) {
    throw new Error(`${side} evidence must contain exactly one selected Run`);
  }
  const run = matches[0];
  validateSelectedRun(caseEvidence, run, selection, side);
  return { run, evidence: caseEvidence, source };
}

function validateSelectedRun(caseEvidence, run, selection, side) {
  if (selection.phase && run.phase !== selection.phase) {
    throw new Error(`${side} selected Run phase mismatch`);
  }
  if (!run.efficiency) throw new Error(`${side} selected Run has no Runtime efficiency metrics`);
  if (run.efficiency.runId !== run.runId || run.efficiency.projectId !== caseEvidence.projectId) {
    throw new Error(`${side} Runtime efficiency identity mismatch`);
  }
  if (run.efficiency.phase !== run.phase) throw new Error(`${side} Runtime efficiency phase mismatch`);
  if (!HASH.test(run.eventStream?.sha256 || "")) {
    throw new Error(`${side} selected Run has no hashed event stream`);
  }
  if (run.promptEfficiency !== undefined && (
    run.promptEfficiency?.schemaVersion !== "run-prompt-efficiency@1" ||
    run.promptEfficiency.runId !== run.runId
  )) {
    throw new Error(`${side} Prompt efficiency identity mismatch`);
  }
  if (run.designProfileIdentity?.schemaVersion !== "run-design-profile-identity@1"
    || run.designProfileIdentity.runId !== run.runId
    || !HASH.test(run.designProfileIdentity.effectiveProfileHash || "")) {
    throw new Error(`${side} selected Run has no frozen Design Profile identity`);
  }
}

function executionIdentity(run, side) {
  const executions = Array.isArray(run.modelExecutions) ? run.modelExecutions : [];
  if (!executions.length) throw new Error(`${side} selected Run has no model execution evidence`);
  const modelResource = unique(executions.map((item) => item.modelResourceId), `${side} modelResourceId`);
  const modelVersion = unique(executions.map((item) => item.physicalModel), `${side} physicalModel`);
  const capabilitySnapshotHash = unique(
    executions.map((item) => item.capabilitySnapshotHash),
    `${side} capabilitySnapshotHash`,
  );
  const revisions = [...new Set(executions.map((item) => item.modelResourceRevision))];
  if (revisions.length !== 1 || !Number.isSafeInteger(revisions[0]) || revisions[0] <= 0) {
    throw new Error(`${side} modelResourceRevision must have one frozen positive value`);
  }
  if (!HASH.test(capabilitySnapshotHash)) throw new Error(`${side} capabilitySnapshotHash must be sha256`);
  return {
    modelResource,
    modelVersion,
    capabilitySnapshotHash,
    providerResourceRevision: revisions[0],
    modelExecutionEvidenceSha256: sha256(JSON.stringify(executions)),
  };
}

function terminalStatus(sourceEvidence, run) {
  if (run.status === "completed") return "completed";
  if (run.terminalClassification === "runtime_idle_timeout"
    || run.terminalClassification === "runtime_total_timeout") return "timeout";
  const message = `${run.summary || ""} ${sourceEvidence.error?.message || ""}`.toLowerCase();
  if (message.includes("timeout") || message.includes("watchdog")) return "timeout";
  if (message.includes("fallback")) return "fallback";
  return "failed";
}

function acceptancePassed(caseEvidence, selected) {
  if (selected.source !== "runs") {
    const lifecycleAccepted = selected.run.status === "completed"
      && selected.evidence.status === "accepted"
      && caseEvidence.status === "accepted";
    if (selected.source === "repair") {
      return lifecycleAccepted
        && selected.evidence.repairVerification?.freshVersionCreated === true
        && selected.evidence.repairVerification?.sourceMutationRecorded === true
        && selected.evidence.repairVerification?.previewPublishRecorded === true
        && selected.evidence.repairVerification?.markerPreserved === true;
    }
    return lifecycleAccepted
      && selected.evidence.draftPreview?.expectedTextFound === true;
  }
  return selected.run.status === "completed" && caseEvidence.status === "accepted";
}

function validatePairIntent(control, candidate, spec) {
  for (const [label, left, right] of [
    ["fixture id", control.id, candidate.id],
    ["content-plan fixture", control.contentPlan?.fixtureId, candidate.contentPlan?.fixtureId],
    ["intent hash", control.contentPlan?.intentSha256, candidate.contentPlan?.intentSha256],
    ["acceptance contract", control.acceptance?.sha256, candidate.acceptance?.sha256],
  ]) {
    if (!hasText(left) || left !== right) throw new Error(`paired ${label} mismatch`);
  }
  const controlSource = spec.control?.source || "runs";
  const candidateSource = spec.candidate?.source || "runs";
  if (controlSource !== candidateSource) {
    throw new Error("paired selection source mismatch");
  }
  if (controlSource !== "runs") {
    const controlLifecycle = control[controlSource];
    const candidateLifecycle = candidate[candidateSource];
    for (const [side, lifecycle] of [
      ["control", controlLifecycle],
      ["candidate", candidateLifecycle],
    ]) {
      if (!lifecycle || typeof lifecycle !== "object") {
        throw new Error(`paired lifecycle evidence missing: ${side}`);
      }
    }
    const promptHash = (lifecycle) => {
      if (HASH.test(lifecycle?.promptSha256 || "")) return lifecycle.promptSha256;
      if (lifecycle?.schemaVersion?.endsWith("@1") && hasText(lifecycle?.prompt)) {
        return sha256(lifecycle.prompt);
      }
      return null;
    };
    const controlPromptHash = promptHash(controlLifecycle);
    const candidatePromptHash = promptHash(candidateLifecycle);
    if (!controlPromptHash || controlPromptHash !== candidatePromptHash) {
      throw new Error("paired lifecycle prompt hash mismatch");
    }
    if (controlSource === "warmEdit") {
      if (
        !hasText(controlLifecycle.warmEditKind) ||
        controlLifecycle.warmEditKind !== candidateLifecycle.warmEditKind
      ) {
        throw new Error("paired Warm Edit kind mismatch");
      }
    } else if (controlSource === "coldDevEdit" && (
      controlLifecycle.lifecycleProfile !== "cold_dev" ||
      candidateLifecycle.lifecycleProfile !== "cold_dev"
    )) {
      throw new Error("paired Cold Dev Edit lifecycle profile mismatch");
    } else if (controlSource === "repair") {
      if (
        controlLifecycle.lifecycleProfile !== "repair_warm" ||
        candidateLifecycle.lifecycleProfile !== "repair_warm" ||
        !hasText(controlLifecycle.repairMarker) ||
        controlLifecycle.repairMarker !== candidateLifecycle.repairMarker
      ) {
        throw new Error("paired Repair lifecycle identity mismatch");
      }
      return;
    }
    const controlExpectedText = controlLifecycle.draftPreview?.expectedText;
    const candidateExpectedText = candidateLifecycle.draftPreview?.expectedText;
    if (hasText(controlExpectedText) && hasText(candidateExpectedText) && controlExpectedText !== candidateExpectedText) {
      throw new Error("paired lifecycle Edit expected text mismatch");
    }
    for (const [side, lifecycle, expectedText] of [
      ["control", controlLifecycle, controlExpectedText],
      ["candidate", candidateLifecycle, candidateExpectedText],
    ]) {
      if (lifecycle.status === "accepted" && !hasText(expectedText)) {
        throw new Error(
          `paired lifecycle Edit accepted evidence is missing expected text: ${side}`,
        );
      }
    }
  }
}

export function collectPairedSamples(
  session,
  spec,
  controlEvidence,
  candidateEvidence,
  restartEvidence = {},
) {
  if (spec?.schemaVersion !== SPEC_SCHEMA) throw new Error(`spec.schemaVersion must be ${SPEC_SCHEMA}`);
  for (const field of ["pairId", "batchId", "bucket"]) {
    if (!hasText(spec[field])) throw new Error(`spec.${field} is required`);
  }
  validatePairIntent(controlEvidence, candidateEvidence, spec);
  const controlSelected = selectedRun(controlEvidence, spec.control, "control");
  const candidateSelected = selectedRun(candidateEvidence, spec.candidate, "candidate");
  const controlRun = controlSelected.run;
  const candidateRun = candidateSelected.run;
  const restartCoverage = Array.isArray(spec.coverage)
    && spec.coverage.includes("runtimeRestart");
  if (restartCoverage) {
    for (const [side, caseEvidence, run] of [
      ["control", controlEvidence, controlRun],
      ["candidate", candidateEvidence, candidateRun],
    ]) {
      validateRuntimeRestartEvidence(restartEvidence[side], {
        side,
        projectId: caseEvidence.projectId,
        runId: run.runId,
        runtimeDeploymentRevision: session.runtimes?.[side]?.deploymentRevision,
      });
    }
  }
  const controlExecution = executionIdentity(controlRun, "control");
  const candidateExecution = executionIdentity(candidateRun, "candidate");
  for (const field of [
    "modelResource",
    "modelVersion",
    "capabilitySnapshotHash",
    "providerResourceRevision",
  ]) {
    if (controlExecution[field] !== candidateExecution[field]) {
      throw new Error(`paired execution identity mismatch: ${field}`);
    }
  }
  const provider = session.providers?.find(
    (item) => item.modelResourceId === controlExecution.modelResource,
  );
  if (
    !provider ||
    provider.modelVersion !== controlExecution.modelVersion ||
    provider.resourceRevision !== controlExecution.providerResourceRevision ||
    !HASH.test(provider.providerParametersHash || "")
  ) {
    throw new Error("paired execution does not match the frozen session Provider Resource");
  }
  const nonVisualCoverage = Array.isArray(spec.coverage)
    && spec.coverage.includes("nonVisualUnavailableMainTaskPassed");
  if (nonVisualCoverage) {
    const visual = candidateSelected.evidence?.visualDelivery;
    if (
      visual?.state !== "unavailable" ||
      visual?.visualBindingsVerified !== true ||
      !HASH.test(visual?.visualBindingSetHash || "") ||
      !HASH.test(visual?.runtimeAttestationHash || "") ||
      visual?.bindingVerificationSource !==
        "frozen-runtime-attestation-plus-unavailable-delivery-metric" ||
      visual?.unavailableMetricRecorded !== true ||
      visual?.mainTaskCompleted !== true ||
      visual?.providerVisionCapable !== false ||
      provider.visionCapable !== false ||
      visual?.providerModelResourceId !== candidateExecution.modelResource ||
      !hasText(visual?.referenceArtifactId) ||
      candidateRun.status !== "completed"
    ) {
      throw new Error(
        "candidate nonVisualUnavailableMainTaskPassed coverage is not proven",
      );
    }
  }
  const multimodalCoverage = Array.isArray(spec.coverage)
    && spec.coverage.includes("multimodalVisualDelivered");
  if (multimodalCoverage) {
    const visual = candidateSelected.evidence?.visualDelivery;
    const gatewayVisualExecution = candidateRun.modelExecutions?.find(
      (execution) =>
        execution.modelResourceId === candidateExecution.modelResource &&
        execution.visualInput?.state === "verified_and_provider_accepted" &&
        Number(execution.visualInput?.imageCount) >= 1 &&
        execution.visualInput?.artifactSha256s?.includes(
          visual?.referenceArtifactSha256,
        ) &&
        execution.visualInput?.mediaTypes?.includes(visual?.referenceMediaType),
    );
    if (
      visual?.state !== "delivered" ||
      visual?.visualBindingsVerified !== true ||
      !HASH.test(visual?.visualBindingSetHash || "") ||
      !HASH.test(visual?.runtimeAttestationHash || "") ||
      visual?.bindingVerificationSource !==
        "frozen-runtime-attestation-plus-gateway-visual-input-attestation" ||
      visual?.unavailableMetricRecorded !== false ||
      visual?.gatewayVisualInputAttested !== true ||
      !Number.isSafeInteger(visual?.gatewayAcceptedImageCount) ||
      visual.gatewayAcceptedImageCount < 1 ||
      visual?.mainTaskCompleted !== true ||
      visual?.providerVisionCapable !== true ||
      provider.visionCapable !== true ||
      !provider.supportedImageMediaTypes?.includes(visual?.referenceMediaType) ||
      visual?.providerModelResourceId !== candidateExecution.modelResource ||
      !hasText(visual?.referenceArtifactId) ||
      !HASH.test(visual?.referenceArtifactSha256 || "") ||
      !hasText(visual?.referenceMediaType) ||
      !gatewayVisualExecution ||
      candidateRun.status !== "completed"
    ) {
      throw new Error("candidate multimodalVisualDelivered coverage is not proven");
    }
  }
  const templateVersion = controlRun.efficiency.template;
  if (!hasText(templateVersion) || templateVersion !== candidateRun.efficiency.template) {
    throw new Error("paired template version mismatch");
  }
  if (controlRun.phase !== candidateRun.phase) throw new Error("paired phase mismatch");
  if (controlRun.designProfileIdentity.effectiveProfileHash
    !== candidateRun.designProfileIdentity.effectiveProfileHash) {
    throw new Error("paired Design Profile identity mismatch");
  }
  const identity = {
    fixtureId: controlEvidence.id,
    modelResource: controlExecution.modelResource,
    providerResourceRevision: controlExecution.providerResourceRevision,
    modelVersion: controlExecution.modelVersion,
    providerParametersHash: provider.providerParametersHash,
    templateVersion,
    designProfileHash: controlRun.designProfileIdentity.effectiveProfileHash,
    capabilitySnapshotHash: controlExecution.capabilitySnapshotHash,
    phase: controlRun.phase,
  };
  const build = (side, caseEvidence, selected, execution) => {
    const run = selected.run;
    const passed =
      acceptancePassed(caseEvidence, selected) &&
      run.efficiency.requiredFidelityPassed !== false;
    return createPairedCohortSample(
      {
        schemaVersion: "generation-context-paired-cohort-sample-metadata@1",
        pairId: spec.pairId,
        batchId: spec.batchId,
        bucket: spec.bucket,
        side,
        status: terminalStatus(selected.evidence, run),
        recordedAt: selected.evidence.finishedAt || caseEvidence.finishedAt,
        identity,
        execution: {
          gatewayMode: "internal_gateway",
          modelResourceId: execution.modelResource,
          providerResourceRevision: execution.providerResourceRevision,
          modelExecutionEvidenceSha256: execution.modelExecutionEvidenceSha256,
        },
        source: {
          storageRef: `evidence://sha256/${run.eventStream.sha256}`,
          contentSha256: run.eventStream.sha256,
        },
        acceptanceEvidenceSha256: sha256(JSON.stringify({
          acceptance: selected.source !== "runs"
            ? {
                lifecycleProfile: selected.evidence.lifecycleProfile,
                warmEditKind: selected.evidence.warmEditKind,
                repairMarker: selected.evidence.repairMarker,
                promptSha256: HASH.test(selected.evidence.promptSha256 || "")
                  ? selected.evidence.promptSha256
                  : sha256(selected.evidence.prompt),
                editImpactPlanHash: selected.evidence.editImpactPlanHash,
                draftPreview: selected.evidence.draftPreview,
                repairVerification: selected.evidence.repairVerification,
                visualDelivery: selected.evidence.visualDelivery,
                status: selected.evidence.status,
                baseCase: {
                  contract: caseEvidence.acceptance,
                  artifact: caseEvidence.artifact,
                  draftPreview: caseEvidence.draftPreview,
                  status: caseEvidence.status,
                  errorClassification: caseEvidence.error?.classification || null,
                },
              }
            : {
                contract: caseEvidence.acceptance,
                artifact: caseEvidence.artifact,
                draftPreview: caseEvidence.draftPreview,
                status: caseEvidence.status,
              },
          runtimeRestartEvidenceSha256: restartCoverage
            ? restartEvidence[side].evidenceSha256
            : null,
        })),
        caseAttemptCount: Array.isArray(caseEvidence.attempts)
          ? caseEvidence.attempts.length
          : 1,
        requiredFidelityPassed: passed,
        coverage: side === "candidate" ? spec.coverage || [] : [],
      },
      { ...run.efficiency, requiredFidelityPassed: passed },
      run.promptEfficiency ?? null,
    );
  };
  return {
    control: build("control", controlEvidence, controlSelected, controlExecution),
    candidate: build("candidate", candidateEvidence, candidateSelected, candidateExecution),
  };
}

function main() {
  const [sessionFile, ledgerFile, specFile] = process.argv.slice(2);
  if (!sessionFile || !ledgerFile || !specFile) {
    throw new Error("usage: collect-generation-context-paired-sample.mjs <session.json> <ledger.ndjson> <pair-spec.json>");
  }
  const session = readJson(sessionFile);
  const spec = readJson(specFile);
  const controlFile = path.resolve(path.dirname(specFile), spec.control?.evidenceFile || "");
  const candidateFile = path.resolve(path.dirname(specFile), spec.candidate?.evidenceFile || "");
  if (!spec.control?.evidenceFile || !spec.candidate?.evidenceFile) {
    throw new Error("pair spec must name control.evidenceFile and candidate.evidenceFile");
  }
  const restartEvidence = {};
  if (spec.coverage?.includes("runtimeRestart")) {
    for (const side of ["control", "candidate"]) {
      const restartFile = spec[side]?.restartEvidenceFile;
      if (!hasText(restartFile)) {
        throw new Error(`pair spec must name ${side}.restartEvidenceFile for runtimeRestart coverage`);
      }
      restartEvidence[side] = readJson(path.resolve(path.dirname(specFile), restartFile));
    }
  }
  const samples = collectPairedSamples(
    session,
    spec,
    readJson(controlFile),
    readJson(candidateFile),
    restartEvidence,
  );
  appendPairedCohortPair(ledgerFile, samples.control, samples.candidate);
  process.stdout.write(`${JSON.stringify({ pairId: spec.pairId, appended: ["control", "candidate"] }, null, 2)}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error?.stack || error}\n`);
    process.exitCode = 1;
  }
}
