#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const SESSION_SCHEMA = "generation-context-paired-cohort-session@1";
const SAMPLE_SCHEMA = "generation-context-paired-cohort-sample@1";
const LEDGER_RECORD_SCHEMA = "generation-context-paired-cohort-ledger-record@1";
const EVIDENCE_SCHEMA = "generation-context-rollout-evidence@1";
const CALCULATOR_VERSION = "generation-context-rollout-calculator@1";
const REQUIRED_BUCKETS = new Set([
  "greenfield",
  "warm_copy_css",
  "warm_structural",
  "cold_dev",
  "repair",
]);
const REQUIRED_COVERAGE = [
  "nextTemplate",
  "fumadocsTemplate",
  "multimodalVisualDelivered",
  "nonVisualUnavailableMainTaskPassed",
  "runtimeRestart",
];
const SHA256 = /^[a-f0-9]{64}$/;
const ISO_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/;
const FORBIDDEN_KEY = /(?:^|_)(?:api_?key|authorization|credential|secret|prompt|text|source_?content|image_?(?:bytes|url)|provider_?(?:response|body)|request_?body|response_?body|signed_?url|temporary_?url)(?:$|_)/i;
const SECRET_VALUE = /(?:\bBearer\s+[A-Za-z0-9._~+/=-]{8,}|\bsk-[A-Za-z0-9_-]{8,}|data:image\/)/i;

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

function nonNegativeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function finiteNonNegative(value) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function rejectSensitiveMaterial(value, location = "document") {
  if (typeof value === "string") {
    if (SECRET_VALUE.test(value)) {
      throw new Error(`${location} contains credential, bearer, or image payload material`);
    }
    return;
  }
  if (Array.isArray(value)) {
    value.forEach((entry, index) => rejectSensitiveMaterial(entry, `${location}[${index}]`));
    return;
  }
  if (!value || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    if (FORBIDDEN_KEY.test(key.replaceAll(/([a-z])([A-Z])/g, "$1_$2"))) {
      throw new Error(`${location}.${key} is forbidden in hashes-only cohort evidence`);
    }
    rejectSensitiveMaterial(child, `${location}.${key}`);
  }
}

function validateIdentity(identity, label) {
  for (const field of [
    "fixtureId",
    "modelResource",
    "modelVersion",
    "templateVersion",
    "phase",
  ]) {
    if (!hasText(identity?.[field])) throw new Error(`${label}.${field} is required`);
  }
  for (const field of ["providerParametersHash", "capabilitySnapshotHash"]) {
    if (!SHA256.test(identity?.[field] || "")) throw new Error(`${label}.${field} must be sha256`);
  }
  if (!positiveInteger(identity?.providerResourceRevision)) {
    throw new Error(`${label}.providerResourceRevision must be a positive integer`);
  }
}

