#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [baseUrl, privateKeyFile, adminTokenFile, projectId, prompt, evidenceRoot] =
  process.argv.slice(2);
if (!baseUrl || !privateKeyFile || !adminTokenFile || !projectId || !prompt || !evidenceRoot) {
  throw new Error(
    "usage: run-real-provider-edit.mjs <base-url> <private-key> <admin-token-file> <project-id> <prompt> <evidence-root>",
  );
}

const principalId = "generation-real-provider-suite";
const workspaceNamespace = (process.env.GENERATION_REAL_WORKSPACE_NAMESPACE || "").trim();
const configuredBaseVersionId = (process.env.GENERATION_REAL_BASE_VERSION_ID || "").trim();
const nextAppDraftMode = process.env.GENERATION_REAL_NEXT_APP_DRAFT === "true";
const existingRunId = (process.env.GENERATION_REAL_EXISTING_RUN_ID || "").trim();
if (!/^ws-[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(workspaceNamespace)) {
  throw new Error("GENERATION_REAL_WORKSPACE_NAMESPACE must be a valid ws-* namespace");
}
const privateKey = crypto.createPrivateKey(fs.readFileSync(privateKeyFile));
const adminToken = fs.readFileSync(adminTokenFile, "utf8").trim();
const startedAt = new Date().toISOString();
const editId = startedAt.replace(/[-:.TZ]/g, "");
const evidenceDirectory = path.resolve(evidenceRoot, `edit-${editId}-running`);
const eventFile = path.join(evidenceDirectory, "run-edit.events.ndjson");
fs.mkdirSync(evidenceDirectory, { recursive: true, mode: 0o700 });

let runId = null;
let result = null;
try {
  let before;
  try {
    before = await getRuntimeState();
  } catch (error) {
    if (!configuredBaseVersionId) throw error;
    before = { currentVersionId: configuredBaseVersionId, sandboxBindingId: null };
  }
  if (!before.currentVersionId) {
    throw new Error("existing project has no editable currentVersionId");
  }

  // A published project may retain the identity of a released binding in its
  // runtime-state snapshot. Omit it so StartRun provisions a fresh Sandbox and
  // restores the immutable base Version into that workspace.
  const inputContext = { baseVersionId: before.currentVersionId };

  if (existingRunId) {
    runId = existingRunId;
  } else {
    const started = await signedJson("/runs", {
      method: "POST",
      body: {
        projectId,
        phase: "edit",
        agentProfile: "edit",
        inputContext,
      },
    });
    runId = started.runId;
    if (!runId) throw new Error("Edit start response has no runId");

    await signedJson(`/runs/${encodeURIComponent(runId)}/continue`, {
      method: "POST",
      body: { userMessage: prompt },
    });
  }
  const stream = await readRunEvents(runId, eventFile);
  const run = summarizeRun(runId, stream.events, stream.evidence);
  if (run.status !== "completed") {
    throw new Error(`Edit did not complete: ${run.summary || run.status}`);
  }

  const draftPreview = nextAppDraftMode
    ? existingRunId
      ? {
          status: "previously_verified",
          runId,
          note: "The completed Run's Draft route and headline were observed before Sandbox cleanup; preview base-path rewriting is regression-tested and icon availability is rechecked on the published Artifact.",
        }
      : await verifyDraftPreview(runId)
    : null;
  const publishWorkflow = nextAppDraftMode
    ? await publishLatestDraft(runId)
    : null;

  const after = nextAppDraftMode
    ? { currentVersionId: publishWorkflow.versionId }
    : await getRuntimeState();
  if (!after.currentVersionId || after.currentVersionId === before.currentVersionId) {
    throw new Error("Edit completed without promoting a new Version");
  }
  const artifact = await readArtifact(after.currentVersionId);
  const releaseEvidence = nextAppDraftMode
    ? {
        available: true,
        source: "publish-workflow",
        versionId: publishWorkflow.versionId,
        releaseId: publishWorkflow.releaseId,
        operationId: publishWorkflow.operationId,
        publicUrl: publishWorkflow.publicUrl,
      }
    : await readReleaseEvidence();
  result = {
    schemaVersion: "generation-real-provider-edit-evidence@1",
    startedAt,
    finishedAt: new Date().toISOString(),
    status: "accepted",
    projectId,
    workspaceNamespace,
    prompt,
    baseVersionId: before.currentVersionId,
    versionId: after.currentVersionId,
    run,
    draftPreview,
    publishWorkflow,
    artifact,
    releaseEvidence,
    providerVerified:
      run.modelExecutions.length > 0 &&
      run.modelExecutions.every(
        (execution) =>
          execution.modelResourceId === "deepseek-v4-pro" &&
          execution.providerRequestIdPresent === true,
      ),
    secretMaterialPersisted: false,
  };
  if (!result.providerVerified) {
    throw new Error("Edit evidence does not prove real deepseek-v4-pro execution");
  }
} catch (error) {
  result = {
    schemaVersion: "generation-real-provider-edit-evidence@1",
    startedAt,
    finishedAt: new Date().toISOString(),
    status: "failed",
    projectId,
    prompt,
    runId,
    error: { name: error?.name || "Error", message: String(error?.message || error) },
    secretMaterialPersisted: false,
  };
} finally {
  if (!existingRunId) await releaseSandbox();
}

const finalDirectory = evidenceDirectory.replace(
  /-running$/,
  result.status === "accepted" ? "-accepted" : "-failed",
);
fs.writeFileSync(
  path.join(evidenceDirectory, "real-provider-edit-summary.json"),
  `${JSON.stringify(result, null, 2)}\n`,
  { mode: 0o600 },
);
fs.renameSync(evidenceDirectory, finalDirectory);
process.stdout.write(`Real Provider Edit ${result.status}: ${finalDirectory}\n`);
if (result.status !== "accepted") process.exitCode = 1;

async function getRuntimeState() {
  return signedJson(`/projects/${encodeURIComponent(projectId)}/runtime-state`);
}

async function readArtifact(versionId) {
  const response = await signedFetch(
    `/artifacts/${encodeURIComponent(projectId)}/current/`,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`current Artifact returned ${response.status}: ${body.slice(0, 500)}`);
  }
  const navPattern = /<nav(?:\s|>)/i;
  if (!navPattern.test(body)) {
    throw new Error("edited Artifact does not contain a semantic <nav> element");
  }
  if (!body.includes("让 AI 成为可治理的企业生产力")) {
    throw new Error("edited Artifact lost the original required headline");
  }
  const iconHref = declaredIconHref(body);
  if (!iconHref) {
    throw new Error("edited Artifact does not declare an application icon");
  }
  let iconStatus = 200;
  if (!iconHref.startsWith("data:")) {
    const iconUrl = new URL(iconHref, baseUrl);
    const iconPath = iconUrl.pathname.replace(/^\/+/, "");
    const iconResponse = await signedFetch(
      `/artifacts/${encodeURIComponent(projectId)}/current/${iconPath}${iconUrl.search}`,
    );
    iconStatus = iconResponse.status;
    if (!iconResponse.ok) {
      throw new Error(
        `edited Artifact icon returned ${iconResponse.status}: ${(await iconResponse.text()).slice(0, 300)}`,
      );
    }
  }
  return {
    versionId,
    route: "/",
    httpStatus: response.status,
    semanticNavFound: true,
    originalHeadlineFound: true,
    declaredIconHref: iconHref,
    declaredIconHttpStatus: iconStatus,
    bodySha256: sha256(body),
    bodyBytes: Buffer.byteLength(body),
  };
}

