#!/usr/bin/env node

import assert from "node:assert/strict";
import {
  buildCanarySourceLedger,
  createCanaryLedgerRecord,
  validateDesignContextCanaryEvidence,
} from "./validate-design-context-canary-evidence.mjs";

const sha = "a".repeat(64);
const image = { ref: "registry.example/runtime:v1", manifestDigest: `sha256:${sha}`, configDigest: `sha256:${sha}` };
const cohort = {
  projectId: "project-1",
  designProfileId: "profile-1",
  designProfileVersion: 1,
  observePolicyRevision: 2,
  policyRevision: 3,
  policyUpdatedBy: "operator-1",
  thresholdVersion: "website-dcp-canary-thresholds@1",
};
const applied = (operationId, sourceRunId, childRunId) => ({
  ...cohort,
  operationId,
  sourceRunId,
  childRunId,
  planHash: sha,
  beforeTokenSnapshotHash: sha,
  afterTokenSnapshotHash: "b".repeat(64),
  status: "applied",
});
const run = mode => ({
  ...cohort,
  policyRevision: mode === "observe" ? cohort.observePolicyRevision : cohort.policyRevision,
  mode,
  runId: `run-${mode}`,
  candidateVersionId: `version-${mode}`,
  candidateManifestHash: sha,
  designContextContentHash: sha,
  artifactManifestHash: sha,
  materializationHash: sha,
  verifierCapabilitySnapshotHash: sha,
  workerProvider: "chrome",
  workerVersion: "127.0.0",
  verificationPolicyId: "website-verification@1",
  artifactUri: `artifact://run-${mode}`,
  evidenceUri: `evidence://run-${mode}`,
  requiredReadPaths: ["inputs/design-profile.json"],
  readFiles: ["inputs/design-profile.json"],
  status: "completed",
  publishVerdict: "pass",
  requiredReadsPassed: true,
  enforcementPolicy: {
    source: "persistent",
    enabled: mode === "enforced",
    policyRevision: mode === "observe" ? cohort.observePolicyRevision : cohort.policyRevision,
    policyUpdatedBy: "operator-1",
  },
});

const fixture = {
  schemaVersion: "design-context-canary-evidence@1",
  result: "pass",
  recordedAt: "2026-07-15T02:00:00Z",
  provider: { mode: "approved-real", name: "provider", model: "provider-model", approvalReference: "APP-1", credentialPresent: true },
  cohort,
  images: { runtime: image, bff: { ...image, ref: "registry.example/bff:v1" } },
  websiteRuns: [run("observe"), run("enforced")],
  requiredFindingRepair: {
    ...cohort,
    ruleId: "a11y.button-name",
    findingId: "finding-1",
    findingStatus: "fixed",
    blockedRunId: "run-blocked",
    reviewRunId: "run-review",
    repairRunId: "run-repair",
    blockedCandidateManifestHash: sha,
    promotedCandidateManifestHash: "b".repeat(64),
    blockedPublish: true,
    repairPublishVerdict: "pass",
    promoted: true,
  },
  profileSync: {
    cleanApply: applied("operation-1", "run-clean-source", "run-clean-child"),
    conflictApply: { ...applied("operation-2", "run-conflict-source", "run-conflict-child"), conflictDecisionCount: 1 },
    planMismatch: { rejected: true, errorCode: "profile_sync_plan_mismatch" },
    recovery: {
      ...applied("operation-3", "run-recovery-source", "run-recovery-child"),
      recoveryRequiredObserved: true,
      reusedChildRun: true,
    },
  },
  bffRuntime: {
    projectId: "project-1",
    runId: "run-conflict-source",
    principalProjectId: "project-1",
    operationId: "operation-2",
    childRunId: "run-conflict-child",
    planHash: sha,
    childRunUnique: true,
  },
  metrics: {
    baselineStartedAt: "2026-07-01T00:00:00Z",
    baselineEndedAt: "2026-07-08T00:00:00Z",
    observationStartedAt: "2026-07-08T00:00:00Z",
    observationEndedAt: "2026-07-15T00:00:00Z",
    observationWindowMinutes: 7 * 24 * 60,
    baselinePublishCount: 30,
    enforcedPublishCount: 30,
    conclusionRecordedBy: "operator-1",
    verifierUnavailableCount: 0,
    verifierRuntimeLostCount: 0,
    unexpectedReadGateBlockCount: 0,
    recoveryRequiredOver24hCount: 0,
    requiredFindingRepairRate: 1,
    publishFailureRateDeltaPp: 0,
    alertsTriggered: false,
  },
  rollback: {
    ...cohort,
    updatedBy: "operator-1",
    recordedAt: "2026-07-15T01:00:00Z",
    postRollbackRunId: "run-post-rollback",
    policyEnabledAfterRollback: false,
    policyRevisionAfterRollback: 4,
    postRollbackMode: "observe",
    newRunReadGateBlocked: false,
    operationRecoveryPreserved: true,
  },
  compatibility: {
    noProfileWebsitePassed: true,
    noProfileWebsiteRunId: "run-no-profile",
    docsBuildPassed: true,
    docsBuildRunId: "run-docs",
    legacyEditRepairPassed: true,
    legacyEditRunId: "run-legacy-edit",
    legacyRepairRunId: "run-legacy-repair",
  },
};

