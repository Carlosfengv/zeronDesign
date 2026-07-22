#!/usr/bin/env node

import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  appendCanaryEvent,
  finalizeCanaryLedger,
  initializeCanaryLedger,
  readCanaryLedger,
  summarizeCanaryLedger,
} from "./design-context-canary-ledger.mjs";
import { validateDesignContextCanaryEvidence } from "./validate-design-context-canary-evidence.mjs";

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
const provider = {
  mode: "approved-real",
  name: "deepseek",
  model: "deepseek-chat",
  approvalReference: "APP-1",
  credentialPresent: true,
};
const images = { runtime: image, bff: { ...image, ref: "registry.example/bff:v1" } };
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
const websiteRun = mode => ({
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

const generationContextWebsiteRun = mode => {
  const value = websiteRun(mode);
  delete value.designContextContentHash;
  delete value.requiredReadPaths;
  delete value.readFiles;
  delete value.requiredReadsPassed;
  value.templateVersion = "next-app@2";
  value.generationContext = {
    schemaVersion: "generation-context@1",
    status: "compiled",
    contextContentHash: "b".repeat(64),
    runContextBindingHash: "c".repeat(64),
    runtimeAttestationHash: "d".repeat(64),
  };
  value.attestation = { state: "verified", runtimeAttestationHash: "d".repeat(64) };
  value.efficiency = {
    schemaVersion: "run-efficiency-metrics@1",
    uniqueContextReads: 0,
    uniqueSourceReads: 2,
    duplicateReads: 0,
    duplicateReadTokens: 0,
    unchangedReadStubs: 0,
    postCompactSourceRestores: 0,
    prebuildLists: 0,
  };
  return value;
};

const requiredFindingRepair = {
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
};
const profileSync = {
  cleanApply: applied("operation-1", "run-clean-source", "run-clean-child"),
  conflictApply: { ...applied("operation-2", "run-conflict-source", "run-conflict-child"), conflictDecisionCount: 1 },
  planMismatch: { rejected: true, errorCode: "profile_sync_plan_mismatch" },
  recovery: {
    ...applied("operation-3", "run-recovery-source", "run-recovery-child"),
    recoveryRequiredObserved: true,
    reusedChildRun: true,
  },
};
const bffRuntime = {
  projectId: cohort.projectId,
  runId: profileSync.conflictApply.sourceRunId,
  principalProjectId: cohort.projectId,
  operationId: profileSync.conflictApply.operationId,
  childRunId: profileSync.conflictApply.childRunId,
  planHash: profileSync.conflictApply.planHash,
  childRunUnique: true,
};
const rollback = {
  ...cohort,
  updatedBy: "operator-1",
  recordedAt: "2026-07-15T01:00:00Z",
  postRollbackRunId: "run-post-rollback",
  policyEnabledAfterRollback: false,
  policyRevisionAfterRollback: 4,
  postRollbackMode: "observe",
  newRunReadGateBlocked: false,
  operationRecoveryPreserved: true,
};
const compatibility = {
  noProfileWebsitePassed: true,
  noProfileWebsiteRunId: "run-no-profile",
  docsBuildPassed: true,
  docsBuildRunId: "run-docs",
  legacyEditRepairPassed: true,
  legacyEditRunId: "run-legacy-edit",
  legacyRepairRunId: "run-legacy-repair",
};

function publishSamples() {
  const samples = [];
  for (let index = 0; index < 30; index += 1) {
    samples.push({
      ...cohort,
      policyRevision: cohort.observePolicyRevision,
      sampleId: `baseline-${index + 1}`,
      runId: `run-baseline-${index + 1}`,
      mode: "baseline",
      observedAt: `2026-07-${index < 15 ? "02" : "06"}T${String(index % 15).padStart(2, "0")}:00:00Z`,
      publishVerdict: "pass",
      dcpCausedFailure: false,
    });
    samples.push({
      ...cohort,
      sampleId: `enforced-${index + 1}`,
      runId: `run-enforced-${index + 1}`,
      mode: "enforced",
      observedAt: `2026-07-${index < 15 ? "09" : "13"}T${String(index % 15).padStart(2, "0")}:00:00Z`,
      publishVerdict: "pass",
      dcpCausedFailure: false,
    });
  }
  return samples;
}

const root = await mkdtemp(join(tmpdir(), "design-context-canary-ledger-"));
try {
  const ledgerPath = join(root, "canary.ndjson");
  const outputPath = join(root, "canary-evidence.json");
  const configPath = join(root, "session.json");
  await writeFile(configPath, `${JSON.stringify({
    schemaVersion: "design-context-canary-session-config@1",
    recordedAt: "2026-07-08T00:00:00Z",
    sourceUri: "evidence://canary/session",
    provider,
    cohort,
    images,
    window: {
      baselineStartedAt: "2026-07-01T00:00:00Z",
      baselineEndedAt: "2026-07-08T00:00:00Z",
      observationStartedAt: "2026-07-08T00:00:00Z",
    },
  }, null, 2)}\n`);
  await initializeCanaryLedger({ ledgerPath, configPath });

  const events = [
    ["alert.destination-probe", "2026-07-08T00:01:00Z", {
      destinationId: "primary-oncall",
      operatorId: "operator-1",
      eventId: "c".repeat(64),
      probeStatus: "delivered",
      responseStatus: 204,
      attemptCount: 1,
    }],
    ["website.run", "2026-07-08T00:05:00Z", websiteRun("observe")],
    ["website.run", "2026-07-08T00:06:00Z", generationContextWebsiteRun("enforced")],
    ["required-finding.repair", "2026-07-08T01:00:00Z", requiredFindingRepair],
    ["profile-sync.clean-apply", "2026-07-08T02:00:00Z", profileSync.cleanApply],
    ["profile-sync.conflict-apply", "2026-07-08T03:00:00Z", profileSync.conflictApply],
    ["profile-sync.plan-mismatch", "2026-07-08T04:00:00Z", profileSync.planMismatch],
    ["profile-sync.recovery", "2026-07-08T05:00:00Z", profileSync.recovery],
    ["bff-runtime", "2026-07-08T06:00:00Z", bffRuntime],
    ["metrics.snapshot", "2026-07-10T00:01:00Z", {
      observationEndedAt: "2026-07-10T00:00:00Z",
      conclusionRecordedBy: "operator-1",
      verifierUnavailableCount: 1,
      verifierRuntimeLostCount: 0,
      unexpectedReadGateBlockCount: 0,
      recoveryRequiredOver24hCount: 0,
      requiredFindingRepairRate: 0.5,
      alertsTriggered: true,
    }],
    ["alert.delivery", "2026-07-10T00:02:00Z", {
      operationalExportGeneratedAt: "2026-07-10T00:01:00Z",
      observationEndedAt: "2026-07-10T00:00:00Z",
      cohort,
      eventId: sha,
      destinationId: "primary-oncall",
      deliveryRequired: true,
      deliveryStatus: "delivered",
      responseStatus: 202,
      attemptCount: 1,
      triggeredAlertCodes: ["verifier_unavailable"],
    }],
    ["publish.samples", "2026-07-15T00:00:00Z", { samples: publishSamples() }],
    ["metrics.snapshot", "2026-07-15T00:01:00Z", {
      observationEndedAt: "2026-07-15T00:00:00Z",
      conclusionRecordedBy: "operator-1",
      verifierUnavailableCount: 0,
      verifierRuntimeLostCount: 0,
      unexpectedReadGateBlockCount: 0,
      recoveryRequiredOver24hCount: 0,
      requiredFindingRepairRate: 1,
      alertsTriggered: false,
    }],
    ["alert.delivery", "2026-07-15T00:02:00Z", {
      operationalExportGeneratedAt: "2026-07-15T00:01:00Z",
      observationEndedAt: "2026-07-15T00:00:00Z",
      cohort,
      eventId: "b".repeat(64),
      destinationId: "primary-oncall",
      deliveryRequired: false,
      deliveryStatus: "not-required",
      responseStatus: null,
      attemptCount: 0,
      triggeredAlertCodes: [],
    }],
    ["rollback", "2026-07-15T01:00:00Z", rollback],
    ["compatibility", "2026-07-15T01:10:00Z", compatibility],
  ];
  for (const [index, [type, recordedAt, payload]] of events.entries()) {
    const eventPath = join(root, `event-${index + 1}-${type.replaceAll(".", "-")}.json`);
    await writeFile(eventPath, `${JSON.stringify({
      schemaVersion: "design-context-canary-event@1",
      type,
      recordedAt,
      sourceUri: `evidence://canary/${index + 1}-${type}`,
      payload,
    }, null, 2)}\n`);
    await appendCanaryEvent({ ledgerPath, eventPath });
  }

  const records = await readCanaryLedger(ledgerPath);
  const status = summarizeCanaryLedger(records, new Date("2026-07-15T02:00:00Z"));
  assert.equal(status.baselinePublishCount, 30);
  assert.equal(status.enforcedPublishCount, 30);
  assert.equal(status.recordCount, 17);
  const evidence = await finalizeCanaryLedger({
    ledgerPath,
    outputPath,
    recordedAt: "2026-07-15T02:00:00Z",
  });
  assert.deepEqual(validateDesignContextCanaryEvidence(evidence), []);
  assert.equal(evidence.metrics.observationWindowMinutes, 7 * 24 * 60);
  assert.equal(evidence.metrics.publishFailureRateDeltaPp, 0);
  assert.equal(evidence.sourceLedger.recordCount, records.length);
  assert.equal(JSON.parse(await readFile(outputPath, "utf8")).result, "pass");

  const tamperedPath = join(root, "tampered.ndjson");
  const tampered = (await readFile(ledgerPath, "utf8")).replace("run-observe", "run-tampered");
  await writeFile(tamperedPath, tampered);
  await assert.rejects(() => readCanaryLedger(tamperedPath), /hash chain/);

  const secretEventPath = join(root, "secret-event.json");
  await writeFile(secretEventPath, JSON.stringify({
    schemaVersion: "design-context-canary-event@1",
    type: "compatibility",
    recordedAt: "2026-07-15T02:00:00Z",
    sourceUri: "evidence://canary/secret",
    api_key: `sk-${"x".repeat(24)}`,
    payload: compatibility,
  }));
  await assert.rejects(() => appendCanaryEvent({ ledgerPath, eventPath: secretEventPath }), /credential-like/);
} finally {
  await rm(root, { recursive: true, force: true });
}

process.stdout.write("design-context canary ledger tests passed\n");
