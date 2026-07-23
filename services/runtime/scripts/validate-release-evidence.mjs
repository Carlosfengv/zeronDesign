#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { generationContextProtocol, validateGenerationContextRunEvidence } from "./generation-context-evidence.mjs";
import {
  enforceReleaseBudgetPolicy,
  validateReleaseBudgetPolicy,
} from "../../../infra/generation-reliability/runtime-release-budget-policy.mjs";
import { evaluateRuntimeEfficiencyBenchmark } from "../../../infra/generation-reliability/runtime-efficiency-benchmark.mjs";
import {
  assembleBenchmarkCohort,
  evaluateBenchmarkLedger,
} from "../../../infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs";
import { validateRuntimeEfficiencyBenchmarkSourceBinding } from "../../../infra/generation-reliability/collect-runtime-efficiency-benchmark.mjs";

function fail(errors, message) {
  errors.push(message);
}

function artifactAvailableAfterRelease(project) {
  return project?.artifactHttpStatusAfterRelease === 200
    && project?.artifactAccessAfterRelease?.authentication === "project-principal"
    && project?.artifactAccessAfterRelease?.authenticated === true
    && project?.artifactAccessAfterRelease?.projectId === project?.projectId
    && project?.artifactAccessAfterRelease?.httpStatus === 200;
}