function validateSession(session) {
  rejectSensitiveMaterial(session, "session");
  if (session?.schemaVersion !== SESSION_SCHEMA) throw new Error(`session.schemaVersion must be ${SESSION_SCHEMA}`);
  if (!hasText(session.sessionId)) throw new Error("session.sessionId is required");
  if (!ISO_TIMESTAMP.test(session.createdAt || "")) throw new Error("session.createdAt must be an ISO UTC timestamp");
  if (session.calculatorVersion !== CALCULATOR_VERSION) {
    throw new Error(`session.calculatorVersion must be ${CALCULATOR_VERSION}`);
  }
  if (!Number.isSafeInteger(session.bootstrap?.iterations) || session.bootstrap.iterations < 100) {
    throw new Error("session.bootstrap.iterations must be at least 100");
  }
  if (!Number.isSafeInteger(session.bootstrap?.seed)) throw new Error("session.bootstrap.seed must be an integer");
  if (!SHA256.test(session.fixtureManifestSha256 || "")) {
    throw new Error("session.fixtureManifestSha256 must be sha256");
  }
  if (session.sourcePolicy !== "hashes_only") throw new Error("session.sourcePolicy must be hashes_only");
  if (!Array.isArray(session.providers) || !session.providers.length) {
    throw new Error("session.providers must contain at least one frozen Provider Resource snapshot");
  }
  const providerIds = new Set();
  for (const [index, provider] of session.providers.entries()) {
    const label = `session.providers[${index}]`;
    if (provider?.gatewayMode !== "internal_gateway") throw new Error(`${label}.gatewayMode must be internal_gateway`);
    for (const field of ["modelResourceId", "modelVersion"]) {
      if (!hasText(provider?.[field])) throw new Error(`${label}.${field} is required`);
    }
    if (providerIds.has(provider.modelResourceId)) throw new Error(`duplicate session Provider Resource: ${provider.modelResourceId}`);
    providerIds.add(provider.modelResourceId);
    if (!positiveInteger(provider.resourceRevision)) throw new Error(`${label}.resourceRevision must be a positive integer`);
    if (!SHA256.test(provider.providerParametersHash || "")) throw new Error(`${label}.providerParametersHash must be sha256`);
  }
  if (session.runtimes?.control?.generationContextMode !== "off") {
    throw new Error("session.runtimes.control.generationContextMode must be off");
  }
  if (session.runtimes?.candidate?.generationContextMode !== "enabled") {
    throw new Error("session.runtimes.candidate.generationContextMode must be enabled");
  }
  for (const side of ["control", "candidate"]) {
    if (!hasText(session.runtimes?.[side]?.deploymentRevision)) {
      throw new Error(`session.runtimes.${side}.deploymentRevision is required`);
    }
    const allowed = session.runtimes?.[side]?.allowedModelResourceIds;
    if (!Array.isArray(allowed) || allowed.length !== providerIds.size
      || allowed.some((id) => !providerIds.has(id)) || new Set(allowed).size !== allowed.length) {
      throw new Error(`session.runtimes.${side}.allowedModelResourceIds must exactly match session.providers`);
    }
  }
}

function validateMetrics(metrics, label) {
  if (!metrics || typeof metrics !== "object" || Array.isArray(metrics)) {
    throw new Error(`${label} must be an object`);
  }
  for (const [field, value] of Object.entries(metrics)) {
    if (!finiteNonNegative(value)) throw new Error(`${label}.${field} must be a finite non-negative number`);
  }
  for (const field of [
    "modelTurnAtFirstSourceMutation",
    "prebuildFsListCount",
    "prebuildFsSearchCount",
    "duplicateFullReadRateBasisPoints",
    "outOfScopeMutationCount",
  ]) {
    if (field in metrics && !nonNegativeInteger(metrics[field])) {
      throw new Error(`${label}.${field} must be a non-negative integer`);
    }
  }
  if ((metrics.duplicateFullReadRateBasisPoints ?? 0) > 10_000) {
    throw new Error(`${label}.duplicateFullReadRateBasisPoints cannot exceed 10000`);
  }
}

