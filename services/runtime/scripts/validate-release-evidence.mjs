#!/usr/bin/env node

import { readFile } from "node:fs/promises";

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

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function sha256(value) {
  return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
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
    if (stage?.runId !== runId
      || stage?.package?.effectiveCompatibilityMode !== "enforced"
      || !sha256(stage?.package?.contentHash)
      || stage?.gate !== "ready"
      || stage?.materialization?.ready !== true
      || !sha256(stage?.materialization?.hash)
      || stage?.styleContract?.verified !== true
      || !Array.isArray(stage?.missingRequiredReads)
      || stage.missingRequiredReads.length !== 0
      || ["computed-style", "a11y", "viewport"].some(capability => stage?.verification?.capabilities?.[capability]?.available !== true)) {
      fail(errors, `${label} ${name} diagnostics are missing or failed`);
    }
  }
  if (new Set(stages.map(([, stage]) => stage?.package?.contentHash)).size !== 1) {
    fail(errors, `${label} content hash changed across Build/Edit/Repair`);
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
  if (evidence?.provider?.credentialPresent !== true) fail(errors, "provider credential presence was not recorded");

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
    for (const field of ["projectId", "buildRunId", "editRunId", "bindingId", "podUid", "buildId", "candidateManifestHash", "sourceSnapshotUri", "previewLeaseId", "screenshotId", "versionAfterCas", "artifactManifestHash", "artifactUrl", "sandboxReleasedAt"]) {
      if (!hasText(project[field])) fail(errors, `${kind}.${field} is required`);
    }
    if (project.buildRunId === project.editRunId) fail(errors, `${kind} Build and Edit run IDs must differ`);
    if (!(project.nonblankPixelRatio > 0)) fail(errors, `${kind} screenshot is blank`);
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
    if (project.cancelCleanup?.passed !== true || project.cancelCleanup?.runStatus !== "cancelled" || project.cancelCleanup?.previewHttpStatusAfterCancel !== 404 || !hasText(project.cancelCleanup?.runId)) {
      fail(errors, `${kind} run cancellation did not clean Preview resources`);
    }
    if (project.dependencyEvidence?.passed !== true || project.dependencyEvidence?.nodeModulesPresent !== true || !sha256(project.dependencyEvidence?.lockfileSha256) || !(project.dependencyEvidence?.tarballRequestCount > 0)) {
      fail(errors, `${kind} Runtime dependency installation evidence is missing or failed`);
    }
    if (project.events?.sequenceValid !== true || !hasText(project.events?.previewUpdated) || !hasText(project.events?.runCompleted)) fail(errors, `${kind} event sequence is invalid`);
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
  const path = process.argv[2];
  if (!path) throw new Error("usage: validate-release-evidence.mjs <release-evidence.json>");
  const evidence = JSON.parse(await readFile(path, "utf8"));
  const errors = validateReleaseEvidence(evidence);
  if (errors.length) {
    for (const error of errors) process.stderr.write(`release evidence: ${error}\n`);
    process.exitCode = 1;
    return;
  }
  process.stdout.write(`Release evidence valid: ${path}\n`);
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  await main();
}
