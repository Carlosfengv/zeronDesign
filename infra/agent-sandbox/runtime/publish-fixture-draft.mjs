#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";

const [baseUrl, privateKeyFile, projectId, principalId, runId] = process.argv.slice(2);
if (!baseUrl || !privateKeyFile || !projectId || !principalId || !runId) {
  throw new Error(
    "usage: publish-fixture-draft.mjs <base-url> <private-key> <project-id> <principal-id> <run-id>",
  );
}

const privateKey = crypto.createPrivateKey(fs.readFileSync(privateKeyFile));
const history = await request(`/projects/${encodeURIComponent(projectId)}/history`);
const snapshot = (history.items || [])
  .filter((item) => item.kind === "draft_snapshot")
  .map((item) => item.snapshot)
  .find((candidate) => candidate.createdByRunId === runId);
if (!snapshot) {
  throw new Error(`project history has no DraftSnapshot created by ${runId}`);
}

const deploymentResponse = await signedFetch(
  `/projects/${encodeURIComponent(projectId)}/deployment-state`,
);
let expectedGeneration = 0;
let expectedCurrentReleaseId = null;
if (deploymentResponse.ok) {
  const deployment = await deploymentResponse.json();
  expectedGeneration = Number(deployment.runtime?.desiredGeneration || 0);
  expectedCurrentReleaseId = deployment.runtime?.currentReleaseId || null;
} else if (deploymentResponse.status !== 404) {
  throw new Error(
    `deployment state returned ${deploymentResponse.status}: ${(await deploymentResponse.text()).slice(0, 500)}`,
  );
}

const started = await request(
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
      idempotencyKey: `runtime-rc-${runId}-${expectedGeneration}-${expectedCurrentReleaseId || "initial"}`,
      expectedCurrentReleaseId,
      expectedGeneration,
      visualReviewMode: "advisory",
      runtimeProfileId: "static-web-v1",
    },
  },
);
let workflow = started.workflow;
const observations = [];
const deadline = Date.now() + 10 * 60_000;
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
  if (
    ["completed", "failed", "cancelled", "rolled_back", "rollback_failed"].includes(
      workflow.status,
    )
  ) {
    break;
  }
  await new Promise((resolve) => setTimeout(resolve, 2_000));
  workflow = (
    await request(`/publish-workflows/${encodeURIComponent(workflow.id)}`)
  ).workflow;
}
if (workflow?.status !== "completed" || !workflow.versionId || !workflow.publicUrl) {
  throw new Error(
    `PublishWorkflow did not complete: ${JSON.stringify({ workflow, observations }).slice(0, 2000)}`,
  );
}

process.stdout.write(
  `${JSON.stringify({
    workflowId: workflow.id,
    status: workflow.status,
    checkpoint: workflow.checkpoint,
    versionId: workflow.versionId,
    releaseId: workflow.releaseId,
    operationId: workflow.publicationOperationId,
    publicUrl: workflow.publicUrl,
    expectedGeneration,
    expectedCurrentReleaseId,
    snapshotId: snapshot.snapshotId,
    sourceHash: snapshot.sourceHash,
    observations,
  })}\n`,
);

async function request(target, options = {}) {
  const response = await signedFetch(target, options);
  const text = await response.text();
  if (!response.ok) {
    throw new Error(
      `${options.method || "GET"} ${target} returned ${response.status}: ${text.slice(0, 500)}`,
    );
  }
  return text ? JSON.parse(text) : {};
}

async function signedFetch(target, options = {}) {
  return fetch(new URL(target, baseUrl), {
    method: options.method || "GET",
    headers: {
      authorization: `Bearer ${issuePrincipalToken()}`,
      ...(options.body ? { "content-type": "application/json" } : {}),
    },
    body: options.body ? JSON.stringify(options.body) : undefined,
    signal: AbortSignal.timeout(120_000),
  });
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
    kid: `ed25519-${crypto.createHash("sha256").update(publicDer).digest("hex").slice(0, 16)}`,
  });
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