function attachSourceLedger(evidence) {
  const records = [];
  const append = (type, recordedAt, payload) => {
    records.push(createCanaryLedgerRecord({
      sequence: records.length + 1,
      type,
      recordedAt,
      sourceUri: `evidence://canary-fixture/${records.length + 1}`,
      sourceSha256: sha,
      payload,
      previousRecordHash: records.at(-1)?.recordHash ?? null,
    }));
  };
  append("session.started", "2026-07-08T00:00:00Z", {
    provider: evidence.provider,
    cohort: evidence.cohort,
    images: evidence.images,
    window: {
      baselineStartedAt: evidence.metrics.baselineStartedAt,
      baselineEndedAt: evidence.metrics.baselineEndedAt,
      observationStartedAt: evidence.metrics.observationStartedAt,
    },
  });
  append("alert.destination-probe", "2026-07-08T00:01:00Z", {
    destinationId: "primary-oncall",
    operatorId: "operator-1",
    eventId: "c".repeat(64),
    probeStatus: "delivered",
    responseStatus: 204,
    attemptCount: 1,
  });
  append("website.run", "2026-07-08T00:05:00Z", evidence.websiteRuns[0]);
  append("website.run", "2026-07-08T00:06:00Z", evidence.websiteRuns[1]);
  append("required-finding.repair", "2026-07-08T01:00:00Z", evidence.requiredFindingRepair);
  append("profile-sync.clean-apply", "2026-07-08T02:00:00Z", evidence.profileSync.cleanApply);
  append("profile-sync.conflict-apply", "2026-07-08T03:00:00Z", evidence.profileSync.conflictApply);
  append("profile-sync.plan-mismatch", "2026-07-08T04:00:00Z", evidence.profileSync.planMismatch);
  append("profile-sync.recovery", "2026-07-08T05:00:00Z", evidence.profileSync.recovery);
  append("bff-runtime", "2026-07-08T06:00:00Z", evidence.bffRuntime);
  const samples = [];
  for (let index = 0; index < 30; index += 1) {
    samples.push({
      ...evidence.cohort,
      policyRevision: evidence.cohort.observePolicyRevision,
      sampleId: `baseline-${index + 1}`,
      runId: `run-baseline-${index + 1}`,
      mode: "baseline",
      observedAt: `2026-07-${String(index < 15 ? 2 : 6).padStart(2, "0")}T${String(index % 15).padStart(2, "0")}:00:00Z`,
      publishVerdict: "pass",
      dcpCausedFailure: false,
    });
    samples.push({
      ...evidence.cohort,
      sampleId: `enforced-${index + 1}`,
      runId: `run-enforced-${index + 1}`,
      mode: "enforced",
      observedAt: `2026-07-${String(index < 15 ? 9 : 13).padStart(2, "0")}T${String(index % 15).padStart(2, "0")}:00:00Z`,
      publishVerdict: "pass",
      dcpCausedFailure: false,
    });
  }
  append("publish.samples", "2026-07-15T00:00:00Z", { samples });
  append("metrics.snapshot", "2026-07-15T00:01:00Z", {
    observationEndedAt: evidence.metrics.observationEndedAt,
    conclusionRecordedBy: evidence.metrics.conclusionRecordedBy,
    verifierUnavailableCount: evidence.metrics.verifierUnavailableCount,
    verifierRuntimeLostCount: evidence.metrics.verifierRuntimeLostCount,
    unexpectedReadGateBlockCount: evidence.metrics.unexpectedReadGateBlockCount,
    recoveryRequiredOver24hCount: evidence.metrics.recoveryRequiredOver24hCount,
    requiredFindingRepairRate: evidence.metrics.requiredFindingRepairRate,
    alertsTriggered: evidence.metrics.alertsTriggered,
  });
  append("alert.delivery", "2026-07-15T00:02:00Z", {
    operationalExportGeneratedAt: "2026-07-15T00:01:00Z",
    observationEndedAt: evidence.metrics.observationEndedAt,
    cohort: evidence.cohort,
    eventId: sha,
    destinationId: "primary-oncall",
    deliveryRequired: false,
    deliveryStatus: "not-required",
    responseStatus: null,
    attemptCount: 0,
    triggeredAlertCodes: [],
  });
  append("rollback", "2026-07-15T01:00:00Z", evidence.rollback);
  append("compatibility", "2026-07-15T01:10:00Z", evidence.compatibility);
  evidence.sourceLedger = buildCanarySourceLedger(records);
}

