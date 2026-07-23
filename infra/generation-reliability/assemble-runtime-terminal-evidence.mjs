#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { evidenceRedactionViolations } from "./runtime-evidence-redaction.mjs";
import {
  calculateEvidenceUsage,
  createBudgetProfilesEvidence,
  createRunModelUsageEvidence,
} from "./runtime-budget-evidence.mjs";
import { validateRuntimeRestartEvidence } from "../../services/runtime/scripts/generation-context-runtime-restart-evidence.mjs";

const REQUIRED_FILES = [
  "manifest.json",
  "events.ndjson",
  "usage.json",
  "run-model-usage.json",
  "budget-profiles.json",
  "case-summary.json",
  "artifact-route-identity.json",
  "validation-summary.json",
  "provider-cache-summary.json",
];
const HASH = /^[a-f0-9]{64}$/;

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function invariant(condition, message) {
  if (!condition) throw new Error(message);
}

function nonEmptyString(value) {
  return typeof value === "string" && value.length > 0;
}

function nonNegativeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || !value) throw new Error(`invalid argument: ${key || "<missing>"}`);
    args[key.slice(2)] = value;
  }
  for (const required of ["suite", "cache", "out"]) {
    if (!args[required]) throw new Error(`--${required} is required`);
  }
  invariant([args.case, args.edit, args.repair, args.restart].filter(Boolean).length === 1,
    "exactly one of --case, --edit, --repair, or --restart is required");
  return args;
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, { flag: "wx", mode: 0o600 });
}

function jsonBytes(value) {
  return Buffer.from(`${JSON.stringify(value, null, 2)}\n`);
}

function providerCacheSummary(cacheAudit, cacheFile) {
  return {
    schemaVersion: cacheAudit.schemaVersion,
    toolSetHashVersion: cacheAudit.toolSetHashVersion,
    status: cacheAudit.status,
    releaseEligible: cacheAudit.releaseEligible,
    sourceCommit: cacheAudit.sourceCommit,
    modelResourceId: cacheAudit.modelResourceId,
    providerResourceRevision: cacheAudit.providerResourceRevision,
    providerConfigSha256: cacheAudit.providerConfigSha256,
    auditedRunCount: cacheAudit.auditedRunCount,
    stableRunCount: cacheAudit.stableRunCount,
    grossInputTokens: cacheAudit.grossInputTokens,
    cachedInputTokens: cacheAudit.cachedInputTokens,
    sourceAuditSha256: sha256(fs.readFileSync(path.resolve(cacheFile))),
  };
}

function writeTerminalBundle({
  outputDirectory,
  eventBytes,
  usage,
  runModelUsage,
  budgetProfiles,
  caseSummary,
  routeIdentity,
  validation,
  cacheSummary,
  manifest,
  additionalFiles = {},
}) {
  fs.mkdirSync(path.dirname(outputDirectory), { recursive: true });
  fs.mkdirSync(outputDirectory, { mode: 0o700 });
  fs.writeFileSync(path.join(outputDirectory, "events.ndjson"), eventBytes, { flag: "wx", mode: 0o600 });
  writeJson(path.join(outputDirectory, "usage.json"), usage);
  invariant(sha256(jsonBytes(runModelUsage)) === manifest.runModelUsageSha256,
    "RunModelUsage Manifest hash mismatch before write");
  writeJson(path.join(outputDirectory, "run-model-usage.json"), runModelUsage);
  invariant(sha256(jsonBytes(budgetProfiles)) === manifest.budgetProfilesSha256,
    "Budget Profiles Manifest hash mismatch before write");
  writeJson(path.join(outputDirectory, "budget-profiles.json"), budgetProfiles);
  writeJson(path.join(outputDirectory, "case-summary.json"), caseSummary);
  writeJson(path.join(outputDirectory, "artifact-route-identity.json"), routeIdentity);
  writeJson(path.join(outputDirectory, "validation-summary.json"), validation);
  writeJson(path.join(outputDirectory, "provider-cache-summary.json"), cacheSummary);
  writeJson(path.join(outputDirectory, "manifest.json"), manifest);
  for (const [file, value] of Object.entries(additionalFiles)) {
    invariant(/^[A-Za-z0-9._-]+$/.test(file), `invalid additional evidence filename: ${file}`);
    if (Buffer.isBuffer(value)) {
      fs.writeFileSync(path.join(outputDirectory, file), value, { flag: "wx", mode: 0o600 });
    } else {
      writeJson(path.join(outputDirectory, file), value);
    }
  }
  const checksumFiles = [...REQUIRED_FILES, ...Object.keys(additionalFiles)];
  const checksumLines = checksumFiles.map((file) =>
    `${sha256(fs.readFileSync(path.join(outputDirectory, file)))}  ${file}`);
  fs.writeFileSync(
    path.join(outputDirectory, "checksums.sha256"),
    `${checksumLines.join("\n")}\n`,
    { flag: "wx", mode: 0o600 },
  );
  process.stdout.write(`Runtime terminal evidence bundle assembled: ${outputDirectory}\n`);
}

