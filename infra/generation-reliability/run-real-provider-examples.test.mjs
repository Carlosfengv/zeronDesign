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
  maxCaseAttempts = 1,
  privateKeyFile,
  adminTokenFile,
  manifestFile,
}) {
  let runCounter = 0;
  let briefEventRequests = 0;
  const receivedBriefPrompts = [];
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (request.method === "PUT" && url.pathname.endsWith("/access")) {
      return json(response, 200, { granted: true });
    }
    if (request.method === "POST" && url.pathname === "/runs") {
      collectJson(request).then((body) => {
        if (body.phase === "brief") {
          receivedBriefPrompts.push(body.inputContext?.contentSources?.[0]?.text);
        }
        json(response, 200, {
          runId: `${body.phase}-run-${++runCounter}`,
        });
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
    if (request.method === "POST" && url.pathname.endsWith("/continue")) {
      return json(response, 200, { continued: true });
    }
    if (request.method === "GET" && url.pathname.endsWith("/events")) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      const phase = url.pathname.includes("brief-run") ? "brief" : "build";
      const transientBriefFailure =
        phase === "brief" && briefEventRequests++ < briefFailuresBeforeSuccess;
      const events = [
        ...(includeModelExecution
          ? [{ type: "model.execution", snapshot: { modelResourceId: "deepseek-v4-pro" } }]
          : []),
        {
          type: "model.usage",
          inputTokens: usageInputTokens,
          outputTokens: usageOutputTokens,
          cachedInputTokens: 2,
        },
        { type: "tool.started", toolName: "fixture.tool" },
        {
          type: "run.completed",
          status:
            phase === "build"
              ? buildStatus
              : transientBriefFailure
                ? "blocked"
                : briefStatus,
          summary:
            phase === "build"
              ? buildSummary
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
      url.pathname.endsWith("/release-evidence")
    ) {
      response.writeHead(409, { "content-type": "text/plain" });
      return response.end(
        "release evidence requires an Edit promotion with a base version",
      );
    }
    if (
      request.method === "POST" &&
      url.pathname.endsWith("/release-sandbox")
    ) {
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
    if (expectedClassification !== null) {
      assert.equal(
        summary.cases[0].error?.classification || null,
        expectedClassification,
      );
    }
    assert.equal(summary.cases[0].acceptanceContractSha256.length, 64);
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
    assert.equal(caseEvidence.attempts.length, expectedAttemptCount);
    assert.ok(receivedBriefPrompts.length >= 1);
    assert.ok(
      receivedBriefPrompts.every((prompt) => prompt === "Create a fixture artifact"),
      "runner must send the user's prompt without appending internal acceptance instructions",
    );
    for (const run of caseEvidence.runs) {
      assert.equal(run.eventStream.incremental, true);
      assert.equal(run.eventStream.format, "ndjson");
      assert.equal(run.eventStream.eventCount, includeModelExecution ? 4 : 3);
      assert.equal(run.toolCalls, 1);
      assert.ok(fs.existsSync(path.join(suiteDirectory, run.eventStream.path)));
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
