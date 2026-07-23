#!/usr/bin/env node
import { spawn } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

const appRoot = path.resolve(import.meta.dirname, "..");
const temporary = await mkdtemp(path.join(tmpdir(), "zerondesign-bff-dcp-"));
const runtimeRequests = [];
let web;
let runtime;
let smokeRuntimeProjectId = "project";
const realRuntimeFixtureUrl = process.env.RUNTIME_REAL_FIXTURE_URL?.trim().replace(/\/+$/, "");
const holdOpenForBrowser = process.env.BFF_SMOKE_HOLD_OPEN === "1";
const exerciseRealRuntimeBrowser = process.env.BFF_SMOKE_BROWSER === "1";
const simulateProfileDrift = process.env.BFF_SMOKE_PROFILE_DRIFT === "1";
const simulateProfileConflict = process.env.BFF_SMOKE_PROFILE_CONFLICT === "1";

const HASH_A = "a".repeat(64);
const HASH_B = "b".repeat(64);
const HASH_C = "c".repeat(64);
const HASH_D = "d".repeat(64);
const HASH_E = "e".repeat(64);
const HASH_F = "f".repeat(64);
const TIMESTAMP = "2026-07-14T00:00:00.000Z";
const WORKSPACE_NAMESPACE = "ws-bff-smoke";