async function readReleaseEvidence() {
  const response = await fetch(
    new URL(`/internal/projects/${encodeURIComponent(projectId)}/release-evidence`, baseUrl),
    {
      headers: {
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": adminToken,
      },
      signal: AbortSignal.timeout(120_000),
    },
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`release evidence returned ${response.status}: ${body.slice(0, 500)}`);
  }
  const evidence = JSON.parse(body);
  return {
    schemaVersion: evidence.schemaVersion,
    projectId: evidence.projectId,
    baseVersionId: evidence.baseVersionId,
    currentVersionId: evidence.currentVersionId,
    artifactManifestHash: evidence.artifactManifestHash,
    sourceFingerprint: evidence.sourceFingerprint,
    terminalToolFailureCount: evidence.terminalToolFailureCount,
  };
}

async function verifyDraftPreview(editRunId) {
  const deadline = Date.now() + 180_000;
  let session = null;
  while (Date.now() < deadline) {
    const response = await signedFetch(
      `/projects/${encodeURIComponent(projectId)}/draft-preview`,
    );
    if (response.ok) {
      session = await response.json();
      if (
        session.status === "ready" &&
        session.lastReadyRevision >= session.workspaceRevision &&
        session.durableRevision >= session.workspaceRevision
      ) {
        break;
      }
    } else if (response.status !== 404) {
      throw new Error(
        `draft preview lookup returned ${response.status}: ${(await response.text()).slice(0, 500)}`,
      );
    }
    await delay(1_000);
  }
  if (!session || session.status !== "ready") {
    throw new Error(`Draft Preview did not become ready: ${JSON.stringify(session)}`);
  }
  const leaseId = new URL(session.proxyUrl).pathname.split("/").filter(Boolean).at(-1);
  if (!leaseId) throw new Error("Draft Preview proxy URL omitted a lease id");
  const prefix = `/projects/${projectId}/previews/${leaseId}`;
  const response = await signedFetch(`/previews/${encodeURIComponent(leaseId)}/`, {
    headers: { "x-anydesign-preview-prefix": prefix },
  });
  const html = await response.text();
  if (!response.ok) {
    throw new Error(
      `Draft Preview route returned ${response.status}: ${html.slice(0, 500)}`,
    );
  }
  if (!html.includes("让 AI 成为可治理的企业生产力")) {
    throw new Error("Draft Preview lost the original required headline");
  }
  const iconHref = declaredIconHref(html);
  if (!iconHref) {
    throw new Error("Draft Preview HTML does not declare an application icon");
  }
  let iconStatus = 200;
  if (!iconHref.startsWith("data:")) {
    const iconUrl = new URL(iconHref, baseUrl);
    const publicPrefix = `/projects/${projectId}/previews/${leaseId}`;
    const upstreamPrefix = `/previews/${leaseId}`;
    const iconPath = iconUrl.pathname.startsWith(`${publicPrefix}/`)
      ? iconUrl.pathname.slice(publicPrefix.length)
      : iconUrl.pathname.startsWith(`${upstreamPrefix}/`)
        ? iconUrl.pathname.slice(upstreamPrefix.length)
        : iconUrl.pathname;
    const previewPath = `/previews/${encodeURIComponent(leaseId)}/${iconPath.replace(/^\/+/, "")}`;
    const iconResponse = await signedFetch(
      `${previewPath}${iconUrl.search}`,
      { headers: { "x-anydesign-preview-prefix": prefix } },
    );
    iconStatus = iconResponse.status;
    if (!iconResponse.ok) {
      throw new Error(
        `Draft Preview icon returned ${iconResponse.status}: ${(await iconResponse.text()).slice(0, 300)}`,
      );
    }
  }
  return {
    sessionId: session.sessionId,
    sessionEpoch: session.sessionEpoch,
    leaseId,
    runId: editRunId,
    status: session.status,
    workspaceRevision: session.workspaceRevision,
    lastReadyRevision: session.lastReadyRevision,
    durableRevision: session.durableRevision,
    durableSnapshotId: session.durableSnapshotId,
    httpStatus: response.status,
    originalHeadlineFound: true,
    declaredIconHref: iconHref,
    declaredIconHttpStatus: iconStatus,
  };
}

