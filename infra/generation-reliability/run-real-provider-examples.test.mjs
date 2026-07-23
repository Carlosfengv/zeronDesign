#!/usr/bin/env node

import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const runner = path.join(scriptDirectory, "run-real-provider-examples.mjs");
const temporaryDirectory = fs.mkdtempSync(
  path.join(os.tmpdir(), "real-provider-evidence-v2-"),
);

try {
  const { privateKey } = crypto.generateKeyPairSync("ed25519");
  const privateKeyFile = path.join(temporaryDirectory, "principal.pem");
  const adminTokenFile = path.join(temporaryDirectory, "admin-token");
  const manifestFile = path.join(temporaryDirectory, "cases.json");
  fs.writeFileSync(
    privateKeyFile,
    privateKey.export({ type: "pkcs8", format: "pem" }),
    { mode: 0o600 },
  );
  fs.writeFileSync(adminTokenFile, "fixture-admin-token\n", { mode: 0o600 });
  fs.writeFileSync(
    manifestFile,
    `${JSON.stringify(fixtureManifest(), null, 2)}\n`,
  );

  await runScenario({
    name: "accepted",
    artifactText: "Fixture Expected Text",
    expectedExitCode: 0,
    expectedStatus: "accepted",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "keep-sandbox-accepted",
    artifactText: "Fixture Expected Text",
    keepSandbox: true,
    expectedExitCode: 0,
    expectedStatus: "accepted",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "sandbox-release-transient-retry",
    artifactText: "Fixture Expected Text",
    releaseFailuresBeforeSuccess: 1,
    expectedReleaseRequestCount: 3,
    expectedExitCode: 0,
    expectedStatus: "accepted",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "sandbox-release-failure",
    artifactText: "Fixture Expected Text",
    releaseFailuresBeforeSuccess: 3,
    expectedReleaseRequestCount: 3,
    expectedExitCode: 1,
    expectedStatus: "failed",
    expectedClassification: "sandbox_release_failed",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "static-draft-preview-accepted",
    artifactText: "Fixture Expected Text",
    draftPreviewAcceptance: true,
    expectedExitCode: 0,
    expectedStatus: "accepted",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "dev-draft-preview-accepted",
    artifactText: "Fixture Expected Text",
    draftPreviewAcceptance: true,
    devDraftPreviewAcceptance: true,
    expectedExitCode: 0,
    expectedStatus: "accepted",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "rejected",
    artifactText: "Wrong generated content",
    expectedExitCode: 1,
    expectedStatus: "rejected",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "no-progress",
    artifactText: "Fixture Expected Text",
    buildStatus: "partial",
    buildSummary:
      "Run stopped for no_progress: consecutive_turns=5, limit=5, fingerprint=fixture",
    expectedExitCode: 1,
    expectedStatus: "failed",
    expectedClassification: "no_progress",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "no-progress-retry",
    artifactText: "Fixture Expected Text",
    buildFailuresBeforeSuccess: 1,
    maxCaseAttempts: 2,
    expectedExitCode: 0,
    expectedStatus: "accepted",
    expectedRunCount: 4,
    expectedAttemptCount: 2,
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "acceptance-repair-exhausted",
    artifactText: "Fixture Expected Text",
    buildStatus: "failed",
    buildSummary:
      "preview.publish exhausted the frozen Brief acceptance repair budget",
    expectedExitCode: 1,
    expectedStatus: "rejected",
    expectedClassification: "acceptance_repair_exhausted",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "model-turn-budget",
    artifactText: "Fixture Expected Text",
    buildStatus: "failed",
    buildSummary: "Reached model-turn budget: limit=60",
    expectedExitCode: 1,
    expectedStatus: "failed",
    expectedClassification: "model_turn_budget_exhausted",
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "brief-blocked",
    artifactText: "Fixture Expected Text",
    briefId: null,
    briefStatus: "blocked",
    briefSummary:
      "model gateway request failed: status=503 Service Unavailable code=provider_unavailable retryable=true",
    expectedExitCode: 1,
    expectedStatus: "failed",
    expectedClassification: "run_incomplete",
    expectedRunCount: 1,
    expectedMaxDurationMs: 5_000,
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "brief-transient-retry",
    artifactText: "Fixture Expected Text",
    briefFailuresBeforeSuccess: 1,
    maxCaseAttempts: 2,
    expectedExitCode: 0,
    expectedStatus: "accepted",
    expectedRunCount: 3,
    expectedAttemptCount: 2,
    expectedMaxDurationMs: 5_000,
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "budget-reservation",
    artifactText: "Fixture Expected Text",
    usageInputTokens: 600000,
    usageOutputTokens: 79950,
    totalTokenCeiling: 680000,
    expectedExitCode: 1,
    expectedStatus: "failed",
    expectedClassification: "suite_budget_reservation_exhausted",
    expectedRunCount: 1,
    expectedRemainingTokens: 50,
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });
  await runScenario({
    name: "unverified-provider",
    artifactText: "Fixture Expected Text",
    includeModelExecution: false,
    expectedExitCode: 1,
    expectedStatus: "failed",
    expectedCaseStatus: "accepted",
    expectedProviderVerified: false,
    privateKeyFile,
    adminTokenFile,
    manifestFile,
  });

  process.stdout.write("real-provider evidence v2 streaming tests passed\n");
} finally {
  fs.rmSync(temporaryDirectory, { recursive: true, force: true });
}