try {
  runtime = realRuntimeFixtureUrl
    ? { baseUrl: realRuntimeFixtureUrl, close: async () => {} }
    : await startRuntimeMock(runtimeRequests);
  const webPort = await freePort();
  web = spawn("npm", ["run", "dev", "--", "--hostname", "127.0.0.1", "--port", String(webPort)], {
    cwd: appRoot,
    env: {
      ...process.env,
      NODE_ENV: "development",
      ZERONDESIGN_DEV_USER_ID: "bff-smoke-owner",
      ZERONDESIGN_PLATFORM_ADMIN_IDS: "bff-smoke-owner",
      ZERONDESIGN_PRODUCT_DB_PATH: path.join(temporary, "product.sqlite"),
      RUNTIME_BASE_URL: runtime.baseUrl,
      RUNTIME_INTERNAL_ADMIN_TOKEN:
        process.env.RUNTIME_INTERNAL_ADMIN_TOKEN || "bff-smoke-internal-token",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const baseUrl = `http://127.0.0.1:${webPort}`;
  await waitFor(`${baseUrl}/api/projects`);
  await requestJson(`${baseUrl}/api/admin/workspaces`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      namespace: WORKSPACE_NAMESPACE,
      name: "BFF Smoke",
      ownerPrincipalId: "bff-smoke-owner",
    }),
  });

  const created = await requestJson(`${baseUrl}/api/projects`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      name: "DCP BFF smoke",
      kind: "website",
      workspaceNamespace: WORKSPACE_NAMESPACE,
    }),
  });
  const projectId = created.project.id;
  smokeRuntimeProjectId = projectId;
  const seeded = realRuntimeFixtureUrl
    ? await seedRealRuntime(runtime.baseUrl, projectId, "clean")
    : null;
  const runId = seeded?.runId ?? "run-1";
  if (seeded) {
    recordProductRun(path.join(temporary, "product.sqlite"), runId, projectId);
  } else {
    const started = await requestJson(`${baseUrl}/api/projects/${projectId}/build-runs`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ briefId: "brief-1", modelServiceId: "model-service-1" }),
    });
    assert(started.runId === "run-1", "BFF did not record the mocked Build Run");
    const runtimeStart = runtimeRequests.find(
      (request) => request.method === "POST" && request.path === "/runs",
    );
    assert(
      runtimeStart?.body?.inputContext?.modelResourceId === "model-service-1",
      "BFF did not map modelServiceId to Runtime modelResourceId",
    );
    const edited = await requestJson(`${baseUrl}/api/projects/${projectId}/edit-runs`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ message: "Use another model", modelServiceId: "model-service-2" }),
    });
    assert(edited.runId === "run-edit-1", "BFF did not start the mocked Edit Run");
    const runtimeEdit = runtimeRequests.find(
      (request) => request.method === "POST" && request.path === "/runs"
        && request.body?.phase === "edit",
    );
    assert(
      runtimeEdit?.body?.inputContext?.modelResourceId === "model-service-2",
      "BFF did not map the replacement modelServiceId for Edit",
    );
  }

  const context = await requestJson(
    `${baseUrl}/api/projects/${projectId}/runs/${runId}/design-context`,
  );
  assert(context.manifest.package.contentHash, "DCP manifest was not proxied");
  assert(context.diagnostics.gate === "ready", "DCP diagnostics were not proxied");
  assert(context.diagnostics.fidelity === null, "BFF did not preserve the no-fidelity-report state");
  assert(context.syncTarget?.effectiveProfileHash, "BFF did not derive the bound target");

  const forbidden = await fetch(`${baseUrl}/api/projects/${projectId}/runs/not-owned/design-context`);
  assert(forbidden.status === 404, "BFF exposed a Run that is not owned by the project");

  const planned = await requestJson(`${baseUrl}/api/projects/${projectId}/runs/${runId}/profile-sync`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ idempotencyKey: "bff-plan-key" }),
  });
  assert(planned.operationId, "BFF did not return the sync operation");
  if (!realRuntimeFixtureUrl) {
    const runtimePlan = runtimeRequests.find((request) => request.path.endsWith("design-profile-sync-plan"));
    assert(runtimePlan, "BFF did not call the Runtime sync plan route");
    assert(runtimePlan.body.targetDesignProfileId === "profile-1", "BFF accepted a browser-selected target");
    assert(runtimePlan.body.targetEffectiveProfileHash === HASH_D, "BFF did not derive target effective hash");
    assert(runtimePlan.body.expectedSourceContentHash === HASH_A, "BFF did not derive frozen source hash");
    assert(Object.keys(runtimePlan.body).length === 5, "BFF forwarded unexpected browser plan fields");
  }

  const confirmed = await requestJson(
    `${baseUrl}/api/projects/${projectId}/runs/${runId}/profile-sync/${planned.operationId}/confirm`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ planHash: planned.planHash, conflictDecisions: {}, idempotencyKey: "bff-confirm-key" }),
    },
  );
  assert(confirmed.status === "applied" && confirmed.childRunId, "BFF did not confirm sync");
  const childContext = await requestJson(
    `${baseUrl}/api/projects/${projectId}/runs/${confirmed.childRunId}/design-context`,
  );
  assert(
    childContext.manifest.package.contentHash,
    "BFF did not record Profile Sync child Run ownership",
  );
  let browserEvidence = null;
  if (realRuntimeFixtureUrl) {
    const replay = await requestJson(
      `${baseUrl}/api/projects/${projectId}/runs/${runId}/profile-sync/${planned.operationId}/confirm`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ planHash: planned.planHash, conflictDecisions: {}, idempotencyKey: "bff-confirm-key" }),
      },
    );
    assert(replay.childRunId === confirmed.childRunId, "BFF duplicate confirm created another child Run");
    await assertConfirmMismatchRejected({
      baseUrl,
      projectId,
      runId,
      operationId: planned.operationId,
      planHash: planned.planHash,
    });
    await exerciseConflictThroughBff({ baseUrl, runtimeBaseUrl: runtime.baseUrl, productDbPath: path.join(temporary, "product.sqlite") });
    if (exerciseRealRuntimeBrowser) {
      browserEvidence = await exerciseConflictThroughBrowser({
        baseUrl,
        runtimeBaseUrl: runtime.baseUrl,
        productDbPath: path.join(temporary, "product.sqlite"),
      });
    }
  }

  console.log(JSON.stringify({
    ok: true,
    baseUrl,
    projectId,
    checks: ["ownership", "dcp-diagnostics", "server-derived-sync-plan", "sync-confirm", "sync-child-run-ownership", ...(!realRuntimeFixtureUrl ? ["build-model-selection", "edit-model-switch"] : []), ...(realRuntimeFixtureUrl ? ["runtime-writeback", "conflict-resolution", "confirm-replay", "confirm-mismatch-rejected"] : []), ...(browserEvidence ? ["browser-real-runtime-profile-sync"] : [])],
    ...(browserEvidence ? { browserEvidence } : {}),
  }, null, 2));
  if (holdOpenForBrowser) {
    console.log(`BROWSER_FIXTURE_URL=${baseUrl}`);
    await waitForTerminationSignal();
  }
} finally {
  web?.kill("SIGTERM");
  await runtime?.close();
  await rm(temporary, { recursive: true, force: true });
}

