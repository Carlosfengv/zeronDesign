#!/usr/bin/env node

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

function allDistinct(values) {
  return values.every(hasText) && new Set(values).size === values.length;
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
  if (!sameCohort(run, cohort)) fail(errors, `${mode} Website run does not match the exact canary cohort`);
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