function validateSample(sample, session) {
  rejectSensitiveMaterial(sample, "sample");
  if (sample?.schemaVersion !== SAMPLE_SCHEMA) throw new Error(`sample.schemaVersion must be ${SAMPLE_SCHEMA}`);
  for (const field of ["pairId", "batchId"]) {
    if (!hasText(sample[field])) throw new Error(`sample.${field} is required`);
  }
  if (!REQUIRED_BUCKETS.has(sample.bucket)) throw new Error(`sample.bucket is invalid: ${sample.bucket}`);
  if (!['control', 'candidate'].includes(sample.side)) throw new Error("sample.side must be control or candidate");
  const expectedFlag = sample.side === "control" ? "legacy" : "generation_context";
  if (sample.flag !== expectedFlag) throw new Error(`sample.flag must be ${expectedFlag} for ${sample.side}`);
  if (!["completed", "failed", "timeout", "fallback"].includes(sample.status)) {
    throw new Error("sample.status must be completed, failed, timeout, or fallback");
  }
  if (!ISO_TIMESTAMP.test(sample.recordedAt || "")) throw new Error("sample.recordedAt must be an ISO UTC timestamp");
  validateIdentity(sample.identity, "sample.identity");
  const provider = session.providers.find((candidate) => candidate.modelResourceId === sample.identity.modelResource);
  if (!provider || sample.identity.modelVersion !== provider.modelVersion
    || sample.identity.providerResourceRevision !== provider.resourceRevision
    || sample.identity.providerParametersHash !== provider.providerParametersHash) {
    throw new Error("sample identity must match the frozen Provider Resource snapshot");
  }
  if (sample.execution?.gatewayMode !== "internal_gateway") {
    throw new Error("sample.execution.gatewayMode must be internal_gateway");
  }
  if (sample.execution?.modelResourceId !== provider.modelResourceId
    || sample.execution?.providerResourceRevision !== provider.resourceRevision) {
    throw new Error("sample execution must prove the frozen Provider Resource and revision");
  }
  if (!SHA256.test(sample.execution?.modelExecutionEvidenceSha256 || "")) {
    throw new Error("sample.execution.modelExecutionEvidenceSha256 must be sha256");
  }
  if (!hasText(sample.source?.storageRef) || !SHA256.test(sample.source?.contentSha256 || "")) {
    throw new Error("sample.source must contain storageRef and contentSha256");
  }
  if (!SHA256.test(sample.acceptanceEvidenceSha256 || "")) {
    throw new Error("sample.acceptanceEvidenceSha256 must be sha256");
  }
  for (const field of ["firstBuildSucceeded", "requiredFidelityPassed"]) {
    if (typeof sample[field] !== "boolean") throw new Error(`sample.${field} must be boolean`);
  }
  if (!Array.isArray(sample.coverage)
    || new Set(sample.coverage).size !== sample.coverage.length
    || sample.coverage.some((field) => !REQUIRED_COVERAGE.includes(field))) {
    throw new Error(`sample.coverage must contain unique supported coverage claims`);
  }
  if (sample.side === "control" && sample.coverage.length) {
    throw new Error("control samples cannot claim Generation Context rollout coverage");
  }
  validateMetrics(sample.metrics, "sample.metrics");
}

function recordPayload(record) {
  const { recordHash, ...payload } = record;
  return payload;
}

function ledgerRecord(kind, payload, previousRecordHash) {
  const record = {
    schemaVersion: LEDGER_RECORD_SCHEMA,
    kind,
    previousRecordHash,
    payload,
  };
  return { ...record, recordHash: sha256(canonical(record)) };
}

function parseLedger(ledgerFile) {
  const lines = fs.readFileSync(ledgerFile, "utf8").split(/\r?\n/).filter(Boolean);
  if (!lines.length) throw new Error("paired-cohort ledger is empty");
  const records = lines.map((line, index) => {
    try {
      return JSON.parse(line);
    } catch {
      throw new Error(`ledger line ${index + 1} is not valid JSON`);
    }
  });
  let previousRecordHash = null;
  records.forEach((record, index) => {
    if (record.schemaVersion !== LEDGER_RECORD_SCHEMA) throw new Error(`ledger line ${index + 1} has an unsupported schema`);
    if (record.previousRecordHash !== previousRecordHash) throw new Error(`ledger hash chain breaks at line ${index + 1}`);
    const expected = sha256(canonical(recordPayload(record)));
    if (record.recordHash !== expected) throw new Error(`ledger record hash mismatch at line ${index + 1}`);
    previousRecordHash = record.recordHash;
  });
  if (records[0].kind !== "session" || records.slice(1).some((record) => record.kind !== "sample")) {
    throw new Error("ledger must contain exactly one leading session followed by sample records");
  }
  const session = records[0].payload;
  validateSession(session);
  const samples = records.slice(1).map((record) => record.payload);
  const keys = new Set();
  for (const sample of samples) {
    validateSample(sample, session);
    const key = `${sample.pairId}\u0000${sample.side}`;
    if (keys.has(key)) throw new Error(`duplicate sample side for pair ${sample.pairId}: ${sample.side}`);
    keys.add(key);
  }
  return { records, session, samples, headRecordHash: previousRecordHash };
}

