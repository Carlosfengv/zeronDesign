#!/usr/bin/env node

import assert from "node:assert/strict";
import { validateReleaseEvidence } from "./validate-release-evidence.mjs";

const sha = "a".repeat(64);
const fixture = {
  schemaVersion: "release-evidence@1",
  releaseEligible: true,
  result: "pass",
  repository: { commit: "abc123", dirty: false, lockHash: sha },
  cluster: { name: "fresh", kubeContext: "k3d-fresh", createdAt: "2026-07-11T00:00:00Z", nodeUid: "node-1" },
  images: {
    runtime: { ref: "runtime:test", configDigest: `sha256:${sha}`, manifestDigest: `sha256:${sha}`, reportedCommit: "abc123" },
    sandbox: { ref: "sandbox:test", configDigest: `sha256:${sha}` },
    controller: `controller@sha256:${sha}`,
    npmProxy: `proxy@sha256:${sha}`,
    dockerfileFrontend: `frontend@sha256:${sha}`,
  },
  transport: { mode: "mtls", mtlsVerified: true, rotationWindowVerified: true, runtimeSanHash: sha, sandboxSanHash: sha, runtimeCertSerialHash: sha, sandboxCertSerialHash: sha, runtimeCertExpiresAt: "2026-07-13T00:00:00Z", sandboxCertExpiresAt: "2026-07-13T00:00:00Z" },
  auth: { principalMode: "required", projectOwnershipVerified: true, channelJwtVerified: true },
  provider: { mode: "real", model: "deepseek-v4-pro", credentialPresent: true },
  preflight: {
    schemaVersion: "runtime-rc-preflight@1", passed: true, prefetchImages: true, lockHash: sha,
    entries: [{ name: "runtime", canonicalRef: "registry.example/runtime:v1", lockedDigest: `sha256:${sha}`, lockedDigestVerified: true, mutableTagMatchesLock: true, pulled: true }],
  },
  projects: ["website", "docs"].map(kind => ({
    kind, projectId: kind, buildRunId: `${kind}-build`, editRunId: `${kind}-edit`, bindingId: `${kind}-binding`, podUid: `${kind}-pod`,
    buildId: `${kind}-build-id`, candidateManifestHash: sha, sourceSnapshotUri: `runtime://snapshot/${kind}`,
    previewLeaseId: `${kind}-lease`, screenshotId: `${kind}-shot`, nonblankPixelRatio: 0.1,
    versionBeforeCas: `${kind}-v1`, versionAfterCas: `${kind}-v2`, artifactManifestHash: sha,
    artifactUrl: `/artifacts/${kind}/current/`, events: { previewUpdated: "1", runCompleted: "2", sequenceValid: true },
    artifactAssertions: {
      route: kind === "docs" ? "/docs/" : "/",
      content: { expectedTextSha256: sha, documentSha256: sha, matched: true },
      computedStyle: { selector: "body", display: "block", color: "rgb(0, 0, 0)", fontFamily: "sans-serif", passed: true },
    },
    cancelCleanup: { runId: `${kind}-cancel`, runStatus: "cancelled", previewHttpStatusAfterCancel: 404, passed: true },
    dependencyEvidence: { podUid: `${kind}-pod`, pod: `${kind}-pod-name`, podIp: `10.0.0.${kind === "website" ? 1 : 2}`, nodeModulesPresent: true, lockfileSha256: kind === "website" ? "d".repeat(64) : "e".repeat(64), tarballRequestCount: 2, passed: true },
    recoverableToolFailureCount: 0, terminalToolFailureCount: 0,
    sandboxReleasedAt: "2026-07-11T00:00:00Z", artifactHttpStatusAfterRelease: 200,
    artifactAccessAfterRelease: { authentication: "project-principal", projectId: kind, httpStatus: 200, authenticated: true },
  })),
  recoveryScenarios: [
    ["runtime-restart", "active-preview-lease"],
    ["port-forward-kill", "active-preview-lease"],
    ["sandbox-pod-replacement", "ready-warm-pod"],
    ["channel-lease-pod-uid-change", "ready-channel-lease"],
    ["checkpoint-runtime-restart", "persisted-partial-run"],
    ["artifact-staged-before-cas", "promotion-wal"],
    ["cas-before-event", "promotion-outbox"],
    ["run-cancel", "active-preview-process"],
  ].map(([scenario, injectionPoint]) => ({ scenario, injectionPoint, result: "pass", orphanCount: 0 })),
  networkChecks: { directRegistryDenied: true, npmProxyInstallPassed: true },
  secretScan: { patternSet: "runtime-credentials@1", filesScanned: 5, matches: [] },
};
fixture.fixture = {
  deployed: true,
  concurrent: true,
  projects: structuredClone(fixture.projects),
};
fixture.fixture.projects[0].podUid = "fixture-website-pod";
fixture.fixture.projects[1].podUid = "fixture-docs-pod";
fixture.fixture.projects[0].artifactManifestHash = "b".repeat(64);
fixture.fixture.projects[1].artifactManifestHash = "c".repeat(64);
const dcpStage = runId => ({
  runId,
  gate: "ready",
  missingRequiredReads: [],
  materialization: { hash: sha, ready: true },
  package: { contentHash: sha, effectiveCompatibilityMode: "enforced" },
  styleContract: { verified: true },
  verification: { capabilities: {
    "computed-style": { available: true }, a11y: { available: true }, viewport: { available: true },
  } },
});
const generationDcpStage = (runId, index) => {
  const stage = dcpStage(runId);
  delete stage.package;
  delete stage.missingRequiredReads;
  stage.templateVersion = "next-app@2";
  stage.generationContext = {
    schemaVersion: "generation-context-status@1",
    runContractVersion: "generation-context@1",
    status: "compiled",
    contextContentHash: String(index + 1).repeat(64),
    runContextBindingHash: String(index + 4).repeat(64),
    runtimeAttestationHash: String(index + 7).repeat(64),
  };
  stage.attestation = {
    state: "verified",
    runtimeAttestationHash: stage.generationContext.runtimeAttestationHash,
  };
  stage.efficiency = {
    schemaVersion: "run-efficiency-metrics@1",
    uniqueContextReads: 0,
    uniqueSourceReads: 3,
    duplicateReads: 0,
    duplicateReadTokens: 0,
    unchangedReadStubs: 0,
    postCompactSourceRestores: 0,
    prebuildLists: 1,
  };
  return stage;
};
fixture.enforcedDcpFixture = {
  reviewRepair: {
    editRunId: "dcp-edit",
    reviewRunId: "dcp-review",
    repairRunId: "dcp-repair",
    findings: [{ findingId: "dcp-finding", versionId: "dcp-v2", severity: "blocking", repairable: true, status: "fixed" }],
  },
  designContextEnforced: {
    lifecycle: {
      buildRunId: "dcp-build", editRunId: "dcp-edit", reviewRunId: "dcp-review", repairRunId: "dcp-repair",
      findingId: "dcp-finding", candidateVersionId: "dcp-v2", findingStatus: "fixed",
    },
    build: dcpStage("dcp-build"), edit: dcpStage("dcp-edit"), repair: dcpStage("dcp-repair"),
  },
};
fixture.providerDcpProject = {
  ...structuredClone(fixture.projects[0]),
  projectId: "provider-dcp-website",
  buildRunId: "provider-dcp-build",
  editRunId: "provider-dcp-edit",
  bindingId: "provider-dcp-binding",
  podUid: "provider-dcp-pod",
  buildId: "provider-dcp-build-id",
  sourceSnapshotUri: "runtime://snapshot/provider-dcp",
  previewLeaseId: "provider-dcp-lease",
  screenshotId: "provider-dcp-shot",
  artifactUrl: "/artifacts/provider-dcp-website/current/",
  artifactAccessAfterRelease: { authentication: "project-principal", projectId: "provider-dcp-website", httpStatus: 200, authenticated: true },
  dependencyEvidence: { ...structuredClone(fixture.projects[0].dependencyEvidence), podUid: "provider-dcp-pod", pod: "provider-dcp-pod-name" },
  reviewRepair: {
    editRunId: "provider-dcp-edit",
    reviewRunId: "provider-dcp-review",
    repairRunId: "provider-dcp-repair",
    findings: [{ findingId: "provider-dcp-finding", versionId: "provider-dcp-v2", severity: "blocking", repairable: true, status: "fixed" }],
  },
  designContextEnforced: {
    lifecycle: {
      buildRunId: "provider-dcp-build", editRunId: "provider-dcp-edit", reviewRunId: "provider-dcp-review", repairRunId: "provider-dcp-repair",
      findingId: "provider-dcp-finding", candidateVersionId: "provider-dcp-v2", findingStatus: "fixed",
    },
    build: dcpStage("provider-dcp-build"), edit: dcpStage("provider-dcp-edit"), repair: dcpStage("provider-dcp-repair"),
    reviewProvenance: {
      source: "fixture-seeded", mutationProvider: "deepseek", reviewProvider: "deterministic-tool-sequence", repairProvider: "deepseek",
    },
  },
};