async function runScenario({
  name,
  artifactText,
  expectedExitCode,
  expectedStatus,
  expectedCaseStatus = expectedStatus,
  expectedProviderVerified = true,
  expectedClassification = null,
  buildStatus = "completed",
  buildSummary = "build complete",
  briefId = "brief-1",
  briefStatus = "completed",
  briefSummary = "brief complete",
  usageInputTokens = 12,
  usageOutputTokens = 3,
  totalTokenCeiling = null,
  expectedRunCount = 2,
  expectedAttemptCount = 1,
  expectedRemainingTokens = null,
  expectedMaxDurationMs = null,
  includeModelExecution = true,
  briefFailuresBeforeSuccess = 0,
  buildFailuresBeforeSuccess = 0,
  maxCaseAttempts = 1,
  draftPreviewAcceptance = false,
  devDraftPreviewAcceptance = false,
  keepSandbox = false,
  releaseFailuresBeforeSuccess = 0,
  expectedReleaseRequestCount = null,
  privateKeyFile,
  adminTokenFile,
  manifestFile,
}) {
  let runCounter = 0;
  let briefEventRequests = 0;
  let buildEventRequests = 0;
  let releaseSandboxRequests = 0;
  const runs = new Map();
  const receivedBriefPrompts = [];
  const receivedWorkspaceNamespaces = [];
  const receivedContentPlans = [];
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (request.method === "PUT" && url.pathname.endsWith("/access")) {
      collectJson(request).then((body) => {
        receivedWorkspaceNamespaces.push(body.workspaceNamespace);
        json(response, 200, { granted: true });
      });
      return;
    }
    if (request.method === "POST" && url.pathname.endsWith("/content-plan-changes")) {
      collectJson(request).then((body) => {
        receivedContentPlans.push({
          kind: "change",
          body,
          internal: request.headers["x-anydesign-internal"] === "true",
          hasAdminToken: typeof request.headers["x-runtime-admin-token"] === "string",
          hasPrincipalToken: typeof request.headers.authorization === "string",
        });
        json(response, 200, { recorded: true });
      });
      return;
    }
    if (request.method === "POST" && url.pathname.endsWith("/content-plan-approvals")) {
      collectJson(request).then((body) => {
        receivedContentPlans.push({
          kind: "approval",
          body,
          internal: request.headers["x-anydesign-internal"] === "true",
          hasAdminToken: typeof request.headers["x-runtime-admin-token"] === "string",
          hasPrincipalToken: typeof request.headers.authorization === "string",
        });
        json(response, 200, { state: "verified" });
      });
      return;
    }
    if (request.method === "POST" && url.pathname === "/runs") {
      collectJson(request).then((body) => {
        if (body.phase === "brief") {
          receivedBriefPrompts.push(body.inputContext?.contentSources?.[0]?.text);
        }
        const runId = `${body.phase}-run-${++runCounter}`;
        runs.set(runId, { projectId: body.projectId, phase: body.phase });
        json(response, 200, { runId });
      });
      return;
    }
    if (request.method === "GET" && url.pathname.endsWith("/conversation")) {
      const visibleBriefId =
        briefFailuresBeforeSuccess > 0 &&
        briefEventRequests <= briefFailuresBeforeSuccess
          ? null
          : briefId;
      return json(response, 200, {
        items: visibleBriefId ? [{ metadata: { briefId: visibleBriefId } }] : [],
      });
    }
    if (
      request.method === "GET" &&
      devDraftPreviewAcceptance &&
      url.pathname.endsWith("/draft-preview")
    ) {
      return json(response, 200, {
        sessionId: "draft-session-fixture",
        sessionEpoch: 1,
        templateId: "next-app",
        status: "ready",
        workspaceRevision: 1,
        lastReadyRevision: 1,
        durableRevision: 1,
        durableSnapshotId: "draft-snapshot-fixture",
        proxyUrl: `http://${request.headers.host}/previews/fixture-dev-lease/`,
      });
    }
    if (request.method === "POST" && url.pathname.endsWith("/continue")) {
      return json(response, 200, { continued: true });
    }
    if (
      request.method === "GET" &&
      url.pathname.endsWith("/design-context-manifest")
    ) {
      const runId = url.pathname.split("/").at(-2);
      const run = runs.get(runId);
      if (!run) return json(response, 404, { error: "run not found" });
      return json(response, 200, {
        runId,
        package: {
          designProfileId: "fixture-design-profile",
          designProfileVersion: 1,
          effectiveProfileHash: "9".repeat(64),
        },
        artifacts: [],
      });
    }
    if (
      request.method === "GET" &&
      url.pathname.endsWith("/efficiency-metrics")
    ) {
      const runId = url.pathname.split("/").at(-2);
      const run = runs.get(runId);
      if (!run) return json(response, 404, { error: "run not found" });
      const terminalStatus = run.terminalStatus ||
        (run.phase === "build" ? buildStatus : briefStatus);
      return json(response, 200, {
        schemaVersion: "run-efficiency-metrics@1",
        calculatorVersion: "run-efficiency-calculator@1",
        runId,
        projectId: run.projectId,
        phase: run.phase,
        model: "deepseek-v4-pro",
        template: run.phase === "build" ? "next-app@2" : null,
        status: terminalStatus,
        inputTokens: usageInputTokens,
        outputTokens: usageOutputTokens,
        cachedInputTokens: 2,
        prebuildFsReadCount: 0,
        prebuildFsListCount: 0,
        prebuildFsSearchCount: 0,
        contextReadDeliveries: 0,
        sourceReadDeliveries: 0,
        diagnosticReadDeliveries: 0,
        verificationReadDeliveries: 0,
        fullReadDeliveries: 0,
        duplicateFullReadDeliveries: 0,
        duplicateFullReadRateBasisPoints: 0,
        duplicateReadEstimatedTokens: 0,
        outOfScopeMutationCount: 0,
        firstBuildSucceeded: run.phase === "build" && terminalStatus === "completed",
        requiredFidelityPassed: run.phase === "build" ? terminalStatus === "completed" : null,
      });
    }
    if (
      request.method === "GET" &&
      url.pathname.endsWith("/prompt-efficiency")
    ) {
      const runId = url.pathname.split("/").at(-2);
      const run = runs.get(runId);
      if (!run) return json(response, 404, { error: "run not found" });
      return json(response, 200, {
        schemaVersion: "run-prompt-efficiency@1",
        runId,
        grossInputTokens: usageInputTokens,
        cachedInputTokens: 2,
        uncachedInputTokens: Math.max(0, usageInputTokens - 2),
        outputTokens: usageOutputTokens,
        turnCount: 1,
        maxTurnInputTokens: usageInputTokens,
        averageTurnInputTokens: usageInputTokens,
        cacheHitRateBasisPoints: 200,
        generationContextEstimatedTokens: 0,
        generationContextRepeatedEstimatedTokens: 0,
        promptCompactionCount: 0,
        promptTokensRemovedByCompaction: 0,
        largeToolArgumentTokensRetainedPeak: 0,
        retryAmplificationBasisPoints: null,
        estimated: false,
      });
    }
    if (
      request.method === "GET" &&
      url.pathname.endsWith("/budget-profile")
    ) {
      const runId = url.pathname.split("/").at(-2);
      const run = runs.get(runId);
      if (!run) return json(response, 404, { error: "run not found" });
      return json(response, 200, fixtureBudgetProfile(run.phase));
    }
    if (
      request.method === "GET" &&
      url.pathname.endsWith("/generation-context-status")
    ) {
      const runId = url.pathname.split("/").at(-2);
      const run = runs.get(runId);
      if (!run) return json(response, 404, { error: "run not found" });
      const budgetProfile = fixtureBudgetProfile(run.phase);
      return json(response, 200, {
        schemaVersion: "generation-context-status@1",
        runId,
        runContractVersion: "generation-context@1",
        status: "compiled",
        runtimeMode: "enabled",
        compilerVersion: "generation-context-compiler@1",
        contextContentHash: "c".repeat(64),
        runContextBindingHash: "d".repeat(64),
        runtimeAttestationHash: "e".repeat(64),
        visualBindingSetHash: null,
        visualDeliveryState: "not_applicable",
        executionProfile: "greenfield_static",
        budgetProfileId: budgetProfile.profileId,
        budgetProfileHash: budgetProfile.profileHash,
        budgetProfileRolloutMode: "shadow",
        workflowState: "completed",
        contextWindowEpoch: 0,
        contextInjectedTurn: 1,
        operationId: `operation-${runId}`,
        operationAttempt: 1,
        predecessorRunId: null,
        successorRunId: null,
        continuationSnapshotId: null,
        contentPlan: null,
        approvalId: null,
        approvalState: null,
        designSourceKind: "template_default",
      });
    }
    if (request.method === "GET" && url.pathname.endsWith("/events")) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      const phase = url.pathname.includes("brief-run") ? "brief" : "build";
      const transientBriefFailure =
        phase === "brief" && briefEventRequests++ < briefFailuresBeforeSuccess;
      const transientBuildFailure =
        phase === "build" && buildEventRequests++ < buildFailuresBeforeSuccess;
      const runId = url.pathname.split("/").at(-2);
      const run = runs.get(runId);
      if (run) {
        run.terminalStatus = transientBuildFailure
          ? "partial"
          : phase === "build"
            ? buildStatus
            : transientBriefFailure
              ? "blocked"
              : briefStatus;
      }
      const events = [
        ...(includeModelExecution
          ? [{
              type: "model.execution",
              snapshot: {
                modelResourceId: "deepseek-v4-pro",
                providerRequestId: "provider-request-id-must-not-persist",
              },
            }]
          : []),
        {
          type: "prompt.composition",
          turn: 1,
          staticPrefixHash: "a".repeat(64),
          toolSetHashVersion: "tool-definition-set@1",
          toolSetHash: "b".repeat(64),
          estimatedInputTokens: usageInputTokens,
        },
        {
          type: "model.usage",
          inputTokens: usageInputTokens,
          outputTokens: usageOutputTokens,
          cachedInputTokens: 2,
        },
        { type: "tool.started", runId, toolName: "fixture.tool" },
        ...(phase === "build"
          ? [{
              type: "tool.completed",
              tool: "project.build",
              metadata: {
                postToolUseSuccess: {
                  effect: "build_state_updated",
                  buildId: "fixture-build",
                  sourceSnapshotUri: "runtime://source-snapshots/fixture/build",
                  sourceFingerprint: "c".repeat(64),
                  candidateManifestHash: "d".repeat(64),
                  artifactRouteManifestPath: ".anydesign-artifact-routes.json",
                  artifactRouteManifestHash: "e".repeat(64),
                },
              },
            }]
          : []),
        ...(phase === "build" && draftPreviewAcceptance
          ? [
              {
                type: "tool.completed",
                tool: devDraftPreviewAcceptance ? "preview.dev_start" : "preview.start",
                metadata: {
                  postToolUseSuccess: {
                    url: `http://${request.headers.host}/previews/${devDraftPreviewAcceptance ? "fixture-dev-lease" : "fixture-lease"}/`,
                  },
                },
              },
              ...(devDraftPreviewAcceptance ? [{
                type: "tool.completed",
                tool: "preview.dev_status",
                metadata: { postToolUseSuccess: { status: "ready", revision: 1 } },
              }] : []),
              {
                type: "tool.completed",
                tool: "draft.snapshot_create",
                metadata: {
                  postToolUseSuccess: {
                    previewUrl: `http://${request.headers.host}/previews/${devDraftPreviewAcceptance ? "fixture-dev-lease" : "fixture-lease"}/`,
                    snapshotId: "draft-snapshot-fixture",
                  },
                },
              },
            ]
          : []),
        {
          type: "run.completed",
          status:
            phase === "build"
              ? transientBuildFailure
                ? "partial"
                : buildStatus
              : transientBriefFailure
                ? "blocked"
                : briefStatus,
          summary:
            phase === "build"
              ? transientBuildFailure
                ? "Run stopped for no_progress: consecutive_turns=5, limit=5, fingerprint=fixture-retry"
                : buildSummary
              : transientBriefFailure
                ? "model gateway request failed: status=503 Service Unavailable code=provider_unavailable retryable=true"
                : briefSummary,
        },
      ];
      for (const event of events) response.write(`data: ${JSON.stringify(event)}\n\n`);
      return response.end();
    }
    if (request.method === "GET" && url.pathname.includes("/artifacts/")) {
      response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return response.end(`<main>${artifactText}</main>`);
    }
    if (
      request.method === "GET" &&
      (url.pathname.startsWith("/previews/fixture-lease/") ||
        url.pathname.startsWith("/previews/fixture-dev-lease/"))
    ) {
      response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      return response.end(`<main>${artifactText}</main>`);
    }
    if (
      request.method === "GET" &&
      url.pathname.endsWith("/release-evidence")
    ) {
      response.writeHead(draftPreviewAcceptance ? 404 : 409, {
        "content-type": "text/plain",
      });
      return response.end(
        draftPreviewAcceptance
          ? "current version not found: fixture-project"
          : "release evidence requires an Edit promotion with a base version",
      );
    }
    if (
      request.method === "POST" &&
      url.pathname.endsWith("/release-sandbox")
    ) {
      releaseSandboxRequests += 1;
      if (releaseSandboxRequests <= releaseFailuresBeforeSuccess) {
        return json(response, 503, { released: false });
      }
      return json(response, 200, { released: true });
    }
    response.writeHead(404);
    response.end("not found");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  const evidenceRoot = path.join(temporaryDirectory, name);

  try {
    const startedAt = Date.now();
    const execution = await spawnAndCollect(
      process.execPath,
      [runner, manifestFile, baseUrl, privateKeyFile, adminTokenFile, evidenceRoot],
      {
        ...process.env,
        GENERATION_REAL_CASE_LIMIT: "1",
        GENERATION_REAL_RUN_TIMEOUT_MS: "60000",
        GENERATION_REAL_RUN_IDLE_TIMEOUT_MS: "30000",
        GENERATION_REAL_MAX_CASE_ATTEMPTS: String(maxCaseAttempts),
        GENERATION_REAL_CASE_RETRY_COOLDOWN_MS: "0",
        GENERATION_REAL_SANDBOX_RELEASE_RETRY_COOLDOWN_MS: "0",
        GENERATION_REAL_WORKSPACE_NAMESPACE: "ws-real-provider-test",
        ...(draftPreviewAcceptance
          ? { GENERATION_REAL_DRAFT_PREVIEW_ACCEPTANCE: "1" }
          : {}),
        ...(keepSandbox ? { GENERATION_REAL_KEEP_SANDBOX: "true" } : {}),
        GENERATION_SOURCE_COMMIT: "fixture-commit",
        GENERATION_SOURCE_DIRTY: "false",
        GENERATION_PROVIDER_CONFIG_DIGEST: "a".repeat(64),
        GENERATION_PROVIDER_CONFIG_REVISION: "2",
        ...(totalTokenCeiling === null
          ? {}
          : { GENERATION_REAL_TOTAL_TOKEN_CEILING: String(totalTokenCeiling) }),
      },
    );
    if (expectedMaxDurationMs !== null) {
      assert.ok(
        Date.now() - startedAt < expectedMaxDurationMs,
        `${name} exceeded ${expectedMaxDurationMs}ms terminal convergence bound`,
      );
    }
    assert.equal(
      execution.code,
      expectedExitCode,
      execution.stderr || execution.stdout,
    );
    const directories = fs
      .readdirSync(evidenceRoot, { withFileTypes: true })
      .filter((entry) => entry.isDirectory());
    assert.equal(directories.length, 1);
    assert.match(directories[0].name, new RegExp(`-${expectedStatus}$`));
    const suiteDirectory = path.join(evidenceRoot, directories[0].name);
    const summary = JSON.parse(
      fs.readFileSync(
        path.join(suiteDirectory, "real-provider-examples-summary.json"),
        "utf8",
      ),
    );
    assert.equal(summary.schemaVersion, "generation-real-provider-suite-evidence@2");
    assert.equal(summary.status, expectedStatus);
    assert.equal(summary.provenance.providerResourceRevision, 2);
    assert.equal(summary.cases[0].status, expectedCaseStatus);
    assert.equal(summary.provider.realProviderVerified, expectedProviderVerified);
    assert.equal(summary.approval.contentPlanApprovalMode, "automated_fixture_exact_identity");
    assert.equal(summary.approval.verifiedContentPlanCount, 1);
    if (expectedClassification !== null) {
      assert.equal(
        summary.cases[0].error?.classification || null,
        expectedClassification,
      );
    }
    assert.equal(summary.cases[0].acceptanceContractSha256.length, 64);
    assert.equal(summary.cases[0].contentPlan.state, "verified");
    assert.equal(summary.cases[0].contentPlan.intentSha256.length, 64);
    if (expectedRemainingTokens !== null) {
      assert.equal(summary.budget.configuredTotalTokens, totalTokenCeiling);
      assert.equal(summary.budget.remainingTokens, expectedRemainingTokens);
      assert.equal(summary.budget.exceeded, false);
    }
    const caseEvidence = JSON.parse(
      fs.readFileSync(
        path.join(suiteDirectory, "real-provider-case-case-1.json"),
        "utf8",
      ),
    );
    assert.equal(caseEvidence.schemaVersion, "generation-real-provider-case-evidence@2");
    assert.equal(caseEvidence.runs.length, expectedRunCount);
    if (expectedStatus === "accepted") {
      const primaryRuns = caseEvidence.runs.filter((run) =>
        (run.phase === "brief" || run.phase === "build") && run.status === "completed"
      );
      assert.ok(primaryRuns.length >= 2);
      assert.ok(primaryRuns.every((run) =>
        run.promptEfficiency?.schemaVersion === "run-prompt-efficiency@1"
      ));
      assert.ok(primaryRuns.every((run) =>
        run.promptCompositions?.[0]?.staticPrefixHash === "a".repeat(64)
        && run.promptCompositions?.[0]?.toolSetHashVersion === "tool-definition-set@1"
      ));
      assert.ok(primaryRuns.every((run) =>
        run.budgetProfile?.schemaVersion === "run-budget-profile@1"
        && run.budgetProfile.phase === run.phase
        && run.budgetProfile.profileHash.length === 64
      ));
      assert.equal(
        primaryRuns.find((run) => run.phase === "build")?.generationContextStatus?.contextContentHash,
        "c".repeat(64),
      );
      assert.equal(
        primaryRuns.find((run) => run.phase === "build")?.buildEvidence?.artifactRouteManifestHash,
        "e".repeat(64),
      );
    }
    assert.equal(caseEvidence.attempts.length, expectedAttemptCount);
    assert.equal(caseEvidence.sandboxRelease.required, !keepSandbox);
    assert.equal(
      caseEvidence.sandboxRelease.released,
      keepSandbox ? false : expectedClassification !== "sandbox_release_failed",
    );
    if (draftPreviewAcceptance) {
      if (devDraftPreviewAcceptance) {
        assert.equal(caseEvidence.draftPreview.durableSnapshotId, "draft-snapshot-fixture");
        assert.equal(caseEvidence.draftPreview.hmr, true);
      } else {
        assert.equal(caseEvidence.draftPreview.snapshotId, "draft-snapshot-fixture");
        assert.equal(caseEvidence.draftPreview.hmr, false);
      }
      assert.equal(caseEvidence.draftPreview.expectedTextFound, true);
      assert.equal(
        caseEvidence.releaseEvidence.reason,
        "draft-only-suite-does-not-create-current-version",
      );
    }
    assert.ok(receivedBriefPrompts.length >= 1);
    assert.ok(receivedWorkspaceNamespaces.length >= 1);
    assert.equal(receivedContentPlans.length, expectedAttemptCount * 2);
    assert.ok(receivedContentPlans.every(({ body }) => body.contentHash.length === 64));
    assert.ok(receivedContentPlans.every(({ internal, hasAdminToken }) => internal && hasAdminToken));
    assert.ok(receivedContentPlans.every(({ hasPrincipalToken }) => !hasPrincipalToken));
    assert.equal(
      releaseSandboxRequests,
      expectedReleaseRequestCount ?? (keepSandbox ? 0 : expectedAttemptCount * 2),
      keepSandbox
        ? "keep-sandbox mode must defer release to the Runtime Restart probe"
        : "the ordinary runner must release every attempted Sandbox",
    );
    assert.ok(
      receivedWorkspaceNamespaces.every(
        (namespace) => namespace === "ws-real-provider-test",
      ),
      "runner must bind every generated project to the configured Workspace namespace",
    );
    assert.ok(
      receivedBriefPrompts.every((prompt) => prompt === "Create a fixture artifact"),
      "runner must send the user's prompt without appending internal acceptance instructions",
    );
    for (const run of caseEvidence.runs) {
      assert.equal(run.eventStream.incremental, true);
      assert.equal(run.eventStream.format, "ndjson");
      assert.equal(
        run.eventStream.eventCount,
        (includeModelExecution ? 5 : 4) +
          (run.phase === "build" ? 1 : 0) +
          (draftPreviewAcceptance && run.phase === "build"
            ? devDraftPreviewAcceptance ? 3 : 2
            : 0),
      );
      assert.equal(run.toolCalls, 1);
      if (run.status === "completed") {
        const owningAttempt = caseEvidence.attempts.find((attempt) =>
          attempt.runIds.includes(run.runId),
        );
        assert.ok(owningAttempt, `missing owning attempt for ${run.runId}`);
        assert.equal(run.efficiency.runId, run.runId);
        assert.equal(run.efficiency.projectId, owningAttempt.projectId);
      }
      assert.ok(fs.existsSync(path.join(suiteDirectory, run.eventStream.path)));
      for (const execution of run.modelExecutions) {
        assert.equal(execution.providerRequestId, undefined);
        assert.equal(execution.providerRequestIdPresent, true);
      }
      const eventText = fs.readFileSync(
        path.join(suiteDirectory, run.eventStream.path),
        "utf8",
      );
      assert.ok(!eventText.includes("provider-request-id-must-not-persist"));
      assert.ok(!eventText.includes('"summary":'));
      assert.ok(eventText.includes('"summarySha256":'));
      if (includeModelExecution) {
        assert.ok(eventText.includes('"providerRequestIdPresent":true'));
      }
    }
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
}