function validateRunIdentity(run, summary, phase) {
  invariant(run?.phase === phase && run?.status === "completed" && nonEmptyString(run?.runId), `${phase} Run is incomplete`);
  const context = run.generationContextStatus;
  const build = run.buildEvidence;
  invariant(nonEmptyString(build?.buildId)
    && nonEmptyString(build?.sourceSnapshotUri)
    && build?.artifactRouteManifestPath === ".anydesign-artifact-routes.json"
    && HASH.test(build?.artifactRouteManifestHash || "")
    && HASH.test(build?.candidateManifestHash || "")
    && HASH.test(build?.sourceFingerprint || ""), "Build identity is incomplete");
  invariant(context?.schemaVersion === "generation-context-status@1"
    && context?.runContractVersion === "generation-context@1"
    && context?.status === "compiled"
    && context?.runId === run.runId
    && nonEmptyString(context?.operationId)
    && HASH.test(context?.contextContentHash || "")
    && HASH.test(context?.runContextBindingHash || "")
    && nonEmptyString(context?.budgetProfileId)
    && HASH.test(context?.budgetProfileHash || "")
    && new Set(["shadow", "enforced"]).has(context?.budgetProfileRolloutMode), "Generation Context identity is incomplete");
  invariant(nonEmptyString(run.efficiency?.template), "template identity is incomplete");
  const executions = run.modelExecutions || [];
  invariant(executions.length > 0 && executions.every((execution) =>
    execution.modelResourceId === summary.provider.modelResourceId
    && execution.modelResourceRevision === summary.provenance.providerResourceRevision
    && execution.providerRequestIdPresent === true), "Provider execution identity mismatch");
  return { context, build };
}

function readEventStream(run, baseDirectory, containmentDirectory) {
  const stream = run.eventStream;
  invariant(stream?.schemaVersion === "generation-run-event-stream@1"
    && stream?.format === "ndjson"
    && nonEmptyString(stream?.path)
    && nonNegativeInteger(stream?.eventCount)
    && stream.eventCount > 0
    && HASH.test(stream?.sha256 || ""), "event stream contract is incomplete");
  const eventFile = fs.realpathSync(path.resolve(baseDirectory, stream.path));
  invariant(eventFile.startsWith(`${containmentDirectory}${path.sep}`)
    && fs.statSync(eventFile).isFile(), "event stream path escapes the suite");
  const eventBytes = fs.readFileSync(eventFile);
  invariant(sha256(eventBytes) === stream.sha256, "event stream hash mismatch");
  const events = eventBytes.toString("utf8").trim().split(/\r?\n/).filter(Boolean).map(JSON.parse);
  invariant(events.length === stream.eventCount, "event stream count mismatch");
  invariant(evidenceRedactionViolations(events, "eventStream").length === 0,
    "event stream contains forbidden raw evidence fields");
  const usage = calculateEvidenceUsage(events);
  invariant(usage.aggregate.modelCalls > 0
    && usage.aggregate.inputTokens === run.usage.inputTokens
    && usage.aggregate.cachedInputTokens === run.usage.cachedInputTokens
    && usage.aggregate.outputTokens === run.usage.outputTokens,
  `${run.phase} usage does not replay from the event stream`);
  return { stream, eventBytes, events, usage };
}

function attachBudgetEvidence(runs, usage) {
  const { evidence, conformance } = createBudgetProfilesEvidence(runs, usage);
  usage.budgetConformance = conformance;
  return {
    budgetProfiles: evidence,
    budgetProfilesSha256: sha256(jsonBytes(evidence)),
  };
}

function operationRunsForPrimary(allRuns, primaryRun) {
  const operationId = primaryRun?.generationContextStatus?.operationId;
  invariant(nonEmptyString(operationId), "primary Run Operation identity is missing");
  const runs = (allRuns || []).filter((run) =>
    run?.generationContextStatus?.operationId === operationId);
  invariant(runs.some((run) => run.runId === primaryRun.runId),
    "primary Run is missing from its Operation evidence");
  return runs.sort((left, right) =>
    Number(left.generationContextStatus.operationAttempt)
      - Number(right.generationContextStatus.operationAttempt));
}

function readCombinedOperationStreams(runs, baseDirectory, containmentDirectory) {
  const streams = runs.map((run) =>
    readEventStream(run, baseDirectory, containmentDirectory));
  invariant(streams.every(({ eventBytes }) => eventBytes.at(-1) === 10),
    "Operation event streams must be newline-terminated NDJSON");
  const eventBytes = Buffer.concat(streams.map((stream) => stream.eventBytes));
  const events = eventBytes.toString("utf8").trim().split(/\r?\n/).filter(Boolean).map(JSON.parse);
  return {
    eventBytes,
    events,
    usage: calculateEvidenceUsage(events),
  };
}