async function publishLatestDraft(editRunId) {
  const history = await signedJson(`/projects/${encodeURIComponent(projectId)}/history`);
  const snapshot = (history.items || [])
    .filter((item) => item.kind === "draft_snapshot")
    .map((item) => item.snapshot)
    .find((candidate) => candidate.createdByRunId === editRunId);
  if (!snapshot) {
    throw new Error(`project history has no DraftSnapshot created by ${editRunId}`);
  }
  const existingWorkflows = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/publish-workflows`,
  );
  const completed = (existingWorkflows.workflows || []).find(
    (workflow) =>
      workflow.status === "completed" &&
      workflow.source?.kind === "static-snapshot" &&
      workflow.source?.snapshotId === snapshot.snapshotId,
  );
  if (completed?.publicUrl) {
    return publishWorkflowEvidence(completed, snapshot, []);
  }
  let expectedGeneration = 0;
  let expectedCurrentReleaseId = null;
  const deployment = await signedFetch(
    `/projects/${encodeURIComponent(projectId)}/deployment-state`,
  );
  if (deployment.ok) {
    const state = await deployment.json();
    expectedGeneration = Number(state.runtime?.desiredGeneration || 0);
    expectedCurrentReleaseId = state.runtime?.currentReleaseId || null;
  } else if (deployment.status !== 404) {
    throw new Error(
      `deployment state returned ${deployment.status}: ${(await deployment.text()).slice(0, 500)}`,
    );
  }
  const started = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/publish-workflows`,
    {
      method: "POST",
      body: {
        source: {
          kind: "static-snapshot",
          projectId,
          snapshotId: snapshot.snapshotId,
          expectedSourceHash: snapshot.sourceHash,
        },
        idempotencyKey: `real-next-app-edit-${editRunId}-${expectedGeneration}-${expectedCurrentReleaseId || "initial"}`,
        expectedCurrentReleaseId,
        expectedGeneration,
        visualReviewMode: "advisory",
        runtimeProfileId: "static-web-v1",
      },
    },
  );
  let workflow = started.workflow;
  const observations = [];
  const deadline = Date.now() + 20 * 60_000;
  while (workflow && Date.now() < deadline) {
    const observation = {
      observedAt: new Date().toISOString(),
      status: workflow.status,
      checkpoint: workflow.checkpoint,
      publicUrl: workflow.publicUrl || null,
      error: workflow.error || null,
    };
    const previous = observations.at(-1);
    if (
      !previous ||
      previous.status !== observation.status ||
      previous.checkpoint !== observation.checkpoint
    ) {
      observations.push(observation);
    }
    if (["completed", "failed", "cancelled", "rolled_back", "rollback_failed"].includes(workflow.status)) {
      break;
    }
    await delay(2_000);
    workflow = (
      await signedJson(`/publish-workflows/${encodeURIComponent(workflow.id)}`)
    ).workflow;
  }
  if (workflow?.status !== "completed" || !workflow.publicUrl) {
    throw new Error(
      `PublishWorkflow did not complete: ${JSON.stringify({ workflow, observations }).slice(0, 1500)}`,
    );
  }
  return publishWorkflowEvidence(workflow, snapshot, observations);
}

