#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [
  baseUrl,
  privateKeyFile,
  adminTokenFile,
  projectId,
  runId,
  route,
  markerFile,
  artifactMode,
  snapshotId,
  previewLeaseId,
  outputFile,
] = process.argv.slice(2);

if (!baseUrl || !privateKeyFile || !adminTokenFile || !projectId || !runId
  || !route || !markerFile || !new Set(["current_version", "draft_preview"]).has(artifactMode)
  || !snapshotId || !previewLeaseId || !outputFile) {
  throw new Error(
    "usage: probe-generation-context-runtime-restart.mjs <base-url> <private-key> <admin-token-file> <project-id> <run-id> <route> <marker-file> <current_version|draft_preview> <snapshot-id|-> <preview-lease-id|-> <output.json>",
  );
}

const privateKey = crypto.createPrivateKey(fs.readFileSync(privateKeyFile));
const adminToken = fs.readFileSync(adminTokenFile, "utf8").trim();
const marker = fs.readFileSync(markerFile, "utf8");

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function jsonHash(value) {
  return value === null || value === undefined ? null : sha256(canonical(value));
}

function issuePrincipalToken() {
  const publicDer = crypto
    .createPublicKey(privateKey)
    .export({ type: "spki", format: "der" });
  const now = Math.floor(Date.now() / 1_000);
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const header = encode({
    alg: "EdDSA",
    typ: "JWT",
    kid: `ed25519-${sha256(publicDer).slice(0, 16)}`,
  });
  const payload = encode({
    iss: "anydesign-bff",
    aud: "anydesign-runtime-public",
    // The paired suite granted project access to this frozen principal before
    // the Build. A restart probe must reuse that identity, not mint a new
    // principal that has never been authorized for the Project.
    sub: "generation-real-provider-suite",
    jti: crypto.randomBytes(16).toString("hex"),
    iat: now,
    exp: now + 120,
    projectId,
    operations: ["preview.read", "project.read", "publication.read"],
  });
  const input = `${header}.${payload}`;
  return `${input}.${crypto.sign(null, Buffer.from(input), privateKey).toString("base64url")}`;
}

async function readJson(pathname) {
  const response = await fetch(new URL(pathname, baseUrl), {
    headers: { authorization: `Bearer ${issuePrincipalToken()}` },
    signal: AbortSignal.timeout(120_000),
  });
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`${pathname} returned ${response.status}: ${body.slice(0, 300)}`);
  }
  return JSON.parse(body);
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
  if (![200, 404, 409].includes(response.status)) {
    throw new Error(`release evidence returned ${response.status}: ${body.slice(0, 300)}`);
  }
  let parsed;
  try {
    parsed = JSON.parse(body);
  } catch {
    throw new Error("release evidence returned a non-JSON response");
  }
  const stableState = response.status === 200
    ? {
        projectId: parsed.projectId,
        artifactManifestHash: parsed.artifactManifestHash,
        candidateManifestHash: parsed.candidateManifestHash,
        bindingId: parsed.bindingId,
        buildId: parsed.buildId,
        buildRunId: parsed.buildRunId,
        editRunId: parsed.editRunId,
        previewLeaseId: parsed.previewLeaseId,
        previewLeaseStatus: parsed.previewLeaseStatus,
        terminalToolFailureCount: parsed.terminalToolFailureCount,
        versionBeforeCas: parsed.versionBeforeCas,
        versionAfterCas: parsed.versionAfterCas,
        eventSequenceValid: parsed.events?.sequenceValid,
        publicationSha256: jsonHash(parsed.publication),
        reviewRepairSha256: jsonHash(parsed.reviewRepair),
        visualReviewSha256: jsonHash(parsed.visualReview),
        sourceSnapshotRefSha256: typeof parsed.sourceSnapshotUri === "string"
          ? sha256(parsed.sourceSnapshotUri)
          : null,
      }
    : { error: parsed.error || null };
  return {
    httpStatus: response.status,
    available: response.status === 200,
    canonicalResponseSha256: sha256(canonical(parsed)),
    stableStateSha256: sha256(canonical(stableState)),
  };
}

