#!/usr/bin/env node

import { readFile, open, unlink } from "node:fs/promises";
import {
  buildCanarySourceLedger,
  canarySha256,
  createCanaryLedgerRecord,
  validateCanaryLedgerChain,
  validateDesignContextCanaryEvidence,
} from "./validate-design-context-canary-evidence.mjs";
import { validateDualReadRunEvidence } from "./generation-context-evidence.mjs";

const EVENT_TYPES = new Set([
  "website.run",
  "required-finding.repair",
  "profile-sync.clean-apply",
  "profile-sync.conflict-apply",
  "profile-sync.plan-mismatch",
  "profile-sync.recovery",
  "bff-runtime",
  "publish.samples",
  "metrics.snapshot",
  "alert.destination-probe",
  "alert.delivery",
  "rollback",
  "compatibility",
]);

const SENSITIVE_PATTERNS = [
  /sk-[A-Za-z0-9]{20,}/,
  /Bearer\s+[^\s"']+/i,
  /eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/,
  /"(?:api[_-]?key|access[_-]?token|refresh[_-]?token|authorization|password|private[_-]?key|secret)"\s*:/i,
  /[?&](?:token|access_token|api_key|sig|signature)=[^&\s]+/i,
];

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function isoTimestamp(value) {
  return hasText(value) && Number.isFinite(Date.parse(value));
}

function sha256(value) {
  return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
}

function positiveInteger(value) {
  return Number.isInteger(value) && value > 0;
}

function nonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0;
}

function imageDigest(value) {
  return typeof value === "string" && /^sha256:[a-f0-9]{64}$/.test(value);
}

function sameCohort(value, cohort) {
  return value?.projectId === cohort?.projectId
    && value?.designProfileId === cohort?.designProfileId
    && value?.designProfileVersion === cohort?.designProfileVersion
    && value?.policyRevision === cohort?.policyRevision;
}

function sameWebsiteCohort(value, cohort, mode) {
  return value?.projectId === cohort?.projectId
    && value?.designProfileId === cohort?.designProfileId
    && value?.designProfileVersion === cohort?.designProfileVersion
    && value?.policyRevision === (mode === "observe" ? cohort?.observePolicyRevision : cohort?.policyRevision);
}

function requireFields(value, fields, label) {
  for (const field of fields) if (!hasText(value?.[field])) throw new Error(`${label}.${field} is required`);
}

function validateAppliedProfileSync(value, label, cohort) {
  requireFields(value, ["operationId", "sourceRunId", "childRunId"], label);
  for (const field of ["planHash", "beforeTokenSnapshotHash", "afterTokenSnapshotHash"]) {
    if (!sha256(value?.[field])) throw new Error(`${label}.${field} must be sha256`);
  }
  if (!sameCohort(value, cohort) || value.status !== "applied"
    || value.beforeTokenSnapshotHash === value.afterTokenSnapshotHash) {
    throw new Error(`${label} must be an applied same-cohort sync with a real token change`);
  }
}

