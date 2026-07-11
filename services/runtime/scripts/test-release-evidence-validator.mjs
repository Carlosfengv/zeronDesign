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
  provider: { mode: "approved-real", model: "approved-model", approvalReference: "R17-1", credentialPresent: true },
  projects: ["website", "docs"].map(kind => ({
    kind, projectId: kind, buildRunId: `${kind}-build`, editRunId: `${kind}-edit`, bindingId: `${kind}-binding`, podUid: `${kind}-pod`,
    buildId: `${kind}-build-id`, candidateManifestHash: sha, sourceSnapshotUri: `runtime://snapshot/${kind}`,
    previewLeaseId: `${kind}-lease`, screenshotId: `${kind}-shot`, nonblankPixelRatio: 0.1,
    versionBeforeCas: `${kind}-v1`, versionAfterCas: `${kind}-v2`, artifactManifestHash: sha,
    artifactUrl: `/artifacts/${kind}/current/`, events: { previewUpdated: "1", runCompleted: "2", sequenceValid: true },
    artifactAssertions: {
      content: { expectedTextSha256: sha, documentSha256: sha, matched: true },
      computedStyle: { selector: "body", display: "block", color: "rgb(0, 0, 0)", fontFamily: "sans-serif", passed: true },
    },
    cancelCleanup: { runId: `${kind}-cancel`, runStatus: "cancelled", previewHttpStatusAfterCancel: 404, passed: true },
    dependencyEvidence: { podUid: `${kind}-pod`, pod: `${kind}-pod-name`, podIp: `10.0.0.${kind === "website" ? 1 : 2}`, nodeModulesPresent: true, lockfileSha256: kind === "website" ? "d".repeat(64) : "e".repeat(64), tarballRequestCount: 2, passed: true },
    recoverableToolFailureCount: 0, terminalToolFailureCount: 0,
    sandboxReleasedAt: "2026-07-11T00:00:00Z", artifactHttpStatusAfterRelease: 200,
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

assert.deepEqual(validateReleaseEvidence(fixture), []);
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
const missingRuntimeManifest = structuredClone(fixture);
delete missingRuntimeManifest.images.runtime.manifestDigest;
assert(validateReleaseEvidence(missingRuntimeManifest).some(error => error.includes("manifest digest")));
const leakedPreview = structuredClone(fixture);
leakedPreview.projects[0].cancelCleanup.previewHttpStatusAfterCancel = 200;
assert(validateReleaseEvidence(leakedPreview).some(error => error.includes("cancellation")));
const sequentialFixture = structuredClone(fixture);
sequentialFixture.fixture.concurrent = false;
assert(validateReleaseEvidence(sequentialFixture).some(error => error.includes("concurrent fixture")));
process.stdout.write("release evidence validator tests passed\n");
