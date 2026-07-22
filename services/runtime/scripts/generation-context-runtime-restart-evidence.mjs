#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const SNAPSHOT_SCHEMA = "generation-context-runtime-restart-snapshot@1";
const EVIDENCE_SCHEMA = "generation-context-runtime-restart-evidence@1";
const HASH = /^[a-f0-9]{64}$/;
const ISO_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/;
const SENSITIVE_VALUE = /(?:\bBearer\s+[A-Za-z0-9._~+/=-]{8,}|\bsk-[A-Za-z0-9_-]{8,}|data:image\/)/i;

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function positiveInteger(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function rejectSensitiveValues(value, location = "restartEvidence") {
  if (typeof value === "string") {
    if (SENSITIVE_VALUE.test(value)) {
      throw new Error(`${location} contains credential or payload material`);
    }
    return;
  }
  if (Array.isArray(value)) {
    value.forEach((entry, index) => rejectSensitiveValues(entry, `${location}[${index}]`));
    return;
  }
  if (!value || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    rejectSensitiveValues(child, `${location}.${key}`);
  }
}

function validateSnapshot(snapshot, label, side) {
  if (snapshot?.schemaVersion !== SNAPSHOT_SCHEMA) {
    throw new Error(`${label}.schemaVersion must be ${SNAPSHOT_SCHEMA}`);
  }
  if (!ISO_TIMESTAMP.test(snapshot.recordedAt || "")) {
    throw new Error(`${label}.recordedAt must be an ISO UTC timestamp`);
  }
  for (const field of ["projectId", "runId"]) {
    if (!hasText(snapshot[field])) throw new Error(`${label}.${field} is required`);
  }
  if (snapshot.healthReady !== true) throw new Error(`${label}.healthReady must be true`);
  if (snapshot.generationContextStatus?.runId !== snapshot.runId) {
    throw new Error(`${label} Generation Context Run identity mismatch`);
  }
  const status = snapshot.generationContextStatus;
  if (side === "candidate") {
    if (status.runContractVersion !== "generation-context@1"
      || status.status !== "compiled"
      || status.runtimeMode !== "enabled") {
      throw new Error(`${label} candidate Generation Context was not compiled in enabled mode`);
    }
    for (const field of ["contextContentHash", "runContextBindingHash", "runtimeAttestationHash"]) {
      if (!HASH.test(status[field] || "")) {
        throw new Error(`${label}.generationContextStatus.${field} must be sha256`);
      }
    }
  } else if (side === "control") {
    if (status.runContractVersion !== "legacy@1"
      || status.status !== "not_compiled"
      || status.runtimeMode !== null
      || status.contextContentHash !== null
      || status.runContextBindingHash !== null) {
      throw new Error(`${label} control must preserve the legacy non-compiled contract`);
    }
  } else {
    throw new Error(`unsupported restart evidence side: ${side}`);
  }
  if (snapshot.efficiency?.schemaVersion !== "run-efficiency-metrics@1"
    || snapshot.efficiency.calculatorVersion !== "run-efficiency-calculator@1"
    || snapshot.efficiency.runId !== snapshot.runId
    || snapshot.efficiency.projectId !== snapshot.projectId
    || snapshot.efficiency.status !== "completed") {
    throw new Error(`${label} Run efficiency evidence is incomplete or mismatched`);
  }
  const projectState = snapshot.projectState;
  const stateKind = projectState?.stateKind || "published_version";
  for (const field of ["sourceSnapshotRefSha256", "templateKey"]) {
    if (!hasText(projectState?.[field])) throw new Error(`${label}.projectState.${field} is required`);
  }
  if (stateKind === "published_version") {
    for (const field of ["currentVersionId", "sandboxBindingId"]) {
      if (!hasText(projectState?.[field])) {
        throw new Error(`${label}.projectState.${field} is required for a published Version`);
      }
    }
  } else if (stateKind === "durable_draft") {
    if (projectState.currentVersionId !== null || projectState.sandboxBindingId !== null
      || !hasText(projectState.draftSnapshotId) || !hasText(projectState.previewLeaseId)) {
      throw new Error(`${label}.projectState durable Draft identity is incomplete`);
    }
  } else {
    throw new Error(`${label}.projectState.stateKind is unsupported`);
  }
  if (!HASH.test(projectState.sourceSnapshotRefSha256)) {
    throw new Error(`${label}.projectState.sourceSnapshotRefSha256 must be sha256`);
  }
  for (const field of ["styleContractSha256", "latestBuildSha256", "dependencyStateSha256", "previewSha256"]) {
    if (projectState[field] !== null && !HASH.test(projectState[field] || "")) {
      throw new Error(`${label}.projectState.${field} must be null or sha256`);
    }
  }
  if (!Number.isSafeInteger(snapshot.history?.itemCount) || snapshot.history.itemCount < 1
    || !HASH.test(snapshot.history?.sha256 || "")) {
    throw new Error(`${label}.history must contain a non-empty hashed project history`);
  }
  if (!HASH.test(snapshot.releaseEvidence?.canonicalResponseSha256 || "")
    || !HASH.test(snapshot.releaseEvidence?.stableStateSha256 || "")
    || typeof snapshot.releaseEvidence?.available !== "boolean"
    || ![200, 404, 409].includes(snapshot.releaseEvidence?.httpStatus)) {
    throw new Error(`${label}.releaseEvidence must contain a supported hashed response`);
  }
  const artifactSource = snapshot.artifact?.source || "current_version";
  if (!new Set(["current_version", "draft_preview"]).has(artifactSource)
    || (stateKind === "durable_draft") !== (artifactSource === "draft_preview")
    || snapshot.artifact?.httpStatus !== 200
    || snapshot.artifact.markerFound !== true
    || !HASH.test(snapshot.artifact.markerSha256 || "")
    || !HASH.test(snapshot.artifact.bodySha256 || "")
    || !positiveInteger(snapshot.artifact.bodyBytes)) {
    throw new Error(`${label}.artifact must prove the frozen acceptance marker after HTTP 200`);
  }
}

function stableArtifact(artifact) {
  if (artifact?.source !== "draft_preview") return artifact;
  return {
    source: artifact.source,
    httpStatus: artifact.httpStatus,
    markerFound: artifact.markerFound,
    markerSha256: artifact.markerSha256,
  };
}

function stableSnapshot(snapshot) {
  const { recordedAt: _recordedAt, ...stable } = snapshot;
  return {
    ...stable,
    releaseEvidence: {
      httpStatus: stable.releaseEvidence.httpStatus,
      available: stable.releaseEvidence.available,
      stableStateSha256: stable.releaseEvidence.stableStateSha256,
    },
    artifact: stableArtifact(stable.artifact),
  };
}

export function createRuntimeRestartEvidence(metadata, before, after) {
  const evidence = {
    schemaVersion: EVIDENCE_SCHEMA,
    recordedAt: metadata.recordedAt,
    side: metadata.side,
    deployment: metadata.deployment,
    runtimeDeploymentRevision: metadata.runtimeDeploymentRevision,
    deploymentUid: metadata.deploymentUid,
    deploymentGeneration: metadata.deploymentGeneration,
    deploymentTemplateSha256: metadata.deploymentTemplateSha256,
    restartDurationMs: metadata.restartDurationMs,
    podBefore: metadata.podBefore,
    podAfter: metadata.podAfter,
    before,
    after,
    verification: {
      podUidChanged: metadata.podBefore?.uid !== metadata.podAfter?.uid,
      deploymentIdentityPreserved: metadata.deploymentUid === metadata.deploymentUidAfter
        && metadata.deploymentGeneration === metadata.deploymentGenerationAfter
        && metadata.deploymentTemplateSha256 === metadata.deploymentTemplateSha256After,
      runtimeReadyAfterRestart: after?.healthReady === true,
      generationContextIdentityPreserved:
        canonical(before?.generationContextStatus) === canonical(after?.generationContextStatus),
      workflowStatePreserved:
        before?.generationContextStatus?.workflowState === after?.generationContextStatus?.workflowState,
      runMetricsPreserved: canonical(before?.efficiency) === canonical(after?.efficiency),
      projectStatePreserved: canonical(before?.projectState) === canonical(after?.projectState),
      projectHistoryPreserved: canonical(before?.history) === canonical(after?.history),
      releaseEvidencePreserved:
        before?.releaseEvidence?.httpStatus === after?.releaseEvidence?.httpStatus
        && before?.releaseEvidence?.available === after?.releaseEvidence?.available
        && before?.releaseEvidence?.stableStateSha256 === after?.releaseEvidence?.stableStateSha256,
      artifactPreserved:
        canonical(stableArtifact(before?.artifact))
        === canonical(stableArtifact(after?.artifact)),
    },
    observations: {
      releaseCanonicalResponsePreserved:
        before?.releaseEvidence?.canonicalResponseSha256
        === after?.releaseEvidence?.canonicalResponseSha256,
      artifactBodySha256Preserved:
        before?.artifact?.bodySha256 === after?.artifact?.bodySha256,
    },
  };
  evidence.status = Object.values(evidence.verification).every((value) => value === true)
    ? "accepted"
    : "rejected";
  evidence.evidenceSha256 = sha256(canonical({ ...evidence, evidenceSha256: undefined }));
  validateRuntimeRestartEvidence(evidence, {
    side: metadata.side,
    projectId: before?.projectId,
    runId: before?.runId,
    runtimeDeploymentRevision: metadata.runtimeDeploymentRevision,
  });
  return evidence;
}

export function validateRuntimeRestartEvidence(evidence, expected) {
  rejectSensitiveValues(evidence);
  if (evidence?.schemaVersion !== EVIDENCE_SCHEMA) {
    throw new Error(`restart evidence schema must be ${EVIDENCE_SCHEMA}`);
  }
  if (!ISO_TIMESTAMP.test(evidence.recordedAt || "")) {
    throw new Error("restart evidence recordedAt must be an ISO UTC timestamp");
  }
  if (evidence.side !== expected.side) throw new Error("restart evidence side mismatch");
  for (const field of ["deployment", "runtimeDeploymentRevision", "deploymentUid", "deploymentTemplateSha256"]) {
    if (!hasText(evidence[field])) throw new Error(`restart evidence ${field} is required`);
  }
  if (evidence.runtimeDeploymentRevision !== expected.runtimeDeploymentRevision) {
    throw new Error("restart evidence Runtime deployment revision drift");
  }
  if (!HASH.test(evidence.deploymentTemplateSha256)) {
    throw new Error("restart evidence deploymentTemplateSha256 must be sha256");
  }
  if (!positiveInteger(evidence.deploymentGeneration)
    || !Number.isSafeInteger(evidence.restartDurationMs)
    || evidence.restartDurationMs < 0) {
    throw new Error("restart evidence deployment generation and duration are invalid");
  }
  for (const label of ["podBefore", "podAfter"]) {
    if (!hasText(evidence[label]?.name) || !hasText(evidence[label]?.uid)) {
      throw new Error(`restart evidence ${label} identity is required`);
    }
  }
  if (evidence.podBefore.uid === evidence.podAfter.uid) {
    throw new Error("restart evidence must replace the Runtime Pod UID");
  }
  validateSnapshot(evidence.before, "restartEvidence.before", evidence.side);
  validateSnapshot(evidence.after, "restartEvidence.after", evidence.side);
  if (evidence.before.projectId !== expected.projectId
    || evidence.after.projectId !== expected.projectId
    || evidence.before.runId !== expected.runId
    || evidence.after.runId !== expected.runId) {
    throw new Error("restart evidence Project or Run identity mismatch");
  }
  if (canonical(stableSnapshot(evidence.before)) !== canonical(stableSnapshot(evidence.after))) {
    throw new Error("restart evidence state changed across Runtime Pod replacement");
  }
  const required = [
    "podUidChanged",
    "deploymentIdentityPreserved",
    "runtimeReadyAfterRestart",
    "generationContextIdentityPreserved",
    "workflowStatePreserved",
    "runMetricsPreserved",
    "projectStatePreserved",
    "projectHistoryPreserved",
    "releaseEvidencePreserved",
    "artifactPreserved",
  ];
  if (evidence.status !== "accepted"
    || required.some((field) => evidence.verification?.[field] !== true)) {
    throw new Error("restart evidence did not satisfy every recovery invariant");
  }
  const expectedHash = sha256(canonical({ ...evidence, evidenceSha256: undefined }));
  if (evidence.evidenceSha256 !== expectedHash) {
    throw new Error("restart evidence hash mismatch");
  }
  return evidence;
}

function writeExclusive(file, value) {
  fs.mkdirSync(path.dirname(path.resolve(file)), { recursive: true });
  const descriptor = fs.openSync(file, "wx", 0o600);
  try {
    fs.writeFileSync(descriptor, `${JSON.stringify(value, null, 2)}\n`);
  } finally {
    fs.closeSync(descriptor);
  }
}

function main() {
  const [metadataFile, beforeFile, afterFile, outputFile] = process.argv.slice(2);
  if (!metadataFile || !beforeFile || !afterFile || !outputFile) {
    throw new Error("usage: generation-context-runtime-restart-evidence.mjs <metadata.json> <before.json> <after.json> <output.json>");
  }
  writeExclusive(
    outputFile,
    createRuntimeRestartEvidence(
      JSON.parse(fs.readFileSync(metadataFile, "utf8")),
      JSON.parse(fs.readFileSync(beforeFile, "utf8")),
      JSON.parse(fs.readFileSync(afterFile, "utf8")),
    ),
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error?.stack || error}\n`);
    process.exitCode = 1;
  }
}