function validateEventPayload(type, payload, session) {
  const cohort = session.cohort;
  if (type === "website.run") {
    requireFields(payload, ["runId", "candidateVersionId", "workerProvider", "workerVersion", "verificationPolicyId", "artifactUri", "evidenceUri"], type);
    for (const field of ["candidateManifestHash", "artifactManifestHash", "materializationHash", "verifierCapabilitySnapshotHash"]) {
      if (!sha256(payload[field])) throw new Error(`${type}.${field} must be sha256`);
    }
    const protocolErrors = validateDualReadRunEvidence(payload, type);
    if (protocolErrors.length) throw new Error(protocolErrors.join("; "));
    const policy = payload.enforcementPolicy;
    const expectedRevision = payload.mode === "observe" ? cohort.observePolicyRevision : cohort.policyRevision;
    if (!["observe", "enforced"].includes(payload.mode) || !sameWebsiteCohort(payload, cohort, payload.mode)
      || payload.artifactManifestHash !== payload.materializationHash
      || payload.status !== "completed" || payload.publishVerdict !== "pass"
      || policy?.source !== "persistent" || policy?.enabled !== (payload.mode === "enforced")
      || policy?.policyRevision !== expectedRevision || !hasText(policy?.policyUpdatedBy)) {
      throw new Error("website.run must be a passing same-cohort DCP run with valid protocol evidence");
    }
    return;
  }
  if (type === "required-finding.repair") {
    requireFields(payload, ["ruleId", "findingId", "blockedRunId", "reviewRunId", "repairRunId"], type);
    if (!sameCohort(payload, cohort) || payload.findingStatus !== "fixed"
      || !sha256(payload.blockedCandidateManifestHash) || !sha256(payload.promotedCandidateManifestHash)
      || payload.blockedCandidateManifestHash === payload.promotedCandidateManifestHash
      || payload.blockedPublish !== true || payload.repairPublishVerdict !== "pass" || payload.promoted !== true) {
      throw new Error("required-finding.repair must prove block, repair and new promotion");
    }
    return;
  }
  if (["profile-sync.clean-apply", "profile-sync.conflict-apply", "profile-sync.recovery"].includes(type)) {
    validateAppliedProfileSync(payload, type, cohort);
    if (type === "profile-sync.conflict-apply" && !positiveInteger(payload.conflictDecisionCount)) {
      throw new Error("profile-sync.conflict-apply requires explicit conflict decisions");
    }
    if (type === "profile-sync.recovery"
      && (payload.recoveryRequiredObserved !== true || payload.reusedChildRun !== true)) {
      throw new Error("profile-sync.recovery must observe recovery_required and reuse the child Run");
    }
    return;
  }
  if (type === "profile-sync.plan-mismatch") {
    if (payload.rejected !== true || payload.errorCode !== "profile_sync_plan_mismatch") {
      throw new Error("profile-sync.plan-mismatch must record the stable rejection code");
    }
    return;
  }
  if (type === "bff-runtime") {
    requireFields(payload, ["projectId", "runId", "principalProjectId", "operationId", "childRunId"], type);
    if (payload.projectId !== cohort.projectId || payload.principalProjectId !== cohort.projectId
      || !sha256(payload.planHash) || payload.childRunUnique !== true) {
      throw new Error("bff-runtime must preserve project ownership and child Run uniqueness");
    }
    return;
  }
  if (type === "publish.samples") {
    if (!Array.isArray(payload.samples) || payload.samples.length === 0) throw new Error("publish.samples cannot be empty");
    const ids = new Set();
    for (const sample of payload.samples) {
      if (!hasText(sample?.sampleId) || ids.has(sample.sampleId) || !hasText(sample?.runId)
        || !isoTimestamp(sample?.observedAt) || !["baseline", "enforced"].includes(sample?.mode)
        || !["pass", "fail"].includes(sample?.publishVerdict) || typeof sample?.dcpCausedFailure !== "boolean"
        || !sameWebsiteCohort(sample, cohort, sample?.mode === "baseline" ? "observe" : "enforced")) {
        throw new Error(`invalid or duplicate publish sample: ${sample?.sampleId ?? "unknown"}`);
      }
      ids.add(sample.sampleId);
      const observedAt = Date.parse(sample.observedAt);
      if (sample.mode === "baseline" && (sample.dcpCausedFailure
        || observedAt < Date.parse(session.window.baselineStartedAt)
        || observedAt > Date.parse(session.window.baselineEndedAt))) {
        throw new Error(`baseline publish sample falls outside its immutable window: ${sample.sampleId}`);
      }
      if (sample.mode === "enforced" && observedAt < Date.parse(session.window.observationStartedAt)) {
        throw new Error(`enforced publish sample predates the observation window: ${sample.sampleId}`);
      }
    }
    return;
  }
  if (type === "metrics.snapshot") {
    const forbiddenComputed = ["baselinePublishCount", "enforcedPublishCount", "observationWindowMinutes", "publishFailureRateDeltaPp"];
    if (!isoTimestamp(payload.observationEndedAt) || !hasText(payload.conclusionRecordedBy)
      || forbiddenComputed.some(field => Object.hasOwn(payload, field))
      || Date.parse(payload.observationEndedAt) < Date.parse(session.window.observationStartedAt)
      || !nonNegativeInteger(payload.verifierUnavailableCount)
      || !nonNegativeInteger(payload.verifierRuntimeLostCount)
      || !nonNegativeInteger(payload.unexpectedReadGateBlockCount)
      || !nonNegativeInteger(payload.recoveryRequiredOver24hCount)
      || typeof payload.requiredFindingRepairRate !== "number"
      || payload.requiredFindingRepairRate < 0 || payload.requiredFindingRepairRate > 1
      || typeof payload.alertsTriggered !== "boolean") {
      throw new Error("metrics.snapshot must be a shaped operational export without computed summary fields");
    }
    return;
  }
  if (type === "alert.destination-probe") {
    requireFields(payload, ["destinationId", "operatorId"], type);
    if (!sha256(payload?.eventId) || payload?.probeStatus !== "delivered"
      || payload?.attemptCount !== 1 || !positiveInteger(payload?.responseStatus)
      || payload.responseStatus < 200 || payload.responseStatus >= 300) {
      throw new Error("alert.destination-probe must prove one successful signed HTTPS readiness delivery");
    }
    return;
  }
  if (type === "alert.delivery") {
    const deliveryCohort = payload?.cohort;
    const codes = payload?.triggeredAlertCodes;
    const cohortMatches = deliveryCohort?.projectId === cohort.projectId
      && deliveryCohort?.designProfileId === cohort.designProfileId
      && deliveryCohort?.designProfileVersion === cohort.designProfileVersion
      && deliveryCohort?.observePolicyRevision === cohort.observePolicyRevision
      && deliveryCohort?.policyRevision === cohort.policyRevision;
    if (!isoTimestamp(payload?.operationalExportGeneratedAt) || !isoTimestamp(payload?.observationEndedAt)
      || !sha256(payload?.eventId) || !hasText(payload?.destinationId) || !cohortMatches
      || typeof payload?.deliveryRequired !== "boolean" || !Array.isArray(codes)
      || codes.some(code => !hasText(code)) || new Set(codes).size !== codes.length) {
      throw new Error("alert.delivery must identify a valid Runtime export, cohort, destination and decision");
    }
    if (payload.deliveryRequired) {
      if (payload.deliveryStatus !== "delivered" || !positiveInteger(payload.attemptCount)
        || payload.attemptCount !== 1 || !positiveInteger(payload.responseStatus)
        || payload.responseStatus < 200 || payload.responseStatus >= 300 || codes.length === 0) {
        throw new Error("alert.delivery must prove successful external delivery for triggered alerts");
      }
    } else if (payload.deliveryStatus !== "not-required" || payload.attemptCount !== 0
      || payload.responseStatus !== null || codes.length !== 0) {
      throw new Error("alert.delivery without triggered alerts must be an explicit no-delivery decision");
    }
    return;
  }
  if (type === "rollback") {
    requireFields(payload, ["updatedBy", "postRollbackRunId"], type);
    if (!sameCohort(payload, cohort) || !isoTimestamp(payload.recordedAt)
      || payload.policyEnabledAfterRollback !== false || !positiveInteger(payload.policyRevisionAfterRollback)
      || payload.policyRevisionAfterRollback <= cohort.policyRevision || payload.postRollbackMode !== "observe"
      || payload.newRunReadGateBlocked !== false || payload.operationRecoveryPreserved !== true) {
      throw new Error("rollback must prove an exact enabled=false policy revision and observe post-run");
    }
    return;
  }
  if (type === "compatibility") {
    for (const field of ["noProfileWebsitePassed", "docsBuildPassed", "legacyEditRepairPassed"]) {
      if (payload[field] !== true) throw new Error(`compatibility.${field} must be true`);
    }
    requireFields(payload, ["noProfileWebsiteRunId", "docsBuildRunId", "legacyEditRunId", "legacyRepairRunId"], type);
  }
}