function assembleEditBundle({
  args,
  suiteRealDirectory,
  outputDirectory,
  summary,
  cacheAudit,
}) {
  const requestedEdit = path.isAbsolute(args.edit)
    ? path.resolve(args.edit)
    : path.resolve(suiteRealDirectory, args.edit);
  const editFile = fs.statSync(requestedEdit, { throwIfNoEntry: false })?.isDirectory()
    ? path.join(requestedEdit, "real-provider-edit-summary.json")
    : requestedEdit;
  const editRealFile = fs.realpathSync(editFile);
  invariant(editRealFile.startsWith(`${suiteRealDirectory}${path.sep}`), "Edit evidence path escapes the suite");
  const edit = readJson(editRealFile);
  invariant(edit.schemaVersion === "generation-real-provider-edit-evidence@2"
    && edit.status === "accepted"
    && edit.providerVerified === true
    && edit.secretMaterialPersisted === false
    && !edit.error
    && !edit.cleanupError, "Edit evidence is not release-eligible");
  invariant(nonEmptyString(edit.projectId)
    && HASH.test(edit.promptSha256 || "")
    && nonEmptyString(edit.baseVersionId)
    && nonEmptyString(edit.versionId)
    && edit.baseVersionId !== edit.versionId, "Edit operation identity is incomplete");
  invariant(edit.sandboxRelease?.required === true
    && edit.sandboxRelease?.released === true
    && Number(edit.sandboxRelease?.requiredSuccessfulResponses) >= 2,
  "Edit Sandbox release is incomplete");
  const run = edit.run;
  const { context, build } = validateRunIdentity(run, summary, "edit");
  const { stream, eventBytes, events, usage } = readEventStream(
    run,
    path.dirname(editRealFile),
    suiteRealDirectory,
  );
  const { budgetProfiles, budgetProfilesSha256 } = attachBudgetEvidence([run], usage);
  const runModelUsage = createRunModelUsageEvidence(events, summary.provider.modelResourceId);
  const runModelUsageSha256 = sha256(jsonBytes(runModelUsage));
  const artifact = edit.artifact;
  const artifactAccepted = artifact?.httpStatus === 200
    && HASH.test(artifact?.bodySha256 || "")
    && nonNegativeInteger(artifact?.bodyBytes)
    && (artifact.expectedTextFound === true
      || (artifact.semanticNavFound === true
        && artifact.originalHeadlineFound === true
        && artifact.declaredIconHttpStatus === 200));
  invariant(artifactAccepted && artifact.versionId === edit.versionId && nonEmptyString(artifact.route),
    "Edit accepted HTTP artifact probe is missing");
  invariant(edit.releaseEvidence?.available === true
    && edit.releaseEvidence?.versionId === edit.versionId
    && (nonEmptyString(edit.releaseEvidence?.releaseId)
      || HASH.test(edit.releaseEvidence?.artifactManifestHash || "")), "Edit Publish/Release identity is incomplete");
  if (edit.releaseEvidence.artifactManifestHash !== undefined) {
    invariant(HASH.test(edit.releaseEvidence.artifactManifestHash || ""), "Edit release Artifact Manifest identity is invalid");
  }
  if (edit.releaseEvidence.sourceFingerprint !== undefined) {
    invariant(edit.releaseEvidence.sourceFingerprint === build.sourceFingerprint, "Edit release Source identity mismatch");
  }
  const acceptanceContractSha256 = sha256(JSON.stringify({
    route: artifact.route,
    httpStatus: artifact.httpStatus,
    bodySha256: artifact.bodySha256,
    expectedTextFound: artifact.expectedTextFound === true,
    semanticNavFound: artifact.semanticNavFound === true,
    originalHeadlineFound: artifact.originalHeadlineFound === true,
  }));
  const routeIdentity = {
    schemaVersion: "artifact-route-identity@1",
    buildId: build.buildId,
    candidateManifestHash: build.candidateManifestHash,
    artifactRouteManifestPath: build.artifactRouteManifestPath,
    artifactRouteManifestHash: build.artifactRouteManifestHash,
    sourceFingerprint: build.sourceFingerprint,
    sourceSnapshotUriSha256: sha256(build.sourceSnapshotUri),
    entryRoute: artifact.route,
    httpProbe: {
      status: artifact.httpStatus,
      bodySha256: artifact.bodySha256,
      bodyBytes: artifact.bodyBytes,
      expectedTextFound: true,
    },
  };
  const caseSummary = {
    schemaVersion: "runtime-evidence-case-summary@1",
    suiteId: summary.suiteId,
    caseId: `edit-${run.runId}`,
    kind: "edit",
    projectId: edit.projectId,
    runId: run.runId,
    budgetRunIds: [run.runId],
    operationId: context.operationId,
    status: edit.status,
    promptSha256: edit.promptSha256,
    acceptanceContractSha256,
    contentPlanIntentSha256: HASH.test(edit.editImpactPlanHash || "")
      ? edit.editImpactPlanHash
      : sha256(edit.baseVersionId),
    templateVersion: run.efficiency.template,
    contextContentHash: context.contextContentHash,
    runContextBindingHash: context.runContextBindingHash,
    budgetProfileId: context.budgetProfileId,
    budgetProfileHash: context.budgetProfileHash,
    budgetProfilesSha256,
    runModelUsageSha256,
    budgetProfileRolloutMode: context.budgetProfileRolloutMode,
  };
  const validation = {
    schemaVersion: "runtime-validation-summary@1",
    checks: [
      { id: "case-accepted", status: "passed", owner: "runtime" },
      { id: "entry-route", status: "passed", owner: "serving" },
      { id: "provider-identity", status: "passed", owner: "runtime" },
      { id: "event-redaction", status: "passed", owner: "harness" },
      { id: "sandbox-release", status: "passed", owner: "runtime" },
    ],
  };
  const cacheSummary = providerCacheSummary(cacheAudit, args.cache);
  const manifest = {
    schemaVersion: "runtime-evidence-bundle@1",
    bundleKind: "real_provider_terminal",
    evidenceId: `${summary.suiteId}-edit-${run.runId}`,
    gitSha: summary.provenance.gitCommit,
    harnessRevision: "runtime-terminal-evidence@1",
    runtimeMode: "real_provider",
    scenarioKind: "edit",
    projectId: edit.projectId,
    runId: run.runId,
    operationId: context.operationId,
    modelResourceId: summary.provider.modelResourceId,
    modelResourceRevision: summary.provenance.providerResourceRevision,
    providerConfigSha256: summary.provenance.providerConfigSha256,
    promptSha256: edit.promptSha256,
    generationContextHash: context.contextContentHash,
    runContextBindingHash: context.runContextBindingHash,
    budgetProfileId: context.budgetProfileId,
    budgetProfileHash: context.budgetProfileHash,
    budgetProfilesSha256,
    runModelUsageSha256,
    templateVersion: run.efficiency.template,
    candidateManifestHash: build.candidateManifestHash,
    artifactRouteManifestHash: build.artifactRouteManifestHash,
    sourceFingerprint: build.sourceFingerprint,
    streamSha256: stream.sha256,
    result: { status: "accepted" },
    replayExpectations: {
      usage: usage.aggregate,
      budgetConformance: usage.budgetConformance,
      runModelUsageSha256,
      routeIdentitySha256: sha256(`${JSON.stringify(routeIdentity, null, 2)}\n`),
      validationPassed: true,
      providerCachePassed: true,
    },
  };
  writeTerminalBundle({
    outputDirectory,
    eventBytes,
    usage,
    runModelUsage,
    budgetProfiles,
    caseSummary,
    routeIdentity,
    validation,
    cacheSummary,
    manifest,
  });
}