assert.deepEqual(validateReleaseEvidence(fixture), []);

const publishWorkflowFixture = structuredClone(fixture);
const website = publishWorkflowFixture.projects.find(project => project.kind === "website");
delete website.screenshotId;
delete website.nonblankPixelRatio;
website.visualReview = { mode: "advisory", status: "not_requested" };
website.publication = {
  workflowId: "publish-workflow-website",
  status: "completed",
  checkpoint: "completed",
  versionId: website.versionAfterCas,
  releaseId: "release-website",
  publicationOperationId: "operation-website",
  publicUrl: "https://website.works.example.test",
  externalProbePassed: true,
};
website.events.sequenceValid = false;
assert.deepEqual(validateReleaseEvidence(publishWorkflowFixture), []);

publishWorkflowFixture.projects[0].publication.externalProbePassed = false;
assert.ok(
  validateReleaseEvidence(publishWorkflowFixture).some(error =>
    error.includes("nonblank screenshot or completed advisory PublishWorkflow")),
);
const generationContextFixture = structuredClone(fixture);
for (const target of [generationContextFixture.enforcedDcpFixture, generationContextFixture.providerDcpProject]) {
  const lifecycle = target.designContextEnforced.lifecycle;
  target.designContextEnforced.build = generationDcpStage(lifecycle.buildRunId, 0);
  target.designContextEnforced.edit = generationDcpStage(lifecycle.editRunId, 1);
  target.designContextEnforced.repair = generationDcpStage(lifecycle.repairRunId, 2);
  target.designContextEnforced.edit.efficiency = {
    schemaVersion: "run-efficiency-metrics@1",
    calculatorVersion: "run-efficiency-calculator@1",
    prebuildFsReadCount: 1,
    prebuildFsListCount: 0,
    prebuildFsSearchCount: 0,
    contextReadDeliveries: 0,
    sourceReadDeliveries: 2,
    fullReadDeliveries: 2,
    duplicateFullReadDeliveries: 0,
    duplicateFullReadRateBasisPoints: 0,
    duplicateReadEstimatedTokens: 0,
  };
}
assert.deepEqual(validateReleaseEvidence(generationContextFixture), []);
const reusedGenerationBinding = structuredClone(generationContextFixture);
reusedGenerationBinding.enforcedDcpFixture.designContextEnforced.edit.generationContext.runContextBindingHash =
  reusedGenerationBinding.enforcedDcpFixture.designContextEnforced.build.generationContext.runContextBindingHash;