attachSourceLedger(fixture);

assert.deepEqual(validateDesignContextCanaryEvidence(fixture), []);
const noProvider = structuredClone(fixture);
noProvider.provider.mode = "fixture";
assert(validateDesignContextCanaryEvidence(noProvider).some(error => error.includes("approved real provider")));
const noRepair = structuredClone(fixture);
noRepair.requiredFindingRepair.promoted = false;
assert(validateDesignContextCanaryEvidence(noRepair).some(error => error.includes("block-repair-promotion")));
const noRollback = structuredClone(fixture);
noRollback.rollback.policyEnabledAfterRollback = true;
assert(validateDesignContextCanaryEvidence(noRollback).some(error => error.includes("rollback")));
const failedCanary = structuredClone(fixture);
failedCanary.metrics.verifierUnavailableCount = 1;
assert(validateDesignContextCanaryEvidence(failedCanary).some(error => error.includes("metrics")));
const shortCanary = structuredClone(fixture);
shortCanary.metrics.observationWindowMinutes = 60;
shortCanary.metrics.enforcedPublishCount = 5;
assert(validateDesignContextCanaryEvidence(shortCanary).some(error => error.includes("metrics")));
const wrongBffPrincipal = structuredClone(fixture);
wrongBffPrincipal.bffRuntime.principalProjectId = "project-2";
assert(validateDesignContextCanaryEvidence(wrongBffPrincipal).some(error => error.includes("principal project")));
const reusedWebsiteRun = structuredClone(fixture);
reusedWebsiteRun.websiteRuns[1].runId = reusedWebsiteRun.websiteRuns[0].runId;
assert(validateDesignContextCanaryEvidence(reusedWebsiteRun).some(error => error.includes("distinct Run")));
const reusedSyncChild = structuredClone(fixture);
reusedSyncChild.profileSync.conflictApply.childRunId = reusedSyncChild.profileSync.cleanApply.childRunId;
assert(validateDesignContextCanaryEvidence(reusedSyncChild).some(error => error.includes("distinct child Run")));
const unchangedTokenSnapshot = structuredClone(fixture);
unchangedTokenSnapshot.profileSync.cleanApply.afterTokenSnapshotHash = unchangedTokenSnapshot.profileSync.cleanApply.beforeTokenSnapshotHash;
assert(validateDesignContextCanaryEvidence(unchangedTokenSnapshot).some(error => error.includes("real before/after")));
const wrongBffOperation = structuredClone(fixture);
wrongBffOperation.bffRuntime.operationId = "operation-unrelated";
assert(validateDesignContextCanaryEvidence(wrongBffOperation).some(error => error.includes("recorded conflict")));
const reusedCandidate = structuredClone(fixture);
reusedCandidate.requiredFindingRepair.promotedCandidateManifestHash = reusedCandidate.requiredFindingRepair.blockedCandidateManifestHash;
assert(validateDesignContextCanaryEvidence(reusedCandidate).some(error => error.includes("block-repair-promotion")));
const tamperedLedger = structuredClone(fixture);
tamperedLedger.sourceLedger.records[1].payload.runId = "run-tampered";
assert(validateDesignContextCanaryEvidence(tamperedLedger).some(error => error.includes("hash chain")));
const forgedPublishCount = structuredClone(fixture);
forgedPublishCount.metrics.enforcedPublishCount = 31;
assert(validateDesignContextCanaryEvidence(forgedPublishCount).some(error => error.includes("sourceLedger samples")));
const impossibleSamePolicyRevision = structuredClone(fixture);
impossibleSamePolicyRevision.cohort.observePolicyRevision = impossibleSamePolicyRevision.cohort.policyRevision;
assert(validateDesignContextCanaryEvidence(impossibleSamePolicyRevision).some(error => error.includes("observePolicyRevision")));
const missingFrozenPolicy = structuredClone(fixture);
delete missingFrozenPolicy.websiteRuns[1].enforcementPolicy;
assert(validateDesignContextCanaryEvidence(missingFrozenPolicy).some(error => error.includes("frozen persistent enforcement policy")));
const missingAlertDecision = structuredClone(fixture);
missingAlertDecision.sourceLedger.records = missingAlertDecision.sourceLedger.records.filter(record => record.type !== "alert.delivery");
assert(validateDesignContextCanaryEvidence(missingAlertDecision).some(error => error.includes("one alert.delivery decision")));
const missingDestinationProbe = structuredClone(fixture);
missingDestinationProbe.sourceLedger.records = missingDestinationProbe.sourceLedger.records.filter(record => record.type !== "alert.destination-probe");
assert(validateDesignContextCanaryEvidence(missingDestinationProbe).some(error => error.includes("alert.destination-probe")));
process.stdout.write("design-context canary evidence validator tests passed\n");
