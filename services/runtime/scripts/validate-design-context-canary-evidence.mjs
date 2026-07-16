#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

function fail(errors, message) {
  errors.push(message);
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function sha256(value) {
  return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
}

function imageDigest(value) {
  return typeof value === "string" && /^sha256:[a-f0-9]{64}$/.test(value);
}

function positiveInteger(value) {
  return Number.isInteger(value) && value > 0;
}

function isoTimestamp(value) {
  return hasText(value) && Number.isFinite(Date.parse(value));
}

function sameCohort(record, cohort) {
  return record?.projectId === cohort?.projectId
    && record?.designProfileId === cohort?.designProfileId
    && record?.designProfileVersion === cohort?.designProfileVersion
    && record?.policyRevision === cohort?.policyRevision;
}

function sameWebsiteCohort(record, cohort, mode) {
  const expectedPolicyRevision = mode === "observe"
    ? cohort?.observePolicyRevision
    : cohort?.policyRevision;
  return record?.projectId === cohort?.projectId
    && record?.designProfileId === cohort?.designProfileId
    && record?.designProfileVersion === cohort?.designProfileVersion
    && record?.policyRevision === expectedPolicyRevision;
}

function allDistinct(values) {
  return values.every(hasText) && new Set(values).size === values.length;
}

export function canonicalCanaryJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalCanaryJson).join(",")}]`;
  return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonicalCanaryJson(value[key])}`).join(",")}}`;
}

export function canarySha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function createCanaryLedgerRecord({ sequence, type, recordedAt, sourceUri, sourceSha256, payload, previousRecordHash }) {
  const record = {
    schemaVersion: "design-context-canary-ledger-record@1",
    sequence,
    type,
    recordedAt,
    source: { uri: sourceUri, sha256: sourceSha256 },
    payload,
    previousRecordHash,
  };
  return { ...record, recordHash: canarySha256(canonicalCanaryJson(record)) };
}

export function buildCanarySourceLedger(records) {
  return {
    schemaVersion: "design-context-canary-source-ledger@1",
    recordCount: records.length,
    headHash: records.at(-1)?.recordHash ?? null,
    recordsHash: canarySha256(canonicalCanaryJson(records)),
    records,
  };
}

export function validateCanaryLedgerChain(records) {
  const errors = [];
  let previousRecordHash = null;
  let previousRecordedAt = Number.NEGATIVE_INFINITY;
  const sourceUris = new Set();
  for (const [index, record] of records.entries()) {
    const unsigned = {
      schemaVersion: record?.schemaVersion,
      sequence: record?.sequence,
      type: record?.type,
      recordedAt: record?.recordedAt,
      source: record?.source,
      payload: record?.payload,
      previousRecordHash: record?.previousRecordHash,
    };
    if (record?.schemaVersion !== "design-context-canary-ledger-record@1" || record?.sequence !== index + 1
      || !hasText(record?.type) || !isoTimestamp(record?.recordedAt) || !hasText(record?.source?.uri)
      || !sha256(record?.source?.sha256) || record?.previousRecordHash !== previousRecordHash
      || record?.recordHash !== canarySha256(canonicalCanaryJson(unsigned))) {
      errors.push(`sourceLedger record ${index + 1} is invalid or breaks the hash chain`);
    }
    const recordedAt = Date.parse(record?.recordedAt);
    if (recordedAt < previousRecordedAt) {
      errors.push(`sourceLedger record ${index + 1} moves recordedAt backwards`);
    }
    previousRecordedAt = recordedAt;
    if (sourceUris.has(record?.source?.uri)) {
      errors.push(`sourceLedger record ${index + 1} reuses a source URI`);
    }
    sourceUris.add(record?.source?.uri);
    previousRecordHash = record?.recordHash;
  }
  return errors;
}

function sameJson(left, right) {
  return canonicalCanaryJson(left) === canonicalCanaryJson(right);
}

function validateSourceLedger(evidence, errors) {
  const ledger = evidence?.sourceLedger;
  const records = Array.isArray(ledger?.records) ? ledger.records : [];
  if (ledger?.schemaVersion !== "design-context-canary-source-ledger@1" || records.length === 0) {
    fail(errors, "sourceLedger with hash-chained source records is required");
    return;
  }
  if (ledger.recordCount !== records.length || ledger.recordsHash !== canarySha256(canonicalCanaryJson(records))
    || ledger.headHash !== records.at(-1)?.recordHash) {
    fail(errors, "sourceLedger summary does not match its embedded records");
  }
  errors.push(...validateCanaryLedgerChain(records));

  const byType = type => records.filter(record => record.type === type).map(record => record.payload);
  const singleton = type => {
    const values = byType(type);
    if (values.length !== 1) fail(errors, `sourceLedger must contain exactly one ${type} record`);
    return values[0];
  };
  const latest = type => {
    const values = byType(type);
    if (values.length === 0) fail(errors, `sourceLedger must contain at least one ${type} record`);
    return values.at(-1);
  };
  const session = singleton("session.started");
  const websiteRuns = byType("website.run");
  const requiredFindingRepair = singleton("required-finding.repair");
  const cleanApply = singleton("profile-sync.clean-apply");
  const conflictApply = singleton("profile-sync.conflict-apply");
  const planMismatch = singleton("profile-sync.plan-mismatch");
  const recovery = singleton("profile-sync.recovery");
  const bffRuntime = singleton("bff-runtime");
  const alertDestinationProbe = singleton("alert.destination-probe");
  const metricsSnapshot = latest("metrics.snapshot");
  const rollback = singleton("rollback");
  const compatibility = singleton("compatibility");
  if (!session || !sameJson(session.provider, evidence?.provider) || !sameJson(session.cohort, evidence?.cohort)
    || !sameJson(session.images, evidence?.images)) {
    fail(errors, "sourceLedger session does not match provider, cohort and image identity");
  }
  if (websiteRuns.length !== 2 || !sameJson(websiteRuns, evidence?.websiteRuns)) {
    fail(errors, "sourceLedger Website run records do not match the final evidence");
  }
  if (!alertDestinationProbe || !hasText(alertDestinationProbe.destinationId)
    || !hasText(alertDestinationProbe.operatorId) || !sha256(alertDestinationProbe.eventId)
    || alertDestinationProbe.probeStatus !== "delivered" || alertDestinationProbe.attemptCount !== 1
    || !Number.isInteger(alertDestinationProbe.responseStatus)
    || alertDestinationProbe.responseStatus < 200 || alertDestinationProbe.responseStatus >= 300) {
    fail(errors, "sourceLedger must prove one successful alert destination readiness probe");
  }
  const alertProbeRecord = records.find(record => record.type === "alert.destination-probe");
  const firstCanaryWorkRecord = records.find(record => ["website.run", "metrics.snapshot"].includes(record.type));
  if (!alertProbeRecord || !firstCanaryWorkRecord || alertProbeRecord.sequence >= firstCanaryWorkRecord.sequence) {
    fail(errors, "sourceLedger alert destination readiness probe must precede canary workloads and metrics collection");
  }
  for (const [name, sourceValue, evidenceValue] of [
    ["required finding", requiredFindingRepair, evidence?.requiredFindingRepair],
    ["clean Profile Sync", cleanApply, evidence?.profileSync?.cleanApply],
    ["conflict Profile Sync", conflictApply, evidence?.profileSync?.conflictApply],
    ["plan mismatch", planMismatch, evidence?.profileSync?.planMismatch],
    ["Profile Sync recovery", recovery, evidence?.profileSync?.recovery],
    ["BFF Runtime", bffRuntime, evidence?.bffRuntime],
    ["rollback", rollback, evidence?.rollback],
    ["compatibility", compatibility, evidence?.compatibility],
  ]) {
    if (!sourceValue || !sameJson(sourceValue, evidenceValue)) {
      fail(errors, `sourceLedger ${name} record does not match the final evidence`);
    }
  }

  const samples = byType("publish.samples").flatMap(value => Array.isArray(value?.samples) ? value.samples : []);
  if (samples.length === 0 || !allDistinct(samples.map(sample => sample?.sampleId))) {
    fail(errors, "sourceLedger publish samples must contain distinct sample ids");
  }
  const baselineStart = Date.parse(evidence?.metrics?.baselineStartedAt);
  const baselineEnd = Date.parse(evidence?.metrics?.baselineEndedAt);
  const observationStart = Date.parse(evidence?.metrics?.observationStartedAt);
  const observationEnd = Date.parse(evidence?.metrics?.observationEndedAt);
  let baselineCount = 0;
  let baselineFailureCount = 0;
  let enforcedCount = 0;
  let enforcedDcpFailureCount = 0;
  for (const sample of samples) {
    const observedAt = Date.parse(sample?.observedAt);
    const commonValid = hasText(sample?.runId) && isoTimestamp(sample?.observedAt)
      && ["pass", "fail"].includes(sample?.publishVerdict) && typeof sample?.dcpCausedFailure === "boolean"
      && sameWebsiteCohort(sample, evidence?.cohort, sample?.mode === "baseline" ? "observe" : "enforced");
    if (sample?.mode === "baseline") {
      baselineCount += 1;
      if (sample.publishVerdict === "fail") baselineFailureCount += 1;
      if (!commonValid || sample.dcpCausedFailure !== false || observedAt < baselineStart || observedAt > baselineEnd) {
        fail(errors, `invalid baseline publish sample: ${sample?.sampleId ?? "unknown"}`);
      }
    } else if (sample?.mode === "enforced") {
      enforcedCount += 1;
      if (sample.publishVerdict === "fail" && sample.dcpCausedFailure) enforcedDcpFailureCount += 1;
      if (!commonValid || observedAt < observationStart || observedAt > observationEnd) {
        fail(errors, `invalid enforced publish sample: ${sample?.sampleId ?? "unknown"}`);
      }
    } else {
      fail(errors, `invalid publish sample mode: ${sample?.sampleId ?? "unknown"}`);
    }
  }
  const computedDelta = baselineCount && enforcedCount
    ? ((enforcedDcpFailureCount / enforcedCount) - (baselineFailureCount / baselineCount)) * 100
    : Number.NaN;
  if (evidence?.metrics?.baselinePublishCount !== baselineCount
    || evidence?.metrics?.enforcedPublishCount !== enforcedCount
    || !Number.isFinite(computedDelta)
    || Math.abs(evidence?.metrics?.publishFailureRateDeltaPp - computedDelta) > 1e-9) {
    fail(errors, "canary publish counts or failure-rate delta do not match sourceLedger samples");
  }
  const operationalMetrics = evidence?.metrics ? {
    observationEndedAt: evidence.metrics.observationEndedAt,
    conclusionRecordedBy: evidence.metrics.conclusionRecordedBy,
    verifierUnavailableCount: evidence.metrics.verifierUnavailableCount,
    verifierRuntimeLostCount: evidence.metrics.verifierRuntimeLostCount,
    unexpectedReadGateBlockCount: evidence.metrics.unexpectedReadGateBlockCount,
    recoveryRequiredOver24hCount: evidence.metrics.recoveryRequiredOver24hCount,
    requiredFindingRepairRate: evidence.metrics.requiredFindingRepairRate,
    alertsTriggered: evidence.metrics.alertsTriggered,
  } : null;
  if (!metricsSnapshot || !sameJson(metricsSnapshot, operationalMetrics)
    || !session?.window || session.window.baselineStartedAt !== evidence?.metrics?.baselineStartedAt
    || session.window.baselineEndedAt !== evidence?.metrics?.baselineEndedAt
    || session.window.observationStartedAt !== evidence?.metrics?.observationStartedAt) {
    fail(errors, "sourceLedger metric snapshot or immutable observation window does not match final metrics");
  }

  const metricRecords = records.filter(record => record.type === "metrics.snapshot");
  const alertDeliveryRecords = records.filter(record => record.type === "alert.delivery");
  const alertDeliveries = alertDeliveryRecords.map(record => record.payload);
  if (alertDeliveryRecords.length !== metricRecords.length) {
    fail(errors, "sourceLedger requires one alert.delivery decision for every metrics.snapshot");
  }
  for (const delivery of alertDeliveries) {
    const deliveryCohort = delivery?.cohort;
    const codes = Array.isArray(delivery?.triggeredAlertCodes) ? delivery.triggeredAlertCodes : [];
    if (!isoTimestamp(delivery?.operationalExportGeneratedAt) || !isoTimestamp(delivery?.observationEndedAt)
      || !sha256(delivery?.eventId) || !hasText(delivery?.destinationId)
      || deliveryCohort?.projectId !== evidence?.cohort?.projectId
      || deliveryCohort?.designProfileId !== evidence?.cohort?.designProfileId
      || deliveryCohort?.designProfileVersion !== evidence?.cohort?.designProfileVersion
      || deliveryCohort?.observePolicyRevision !== evidence?.cohort?.observePolicyRevision
      || deliveryCohort?.policyRevision !== evidence?.cohort?.policyRevision
      || typeof delivery?.deliveryRequired !== "boolean"
      || codes.some(code => !hasText(code)) || new Set(codes).size !== codes.length) {
      fail(errors, "sourceLedger alert delivery record is invalid or does not match the exact cohort");
    }
  }
  for (const metricRecord of metricRecords) {
    const matchingRecords = alertDeliveryRecords.filter(record => record.payload?.operationalExportGeneratedAt === metricRecord.recordedAt);
    if (matchingRecords.length !== 1) {
      fail(errors, `sourceLedger metrics snapshot ${metricRecord.recordedAt} requires one matching alert delivery decision`);
      continue;
    }
    const deliveryRecord = matchingRecords[0];
    const delivery = deliveryRecord.payload;
    if (deliveryRecord.sequence <= metricRecord.sequence || Date.parse(deliveryRecord.recordedAt) < Date.parse(metricRecord.recordedAt)) {
      fail(errors, `sourceLedger alert delivery decision must follow metrics snapshot ${metricRecord.recordedAt}`);
    }
    const codes = Array.isArray(delivery?.triggeredAlertCodes) ? delivery.triggeredAlertCodes : [];
    if (metricRecord.payload?.alertsTriggered === true) {
      if (delivery.deliveryRequired !== true || delivery.deliveryStatus !== "delivered"
        || delivery.attemptCount !== 1 || !Number.isInteger(delivery.responseStatus)
        || delivery.responseStatus < 200 || delivery.responseStatus >= 300 || codes.length === 0) {
        fail(errors, `sourceLedger triggered metrics snapshot ${metricRecord.recordedAt} lacks successful external alert delivery`);
      }
    } else if (delivery.deliveryRequired !== false || delivery.deliveryStatus !== "not-required"
      || delivery.attemptCount !== 0 || delivery.responseStatus !== null || codes.length !== 0) {
      fail(errors, `sourceLedger quiet metrics snapshot ${metricRecord.recordedAt} lacks an explicit no-delivery decision`);
    }
  }
}

function passRun(run, mode, cohort, errors) {
  if (!run) {
    fail(errors, `${mode} Website run evidence is missing`);
    return;
  }
  for (const field of ["runId", "candidateVersionId", "workerProvider", "workerVersion", "verificationPolicyId", "artifactUri", "evidenceUri"]) {
    if (!hasText(run[field])) fail(errors, `${mode}.${field} is required`);
  }
  for (const field of ["candidateManifestHash", "designContextContentHash", "artifactManifestHash", "materializationHash", "verifierCapabilitySnapshotHash"]) {
    if (!sha256(run[field])) fail(errors, `${mode}.${field} must be sha256`);
  }
  if (run.artifactManifestHash !== run.materializationHash) {
    fail(errors, `${mode} DCP materialization hash must match its artifact manifest hash`);
  }
  if (!sameWebsiteCohort(run, cohort, mode)) fail(errors, `${mode} Website run does not match its exact canary policy revision`);
  const policy = run.enforcementPolicy;
  const expectedRevision = mode === "observe" ? cohort?.observePolicyRevision : cohort?.policyRevision;
  const expectedEnabled = mode === "enforced";
  if (policy?.source !== "persistent" || policy?.enabled !== expectedEnabled
    || policy?.policyRevision !== expectedRevision || !hasText(policy?.policyUpdatedBy)) {
    fail(errors, `${mode} Website run must carry its frozen persistent enforcement policy binding`);
  }
  const requiredReads = Array.isArray(run.requiredReadPaths) ? run.requiredReadPaths : [];
  const readFiles = Array.isArray(run.readFiles) ? run.readFiles : [];
  if (!requiredReads.length || requiredReads.some(path => !hasText(path) || !readFiles.includes(path))) {
    fail(errors, `${mode} Website run must list every required DCP read and its recorded read evidence`);
  }
  if (run.status !== "completed" || run.publishVerdict !== "pass" || run.requiredReadsPassed !== true) {
    fail(errors, `${mode} Website run must complete with all required reads and a passing publish verdict`);
  }
}

function appliedProfileSync(record, name, cohort, errors) {
  if (!record) {
    fail(errors, `profileSync.${name} is missing`);
    return;
  }
  for (const field of ["operationId", "sourceRunId", "childRunId"]) {
    if (!hasText(record[field])) fail(errors, `profileSync.${name}.${field} is required`);
  }
  for (const field of ["planHash", "beforeTokenSnapshotHash", "afterTokenSnapshotHash"]) {
    if (!sha256(record[field])) fail(errors, `profileSync.${name}.${field} must be sha256`);
  }
  if (!sameCohort(record, cohort)) fail(errors, `profileSync.${name} does not match the exact canary cohort`);
  if (record.beforeTokenSnapshotHash === record.afterTokenSnapshotHash) {
    fail(errors, `profileSync.${name} must record a real before/after token change`);
  }
  if (record.status !== "applied") fail(errors, `profileSync.${name} must be applied`);
}

export function validateDesignContextCanaryEvidence(evidence) {
  const errors = [];
  if (evidence?.schemaVersion !== "design-context-canary-evidence@1") {
    fail(errors, "schemaVersion must be design-context-canary-evidence@1");
  }
  if (evidence?.result !== "pass") fail(errors, "result must be pass");
  if (!isoTimestamp(evidence?.recordedAt)) fail(errors, "recordedAt must be an ISO timestamp");

  const provider = evidence?.provider;
  if (provider?.mode !== "approved-real" || !hasText(provider?.name) || !hasText(provider?.model)
    || !hasText(provider?.approvalReference) || provider?.credentialPresent !== true) {
    fail(errors, "approved real provider evidence is required");
  }

  const cohort = evidence?.cohort;
  for (const field of ["projectId", "designProfileId", "policyUpdatedBy"]) {
    if (!hasText(cohort?.[field])) fail(errors, `cohort.${field} is required`);
  }
  for (const field of ["designProfileVersion", "policyRevision"]) {
    if (!positiveInteger(cohort?.[field])) fail(errors, `cohort.${field} must be a positive integer`);
  }
  if (!positiveInteger(cohort?.observePolicyRevision)
    || cohort.observePolicyRevision >= cohort?.policyRevision) {
    fail(errors, "cohort.observePolicyRevision must precede the enforced policyRevision");
  }
  if (!hasText(cohort?.thresholdVersion)) fail(errors, "cohort.thresholdVersion is required");

  for (const service of ["runtime", "bff"]) {
    const image = evidence?.images?.[service];
    if (!hasText(image?.ref) || !imageDigest(image?.manifestDigest) || !imageDigest(image?.configDigest)) {
      fail(errors, `images.${service} ref, manifestDigest and configDigest are required`);
    }
  }

  const runs = Array.isArray(evidence?.websiteRuns) ? evidence.websiteRuns : [];
  const observeRun = runs.find(run => run?.mode === "observe");
  const enforcedRun = runs.find(run => run?.mode === "enforced");
  if (runs.length !== 2 || runs.filter(run => run?.mode === "observe").length !== 1
    || runs.filter(run => run?.mode === "enforced").length !== 1) {
    fail(errors, "websiteRuns must contain exactly one observe Run and one enforced Run");
  }
  passRun(observeRun, "observe", cohort, errors);
  passRun(enforcedRun, "enforced", cohort, errors);
  if (!allDistinct([observeRun?.runId, enforcedRun?.runId])) {
    fail(errors, "observe and enforced Website evidence must use distinct Run ids");
  }
  if (!allDistinct([observeRun?.candidateVersionId, enforcedRun?.candidateVersionId])) {
    fail(errors, "observe and enforced Website evidence must use distinct candidate versions");
  }

  const repair = evidence?.requiredFindingRepair;
  if (!repair || !hasText(repair.ruleId) || !hasText(repair.findingId) || repair.findingStatus !== "fixed"
    || !hasText(repair.blockedRunId) || !hasText(repair.reviewRunId) || !hasText(repair.repairRunId)
    || !sha256(repair.blockedCandidateManifestHash) || !sha256(repair.promotedCandidateManifestHash)
    || repair.blockedCandidateManifestHash === repair.promotedCandidateManifestHash
    || !allDistinct([repair.blockedRunId, repair.reviewRunId, repair.repairRunId])
    || !sameCohort(repair, cohort) || repair.blockedPublish !== true
    || repair.repairPublishVerdict !== "pass" || repair.promoted !== true) {
    fail(errors, "required finding block-repair-promotion evidence is missing or invalid");
  }

  const profileSync = evidence?.profileSync;
  appliedProfileSync(profileSync?.cleanApply, "cleanApply", cohort, errors);
  appliedProfileSync(profileSync?.conflictApply, "conflictApply", cohort, errors);
  if (!positiveInteger(profileSync?.conflictApply?.conflictDecisionCount)) {
    fail(errors, "profileSync.conflictApply requires at least one explicit conflict decision");
  }
  if (profileSync?.planMismatch?.rejected !== true || profileSync?.planMismatch?.errorCode !== "profile_sync_plan_mismatch") {
    fail(errors, "profileSync.planMismatch must reject with profile_sync_plan_mismatch");
  }
  appliedProfileSync(profileSync?.recovery, "recovery", cohort, errors);
  if (profileSync?.recovery?.recoveryRequiredObserved !== true || profileSync?.recovery?.reusedChildRun !== true) {
    fail(errors, "profileSync.recovery must reuse its child Run");
  }
  const syncRecords = [profileSync?.cleanApply, profileSync?.conflictApply, profileSync?.recovery];
  if (!allDistinct(syncRecords.map(record => record?.operationId))) {
    fail(errors, "profileSync apply/recovery evidence must use distinct operation ids");
  }
  if (!allDistinct(syncRecords.map(record => record?.childRunId))) {
    fail(errors, "profileSync apply/recovery evidence must use distinct child Run ids");
  }

  const bff = evidence?.bffRuntime;
  if (!bff || !hasText(bff.projectId) || !hasText(bff.runId) || !hasText(bff.principalProjectId)
    || !hasText(bff.operationId) || !hasText(bff.childRunId) || !sha256(bff.planHash)
    || bff.childRunUnique !== true) {
    fail(errors, "BFF to Runtime Profile Sync ownership and idempotency evidence is missing");
  }
  if (bff?.projectId !== cohort?.projectId || bff?.principalProjectId !== cohort?.projectId) {
    fail(errors, "BFF principal project does not match the canary project");
  }
  if (bff?.runId !== profileSync?.conflictApply?.sourceRunId
    || bff?.operationId !== profileSync?.conflictApply?.operationId
    || bff?.childRunId !== profileSync?.conflictApply?.childRunId
    || bff?.planHash !== profileSync?.conflictApply?.planHash) {
    fail(errors, "BFF evidence does not identify the recorded conflict Profile Sync operation");
  }

  const metrics = evidence?.metrics;
  const baselineStartedAt = Date.parse(metrics?.baselineStartedAt);
  const baselineEndedAt = Date.parse(metrics?.baselineEndedAt);
  const observationStartedAt = Date.parse(metrics?.observationStartedAt);
  const observationEndedAt = Date.parse(metrics?.observationEndedAt);
  const actualObservationMinutes = (observationEndedAt - observationStartedAt) / 60_000;
  if (!metrics || !isoTimestamp(metrics.baselineStartedAt) || !isoTimestamp(metrics.baselineEndedAt)
    || !isoTimestamp(metrics.observationStartedAt) || !isoTimestamp(metrics.observationEndedAt)
    || baselineEndedAt <= baselineStartedAt || observationEndedAt <= observationStartedAt
    || baselineEndedAt > observationStartedAt
    || !positiveInteger(metrics.observationWindowMinutes)
    || actualObservationMinutes < 7 * 24 * 60
    || Math.abs(actualObservationMinutes - metrics.observationWindowMinutes) > 1
    || !positiveInteger(metrics.enforcedPublishCount) || metrics.enforcedPublishCount < 30
    || !positiveInteger(metrics.baselinePublishCount) || !hasText(metrics.conclusionRecordedBy)
    || metrics.verifierUnavailableCount !== 0 || metrics.verifierRuntimeLostCount !== 0
    || metrics.unexpectedReadGateBlockCount !== 0 || metrics.recoveryRequiredOver24hCount !== 0
    || metrics.requiredFindingRepairRate !== 1
    || typeof metrics.publishFailureRateDeltaPp !== "number" || metrics.publishFailureRateDeltaPp > 2
    || metrics.alertsTriggered !== false) {
    fail(errors, "canary metrics window, thresholds, or alerts are invalid");
  }
  if (isoTimestamp(evidence?.recordedAt) && isoTimestamp(metrics?.observationEndedAt)
    && Date.parse(evidence.recordedAt) < observationEndedAt) {
    fail(errors, "recordedAt cannot precede the completed canary observation window");
  }

  const rollback = evidence?.rollback;
  if (!rollback || !hasText(rollback.updatedBy) || !hasText(rollback.postRollbackRunId)
    || !isoTimestamp(rollback.recordedAt) || !sameCohort(rollback, cohort)
    || rollback.policyEnabledAfterRollback !== false || !positiveInteger(rollback.policyRevisionAfterRollback)
    || rollback.policyRevisionAfterRollback <= cohort?.policyRevision || rollback.postRollbackMode !== "observe"
    || rollback.newRunReadGateBlocked !== false || rollback.operationRecoveryPreserved !== true) {
    fail(errors, "exact policy enabled=false rollback evidence is missing or invalid");
  }
  if (isoTimestamp(rollback?.recordedAt) && isoTimestamp(metrics?.observationEndedAt)
    && Date.parse(rollback.recordedAt) < observationEndedAt) {
    fail(errors, "rollback cannot precede the completed canary observation window");
  }
  if (isoTimestamp(evidence?.recordedAt) && isoTimestamp(rollback?.recordedAt)
    && Date.parse(evidence.recordedAt) < Date.parse(rollback.recordedAt)) {
    fail(errors, "recordedAt cannot precede the rollback evidence");
  }

  const compatibility = evidence?.compatibility;
  for (const field of ["noProfileWebsitePassed", "docsBuildPassed", "legacyEditRepairPassed"]) {
    if (compatibility?.[field] !== true) fail(errors, `compatibility.${field} must be true`);
  }
  for (const field of ["noProfileWebsiteRunId", "docsBuildRunId", "legacyEditRunId", "legacyRepairRunId"]) {
    if (!hasText(compatibility?.[field])) fail(errors, `compatibility.${field} is required`);
  }
  if (!allDistinct([
    compatibility?.noProfileWebsiteRunId,
    compatibility?.docsBuildRunId,
    compatibility?.legacyEditRunId,
    compatibility?.legacyRepairRunId,
  ])) {
    fail(errors, "compatibility evidence must identify four distinct Runs");
  }
  validateSourceLedger(evidence, errors);
  return errors;
}

async function main() {
  const path = process.argv[2];
  if (!path) throw new Error("usage: validate-design-context-canary-evidence.mjs <design-context-canary-evidence.json>");
  const evidence = JSON.parse(await readFile(path, "utf8"));
  const errors = validateDesignContextCanaryEvidence(evidence);
  if (errors.length) {
    for (const error of errors) process.stderr.write(`design-context canary evidence: ${error}\n`);
    process.exitCode = 1;
    return;
  }
  process.stdout.write(`Design-context canary evidence valid: ${path}\n`);
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  await main();
}