function packageSummary() {
  return {
    version: "design-context@1", contentHash: HASH_A, artifactManifestHash: HASH_B,
    compilerVersion: "design-context-compiler@1", briefHash: HASH_C, expectedAppRoot: "project",
    declaredEnforcementMode: "enforced", effectiveCompatibilityMode: "enforced",
    enforcementPolicy: { source: "persistent", enabled: true, policyRevision: 2, policyUpdatedBy: "smoke-operator" },
    verificationPolicyId: "website-verification@1", warnings: [], surface: "website",
    template: "next-app", designProfileId: "profile-1", designProfileVersion: 2,
    effectiveProfileHash: simulateProfileDrift ? HASH_E : HASH_D,
  };
}

function profile() {
  return {
    id: "profile-1", schemaVersion: "design-profile@2", name: "Smoke Profile", status: "active", version: 2,
    scope: { projectId: "ignored-by-runtime-mock" }, source: { kind: "manual" },
    product: { name: "Smoke", category: "test", audience: ["test"], primaryUseCases: ["test"], productQualities: ["test"] },
    brand: { voice: { tone: ["clear"], sentenceStyle: "technical", vocabulary: { prefer: [], avoid: [] }, writingRules: [] }, messaging: { headlineStyle: "specific", bodyStyle: "concise", ctaStyle: "verb", proofStyle: "evidence", forbiddenClaims: [] } },
    visual: { direction: "clear", principles: [], moodKeywords: [], avoidKeywords: [], composition: {}, imagery: {}, motion: {} },
    tokens: { color: {}, typography: {}, radius: {}, shadow: {}, spacing: {} },
    runtimeTokenMapping: {
      "color.background": "#ffffff", "color.surface": "#f8fafc", "color.surfaceStrong": "#e2e8f0",
      "color.text": "#0f172a", "color.muted": "#475569", "color.primary": "#2563eb",
      "color.primaryContrast": "#ffffff", "color.border": "#cbd5e1", "radius.card": "8px",
      "radius.control": "6px", "font.sans": "Inter, sans-serif", "shadow.soft": "none",
    },
    components: { primitives: {
      button: { intent: "clear", usage: ["action"], avoid: ["overuse"] },
      input: { intent: "entry", usage: ["form"], avoid: ["placeholder only"] },
      card: { intent: "group", usage: ["list"], avoid: ["nesting"] },
      badge: { intent: "status", usage: ["state"], avoid: ["decoration"] },
    } }, content: {}, accessibility: {},
    technical: { allowedTemplates: ["next-app"], preferredTemplates: { website: "next-app", docs: "fumadocs-docs" }, cssStrategy: "runtime-style-contract", dependencyPolicy: {}, filePolicy: { designProfilePath: "/workspace/inputs/design-profile.json", designMarkdownPath: "/workspace/inputs/design.md", styleContractPath: "/workspace/state/style-contract.json" } },
    governance: { conflictBehavior: "ask" }, createdAt: TIMESTAMP, updatedAt: TIMESTAMP,
  };
}

function operation(status = "planned", childRunId = null) {
  return {
    operationId: "sync-1", status, expiresAt: TIMESTAMP, planHash: HASH_E, sourceContentHash: HASH_A,
    targetDesignProfileId: "profile-1", targetDesignProfileVersion: 2, targetEffectiveProfileHash: HASH_D,
    styleContractIdentity: { hash: HASH_F, version: "runtime-style-contract@p3", template: "next-app", appRoot: "project", tokenMappings: { "color.primary": "--primary" } },
    snapshots: { baseHash: HASH_A, currentHash: HASH_B, targetHash: HASH_C },
    items: simulateProfileConflict ? [{
      token: "color.primary",
      base: "#2563eb",
      current: "#0ea5e9",
      target: "#f97316",
      state: "conflict",
      resolution: null,
    }] : [],
    conflictDecisions: {}, childRunId,
  };
}