function fixtureManifest() {
  return {
    schemaVersion: "generation-real-provider-suite@1",
    provider: {
      gatewayMode: "internal_gateway",
      modelResourceId: "deepseek-v4-pro",
    },
    approval: { required: false },
    budget: {
      totalTokens: 5000000,
      maxRunsPerCase: 2,
      perRun: {
        maxTurns: 60,
        maxToolCalls: 180,
        maxInputTokens: 600000,
        maxOutputTokens: 80000,
      },
    },
    cases: Array.from({ length: 5 }, (_, index) => ({
      id: `case-${index + 1}`,
      kind: index < 3 ? "website" : "docs",
      locale: "zh-CN",
      title: `Fixture ${index + 1}`,
      expectedRoute: "/",
      expectedText: "Fixture Expected Text",
      prompt: "Create a fixture artifact",
    })),
  };
}

function fixtureBudgetProfile(phase) {
  const enforcedLimits = {
    maxTurns: 60,
    maxToolCalls: 180,
    maxInputTokens: 600000,
    maxGrossInputTokens: 600000,
    maxUncachedInputTokens: 600000,
    maxPromptTokensPerTurn: 600000,
    maxOutputTokens: 80000,
  };
  const profile = {
    schemaVersion: "run-budget-profile@1",
    profileId: `phase-default-${phase}`,
    phase,
    rolloutMode: "shadow",
    tokenBudgetMode: "legacy",
    operationBudgetMode: "shadow",
    enforcedLimits,
    phaseTargetLimits: { ...enforcedLimits },
    operationLimits: {
      maxGrossInputTokens: 5000000,
      maxUncachedInputTokens: 5000000,
      maxOutputTokens: 500000,
      maxTurns: 600,
      maxToolCalls: 1800,
    },
  };
  profile.profileHash = crypto
    .createHash("sha256")
    .update(canonicalJson(profile))
    .digest("hex");
  return profile;
}

function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) =>
      `${JSON.stringify(key)}:${canonicalJson(value[key])}`
    ).join(",")}}`;
  }
  return JSON.stringify(value);
}

function json(response, status, value) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

async function collectJson(request) {
  let value = "";
  for await (const chunk of request) value += chunk;
  return JSON.parse(value);
}

function spawnAndCollect(command, args, env) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { env });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, stdout, stderr }));
  });
}