function assembleRepairBundle({
  args,
  suiteRealDirectory,
  outputDirectory,
  summary,
  cacheAudit,
}) {
  const caseFile = path.join(suiteRealDirectory, `real-provider-case-${args.repair}.json`);
  const caseEvidence = readJson(caseFile);
  const repair = caseEvidence.repair;
  invariant(caseEvidence.schemaVersion === "generation-real-provider-case-evidence@2"
    && caseEvidence.id === args.repair
    && caseEvidence.status === "accepted", "Repair parent case is not accepted");
  invariant(repair?.schemaVersion === "generation-real-provider-repair-evidence@2"
    && repair?.status === "accepted"
    && repair?.providerVerified === true
    && repair?.secretMaterialPersisted === false
    && !repair?.error, "Repair evidence is not release-eligible");
  invariant(nonEmptyString(repair.projectId)
    && repair.projectId === caseEvidence.projectId
    && HASH.test(repair.promptSha256 || "")
    && nonEmptyString(repair.baseVersionId)
    && nonEmptyString(repair.repairedVersionId)
    && repair.baseVersionId !== repair.repairedVersionId,
  "Repair operation identity is incomplete");
  invariant(caseEvidence.sandboxRelease?.required === true
    && caseEvidence.sandboxRelease?.released === true
    && Number(caseEvidence.sandboxRelease?.requiredSuccessfulResponses) >= 2,
  "Repair Sandbox release is incomplete");
  invariant(repair.setupEdit?.status === "accepted"
    && repair.setupEdit?.providerVerified === true
    && repair.setupEdit?.run?.status === "completed"
    && nonEmptyString(repair.setupEdit?.evidencePath),
  "Repair setup Edit evidence is incomplete");
  const setupEditFile = fs.realpathSync(
    path.resolve(suiteRealDirectory, repair.setupEdit.evidencePath),
  );
  invariant(setupEditFile.startsWith(`${suiteRealDirectory}${path.sep}`),
    "Repair setup Edit evidence path escapes the suite");
  const setupEdit = readJson(setupEditFile);
  invariant(setupEdit.schemaVersion === "generation-real-provider-edit-evidence@2"
    && setupEdit.status === "accepted"
    && setupEdit.run?.runId === repair.setupEdit.run.runId
    && setupEdit.promptSha256 === repair.setupEdit.promptSha256
    && setupEdit.providerVerified === true,
  "Repair setup Edit persisted identity mismatch");
  validateRunIdentity(setupEdit.run, summary, "edit");
  const setupStreamEvidence = readEventStream(
    setupEdit.run,
    path.dirname(setupEditFile),
    suiteRealDirectory,
  );
  const reviewRun = repair.reviewRun;
  invariant(reviewRun?.phase === "review"
    && reviewRun?.status === "completed"
    && Array.isArray(reviewRun?.modelExecutions)
    && reviewRun.modelExecutions.length > 0
    && reviewRun.modelExecutions.every((execution) =>
      execution.modelResourceId === summary.provider.modelResourceId
      && execution.modelResourceRevision === summary.provenance.providerResourceRevision
      && execution.providerRequestIdPresent === true),
  "Repair Review Provider identity is incomplete");
  const run = repair.run;
  const { context, build } = validateRunIdentity(run, summary, "repair");
  invariant(repair.generationContextStatus?.runId === run.runId
    && repair.generationContextStatus?.contextContentHash === context.contextContentHash
    && repair.generationContextStatus?.runContextBindingHash === context.runContextBindingHash,
  "Repair Generation Context identity drifted");
  const reviewStreamEvidence = readEventStream(
    reviewRun,
    suiteRealDirectory,
    suiteRealDirectory,
  );
  const repairStreamEvidence = readEventStream(
    run,
    suiteRealDirectory,
    suiteRealDirectory,
  );
  const eventBytes = Buffer.concat([
    setupStreamEvidence.eventBytes,
    reviewStreamEvidence.eventBytes,
    repairStreamEvidence.eventBytes,
  ]);
  const combinedEvents = eventBytes.toString("utf8").trim()
    .split(/\r?\n/).filter(Boolean).map(JSON.parse);
  const usage = calculateEvidenceUsage(combinedEvents);
  const runModelUsage = createRunModelUsageEvidence(
    combinedEvents,
    summary.provider.modelResourceId,
  );
  const runModelUsageSha256 = sha256(jsonBytes(runModelUsage));
  const { budgetProfiles, budgetProfilesSha256 } = attachBudgetEvidence(
    [setupEdit.run, reviewRun, run],
    usage,
  );
  const verification = repair.repairVerification;
  const booleanChecks = [
    "reviewFindingRecorded",
    "findingFixedByCompletedRepair",
    "freshVersionCreated",
    "sourceMutationRecorded",
    "previewPublishRecorded",
    "markerPreserved",
  ];
  invariant(booleanChecks.every((field) => verification?.[field] === true)
    && nonEmptyString(verification?.artifactRoute)
    && verification?.artifactHttpStatus === 200
    && HASH.test(verification?.artifactBodySha256 || "")
    && nonNegativeInteger(verification?.artifactBodyBytes)
    && verification.artifactBodyBytes > 0,
  "Repair verification is incomplete");
  const routeIdentity = {
    schemaVersion: "artifact-route-identity@1",
    buildId: build.buildId,
    candidateManifestHash: build.candidateManifestHash,
    artifactRouteManifestPath: build.artifactRouteManifestPath,
    artifactRouteManifestHash: build.artifactRouteManifestHash,
    sourceFingerprint: build.sourceFingerprint,
    sourceSnapshotUriSha256: sha256(build.sourceSnapshotUri),
    entryRoute: verification.artifactRoute,
    httpProbe: {
      status: verification.artifactHttpStatus,
      bodySha256: verification.artifactBodySha256,
      bodyBytes: verification.artifactBodyBytes,
      expectedTextFound: verification.markerPreserved,
    },
  };
  const caseSummary = {
    schemaVersion: "runtime-evidence-case-summary@1",
    suiteId: summary.suiteId,
    caseId: `${caseEvidence.id}-repair`,
    kind: "repair",
    projectId: repair.projectId,
    runId: run.runId,
    setupEditRunId: setupEdit.run.runId,
    reviewRunId: reviewRun.runId,
    budgetRunIds: [setupEdit.run.runId, reviewRun.runId, run.runId],
    operationId: context.operationId,
    status: repair.status,
    promptSha256: repair.promptSha256,
    acceptanceContractSha256: sha256(JSON.stringify({
      findingId: repair.reviewFinding?.findingId,
      route: verification.artifactRoute,
      repairedVersionId: repair.repairedVersionId,
      bodySha256: verification.artifactBodySha256,
    })),
    contentPlanIntentSha256: caseEvidence.contentPlan?.intentSha256,
    templateVersion: run.efficiency.template,
    contextContentHash: context.contextContentHash,
    runContextBindingHash: context.runContextBindingHash,
    budgetProfileId: context.budgetProfileId,
    budgetProfileHash: context.budgetProfileHash,
    budgetProfilesSha256,
    runModelUsageSha256,
    budgetProfileRolloutMode: context.budgetProfileRolloutMode,
  };
  invariant(HASH.test(caseSummary.contentPlanIntentSha256 || ""),
    "Repair Content Plan identity is incomplete");
  const validation = {
    schemaVersion: "runtime-validation-summary@1",
    checks: [
      { id: "case-accepted", status: "passed", owner: "runtime" },
      { id: "entry-route", status: "passed", owner: "serving" },
      { id: "provider-identity", status: "passed", owner: "runtime" },
      { id: "event-redaction", status: "passed", owner: "harness" },
      { id: "sandbox-release", status: "passed", owner: "runtime" },
    ],
  };
  const cacheSummary = providerCacheSummary(cacheAudit, args.cache);
  const manifest = {
    schemaVersion: "runtime-evidence-bundle@1",
    bundleKind: "real_provider_terminal",
    evidenceId: `${summary.suiteId}-${caseEvidence.id}-repair-${run.runId}`,
    gitSha: summary.provenance.gitCommit,
    harnessRevision: "runtime-terminal-evidence@1",
    runtimeMode: "real_provider",
    scenarioKind: "repair",
    projectId: repair.projectId,
    runId: run.runId,
    operationId: context.operationId,
    modelResourceId: summary.provider.modelResourceId,
    modelResourceRevision: summary.provenance.providerResourceRevision,
    providerConfigSha256: summary.provenance.providerConfigSha256,
    promptSha256: repair.promptSha256,
    generationContextHash: context.contextContentHash,
    runContextBindingHash: context.runContextBindingHash,
    budgetProfileId: context.budgetProfileId,
    budgetProfileHash: context.budgetProfileHash,
    budgetProfilesSha256,
    runModelUsageSha256,
    templateVersion: run.efficiency.template,
    candidateManifestHash: build.candidateManifestHash,
    artifactRouteManifestHash: build.artifactRouteManifestHash,
    sourceFingerprint: build.sourceFingerprint,
    streamSha256: sha256(eventBytes),
    result: { status: "accepted" },
    replayExpectations: {
      usage: usage.aggregate,
      budgetConformance: usage.budgetConformance,
      runModelUsageSha256,
      routeIdentitySha256: sha256(`${JSON.stringify(routeIdentity, null, 2)}\n`),
      validationPassed: true,
      providerCachePassed: true,
    },
  };
  writeTerminalBundle({
    outputDirectory,
    eventBytes,
    usage,
    runModelUsage,
    budgetProfiles,
    caseSummary,
    routeIdentity,
    validation,
    cacheSummary,
    manifest,
  });
}