async function startRuntimeMock(requests) {
  const server = createServer(async (request, response) => {
    const url = new URL(request.url ?? "/", "http://runtime.local");
    const body = await readJson(request);
    requests.push({ method: request.method, path: url.pathname, body });
    const projectId = url.pathname.split("/")[2] || smokeRuntimeProjectId;
    if (request.method === "GET" && url.pathname.endsWith("/events")) {
      const runId = url.pathname.split("/")[2] || "run-1";
      response.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-cache",
        connection: "keep-alive",
      });
      response.end(`data: ${JSON.stringify({
        type: "run.completed",
        runId,
        timestamp: TIMESTAMP,
        status: "completed",
        summary: "Fixture Run completed",
      })}\n\n`);
      return;
    }
    let payload;
    if (request.method === "PUT" && /^\/internal\/projects\/[^/]+\/access$/.test(url.pathname)) {
      payload = {
        projectAccess: {
          projectId,
          ownerPrincipalId: body.ownerPrincipalId,
          workspaceNamespace: body.workspaceNamespace,
          createdAt: TIMESTAMP,
          updatedAt: TIMESTAMP,
        },
      };
    } else if (request.method === "GET" && url.pathname.endsWith("/model-services")) {
      payload = {
        items: [
          {
            id: "model-service-1",
            displayName: "Smoke Model A",
            description: "BFF model selection fixture",
            capabilities: {
              toolCalls: true,
              strictToolSchema: false,
              streaming: true,
              vision: false,
              visionInput: false,
              supportedImageMediaTypes: [],
              maxImageBytes: 0,
              maxImageCount: 0,
            },
            availability: "available",
          },
          {
            id: "model-service-2",
            displayName: "Smoke Model B",
            description: "BFF edit model replacement fixture",
            capabilities: {
              toolCalls: true,
              strictToolSchema: false,
              streaming: true,
              vision: false,
              visionInput: false,
              supportedImageMediaTypes: [],
              maxImageBytes: 0,
              maxImageCount: 0,
            },
            availability: "available",
          },
        ],
      };
    } else if (request.method === "GET" && url.pathname.endsWith("/runtime-state")) {
      payload = {
        projectId,
        currentVersionId: "version-1",
        sandboxBindingId: "sandbox-1",
        sourceSnapshotUri: "snapshot://version-1",
        appRoot: "project",
        templateKey: "next-app",
        modelServiceId: "model-service-1",
        modelServiceDisplayName: "Smoke Model A",
        styleContractPath: null,
        styleContract: null,
        latestBuild: null,
        dependencyState: null,
        preview: null,
      };
    } else if (url.pathname === "/briefs/brief-1") {
      payload = { briefId: "brief-1", projectId: smokeRuntimeProjectId, runId: "brief-run-1", status: "confirmed", runStatus: "completed", brief: { projectType: "website", audience: "test", contentHierarchy: ["hero"], pageStructure: [], visualDirection: "clear", recommendedTemplate: "next-app", assumptions: [], missingInformation: [] } };
    } else if (url.pathname === "/runs" && request.method === "POST") {
      payload = {
        runId: body?.phase === "edit" ? "run-edit-1" : "run-1",
        status: "queued",
      };
    } else if (url.pathname === "/runs/run-edit-1/continue" && request.method === "POST") {
      payload = { runId: "run-edit-1", status: "running" };
    } else if (url.pathname.endsWith("/design-context-manifest")) {
      payload = { runId: "run-1", package: packageSummary(), artifacts: [] };
    } else if (url.pathname.endsWith("/design-context-diagnostics")) {
      payload = { runId: "run-1", package: packageSummary(), requiredReads: [], readFiles: [], missingRequiredReads: [], gate: "ready", materialization: { hash: "materialized", ready: true }, styleContract: { verified: true }, verification: { policyId: "website-verification@1", registryVersion: "runtime-verifier-registry@1", capabilities: {} }, fidelity: null };
    } else if (url.pathname.endsWith("/design-profile") && request.method === "GET") {
      payload = { projectId, designProfile: profile() };
    } else if (url.pathname.includes("/fidelity-report")) {
      payload = { designProfileId: "profile-1", version: 2, schemaVersion: "design-profile@2", surface: "website", template: "next-app", styleContractVersion: "runtime-style-contract@p3", effectiveProfileHash: HASH_D, sourceIntegrity: "verified", sourceHashMatches: true, requiredSignatureRuleIds: [], capsuleIncludedRuleIds: [], capsuleMissingRuleIds: [], unsupportedExtendedTokens: [], warnings: [] };
    } else if (url.pathname.endsWith("/design-profile-sync-plan")) {
      payload = operation();
    } else if (url.pathname.endsWith("/confirm")) {
      payload = operation("applied", "child-edit-1");
    } else {
      response.writeHead(404, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: `unhandled mock route: ${request.method} ${url.pathname}` }));
      return;
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify(payload));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