function assertNoSecrets(raw, path) {
  for (const pattern of SENSITIVE_PATTERNS) {
    if (pattern.test(raw)) throw new Error(`refusing source fragment with credential-like content: ${path}`);
  }
}

async function loadSourceDocument(path) {
  const raw = await readFile(path, "utf8");
  assertNoSecrets(raw, path);
  return { raw, document: JSON.parse(raw), sha256: canarySha256(raw) };
}

async function writeExclusiveDurable(path, contents) {
  const handle = await open(path, "wx", 0o600);
  let completed = false;
  try {
    await handle.writeFile(contents, { encoding: "utf8" });
    await handle.sync();
    completed = true;
  } finally {
    await handle.close().catch(() => {});
    if (!completed) await unlink(path).catch(() => {});
  }
}

async function appendDurable(path, contents) {
  const handle = await open(path, "r+");
  try {
    const { size } = await handle.stat();
    const bytes = Buffer.from(contents, "utf8");
    let offset = 0;
    while (offset < bytes.length) {
      const { bytesWritten } = await handle.write(bytes, offset, bytes.length - offset, size + offset);
      if (bytesWritten <= 0) throw new Error(`failed to append canary ledger: ${path}`);
      offset += bytesWritten;
    }
    await handle.sync();
  } finally {
    await handle.close().catch(() => {});
  }
}