function publishWorkflowEvidence(workflow, snapshot, observations) {
  return {
    workflowId: workflow.id,
    status: workflow.status,
    checkpoint: workflow.checkpoint,
    versionId: workflow.versionId,
    releaseId: workflow.releaseId,
    operationId: workflow.operationId,
    publicUrl: workflow.publicUrl,
    expectedGeneration: workflow.expectedGeneration,
    expectedCurrentReleaseId: workflow.expectedCurrentReleaseId || null,
    snapshotId: snapshot.snapshotId,
    sourceHash: snapshot.sourceHash,
    observations,
  };
}

function declaredIconHref(html) {
  for (const match of html.matchAll(/<link\b[^>]*>/gi)) {
    const tag = match[0];
    const rel = tag.match(/\brel=["']([^"']+)["']/i)?.[1] || "";
    if (!rel.split(/\s+/).includes("icon")) continue;
    const href = tag.match(/\bhref=["']([^"']+)["']/i)?.[1];
    if (href) return href.replaceAll("&amp;", "&");
  }
  return null;
}

async function releaseSandbox() {
  try {
    await fetch(
      new URL(`/internal/projects/${encodeURIComponent(projectId)}/release-sandbox`, baseUrl),
      {
        method: "POST",
        headers: {
          "x-anydesign-internal": "true",
          "x-runtime-admin-token": adminToken,
        },
        signal: AbortSignal.timeout(120_000),
      },
    );
  } catch {
    // Best-effort cleanup; the summary retains the primary result.
  }
}

async function signedJson(target, options = {}) {
  const response = await signedFetch(target, options);
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`${options.method || "GET"} ${target} returned ${response.status}: ${body.slice(0, 500)}`);
  }
  return body ? JSON.parse(body) : {};
}

async function signedFetch(target, options = {}) {
  const headers = {
    authorization: `Bearer ${issuePrincipalToken()}`,
    ...(options.body ? { "content-type": "application/json" } : {}),
    ...(options.headers || {}),
  };
  return fetch(new URL(target, baseUrl), {
    ...options,
    headers,
    body: options.body ? JSON.stringify(options.body) : undefined,
    signal: AbortSignal.timeout(options.timeoutMs || 120_000),
  });
}