function assembleRestartBundle({
  args,
  suiteRealDirectory,
  outputDirectory,
  summary,
  cacheAudit,
}) {
  invariant(nonEmptyString(args["restart-case"]), "--restart-case is required with --restart");
  const caseEvidence = readJson(
    path.join(suiteRealDirectory, `real-provider-case-${args["restart-case"]}.json`),
  );
  invariant(caseEvidence.schemaVersion === "generation-real-provider-case-evidence@2"
    && caseEvidence.id === args["restart-case"]
    && caseEvidence.status === "accepted", "Runtime Restart parent case is not accepted");
  const finalRunIds = new Set(caseEvidence.attempts?.at(-1)?.runIds || []);
  const buildRuns = (caseEvidence.runs || []).filter((run) =>
    run.phase === "build" && run.status === "completed" && finalRunIds.has(run.runId));
  invariant(buildRuns.length === 1, "Runtime Restart case must contain exactly one final accepted Build Run");
  const run = buildRuns[0];
  const cacheRun = (cacheAudit.runs || []).find((candidate) => candidate.runId === run.runId);
  invariant(cacheRun?.redactionValid === true
    && cacheRun?.compositionValid === true
    && cacheRun?.metricsValid === true
    && cacheRun?.providerIdentityValid === true
    && cacheRun?.buildIdentityValid === true
    && cacheRun?.generationContextIdentityValid === true
    && Number(cacheRun?.repeatedStableTurns) >= 2,
  "Runtime Restart Build Run is not covered by the release cache audit");
  const { context, build } = validateRunIdentity(run, summary, "build");
  const operationRuns = operationRunsForPrimary(caseEvidence.runs, run);
  const { eventBytes, events, usage } = readCombinedOperationStreams(
    operationRuns,
    suiteRealDirectory,
    suiteRealDirectory,
  );
  const { budgetProfiles, budgetProfilesSha256 } = attachBudgetEvidence(operationRuns, usage);
  const runModelUsage = createRunModelUsageEvidence(events, summary.provider.modelResourceId);
  const runModelUsageSha256 = sha256(jsonBytes(runModelUsage));
  const requestedRestart = path.isAbsolute(args.restart)
    ? path.resolve(args.restart)
    : path.resolve(suiteRealDirectory, args.restart);
  const restartFile = fs.realpathSync(requestedRestart);
  invariant(restartFile.startsWith(`${suiteRealDirectory}${path.sep}`),
    "Runtime Restart evidence path escapes the suite");
  const restart = readJson(restartFile);
  invariant(restart.schemaVersion === "generation-context-runtime-restart-evidence@2",
    "Runtime Restart terminal bundle requires cleanup-aware evidence@2");
  validateRuntimeRestartEvidence(restart, {
    side: "candidate",
    projectId: caseEvidence.projectId,
    runId: run.runId,
    runtimeDeploymentRevision: restart.runtimeDeploymentRevision,
  });
  invariant(restart.side === "candidate"
    && restart.before.projectId === caseEvidence.projectId
    && restart.before.runId === run.runId
    && restart.before.generationContextStatus?.contextContentHash === context.contextContentHash
    && restart.before.generationContextStatus?.runContextBindingHash === context.runContextBindingHash
    && restart.before.generationContextStatus?.budgetProfileId === context.budgetProfileId
    && restart.before.generationContextStatus?.budgetProfileHash === context.budgetProfileHash
    && restart.before.generationContextStatus?.budgetProfileRolloutMode === context.budgetProfileRolloutMode,
  "Runtime Restart frozen Run/Context identity mismatch");
  invariant(restart.before.projectState?.sourceSnapshotRefSha256 === sha256(build.sourceSnapshotUri),
    "Runtime Restart Source Snapshot identity mismatch");
  invariant(restart.before.efficiency?.inputTokens === run.usage.inputTokens
    && restart.before.efficiency?.template === run.efficiency.template,
  "Runtime Restart efficiency identity mismatch");
  const artifact = restart.before.artifact;
  invariant(artifact?.httpStatus === 200
    && artifact?.markerFound === true
    && artifact?.markerSha256 === sha256(caseEvidence.expectedText || "")
    && HASH.test(artifact?.bodySha256 || "")
    && nonNegativeInteger(artifact?.bodyBytes)
    && artifact.bodyBytes > 0,
  "Runtime Restart pre-replacement Artifact evidence is incomplete");
  const routeIdentity = {
    schemaVersion: "artifact-route-identity@1",
    buildId: build.buildId,
    candidateManifestHash: build.candidateManifestHash,
    artifactRouteManifestPath: build.artifactRouteManifestPath,
    artifactRouteManifestHash: build.artifactRouteManifestHash,
    sourceFingerprint: build.sourceFingerprint,
    sourceSnapshotUriSha256: sha256(build.sourceSnapshotUri),
    entryRoute: caseEvidence.expectedRoute,
    httpProbe: {
      status: artifact.httpStatus,
      bodySha256: artifact.bodySha256,
      bodyBytes: artifact.bodyBytes,
      expectedTextFound: artifact.markerFound,
    },
  };
  const caseSummary = {
    schemaVersion: "runtime-evidence-case-summary@1",
    suiteId: summary.suiteId,
    caseId: `${caseEvidence.id}-runtime-restart`,
    kind: "runtime_restart",
    projectId: caseEvidence.projectId,
    runId: run.runId,
    operationRunIds: operationRuns.map((operationRun) => operationRun.runId),
    budgetRunIds: operationRuns.map((operationRun) => operationRun.runId),
    operationId: context.operationId,
    status: restart.status,
    promptSha256: caseEvidence.promptSha256,
    acceptanceContractSha256: caseEvidence.acceptance?.sha256,
    contentPlanIntentSha256: caseEvidence.contentPlan?.intentSha256,
    templateVersion: run.efficiency.template,
    contextContentHash: context.contextContentHash,
    runContextBindingHash: context.runContextBindingHash,
    budgetProfileId: context.budgetProfileId,
    budgetProfileHash: context.budgetProfileHash,
    budgetProfilesSha256,
    runModelUsageSha256,
    budgetProfileRolloutMode: context.budgetProfileRolloutMode,
    runtimeDeploymentRevision: restart.runtimeDeploymentRevision,
    restartEvidenceSha256: restart.evidenceSha256,
  };
  const validation = {
    schemaVersion: "runtime-validation-summary@1",
    checks: [
      { id: "case-accepted", status: "passed", owner: "runtime" },
      { id: "entry-route", status: "passed", owner: "serving" },
      { id: "provider-identity", status: "passed", owner: "runtime" },
      { id: "event-redaction", status: "passed", owner: "harness" },
      { id: "sandbox-release", status: "passed", owner: "runtime" },
    ],
  };
  const cacheSummary = providerCacheSummary(cacheAudit, args.cache);
  const manifest = {
    schemaVersion: "runtime-evidence-bundle@1",
    bundleKind: "runtime_restart_terminal",
    evidenceId: `${summary.suiteId}-${caseEvidence.id}-runtime-restart-${run.runId}`,
    gitSha: summary.provenance.gitCommit,
    harnessRevision: "runtime-terminal-evidence@1",
    runtimeMode: "real_provider",
    scenarioKind: "runtime_restart",
    projectId: caseEvidence.projectId,
    runId: run.runId,
    operationId: context.operationId,
    modelResourceId: summary.provider.modelResourceId,
    modelResourceRevision: summary.provenance.providerResourceRevision,
    providerConfigSha256: summary.provenance.providerConfigSha256,
    promptSha256: caseEvidence.promptSha256,
    generationContextHash: context.contextContentHash,
    runContextBindingHash: context.runContextBindingHash,
    budgetProfileId: context.budgetProfileId,
    budgetProfileHash: context.budgetProfileHash,
    budgetProfilesSha256,
    runModelUsageSha256,
    templateVersion: run.efficiency.template,
    candidateManifestHash: build.candidateManifestHash,
    artifactRouteManifestHash: build.artifactRouteManifestHash,
    sourceFingerprint: build.sourceFingerprint,
    streamSha256: sha256(eventBytes),
    runtimeDeploymentRevision: restart.runtimeDeploymentRevision,
    podBeforeUid: restart.podBefore.uid,
    podAfterUid: restart.podAfter.uid,
    restartEvidenceSha256: restart.evidenceSha256,
    result: { status: "accepted" },
    replayExpectations: {
      usage: usage.aggregate,
      budgetConformance: usage.budgetConformance,
      runModelUsageSha256,
      routeIdentitySha256: sha256(`${JSON.stringify(routeIdentity, null, 2)}\n`),
      validationPassed: true,
      providerCachePassed: true,
      restartEvidenceSha256: restart.evidenceSha256,
      podUidChanged: true,
      statePreserved: true,
      sandboxReleased: true,
    },
  };
  writeTerminalBundle({
    outputDirectory,
    eventBytes,
    usage,
    runModelUsage,
    budgetProfiles,
    caseSummary,
    routeIdentity,
    validation,
    cacheSummary,
    manifest,
    additionalFiles: { "runtime-restart-evidence.json": restart },
  });
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const suiteDirectory = path.resolve(args.suite);
  const suiteRealDirectory = fs.realpathSync(suiteDirectory);
  const outputDirectory = path.resolve(args.out);
  invariant(!fs.existsSync(outputDirectory), `output already exists: ${outputDirectory}`);
  const summaryFile = path.join(suiteDirectory, "real-provider-examples-summary.json");
  const summary = readJson(summaryFile);
  const cacheAudit = readJson(path.resolve(args.cache));
  invariant(summary.schemaVersion === "generation-real-provider-suite-evidence@2", "unsupported suite summary");
  invariant(summary.status === "accepted" && summary.provider?.realProviderVerified === true, "suite is not accepted real-provider evidence");
  invariant(summary.provenance?.gitDirty === false && nonEmptyString(summary.provenance?.gitCommit), "suite source is not release-eligible");
  invariant(nonEmptyString(summary.provider?.modelResourceId)
    && nonNegativeInteger(summary.provenance?.providerResourceRevision)
    && HASH.test(summary.provenance?.providerConfigSha256 || ""), "suite Provider identity is incomplete");
  invariant(cacheAudit.schemaVersion === "provider-cache-smoke-audit@1"
    && cacheAudit.toolSetHashVersion === "tool-definition-set@1"
    && cacheAudit.status === "passed" && cacheAudit.releaseEligible === true,
  "cache audit is not release-eligible");
  invariant(cacheAudit.sourceCommit === summary.provenance.gitCommit
    && cacheAudit.modelResourceId === summary.provider.modelResourceId
    && cacheAudit.providerResourceRevision === summary.provenance.providerResourceRevision
    && cacheAudit.providerConfigSha256 === summary.provenance.providerConfigSha256,
  "cache audit identity mismatch");
  if (args.edit) {
    assembleEditBundle({
      args,
      suiteRealDirectory,
      outputDirectory,
      summary,
      cacheAudit,
    });
    return;
  }
  if (args.repair) {
    assembleRepairBundle({
      args,
      suiteRealDirectory,
      outputDirectory,
      summary,
      cacheAudit,
    });
    return;
  }
  if (args.restart) {
    assembleRestartBundle({
      args,
      suiteRealDirectory,
      outputDirectory,
      summary,
      cacheAudit,
    });
    return;
  }
  const caseFile = path.join(suiteDirectory, `real-provider-case-${args.case}.json`);
  const caseEvidence = readJson(caseFile);
  invariant(caseEvidence.schemaVersion === "generation-real-provider-case-evidence@2" && caseEvidence.status === "accepted", "case is not accepted");
  invariant(caseEvidence.id === args.case, "case identity mismatch");
  invariant(HASH.test(caseEvidence.promptSha256 || "")
    && HASH.test(caseEvidence.acceptance?.sha256 || "")
    && HASH.test(caseEvidence.contentPlan?.intentSha256 || ""), "case prompt or acceptance identity is incomplete");

  const finalRunIds = new Set(caseEvidence.attempts?.at(-1)?.runIds || []);
  const buildRuns = (caseEvidence.runs || []).filter((run) =>
    run.phase === "build" && run.status === "completed" && finalRunIds.has(run.runId));
  invariant(buildRuns.length === 1, "case must contain exactly one final accepted Build Run");
  const run = buildRuns[0];
  const cacheRun = (cacheAudit.runs || []).find((candidate) => candidate.runId === run.runId);
  invariant(cacheRun?.redactionValid === true
    && cacheRun?.compositionValid === true
    && cacheRun?.metricsValid === true
    && cacheRun?.providerIdentityValid === true
    && cacheRun?.buildIdentityValid === true
    && cacheRun?.generationContextIdentityValid === true
    && Number(cacheRun?.repeatedStableTurns) >= 2,
  "Build Run is not covered by the release cache audit");
  const { context, build } = validateRunIdentity(run, summary, "build");
  const operationRuns = operationRunsForPrimary(caseEvidence.runs, run);
  const { eventBytes, events, usage } = readCombinedOperationStreams(
    operationRuns,
    suiteRealDirectory,
    suiteRealDirectory,
  );
  const { budgetProfiles, budgetProfilesSha256 } = attachBudgetEvidence(operationRuns, usage);
  const runModelUsage = createRunModelUsageEvidence(events, summary.provider.modelResourceId);
  const runModelUsageSha256 = sha256(jsonBytes(runModelUsage));

  const artifactProbe = caseEvidence.artifact || caseEvidence.draftPreview;
  invariant(artifactProbe?.httpStatus === 200
    && artifactProbe?.expectedTextFound === true
    && HASH.test(artifactProbe?.bodySha256 || "")
    && nonNegativeInteger(artifactProbe?.bodyBytes), "accepted HTTP artifact probe is missing");
  invariant(caseEvidence.sandboxRelease?.released === true, "terminal bundle requires released Sandbox evidence");

  const routeIdentity = {
    schemaVersion: "artifact-route-identity@1",
    buildId: build.buildId,
    candidateManifestHash: build.candidateManifestHash,
    artifactRouteManifestPath: build.artifactRouteManifestPath,
    artifactRouteManifestHash: build.artifactRouteManifestHash,
    sourceFingerprint: build.sourceFingerprint,
    sourceSnapshotUriSha256: sha256(build.sourceSnapshotUri || ""),
    entryRoute: caseEvidence.expectedRoute,
    httpProbe: {
      status: artifactProbe.httpStatus,
      bodySha256: artifactProbe.bodySha256,
      bodyBytes: artifactProbe.bodyBytes,
      expectedTextFound: artifactProbe.expectedTextFound,
    },
  };
  const caseSummary = {
    schemaVersion: "runtime-evidence-case-summary@1",
    suiteId: summary.suiteId,
    caseId: caseEvidence.id,
    kind: caseEvidence.kind,
    projectId: caseEvidence.projectId,
    runId: run.runId,
    operationRunIds: operationRuns.map((operationRun) => operationRun.runId),
    budgetRunIds: operationRuns.map((operationRun) => operationRun.runId),
    operationId: context.operationId,
    status: caseEvidence.status,
    promptSha256: caseEvidence.promptSha256,
    acceptanceContractSha256: caseEvidence.acceptance?.sha256,
    contentPlanIntentSha256: caseEvidence.contentPlan?.intentSha256,
    templateVersion: run.efficiency?.template,
    contextContentHash: context.contextContentHash,
    runContextBindingHash: context.runContextBindingHash,
    budgetProfileId: context.budgetProfileId,
    budgetProfileHash: context.budgetProfileHash,
    budgetProfilesSha256,
    runModelUsageSha256,
    budgetProfileRolloutMode: context.budgetProfileRolloutMode,
  };
  const validation = {
    schemaVersion: "runtime-validation-summary@1",
    checks: [
      { id: "case-accepted", status: "passed", owner: "runtime" },
      { id: "entry-route", status: "passed", owner: "serving" },
      { id: "provider-identity", status: "passed", owner: "runtime" },
      { id: "event-redaction", status: "passed", owner: "harness" },
      { id: "sandbox-release", status: "passed", owner: "runtime" },
    ],
  };
  const cacheSummary = providerCacheSummary(cacheAudit, args.cache);
  const manifest = {
    schemaVersion: "runtime-evidence-bundle@1",
    bundleKind: "real_provider_terminal",
    evidenceId: `${summary.suiteId}-${caseEvidence.id}-${run.runId}`,
    gitSha: summary.provenance.gitCommit,
    harnessRevision: "runtime-terminal-evidence@1",
    runtimeMode: "real_provider",
    scenarioKind: caseEvidence.kind,
    projectId: caseEvidence.projectId,
    runId: run.runId,
    operationId: context.operationId,
    modelResourceId: summary.provider.modelResourceId,
    modelResourceRevision: summary.provenance.providerResourceRevision,
    providerConfigSha256: summary.provenance.providerConfigSha256,
    promptSha256: caseEvidence.promptSha256,
    generationContextHash: context.contextContentHash,
    runContextBindingHash: context.runContextBindingHash,
    budgetProfileId: context.budgetProfileId,
    budgetProfileHash: context.budgetProfileHash,
    budgetProfilesSha256,
    runModelUsageSha256,
    templateVersion: run.efficiency?.template,
    candidateManifestHash: build.candidateManifestHash,
    artifactRouteManifestHash: build.artifactRouteManifestHash,
    sourceFingerprint: build.sourceFingerprint,
    streamSha256: sha256(eventBytes),
    result: { status: "accepted" },
    replayExpectations: {
      usage: usage.aggregate,
      budgetConformance: usage.budgetConformance,
      runModelUsageSha256,
      routeIdentitySha256: sha256(`${JSON.stringify(routeIdentity, null, 2)}\n`),
      validationPassed: true,
      providerCachePassed: true,
    },
  };
  writeTerminalBundle({
    outputDirectory,
    eventBytes,
    usage,
    runModelUsage,
    budgetProfiles,
    caseSummary,
    routeIdentity,
    validation,
    cacheSummary,
    manifest,
  });
}

main();