function sourcePayload(document, excludedKeys) {
  return Object.fromEntries(Object.entries(document).filter(([key]) => !excludedKeys.has(key)));
}

async function withLedgerLock(ledgerPath, callback) {
  const lockPath = `${ledgerPath}.lock`;
  let handle;
  try {
    handle = await open(lockPath, "wx", 0o600);
  } catch (error) {
    if (error?.code === "EEXIST") throw new Error(`canary ledger is locked: ${ledgerPath}`);
    throw error;
  }
  try {
    return await callback();
  } finally {
    await handle.close();
    await unlink(lockPath).catch(() => {});
  }
}

export async function readCanaryLedger(ledgerPath) {
  const raw = await readFile(ledgerPath, "utf8");
  const lines = raw.split(/\r?\n/).filter(Boolean);
  const records = lines.map((line, index) => {
    try {
      return JSON.parse(line);
    } catch (error) {
      throw new Error(`invalid JSON at canary ledger line ${index + 1}: ${error.message}`);
    }
  });
  const errors = validateCanaryLedgerChain(records);
  if (errors.length) throw new Error(errors.join("\n"));
  return records;
}

export async function initializeCanaryLedger({ ledgerPath, configPath }) {
  const { document, sha256 } = await loadSourceDocument(configPath);
  if (document?.schemaVersion !== "design-context-canary-session-config@1"
    || !isoTimestamp(document?.recordedAt) || !hasText(document?.sourceUri)
    || document?.provider?.mode !== "approved-real" || !hasText(document?.provider?.name)
    || !hasText(document?.provider?.model) || !hasText(document?.provider?.approvalReference)
    || document?.provider?.credentialPresent !== true
    || !hasText(document?.cohort?.projectId) || !hasText(document?.cohort?.designProfileId)
    || !positiveInteger(document?.cohort?.designProfileVersion) || !positiveInteger(document?.cohort?.policyRevision)
    || !positiveInteger(document?.cohort?.observePolicyRevision)
    || document.cohort.observePolicyRevision >= document.cohort.policyRevision
    || !hasText(document?.cohort?.policyUpdatedBy) || !hasText(document?.cohort?.thresholdVersion)
    || !document?.images || !document?.window
    || ["runtime", "bff"].some(service => !hasText(document.images?.[service]?.ref)
      || !imageDigest(document.images?.[service]?.manifestDigest)
      || !imageDigest(document.images?.[service]?.configDigest))
    || !isoTimestamp(document.window.baselineStartedAt) || !isoTimestamp(document.window.baselineEndedAt)
    || !isoTimestamp(document.window.observationStartedAt)
    || Date.parse(document.window.baselineEndedAt) <= Date.parse(document.window.baselineStartedAt)
    || Date.parse(document.window.baselineEndedAt) > Date.parse(document.window.observationStartedAt)) {
    throw new Error("invalid design-context canary session config");
  }
  const payload = sourcePayload(document, new Set(["schemaVersion", "recordedAt", "sourceUri"]));
  const record = createCanaryLedgerRecord({
    sequence: 1,
    type: "session.started",
    recordedAt: document.recordedAt,
    sourceUri: document.sourceUri,
    sourceSha256: sha256,
    payload,
    previousRecordHash: null,
  });
  await writeExclusiveDurable(ledgerPath, `${JSON.stringify(record)}\n`);
  return record;
}

