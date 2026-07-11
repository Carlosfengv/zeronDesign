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
  if (evidence?.provider?.mode !== "approved-real") fail(errors, "approved real provider evidence is required");
  if (!hasText(evidence?.provider?.model) || !hasText(evidence?.provider?.approvalReference)) fail(errors, "provider model and approval reference are required");
  if (evidence?.provider?.credentialPresent !== true) fail(errors, "provider credential presence was not recorded");

  const fixture = evidence?.fixture;
  const fixtureProjects = Array.isArray(fixture?.projects) ? fixture.projects : [];
  if (fixture?.deployed !== true || fixture?.concurrent !== true) fail(errors, "deployed concurrent fixture gate is required");
  for (const kind of ["website", "docs"]) {
    const project = fixtureProjects.find(item => item?.kind === kind);
    if (!project || !hasText(project.buildRunId) || !hasText(project.editRunId) || project.buildRunId === project.editRunId || project.artifactHttpStatusAfterRelease !== 200 || project.terminalToolFailureCount !== 0 || project.artifactAssertions?.computedStyle?.passed !== true || project.cancelCleanup?.passed !== true || project.dependencyEvidence?.passed !== true) {
      fail(errors, `deployed fixture ${kind} lifecycle evidence is missing or failed`);
    }
  }
  if (fixtureProjects.length >= 2 && (fixtureProjects[0].podUid === fixtureProjects[1].podUid || fixtureProjects[0].artifactManifestHash === fixtureProjects[1].artifactManifestHash)) {
    fail(errors, "deployed fixture Website and Docs are not isolated");
  }

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
    if (project.artifactHttpStatusAfterRelease !== 200) fail(errors, `${kind} artifact is unavailable after Sandbox release`);
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