function withLedgerLock(ledgerFile, operation) {
  const lockFile = `${ledgerFile}.lock`;
  let descriptor;
  try {
    descriptor = fs.openSync(lockFile, "wx", 0o600);
  } catch (error) {
    if (error?.code === "EEXIST") throw new Error(`paired-cohort ledger is locked: ${lockFile}`);
    throw error;
  }
  try {
    return operation();
  } finally {
    fs.closeSync(descriptor);
    fs.unlinkSync(lockFile);
  }
}

export function initializePairedCohortLedger(ledgerFile, session) {
  validateSession(session);
  fs.mkdirSync(path.dirname(path.resolve(ledgerFile)), { recursive: true });
  const record = ledgerRecord("session", session, null);
  const descriptor = fs.openSync(ledgerFile, "wx", 0o600);
  try {
    fs.writeFileSync(descriptor, `${JSON.stringify(record)}\n`);
  } finally {
    fs.closeSync(descriptor);
  }
  return record;
}

export function appendPairedCohortSample(ledgerFile, sample) {
  return withLedgerLock(ledgerFile, () => {
    const ledger = parseLedger(ledgerFile);
    validateSample(sample, ledger.session);
    if (ledger.samples.some((existing) => existing.pairId === sample.pairId && existing.side === sample.side)) {
      throw new Error(`duplicate sample side for pair ${sample.pairId}: ${sample.side}`);
    }
    const opposite = ledger.samples.find((existing) => existing.pairId === sample.pairId);
    if (opposite) {
      if (opposite.batchId !== sample.batchId || opposite.bucket !== sample.bucket
        || canonical(opposite.identity) !== canonical(sample.identity)) {
        throw new Error(`pair ${sample.pairId} control/candidate identity mismatch`);
      }
    }
    const record = ledgerRecord("sample", sample, ledger.headRecordHash);
    fs.appendFileSync(ledgerFile, `${JSON.stringify(record)}\n`, { encoding: "utf8", mode: 0o600 });
    return record;
  });
}

export function appendPairedCohortPair(ledgerFile, control, candidate) {
  return withLedgerLock(ledgerFile, () => {
    const ledger = parseLedger(ledgerFile);
    for (const sample of [control, candidate]) validateSample(sample, ledger.session);
    if (control.side !== "control" || candidate.side !== "candidate") {
      throw new Error("paired append requires control then candidate samples");
    }
    if (
      control.pairId !== candidate.pairId ||
      control.batchId !== candidate.batchId ||
      control.bucket !== candidate.bucket ||
      canonical(control.identity) !== canonical(candidate.identity)
    ) {
      throw new Error(`pair ${control.pairId} control/candidate identity mismatch`);
    }
    if (ledger.samples.some((sample) => sample.pairId === control.pairId)) {
      throw new Error(`pair ${control.pairId} already has recorded samples`);
    }
    const controlRecord = ledgerRecord("sample", control, ledger.headRecordHash);
    const candidateRecord = ledgerRecord(
      "sample",
      candidate,
      controlRecord.recordHash,
    );
    fs.appendFileSync(
      ledgerFile,
      `${JSON.stringify(controlRecord)}\n${JSON.stringify(candidateRecord)}\n`,
      { encoding: "utf8", mode: 0o600 },
    );
    return { control: controlRecord, candidate: candidateRecord };
  });
}

export function verifyPairedCohortLedger(ledgerFile) {
  const ledger = parseLedger(ledgerFile);
  const pendingPairs = [];
  const sidesByPair = new Map();
  for (const sample of ledger.samples) {
    const sides = sidesByPair.get(sample.pairId) || new Set();
    sides.add(sample.side);
    sidesByPair.set(sample.pairId, sides);
  }
  for (const [pairId, sides] of sidesByPair) {
    if (sides.size !== 2) pendingPairs.push(pairId);
  }
  return {
    schemaVersion: "generation-context-paired-cohort-ledger-verification@1",
    sessionId: ledger.session.sessionId,
    recordCount: ledger.records.length,
    sampleCount: ledger.samples.length,
    completePairCount: [...sidesByPair.values()].filter((sides) => sides.size === 2).length,
    pendingPairs,
    headRecordHash: ledger.headRecordHash,
  };
}