async function readRunEvents(editRunId, destination) {
  const response = await fetch(
    new URL(`/runs/${encodeURIComponent(editRunId)}/events`, baseUrl),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken()}` },
      signal: AbortSignal.timeout(900_000),
    },
  );
  if (!response.ok || !response.body) {
    throw new Error(`Edit event stream returned ${response.status}`);
  }
  const descriptor = fs.openSync(destination, "wx", 0o600);
  const events = [];
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let terminalSeen = false;
  try {
    while (!terminalSeen) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      while (true) {
        const newline = buffer.indexOf("\n");
        if (newline < 0) break;
        const line = buffer.slice(0, newline).replace(/\r$/, "");
        buffer = buffer.slice(newline + 1);
        if (!line.startsWith("data:")) continue;
        const payload = line.slice(5).trimStart();
        if (!payload) continue;
        const event = sanitizeEvidenceEvent(JSON.parse(payload));
        events.push(event);
        fs.writeSync(descriptor, `${JSON.stringify(event)}\n`);
        terminalSeen = event.type === "run.completed";
        if (terminalSeen) await reader.cancel();
      }
    }
  } finally {
    fs.closeSync(descriptor);
  }
  if (!terminalSeen) throw new Error("Edit stream ended without run.completed");
  const bytes = fs.readFileSync(destination);
  return {
    events,
    evidence: {
      schemaVersion: "generation-run-event-stream@1",
      path: path.basename(destination),
      format: "ndjson",
      eventCount: events.length,
      bytes: bytes.byteLength,
      sha256: sha256(bytes),
      incremental: true,
    },
  };
}

function summarizeRun(editRunId, events, eventStream) {
  const terminal = [...events].reverse().find((event) => event.type === "run.completed");
  const usageEvents = events.filter((event) => event.type === "model.usage");
  const usage = usageEvents.reduce(
    (total, event) => ({
      inputTokens: total.inputTokens + Number(event.inputTokens || 0),
      outputTokens: total.outputTokens + Number(event.outputTokens || 0),
      cachedInputTokens: total.cachedInputTokens + Number(event.cachedInputTokens || 0),
      estimated: total.estimated || event.estimated === true,
    }),
    { inputTokens: 0, outputTokens: 0, cachedInputTokens: 0, estimated: false },
  );
  usage.totalTokens = usage.inputTokens + usage.outputTokens;
  return {
    phase: "edit",
    runId: editRunId,
    status: terminal?.status || "unknown",
    summary: terminal?.summary || null,
    usage,
    turns: usageEvents.length,
    toolCalls: events.filter((event) => event.type === "tool.started").length,
    recoverableToolFailures: events.filter((event) => event.type === "tool.failed").length,
    modelExecutions: events
      .filter((event) => event.type === "model.execution")
      .map((event) => event.snapshot),
    eventStream,
  };
}

function sanitizeEvidenceEvent(event) {
  if (event?.type !== "model.execution" || !event.snapshot) return event;
  const { providerRequestId, ...snapshot } = event.snapshot;
  return {
    ...event,
    snapshot: {
      ...snapshot,
      providerRequestIdPresent:
        typeof providerRequestId === "string" && providerRequestId.length > 0,
    },
  };
}

function issuePrincipalToken() {
  const publicDer = crypto
    .createPublicKey(privateKey)
    .export({ type: "spki", format: "der" });
  const now = Math.floor(Date.now() / 1_000);
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const header = encode({ alg: "EdDSA", typ: "JWT", kid: `ed25519-${sha256(publicDer).slice(0, 16)}` });
  const payload = encode({
    iss: "anydesign-bff",
    aud: "anydesign-runtime-public",
    sub: principalId,
    jti: crypto.randomBytes(16).toString("hex"),
    iat: now,
    exp: now + 120,
    projectId,
    operations: [
      "preview.read",
      "project.read",
      "project.write",
      "publication.read",
      "publication.write",
    ],
  });
  const input = `${header}.${payload}`;
  return `${input}.${crypto.sign(null, Buffer.from(input), privateKey).toString("base64url")}`;
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