function cancelCleanupPassed(cleanup) {
  const legacyPreviewCleanup = cleanup?.previewHttpStatusAfterCancel === 404;
  const snapshotOnlyCleanup = cleanup?.previewLeaseStatus === "not_applicable"
    && cleanup?.durableDraftCreated === false;
  return cleanup?.passed === true
    && cleanup?.runStatus === "cancelled"
    && hasText(cleanup?.runId)
    && (legacyPreviewCleanup || snapshotOnlyCleanup);
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function sha256(value) {
  return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
}

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

export function validateProviderCacheRawBinding(evidence, rawText) {
  const rawSha256 = createHash("sha256").update(rawText).digest("hex");
  if (rawSha256 !== evidence?.providerCacheSmokeSha256) {
    throw new Error("release Provider Cache raw SHA-256 mismatch");
  }
  const authoritative = JSON.parse(rawText);
  if (authoritative?.toolSetHashVersion !== "tool-definition-set@1"
    || canonical(authoritative) !== canonical(evidence?.providerCacheSmoke)) {
    throw new Error("release Provider Cache raw evidence mismatch");
  }
  return authoritative;
}

function positiveInteger(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function validateEfficiencyBenchmark(errors, benchmark, provider, repositoryCommit) {
  const cohort = benchmark?.cohort;
  const evaluation = benchmark?.evaluation;
  const sourceLedger = benchmark?.sourceLedger;
  const sourceBinding = benchmark?.sourceBinding;
  if (!cohort || !evaluation || !sourceLedger || !sourceBinding) {
    fail(errors, "passing Runtime efficiency Benchmark evidence is required");
    return;
  }
  const recomputed = evaluateRuntimeEfficiencyBenchmark(cohort);
  const profiles = Array.isArray(cohort.profiles) ? cohort.profiles : [];
  const workloads = new Set(profiles.map(profile => profile?.workload));
  if (sourceLedger?.schemaVersion !== "runtime-efficiency-benchmark-ledger-verification@1"
    || sourceLedger?.status !== "passed"
    || !sha256(sourceLedger?.ledgerSha256)
    || sourceLedger.ledgerSha256 !== cohort?.ledger?.sha256
    || sourceBinding?.schemaVersion !== "runtime-efficiency-benchmark-source-binding@1"
    || sourceBinding?.status !== "passed"
    || !sha256(sourceBinding?.pairedLedgerSha256)
    || !sha256(sourceBinding?.pairedLedgerHeadRecordHash)
    || !sha256(sourceBinding?.mappingSha256)
    || sourceBinding?.attemptCount !== sourceLedger?.attemptCount
    || sourceBinding?.source?.commit !== repositoryCommit
    || sourceBinding?.source?.dirty !== false
    || cohort?.source?.commit !== repositoryCommit
    || cohort?.source?.dirty !== false
    || evaluation?.result !== "pass"
    || recomputed.result !== "pass"
    || JSON.stringify(evaluation) !== JSON.stringify(recomputed)
    || !workloads.has("greenfield_build")
    || !workloads.has("style_token_edit")
    || profiles.some(profile =>
      profile?.modelResourceId !== provider?.modelResourceId
      || profile?.providerResourceRevision !== provider?.providerResourceRevision
      || profile?.providerParametersHash !== provider?.providerConfigSha256)) {
    fail(errors, "Runtime efficiency Benchmark identity, sample, or threshold gate failed");
  }
}

function validateDcpStage(errors, stage, label) {
  const generationContext = generationContextProtocol(stage) === "generation-context";
  if (generationContext) {
    const protocolErrors = validateGenerationContextRunEvidence(stage, label);
    errors.push(...protocolErrors);
    if (protocolErrors.length) return false;
  } else if (stage?.package?.effectiveCompatibilityMode !== "enforced"
    || !sha256(stage?.package?.contentHash)
    || !Array.isArray(stage?.missingRequiredReads)
    || stage.missingRequiredReads.length !== 0) {
    return false;
  }
  return stage?.gate === "ready"
    && stage?.materialization?.ready === true
    && sha256(stage?.materialization?.hash)
    && stage?.styleContract?.verified === true
    && ["computed-style", "a11y", "viewport"].every(
      capability => stage?.verification?.capabilities?.[capability]?.available === true,
    );
}

function stageContextHash(stage) {
  return generationContextProtocol(stage) === "generation-context"
    ? stage?.generationContext?.contextContentHash
    : stage?.package?.contentHash;
}

function validateEnforcedDcpFixture(errors, fixture, label = "enforced DCP") {
  const dcp = fixture?.designContextEnforced;
  const lifecycle = dcp?.lifecycle;
  const runIds = [lifecycle?.buildRunId, lifecycle?.editRunId, lifecycle?.reviewRunId, lifecycle?.repairRunId];
  if (runIds.some(value => !hasText(value)) || new Set(runIds).size !== 4) {
    fail(errors, `${label} Build/Edit/Review/Repair lifecycle IDs are missing or not distinct`);
    return;
  }
  if (!hasText(lifecycle?.findingId) || !hasText(lifecycle?.candidateVersionId) || lifecycle?.findingStatus !== "fixed") {
    fail(errors, `${label} repaired finding lineage is missing or not fixed`);
  }
  const stages = [
    ["build", dcp?.build, lifecycle.buildRunId],
    ["edit", dcp?.edit, lifecycle.editRunId],
    ["repair", dcp?.repair, lifecycle.repairRunId],
  ];
  for (const [name, stage, runId] of stages) {
    if (stage?.runId !== runId || !validateDcpStage(errors, stage, `${label}.${name}`)) {
      fail(errors, `${label} ${name} diagnostics are missing or failed`);
    }
  }
  const protocols = stages.map(([, stage]) => generationContextProtocol(stage));
  if (new Set(protocols).size !== 1) {
    fail(errors, `${label} mixes legacy and generation-context stage evidence`);
  } else if (protocols[0] === "legacy"
    && new Set(stages.map(([, stage]) => stageContextHash(stage))).size !== 1) {
    fail(errors, `${label} legacy content hash changed across Build/Edit/Repair`);
  } else if (protocols[0] === "generation-context") {
    if (new Set(stages.map(([, stage]) => stage?.templateVersion)).size !== 1) {
      fail(errors, `${label} template version changed across Build/Edit/Repair`);
    }
    if (new Set(stages.map(([, stage]) => stage?.generationContext?.runContextBindingHash)).size !== stages.length) {
      fail(errors, `${label} Generation Context binding hashes must be distinct per Run`);
    }
  }
  if (new Set(stages.map(([, stage]) => stage?.materialization?.hash)).size !== 1) {
    fail(errors, `${label} materialization hash changed across Build/Edit/Repair`);
  }
  const reviewRepair = fixture?.reviewRepair;
  const finding = reviewRepair?.findings?.find(item => item?.findingId === lifecycle.findingId);
  if (reviewRepair?.reviewRunId !== lifecycle.reviewRunId
    || reviewRepair?.repairRunId !== lifecycle.repairRunId
    || finding?.versionId !== lifecycle.candidateVersionId
    || finding?.severity !== "blocking"
    || finding?.repairable !== true
    || finding?.status !== "fixed") {
    fail(errors, `${label} Review finding and Repair evidence are inconsistent`);
  }
}

function validateProviderDcpProject(errors, project) {
  if (!project || project.kind !== "website") {
    fail(errors, "real-provider enforced DCP Website project is required");
    return;
  }
  for (const field of ["projectId", "buildRunId", "editRunId", "bindingId", "podUid", "buildId", "candidateManifestHash", "sourceSnapshotUri", "previewLeaseId", "screenshotId", "versionAfterCas", "artifactManifestHash", "artifactUrl", "sandboxReleasedAt"]) {
    if (!hasText(project[field])) fail(errors, `providerDcpProject.${field} is required`);
  }
  validateEnforcedDcpFixture(errors, project, "real-provider enforced DCP");
  if (project?.artifactAssertions?.content?.matched !== true
    || project?.artifactAssertions?.computedStyle?.passed !== true
    || project?.artifactAssertions?.route !== "/") {
    fail(errors, "real-provider enforced DCP artifact assertion is missing or failed");
  }
  if (project?.terminalToolFailureCount !== 0
    || project?.dependencyEvidence?.passed !== true
    || project?.cancelCleanup?.passed !== true
    || !artifactAvailableAfterRelease(project)) {
    fail(errors, "real-provider enforced DCP Runtime lifecycle evidence is missing or failed");
  }
  const provenance = project?.designContextEnforced?.reviewProvenance;
  if (provenance?.source !== "fixture-seeded"
    || provenance?.mutationProvider !== "deepseek"
    || provenance?.reviewProvider !== "deterministic-tool-sequence"
    || provenance?.repairProvider !== "deepseek") {
    fail(errors, "real-provider enforced DCP Review provenance must distinguish seeded Review from real Build/Edit/Repair");
  }
}

export function validateReleaseEvidence(evidence) {
  const errors = [];
  if (evidence?.schemaVersion !== "release-evidence@1") fail(errors, "schemaVersion must be release-evidence@1");
  if (evidence?.releaseEligible !== true) fail(errors, "releaseEligible must be true");
  if (evidence?.result !== "pass") fail(errors, "result must be pass");
  if (!hasText(evidence?.repository?.commit)) fail(errors, "repository.commit is required");
  if (evidence?.repository?.dirty !== false) fail(errors, "repository must be clean");
  if (!sha256(evidence?.repository?.lockHash)) fail(errors, "repository.lockHash must be sha256");
  if (!hasText(evidence?.cluster?.name) || !hasText(evidence?.cluster?.kubeContext)) fail(errors, "cluster identity is required");
  if (!hasText(evidence?.cluster?.createdAt) || !hasText(evidence?.cluster?.nodeUid)) fail(errors, "fresh cluster metadata is required");

  const runtime = evidence?.images?.runtime;
  const sandbox = evidence?.images?.sandbox;
  if (!hasText(runtime?.ref) || !hasText(runtime?.configDigest) || !hasText(runtime?.manifestDigest)) fail(errors, "runtime image identity is required");
  if (!/^sha256:[a-f0-9]{64}$/.test(runtime?.manifestDigest ?? "")) fail(errors, "runtime image manifest digest is required");
  if (!hasText(sandbox?.ref) || !hasText(sandbox?.configDigest)) fail(errors, "sandbox image identity is required");
  if (runtime?.reportedCommit !== evidence?.repository?.commit) fail(errors, "Runtime reported commit does not match repository commit");
  for (const name of ["controller", "npmProxy", "dockerfileFrontend"]) {
    if (!/@sha256:[a-f0-9]{64}$/.test(evidence?.images?.[name] ?? "")) fail(errors, `images.${name} must be digest pinned`);
  }

  if (evidence?.transport?.mode !== "mtls" || evidence?.transport?.mtlsVerified !== true) fail(errors, "mTLS gate is required");
  if (!sha256(evidence?.transport?.runtimeSanHash) || !sha256(evidence?.transport?.sandboxSanHash)) fail(errors, "transport SAN hashes are required");
  if (evidence?.transport?.rotationWindowVerified !== true || !sha256(evidence?.transport?.runtimeCertSerialHash) || !sha256(evidence?.transport?.sandboxCertSerialHash) || !hasText(evidence?.transport?.runtimeCertExpiresAt) || !hasText(evidence?.transport?.sandboxCertExpiresAt)) fail(errors, "mTLS certificate rotation and sanitized certificate metadata are required");
  if (evidence?.auth?.principalMode !== "required" || evidence?.auth?.projectOwnershipVerified !== true) fail(errors, "public principal ownership gate is required");
  if (evidence?.auth?.channelJwtVerified !== true) fail(errors, "workspace channel JWT gate is required");
  if (evidence?.provider?.mode !== "real") fail(errors, "real provider evidence is required");
  if (!hasText(evidence?.provider?.model)) fail(errors, "provider model is required");
  if (!hasText(evidence?.provider?.modelResourceId)
    || !positiveInteger(evidence?.provider?.providerResourceRevision)
    || !sha256(evidence?.provider?.providerConfigSha256)) {
    fail(errors, "provider resource identity, revision, and configuration digest are required");
  }
  if (evidence?.provider?.credentialPresent !== true) fail(errors, "provider credential presence was not recorded");
  const providerCache = evidence?.providerCacheSmoke;
  const providerCacheRuns = Array.isArray(providerCache?.runs) ? providerCache.runs : [];
  const providerCacheStableRuns = providerCacheRuns.filter((run) =>
    run?.compositionValid === true
    && run?.metricsValid === true
    && run?.providerIdentityValid === true
    && run?.buildIdentityValid === true
    && run?.generationContextIdentityValid === true
    && run?.redactionValid === true
    && run?.repeatedStableTurns >= 2);
  const providerCacheGrossInputTokens = providerCacheStableRuns.reduce(
    (total, run) => total + Number(run.grossInputTokens || 0),
    0,
  );
  const providerCacheCachedInputTokens = providerCacheStableRuns.reduce(
    (total, run) => total + Number(run.cachedInputTokens || 0),
    0,
  );
  if (providerCache?.schemaVersion !== "provider-cache-smoke-audit@1"
    || providerCache?.toolSetHashVersion !== "tool-definition-set@1"
    || providerCache?.status !== "passed"
    || providerCache?.releaseEligible !== true
    || !positiveInteger(providerCache?.stableRunCount)
    || !positiveInteger(providerCache?.cachedInputTokens)
    || providerCache?.invalidRunCount !== 0
    || providerCache?.auditedRunCount !== providerCacheRuns.length
    || providerCache?.stableRunCount !== providerCacheStableRuns.length
    || providerCache?.grossInputTokens !== providerCacheGrossInputTokens
    || providerCache?.cachedInputTokens !== providerCacheCachedInputTokens
    || providerCache?.sourceDirty !== false
    || providerCache?.sourceCommit !== evidence?.repository?.commit
    || providerCache?.modelResourceId !== evidence?.provider?.modelResourceId
    || providerCache?.providerResourceRevision !== evidence?.provider?.providerResourceRevision
    || providerCache?.providerConfigSha256 !== evidence?.provider?.providerConfigSha256) {
    fail(errors, "release requires passing real-provider stable-prefix cache smoke evidence");
  }
  const terminalBundles = evidence?.terminalBundles;
  const terminalEntries = Array.isArray(terminalBundles?.entries)
    ? terminalBundles.entries
    : [];
  const requiredTerminalKinds = [
    "website",
    "docs",
    "edit",
    "repair",
    "runtime_restart",
  ];
  const terminalKinds = new Set(terminalEntries.map((entry) => entry?.kind));
  const terminalWebsite = terminalEntries.find((entry) => entry?.kind === "website");
  const terminalDocs = terminalEntries.find((entry) => entry?.kind === "docs");
  let releaseBudgetPolicyValid = true;
  try {
    validateReleaseBudgetPolicy(evidence?.releaseBudgetPolicy);
    for (const entry of terminalEntries) {
      const result = enforceReleaseBudgetPolicy(
        evidence.releaseBudgetPolicy,
        entry?.budgetProfiles,
      );
      if (result.status !== "passed"
        || result.runs.length !== entry?.budgetProfileCount
        || !result.runs.some((run) =>
          run.runId === entry?.runId && run.profileHash === entry?.budgetProfileHash)) {
        throw new Error("terminal Budget Policy result does not bind the primary Run");
      }
    }
  } catch {
    releaseBudgetPolicyValid = false;
  }
  if (terminalBundles?.schemaVersion !== "runtime-terminal-bundle-index@1"
    || terminalBundles?.status !== "passed"
    || !sha256(terminalBundles?.setSha256)
    || terminalBundles?.budgetPolicyStatus !== "passed"
    || !sha256(terminalBundles?.budgetPolicySha256)
    || terminalBundles?.budgetPolicySha256 !== evidence?.releaseBudgetPolicySha256
    || releaseBudgetPolicyValid !== true
    || !positiveInteger(terminalBundles?.filesScanned)
    || terminalBundles?.replayedCount !== terminalEntries.length
    || terminalEntries.length < requiredTerminalKinds.length
    || new Set(terminalEntries.map((entry) => entry?.evidenceId)).size !== terminalEntries.length
    || requiredTerminalKinds.some((kind) => !terminalKinds.has(kind))
    || terminalWebsite?.entryRoute !== "/"
    || terminalDocs?.entryRoute !== "/docs/"
    || terminalWebsite?.runId === terminalDocs?.runId
    || terminalWebsite?.projectId === terminalDocs?.projectId
    || terminalEntries.some((entry) =>
      !hasText(entry?.evidenceId)
      || entry?.replayStatus !== "passed"
      || entry?.resultStatus !== "accepted"
      || entry?.bundleKind !== (entry?.kind === "runtime_restart"
        ? "runtime_restart_terminal"
        : "real_provider_terminal")
      || entry?.gitSha !== evidence?.repository?.commit
      || entry?.modelResourceId !== evidence?.provider?.modelResourceId
      || entry?.modelResourceRevision !== evidence?.provider?.providerResourceRevision
      || entry?.providerConfigSha256 !== evidence?.provider?.providerConfigSha256
      || entry?.providerCacheSourceAuditSha256 !== evidence?.providerCacheSmokeSha256
      || !hasText(entry?.budgetProfileId)
      || !sha256(entry?.budgetProfileHash)
      || !sha256(entry?.budgetProfilesSha256)
      || !sha256(entry?.runModelUsageSha256)
      || entry?.budgetConformanceStatus !== "passed"
      || entry?.releaseBudgetPolicyStatus !== "passed"
      || !positiveInteger(entry?.budgetProfileCount)
      || !positiveInteger(entry?.maxTurnInputTokens)
      || !sha256(entry?.checksumsSha256)
      || !sha256(entry?.streamSha256)
      || !hasText(entry?.projectId)
      || !hasText(entry?.runId)
      || !hasText(entry?.operationId))) {
    fail(errors, "release requires replayed terminal Bundles for Website, Docs, Edit, Repair, and Runtime Restart");
  }
  if (!sha256(evidence?.providerCacheSmokeSha256)) {
    fail(errors, "release Provider Cache evidence SHA-256 is required");
  }
  if (!sha256(evidence?.releaseBudgetPolicySha256)) {
    fail(errors, "release Budget Policy SHA-256 is required");
  }
  validateEfficiencyBenchmark(
    errors,
    evidence?.efficiencyBenchmark,
    evidence?.provider,
    evidence?.repository?.commit,
  );

  const preflight = evidence?.preflight;
  if (preflight?.schemaVersion !== "runtime-rc-preflight@1" || preflight?.passed !== true || preflight?.prefetchImages !== true) {
    fail(errors, "successful release-mode preflight evidence is required");
  }
  if (!sha256(preflight?.lockHash) || preflight?.lockHash !== evidence?.repository?.lockHash) {
    fail(errors, "preflight lockHash must match the release repository lockHash");
  }
  if (!Array.isArray(preflight?.entries) || preflight.entries.length === 0
    || preflight.entries.some(entry => entry?.lockedDigestVerified !== true
      || entry?.mutableTagMatchesLock !== true
      || entry?.pulled !== true
      || !hasText(entry?.canonicalRef)
      || !/^sha256:[a-f0-9]{64}$/.test(entry?.lockedDigest ?? ""))) {
    fail(errors, "preflight image inspect/pull evidence is incomplete or failed");
  }

  const fixture = evidence?.fixture;
  const fixtureProjects = Array.isArray(fixture?.projects) ? fixture.projects : [];
  if (fixture?.deployed !== true || fixture?.concurrent !== true) fail(errors, "deployed concurrent fixture gate is required");
  for (const kind of ["website", "docs"]) {
    const project = fixtureProjects.find(item => item?.kind === kind);
    if (!project || !hasText(project.buildRunId) || !hasText(project.editRunId) || project.buildRunId === project.editRunId || !artifactAvailableAfterRelease(project) || project.terminalToolFailureCount !== 0 || project.artifactAssertions?.computedStyle?.passed !== true || project.cancelCleanup?.passed !== true || project.dependencyEvidence?.passed !== true) {
      fail(errors, `deployed fixture ${kind} lifecycle evidence is missing or failed`);
    }
  }
  if (fixtureProjects.length >= 2 && (fixtureProjects[0].podUid === fixtureProjects[1].podUid || fixtureProjects[0].artifactManifestHash === fixtureProjects[1].artifactManifestHash)) {
    fail(errors, "deployed fixture Website and Docs are not isolated");
  }
  validateEnforcedDcpFixture(errors, evidence?.enforcedDcpFixture);
  validateProviderDcpProject(errors, evidence?.providerDcpProject);

  const projects = Array.isArray(evidence?.projects) ? evidence.projects : [];
  for (const kind of ["website", "docs"]) {
    const project = projects.find(item => item?.kind === kind);
    if (!project) {
      fail(errors, `${kind} project evidence is missing`);
      continue;
    }
    for (const field of ["projectId", "buildRunId", "editRunId", "bindingId", "podUid", "buildId", "candidateManifestHash", "sourceSnapshotUri", "previewLeaseId", "versionAfterCas", "artifactManifestHash", "artifactUrl", "sandboxReleasedAt"]) {
      if (!hasText(project[field])) fail(errors, `${kind}.${field} is required`);
    }
    if (project.buildRunId === project.editRunId) fail(errors, `${kind} Build and Edit run IDs must differ`);
    const screenshotPassed = hasText(project.screenshotId) && project.nonblankPixelRatio > 0;
    const publication = project.publication;
    const publishWorkflowPassed = hasText(publication?.workflowId)
      && publication?.status === "completed"
      && publication?.checkpoint === "completed"
      && publication?.versionId === project.versionAfterCas
      && hasText(publication?.releaseId)
      && hasText(publication?.publicationOperationId)
      && /^https:\/\//.test(publication?.publicUrl ?? "")
      && publication?.externalProbePassed === true
      && project?.visualReview?.mode === "advisory"
      && project?.visualReview?.status === "not_requested";
    if (!screenshotPassed && !publishWorkflowPassed) {
      fail(errors, `${kind} requires a nonblank screenshot or completed advisory PublishWorkflow evidence`);
    }
    if (project.artifactAssertions?.content?.matched !== true || !sha256(project.artifactAssertions?.content?.expectedTextSha256) || !sha256(project.artifactAssertions?.content?.documentSha256)) {
      fail(errors, `${kind} artifact content assertion is missing or failed`);
    }
    const expectedArtifactRoute = kind === "docs" ? "/docs/" : "/";
    if (project.artifactAssertions?.route !== expectedArtifactRoute) {
      fail(errors, `${kind} artifact content was not asserted at ${expectedArtifactRoute}`);
    }
    const computed = project.artifactAssertions?.computedStyle;
    if (computed?.passed !== true || computed?.selector !== "body" || !hasText(computed?.display) || computed.display === "none" || !hasText(computed?.color) || !hasText(computed?.fontFamily)) {
      fail(errors, `${kind} artifact computed-style assertion is missing or failed`);
    }
    if (!cancelCleanupPassed(project.cancelCleanup)) {
      fail(errors, `${kind} run cancellation did not clean generation resources`);
    }
    if (project.dependencyEvidence?.passed !== true || project.dependencyEvidence?.nodeModulesPresent !== true || !sha256(project.dependencyEvidence?.lockfileSha256) || !(project.dependencyEvidence?.tarballRequestCount > 0)) {
      fail(errors, `${kind} Runtime dependency installation evidence is missing or failed`);
    }
    const legacyEventSequencePassed = project.events?.sequenceValid === true
      && hasText(project.events?.previewUpdated)
      && hasText(project.events?.runCompleted);
    if (!legacyEventSequencePassed && !publishWorkflowPassed) {
      fail(errors, `${kind} event sequence is invalid and has no completed PublishWorkflow replacement`);
    }
    if (project.terminalToolFailureCount !== 0) fail(errors, `${kind} has terminal tool failures`);
    if (!artifactAvailableAfterRelease(project)) fail(errors, `${kind} artifact is unavailable to its project principal after Sandbox release`);
  }

  const recovery = Array.isArray(evidence?.recoveryScenarios) ? evidence.recoveryScenarios : [];
  const requiredRecoveryScenarios = [
    "runtime-restart",
    "port-forward-kill",
    "sandbox-pod-replacement",
    "channel-lease-pod-uid-change",
    "checkpoint-runtime-restart",
    "artifact-staged-before-cas",
    "cas-before-event",
    "run-cancel",
  ];
  for (const scenario of requiredRecoveryScenarios) {
    if (!recovery.some(item => item?.scenario === scenario)) fail(errors, `recovery scenario is missing: ${scenario}`);
  }
  for (const scenario of recovery) {
    if (scenario?.result !== "pass" || scenario?.orphanCount !== 0) fail(errors, `recovery scenario failed: ${scenario?.scenario ?? "unknown"}`);
  }
  if (evidence?.networkChecks?.directRegistryDenied !== true || evidence?.networkChecks?.npmProxyInstallPassed !== true) fail(errors, "npm network policy gate failed");
  if (evidence?.secretScan?.patternSet !== "runtime-credentials@1" || !(evidence?.secretScan?.filesScanned > 0)) fail(errors, "secret scan metadata is missing");
  if (!Array.isArray(evidence?.secretScan?.matches) || evidence.secretScan.matches.length !== 0) fail(errors, "secret scan found credential-like content");
  return errors;
}

async function main() {
  const [evidencePath, ...rawArgs] = process.argv.slice(2);
  if (!evidencePath || rawArgs.length % 2 !== 0) {
    throw new Error("usage: validate-release-evidence.mjs <release-evidence.json> [--provider-cache <provider-cache-smoke-audit.json> --budget-policy <policy.json>] [--efficiency-benchmark-ledger <ledger.jsonl> --efficiency-source-ledger <paired-ledger.ndjson> --efficiency-import-mapping <mapping.json>]");
  }
  const args = {};
  for (let index = 0; index < rawArgs.length; index += 2) {
    const flag = rawArgs[index];
    if (!flag?.startsWith("--") || rawArgs[index + 1] === undefined) {
      throw new Error(`invalid argument: ${flag ?? "<missing>"}`);
    }
    args[flag.slice(2)] = rawArgs[index + 1];
  }
  if (Object.keys(args).some(key => ![
    "budget-policy",
    "provider-cache",
    "efficiency-benchmark-ledger",
    "efficiency-source-ledger",
    "efficiency-import-mapping",
  ].includes(key))) {
    throw new Error("unsupported release validation argument");
  }
  const evidence = JSON.parse(await readFile(evidencePath, "utf8"));
  if (evidence.releaseEligible === true) {
    if (!args["provider-cache"]) throw new Error("release validation requires --provider-cache");
    if (!args["budget-policy"]) throw new Error("release validation requires --budget-policy");
    if (!args["efficiency-benchmark-ledger"]) {
      throw new Error("release validation requires --efficiency-benchmark-ledger");
    }
    if (!args["efficiency-source-ledger"] || !args["efficiency-import-mapping"]) {
      throw new Error("release validation requires --efficiency-source-ledger and --efficiency-import-mapping");
    }
    const providerCacheRaw = await readFile(args["provider-cache"], "utf8");
    validateProviderCacheRawBinding(evidence, providerCacheRaw);
    const policyRaw = await readFile(args["budget-policy"], "utf8");
    const policySha256 = createHash("sha256").update(policyRaw).digest("hex");
    if (policySha256 !== evidence.releaseBudgetPolicySha256) {
      throw new Error("release Budget Policy raw SHA-256 mismatch");
    }
    const authoritativeBenchmark = evaluateBenchmarkLedger(args["efficiency-benchmark-ledger"]);
    const authoritativeCohort = assembleBenchmarkCohort(args["efficiency-benchmark-ledger"]);
    const authoritativeSourceBinding = validateRuntimeEfficiencyBenchmarkSourceBinding(
      args["efficiency-source-ledger"],
      args["efficiency-benchmark-ledger"],
      JSON.parse(await readFile(args["efficiency-import-mapping"], "utf8")),
    );
    if (authoritativeBenchmark.evaluation.result !== "pass"
      || authoritativeBenchmark.sourceLedger.ledgerSha256
        !== evidence?.efficiencyBenchmark?.sourceLedger?.ledgerSha256
      || JSON.stringify(authoritativeCohort) !== JSON.stringify(evidence?.efficiencyBenchmark?.cohort)
      || JSON.stringify(authoritativeBenchmark.evaluation)
        !== JSON.stringify(evidence?.efficiencyBenchmark?.evaluation)
      || JSON.stringify(authoritativeSourceBinding)
        !== JSON.stringify(evidence?.efficiencyBenchmark?.sourceBinding)) {
      throw new Error("release Runtime efficiency Benchmark raw Ledger mismatch");
    }
  }
  const errors = validateReleaseEvidence(evidence);
  if (errors.length) {
    for (const error of errors) process.stderr.write(`release evidence: ${error}\n`);
    process.exitCode = 1;
    return;
  }
  process.stdout.write(`Release evidence valid: ${evidencePath}\n`);
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  await main();
}