async function readArtifact() {
  if (artifactMode === "draft_preview") {
    const prefix = `/projects/${projectId}/previews/${previewLeaseId}`;
    const response = await fetch(
      new URL(`/previews/${encodeURIComponent(previewLeaseId)}${route}`, baseUrl),
      {
        headers: {
          authorization: `Bearer ${issuePrincipalToken()}`,
          "x-anydesign-preview-prefix": prefix,
        },
        signal: AbortSignal.timeout(120_000),
      },
    );
    const body = await response.text();
    if (!response.ok) {
      throw new Error(`draft preview route returned ${response.status}: ${body.slice(0, 300)}`);
    }
    if (!body.includes(marker)) {
      throw new Error("draft preview route omitted the frozen acceptance marker");
    }
    return {
      source: "draft_preview",
      httpStatus: response.status,
      markerFound: true,
      markerSha256: sha256(marker),
      bodySha256: sha256(body),
      bodyBytes: Buffer.byteLength(body),
    };
  }
  const response = await fetch(
    new URL(`/artifacts/${encodeURIComponent(projectId)}/current${route}`, baseUrl),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken()}` },
      signal: AbortSignal.timeout(120_000),
    },
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`artifact route returned ${response.status}: ${body.slice(0, 300)}`);
  }
  if (!body.includes(marker)) throw new Error("artifact route omitted the frozen acceptance marker");
  return {
    source: "current_version",
    httpStatus: response.status,
    markerFound: true,
    markerSha256: sha256(marker),
    bodySha256: sha256(body),
    bodyBytes: Buffer.byteLength(body),
  };
}

async function readProjectState(history) {
  if (artifactMode === "current_version") {
    const runtimeState = await readJson(
      `/projects/${encodeURIComponent(projectId)}/runtime-state`,
    );
    return {
      stateKind: "published_version",
      currentVersionId: runtimeState.currentVersionId,
      sandboxBindingId: runtimeState.sandboxBindingId,
      sourceSnapshotRefSha256: sha256(runtimeState.sourceSnapshotUri),
      templateKey: runtimeState.templateKey,
      draftSnapshotId: null,
      previewLeaseId: null,
      styleContractSha256: jsonHash(runtimeState.styleContract),
      latestBuildSha256: jsonHash(runtimeState.latestBuild),
      dependencyStateSha256: jsonHash(runtimeState.dependencyState),
      previewSha256: jsonHash(runtimeState.preview),
    };
  }
  const draft = history.items?.find(
    (item) => item.kind === "draft_snapshot" && item.snapshot?.snapshotId === snapshotId,
  )?.snapshot;
  if (!draft || typeof draft.sourceSnapshotUri !== "string") {
    throw new Error(`project history omitted durable DraftSnapshot: ${snapshotId}`);
  }
  if (draft.projectId !== projectId || draft.createdByRunId !== runId) {
    throw new Error("durable DraftSnapshot Project or Run identity mismatch");
  }
  return {
    stateKind: "durable_draft",
    currentVersionId: null,
    sandboxBindingId: null,
    sourceSnapshotRefSha256: sha256(draft.sourceSnapshotUri),
    templateKey: draft.templateId,
    draftSnapshotId: draft.snapshotId,
    previewLeaseId,
    styleContractSha256: null,
    latestBuildSha256: null,
    dependencyStateSha256: null,
    previewSha256: jsonHash({
      snapshotId: draft.snapshotId,
      sourceHash: draft.sourceHash,
      previewLeaseId,
    }),
  };
}

async function releaseSandbox() {
  const response = await fetch(
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
  if (!response.ok) {
    const body = await response.text();
    throw new Error(`release sandbox returned ${response.status}: ${body.slice(0, 300)}`);
  }
}

async function main() {
  const healthResponse = await fetch(new URL("/health", baseUrl), {
    signal: AbortSignal.timeout(30_000),
  });
  if (!healthResponse.ok) throw new Error(`Runtime health returned ${healthResponse.status}`);

  const [generationContextStatus, efficiency, history, releaseEvidence, artifact] =
    await Promise.all([
      readJson(`/runs/${encodeURIComponent(runId)}/generation-context-status`),
      readJson(`/runs/${encodeURIComponent(runId)}/efficiency-metrics`),
      readJson(`/projects/${encodeURIComponent(projectId)}/history`),
      readReleaseEvidence(),
      readArtifact(),
    ]);
  const projectState = await readProjectState(history);

  const snapshot = {
    schemaVersion: "generation-context-runtime-restart-snapshot@1",
    recordedAt: new Date().toISOString(),
    projectId,
    runId,
    healthReady: true,
    generationContextStatus,
    efficiency,
    projectState,
    history: {
      itemCount: Array.isArray(history.items) ? history.items.length : 0,
      sha256: sha256(canonical(history)),
    },
    releaseEvidence,
    artifact,
  };

  fs.mkdirSync(path.dirname(path.resolve(outputFile)), { recursive: true });
  const descriptor = fs.openSync(outputFile, "wx", 0o600);
  try {
    fs.writeFileSync(descriptor, `${JSON.stringify(snapshot, null, 2)}\n`);
  } finally {
    fs.closeSync(descriptor);
  }
  if (process.env.GENERATION_RUNTIME_RESTART_RELEASE_SANDBOX === "1") {
    await releaseSandbox();
  }
}

main().catch((error) => {
  process.stderr.write(`${error?.stack || error}\n`);
  process.exitCode = 1;
});