assert(validateReleaseEvidence(reusedGenerationBinding).some(error => error.includes("binding hashes")));
const dirty = structuredClone(fixture);
dirty.repository.dirty = true;
assert(validateReleaseEvidence(dirty).some(error => error.includes("clean")));
const missingEdit = structuredClone(fixture);
missingEdit.projects[0].editRunId = missingEdit.projects[0].buildRunId;
assert(validateReleaseEvidence(missingEdit).some(error => error.includes("must differ")));
const leaked = structuredClone(fixture);
leaked.secretScan.matches.push({ file: "log", pattern: "token" });
assert(validateReleaseEvidence(leaked).some(error => error.includes("credential-like")));
const missingComputedStyle = structuredClone(fixture);
missingComputedStyle.projects[0].artifactAssertions.computedStyle.passed = false;
assert(validateReleaseEvidence(missingComputedStyle).some(error => error.includes("computed-style")));
const wrongDocsRoute = structuredClone(fixture);
wrongDocsRoute.projects.find(project => project.kind === "docs").artifactAssertions.route = "/";
assert(validateReleaseEvidence(wrongDocsRoute).some(error => error.includes("was not asserted at /docs/")));
const missingRuntimeManifest = structuredClone(fixture);
delete missingRuntimeManifest.images.runtime.manifestDigest;
assert(validateReleaseEvidence(missingRuntimeManifest).some(error => error.includes("manifest digest")));
const missingPreflightPull = structuredClone(fixture);
missingPreflightPull.preflight.entries[0].pulled = false;
assert(validateReleaseEvidence(missingPreflightPull).some(error => error.includes("preflight image")));
const mismatchedPreflightLock = structuredClone(fixture);
mismatchedPreflightLock.preflight.lockHash = "b".repeat(64);
assert(validateReleaseEvidence(mismatchedPreflightLock).some(error => error.includes("preflight lockHash")));
const leakedPreview = structuredClone(fixture);
leakedPreview.projects[0].cancelCleanup.previewHttpStatusAfterCancel = 200;
assert(validateReleaseEvidence(leakedPreview).some(error => error.includes("cancellation")));
const snapshotOnlyCancellation = structuredClone(fixture);
snapshotOnlyCancellation.projects[0].cancelCleanup = {
  runId: "website-cancel",
  runStatus: "cancelled",
  previewLeaseStatus: "not_applicable",
  durableDraftCreated: false,
  passed: true,
};
assert(!validateReleaseEvidence(snapshotOnlyCancellation).some(error => error.includes("cancellation")));
snapshotOnlyCancellation.projects[0].cancelCleanup.durableDraftCreated = true;
assert(validateReleaseEvidence(snapshotOnlyCancellation).some(error => error.includes("cancellation")));
const anonymousArtifactProbe = structuredClone(fixture);
anonymousArtifactProbe.projects[0].artifactAccessAfterRelease.authentication = "anonymous";
assert(validateReleaseEvidence(anonymousArtifactProbe).some(error => error.includes("project principal")));
const sequentialFixture = structuredClone(fixture);
sequentialFixture.fixture.concurrent = false;
assert(validateReleaseEvidence(sequentialFixture).some(error => error.includes("concurrent fixture")));
const missingRepair = structuredClone(fixture);
delete missingRepair.enforcedDcpFixture.designContextEnforced.repair;
assert(validateReleaseEvidence(missingRepair).some(error => error.includes("repair diagnostics")));
const unfixedFinding = structuredClone(fixture);
unfixedFinding.enforcedDcpFixture.reviewRepair.findings[0].status = "repairing";
assert(validateReleaseEvidence(unfixedFinding).some(error => error.includes("inconsistent")));
const missingProviderDcp = structuredClone(fixture);
delete missingProviderDcp.providerDcpProject;
assert(validateReleaseEvidence(missingProviderDcp).some(error => error.includes("real-provider enforced DCP Website")));
const fixtureRepairProvider = structuredClone(fixture);
fixtureRepairProvider.providerDcpProject.designContextEnforced.reviewProvenance.repairProvider = "deterministic-tool-sequence";
assert(validateReleaseEvidence(fixtureRepairProvider).some(error => error.includes("Review provenance")));
const failedProviderDcpRepair = structuredClone(fixture);
failedProviderDcpRepair.providerDcpProject.reviewRepair.findings[0].status = "repairing";
assert(validateReleaseEvidence(failedProviderDcpRepair).some(error => error.includes("real-provider enforced DCP Review finding")));
process.stdout.write("release evidence validator tests passed\n");