export async function appendCanaryEvent({ ledgerPath, eventPath }) {
  const { document, sha256 } = await loadSourceDocument(eventPath);
  if (document?.schemaVersion !== "design-context-canary-event@1" || !EVENT_TYPES.has(document?.type)
    || !isoTimestamp(document?.recordedAt) || !hasText(document?.sourceUri)
    || !document?.payload || typeof document.payload !== "object" || Array.isArray(document.payload)) {
    throw new Error(`invalid design-context canary event fragment: ${eventPath}`);
  }
  return withLedgerLock(ledgerPath, async () => {
    const records = await readCanaryLedger(ledgerPath);
    const previous = records.at(-1);
    validateEventPayload(document.type, document.payload, records[0].payload);
    if (Date.parse(document.recordedAt) < Date.parse(previous.recordedAt)) {
      throw new Error("canary events must be appended in non-decreasing recordedAt order");
    }
    const record = createCanaryLedgerRecord({
      sequence: records.length + 1,
      type: document.type,
      recordedAt: document.recordedAt,
      sourceUri: document.sourceUri,
      sourceSha256: sha256,
      payload: document.payload,
      previousRecordHash: previous.recordHash,
    });
    await appendDurable(ledgerPath, `${JSON.stringify(record)}\n`);
    return record;
  });
}

function exactlyOne(records, type) {
  const values = records.filter(record => record.type === type).map(record => record.payload);
  if (values.length !== 1) throw new Error(`canary ledger requires exactly one ${type} record`);
  return values[0];
}

function latest(records, type) {
  const values = records.filter(record => record.type === type).map(record => record.payload);
  if (values.length === 0) throw new Error(`canary ledger requires at least one ${type} record`);
  return values.at(-1);
}

function collectPublishSamples(records, cohort, window, observationEndedAt) {
  const samples = records
    .filter(record => record.type === "publish.samples")
    .flatMap(record => Array.isArray(record.payload?.samples) ? record.payload.samples : []);
  const sampleIds = new Set();
  let baselinePublishCount = 0;
  let baselineFailureCount = 0;
  let enforcedPublishCount = 0;
  let enforcedDcpFailureCount = 0;
  for (const sample of samples) {
    if (!hasText(sample?.sampleId) || sampleIds.has(sample.sampleId) || !hasText(sample?.runId)
      || !isoTimestamp(sample?.observedAt) || !["pass", "fail"].includes(sample?.publishVerdict)
      || typeof sample?.dcpCausedFailure !== "boolean"
      || sample.projectId !== cohort.projectId || sample.designProfileId !== cohort.designProfileId
      || sample.designProfileVersion !== cohort.designProfileVersion
      || sample.policyRevision !== (sample.mode === "baseline" ? cohort.observePolicyRevision : cohort.policyRevision)) {
      throw new Error(`invalid or duplicate publish sample: ${sample?.sampleId ?? "unknown"}`);
    }
    sampleIds.add(sample.sampleId);
    const observedAt = Date.parse(sample.observedAt);
    if (sample.mode === "baseline") {
      if (sample.dcpCausedFailure || observedAt < Date.parse(window.baselineStartedAt)
        || observedAt > Date.parse(window.baselineEndedAt)) {
        throw new Error(`baseline publish sample falls outside its immutable window: ${sample.sampleId}`);
      }
      baselinePublishCount += 1;
      if (sample.publishVerdict === "fail") baselineFailureCount += 1;
    } else if (sample.mode === "enforced") {
      if (observedAt < Date.parse(window.observationStartedAt) || observedAt > Date.parse(observationEndedAt)) {
        throw new Error(`enforced publish sample falls outside its immutable window: ${sample.sampleId}`);
      }
      enforcedPublishCount += 1;
      if (sample.publishVerdict === "fail" && sample.dcpCausedFailure) enforcedDcpFailureCount += 1;
    } else {
      throw new Error(`unsupported publish sample mode: ${sample?.mode}`);
    }
  }
  if (baselinePublishCount === 0 || enforcedPublishCount === 0) throw new Error("baseline and enforced publish samples are both required");
  return {
    baselinePublishCount,
    enforcedPublishCount,
    publishFailureRateDeltaPp: ((enforcedDcpFailureCount / enforcedPublishCount)
      - (baselineFailureCount / baselinePublishCount)) * 100,
  };
}