async function readJson(request) {
  if (request.method === "GET" || request.method === "HEAD") return undefined;
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  const text = Buffer.concat(chunks).toString("utf8");
  return text ? JSON.parse(text) : undefined;
}

async function requestJson(url, init) {
  const response = await fetch(url, init);
  const payload = await response.json();
  if (!response.ok) {
    const issues = Array.isArray(payload.issues) ? `: ${JSON.stringify(payload.issues)}` : "";
    throw new Error(`${response.status} ${payload.error || "request failed"}${issues}`);
  }
  return payload;
}

async function freePort() {
  const server = createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function waitFor(url) {
  const deadline = Date.now() + 60_000;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`BFF did not start: ${lastError?.message || "timeout"}`);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function waitForTerminationSignal() {
  return new Promise((resolve) => {
    process.once("SIGINT", resolve);
    process.once("SIGTERM", resolve);
  });
}

async function seedRealRuntime(runtimeBaseUrl, projectId, scenario) {
  return requestJson(`${runtimeBaseUrl}/__test/profile-sync-seed`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ projectId, scenario }),
  });
}

function recordProductRun(databasePath, runId, projectId) {
  const database = new DatabaseSync(databasePath);
  database.exec("PRAGMA busy_timeout = 5000;");
  database.prepare(`INSERT INTO project_runs (run_id, project_id, phase, created_at)
                    VALUES (?, ?, 'build', ?)`)
    .run(runId, projectId, new Date().toISOString());
  database.close();
}

async function exerciseConflictThroughBff({ baseUrl, runtimeBaseUrl, productDbPath }) {
  const created = await requestJson(`${baseUrl}/api/projects`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      name: "DCP BFF conflict",
      kind: "website",
      workspaceNamespace: WORKSPACE_NAMESPACE,
    }),
  });
  const projectId = created.project.id;
  const seeded = await seedRealRuntime(runtimeBaseUrl, projectId, "conflict");
  recordProductRun(productDbPath, seeded.runId, projectId);
  const planned = await requestJson(`${baseUrl}/api/projects/${projectId}/runs/${seeded.runId}/profile-sync`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ idempotencyKey: "bff-conflict-plan-key" }),
  });
  const conflict = planned.items.find((item) => item.state === "conflict");
  assert(conflict?.token, "real Runtime fixture did not expose a conflict token");
  const missingDecision = await fetch(
    `${baseUrl}/api/projects/${projectId}/runs/${seeded.runId}/profile-sync/${planned.operationId}/confirm`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ planHash: planned.planHash, conflictDecisions: {}, idempotencyKey: "bff-conflict-confirm-key" }),
    },
  );
  assert(missingDecision.status === 409, "BFF allowed confirm without a required conflict decision");
  const confirmed = await requestJson(
    `${baseUrl}/api/projects/${projectId}/runs/${seeded.runId}/profile-sync/${planned.operationId}/confirm`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        planHash: planned.planHash,
        conflictDecisions: { [conflict.token]: "apply_target" },
        idempotencyKey: "bff-conflict-confirm-key",
      }),
    },
  );
  assert(confirmed.status === "applied" && confirmed.childRunId, "BFF did not apply an explicit conflict decision");
  const replay = await requestJson(
    `${baseUrl}/api/projects/${projectId}/runs/${seeded.runId}/profile-sync/${planned.operationId}/confirm`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        planHash: planned.planHash,
        conflictDecisions: { [conflict.token]: "apply_target" },
        idempotencyKey: "bff-conflict-confirm-key",
      }),
    },
  );
  assert(replay.childRunId === confirmed.childRunId, "BFF conflict confirm replay created another child Run");
}