export function assemblePairedCohortEvidence(ledgerFile) {
  const ledger = parseLedger(ledgerFile);
  const pairs = new Map();
  for (const sample of ledger.samples) {
    const pair = pairs.get(sample.pairId) || {
      id: sample.pairId,
      batchId: sample.batchId,
      bucket: sample.bucket,
      identity: sample.identity,
    };
    if (pair.batchId !== sample.batchId || pair.bucket !== sample.bucket
      || canonical(pair.identity) !== canonical(sample.identity)) {
      throw new Error(`pair ${sample.pairId} control/candidate identity mismatch`);
    }
    pair[sample.side] = {
      flag: sample.flag,
      status: sample.status,
      source: sample.source,
      acceptanceEvidenceSha256: sample.acceptanceEvidenceSha256,
      execution: sample.execution,
      firstBuildSucceeded: sample.firstBuildSucceeded,
      requiredFidelityPassed: sample.requiredFidelityPassed,
      metrics: sample.metrics,
      coverage: sample.coverage,
      recordedAt: sample.recordedAt,
    };
    pairs.set(sample.pairId, pair);
  }
  const incomplete = [...pairs.values()].filter((pair) => !pair.control || !pair.candidate);
  if (incomplete.length) {
    throw new Error(`cannot assemble evidence with incomplete pairs: ${incomplete.map((pair) => pair.id).join(", ")}`);
  }
  const provenCoverage = new Set(
    [...pairs.values()]
      .filter((pair) => pair.candidate.status === "completed" && pair.candidate.requiredFidelityPassed)
      .flatMap((pair) => pair.candidate.coverage),
  );
  return {
    schemaVersion: EVIDENCE_SCHEMA,
    calculatorVersion: ledger.session.calculatorVersion,
    bootstrap: ledger.session.bootstrap,
    coverage: Object.fromEntries(REQUIRED_COVERAGE.map((field) => [field, provenCoverage.has(field)])),
    provenance: {
      sourcePolicy: ledger.session.sourcePolicy,
      sessionId: ledger.session.sessionId,
      fixtureManifestSha256: ledger.session.fixtureManifestSha256,
      ledgerSha256: sha256(fs.readFileSync(ledgerFile)),
      ledgerHeadRecordHash: ledger.headRecordHash,
      providers: ledger.session.providers,
      runtimes: ledger.session.runtimes,
    },
    pairs: [...pairs.values()],
  };
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

async function main() {
  const [command, ledgerFile, inputOrOutput] = process.argv.slice(2);
  if (!command || !ledgerFile) {
    throw new Error("usage: generation-context-paired-cohort-ledger.mjs <init|append|verify|assemble> <ledger.ndjson> [input-or-output.json]");
  }
  if (command === "init") {
    if (!inputOrOutput) throw new Error("init requires a session JSON file");
    initializePairedCohortLedger(ledgerFile, JSON.parse(fs.readFileSync(inputOrOutput, "utf8")));
  } else if (command === "append") {
    if (!inputOrOutput) throw new Error("append requires a sample JSON file");
    appendPairedCohortSample(ledgerFile, JSON.parse(fs.readFileSync(inputOrOutput, "utf8")));
  } else if (command === "verify") {
    process.stdout.write(`${JSON.stringify(verifyPairedCohortLedger(ledgerFile), null, 2)}\n`);
  } else if (command === "assemble") {
    if (!inputOrOutput) throw new Error("assemble requires an output JSON file");
    writeExclusive(inputOrOutput, assemblePairedCohortEvidence(ledgerFile));
  } else {
    throw new Error(`unknown command: ${command}`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`${error?.stack || error}\n`);
    process.exitCode = 1;
  });
}