export function assembleCanaryEvidence(records, recordedAt = new Date().toISOString()) {
  const session = exactlyOne(records, "session.started");
  const websiteRuns = records.filter(record => record.type === "website.run").map(record => record.payload);
  const requiredFindingRepair = exactlyOne(records, "required-finding.repair");
  const metricsSnapshot = latest(records, "metrics.snapshot");
  const observationEndedAt = metricsSnapshot.observationEndedAt;
  const sampleMetrics = collectPublishSamples(records, session.cohort, session.window, observationEndedAt);
  const observationWindowMinutes = (Date.parse(observationEndedAt) - Date.parse(session.window.observationStartedAt)) / 60_000;
  const metrics = {
    ...session.window,
    ...metricsSnapshot,
    observationWindowMinutes,
    ...sampleMetrics,
  };
  return {
    schemaVersion: "design-context-canary-evidence@1",
    result: "pass",
    recordedAt,
    provider: session.provider,
    cohort: session.cohort,
    images: session.images,
    websiteRuns,
    requiredFindingRepair,
    profileSync: {
      cleanApply: exactlyOne(records, "profile-sync.clean-apply"),
      conflictApply: exactlyOne(records, "profile-sync.conflict-apply"),
      planMismatch: exactlyOne(records, "profile-sync.plan-mismatch"),
      recovery: exactlyOne(records, "profile-sync.recovery"),
    },
    bffRuntime: exactlyOne(records, "bff-runtime"),
    metrics,
    rollback: exactlyOne(records, "rollback"),
    compatibility: exactlyOne(records, "compatibility"),
    sourceLedger: buildCanarySourceLedger(records),
  };
}

export async function finalizeCanaryLedger({ ledgerPath, outputPath, recordedAt = new Date().toISOString() }) {
  const records = await readCanaryLedger(ledgerPath);
  const evidence = assembleCanaryEvidence(records, recordedAt);
  const errors = validateDesignContextCanaryEvidence(evidence);
  if (errors.length) throw new Error(errors.map(error => `design-context canary evidence: ${error}`).join("\n"));
  await writeExclusiveDurable(outputPath, `${JSON.stringify(evidence, null, 2)}\n`);
  return evidence;
}

export function summarizeCanaryLedger(records, now = new Date()) {
  const session = records.find(record => record.type === "session.started")?.payload;
  const samples = records.filter(record => record.type === "publish.samples")
    .flatMap(record => Array.isArray(record.payload?.samples) ? record.payload.samples : []);
  const counts = Object.fromEntries([...new Set(records.map(record => record.type))]
    .sort().map(type => [type, records.filter(record => record.type === type).length]));
  return {
    schemaVersion: "design-context-canary-ledger-status@1",
    recordCount: records.length,
    headHash: records.at(-1)?.recordHash ?? null,
    observationElapsedMinutes: session?.window?.observationStartedAt
      ? Math.max(0, (now.getTime() - Date.parse(session.window.observationStartedAt)) / 60_000)
      : 0,
    baselinePublishCount: samples.filter(sample => sample.mode === "baseline").length,
    enforcedPublishCount: samples.filter(sample => sample.mode === "enforced").length,
    recordTypes: counts,
  };
}

function parseCli(argv) {
  const [command, ...rest] = argv;
  const options = {};
  for (let index = 0; index < rest.length; index += 2) {
    const name = rest[index];
    const value = rest[index + 1];
    if (!name?.startsWith("--") || value === undefined) throw new Error(`invalid argument: ${name ?? "missing"}`);
    options[name.slice(2)] = value;
  }
  return { command, options };
}

async function main() {
  const { command, options } = parseCli(process.argv.slice(2));
  if (command === "init" && options.ledger && options.config) {
    const record = await initializeCanaryLedger({ ledgerPath: options.ledger, configPath: options.config });
    process.stdout.write(`Canary ledger initialized: ${options.ledger} head=${record.recordHash}\n`);
    return;
  }
  if (command === "append" && options.ledger && options.event) {
    const record = await appendCanaryEvent({ ledgerPath: options.ledger, eventPath: options.event });
    process.stdout.write(`Canary event appended: sequence=${record.sequence} type=${record.type} head=${record.recordHash}\n`);
    return;
  }
  if (command === "status" && options.ledger) {
    process.stdout.write(`${JSON.stringify(summarizeCanaryLedger(await readCanaryLedger(options.ledger)), null, 2)}\n`);
    return;
  }
  if (command === "finalize" && options.ledger && options.output) {
    await finalizeCanaryLedger({ ledgerPath: options.ledger, outputPath: options.output });
    process.stdout.write(`Design-context canary evidence finalized: ${options.output}\n`);
    return;
  }
  throw new Error("usage: design-context-canary-ledger.mjs init --ledger <path> --config <json> | append --ledger <path> --event <json> | status --ledger <path> | finalize --ledger <path> --output <json>");
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  await main();
}