async function exerciseConflictThroughBrowser({ baseUrl, runtimeBaseUrl, productDbPath }) {
  const created = await requestJson(`${baseUrl}/api/projects`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      name: "DCP browser + real Runtime",
      kind: "website",
      workspaceNamespace: WORKSPACE_NAMESPACE,
    }),
  });
  const projectId = created.project.id;
  const seeded = await seedRealRuntime(runtimeBaseUrl, projectId, "conflict");
  recordProductRun(productDbPath, seeded.runId, projectId);
  const evidence = await runBrowserSmoke({ baseUrl, projectId });
  assert(evidence.ok === true, `browser fixture failed: ${evidence.error || "unknown error"}`);
  assert(evidence.projectId === projectId, "browser fixture returned evidence for a different project");
  assert(evidence.checks?.includes("fidelity-rule-detail"), "browser fixture did not verify fidelity rule detail");
  assert(evidence.checks?.includes("child-run-dcp-owned-and-aligned"), "browser fixture did not verify child Run DCP ownership and alignment");
  assert(/^[a-f0-9]{64}$/.test(evidence.screenshotSha256 || ""), "browser fixture did not return a screenshot digest");
  return evidence;
}

function runBrowserSmoke(input) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ["scripts/run-design-context-browser-smoke.mjs"], {
      cwd: appRoot,
      env: process.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    const timeout = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("browser fixture timed out"));
    }, 75_000);
    child.stdout.on("data", (chunk) => { stdout += String(chunk); });
    child.stderr.on("data", (chunk) => { stderr = `${stderr}${String(chunk)}`.slice(-4096); });
    child.once("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    child.once("exit", (code, signal) => {
      clearTimeout(timeout);
      let parsed;
      try {
        parsed = JSON.parse(stdout);
      } catch {
        reject(new Error(`browser fixture returned invalid JSON (code=${code ?? "unknown"}, signal=${signal ?? "none"}): ${stdout.slice(-2000)} ${stderr}`));
        return;
      }
      if (code !== 0) {
        reject(new Error(`browser fixture failed (code=${code ?? "unknown"}, signal=${signal ?? "none"}): ${parsed.error || stderr || "unknown error"}`));
        return;
      }
      resolve(parsed);
    });
    child.stdin.end(JSON.stringify(input));
  });
}

async function assertConfirmMismatchRejected({ baseUrl, projectId, runId, operationId, planHash }) {
  const malformed = await fetch(
    `${baseUrl}/api/projects/${projectId}/runs/${runId}/profile-sync/${operationId}/confirm`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ planHash: "not-a-hash", conflictDecisions: {}, idempotencyKey: "bff-confirm-key" }),
    },
  );
  assert(malformed.status === 400, `BFF must classify malformed confirm input as 400, got ${malformed.status}`);
  const differentPlanHash = `${planHash.startsWith("0") ? "1" : "0"}${planHash.slice(1)}`;
  for (const [body, expectedErrorCode] of [
    [{ planHash: differentPlanHash, conflictDecisions: {}, idempotencyKey: "bff-confirm-key" }, "profile_sync_plan_mismatch"],
    [{ planHash, conflictDecisions: {}, idempotencyKey: "bff-confirm-key-mismatch" }, "idempotency_key_reused"],
  ]) {
    const response = await fetch(
      `${baseUrl}/api/projects/${projectId}/runs/${runId}/profile-sync/${operationId}/confirm`,
      {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      },
    );
    const payload = await response.json();
    assert(
      response.status === 409 && payload.errorCode === expectedErrorCode,
      `BFF accepted a mismatched confirm identity: ${response.status} ${payload.errorCode || "no_code"} ${payload.error || "request failed"}`,
    );
  }
}
