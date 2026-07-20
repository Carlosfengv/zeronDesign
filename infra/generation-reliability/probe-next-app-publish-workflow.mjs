#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";

const [baseUrl, privateKeyFile, adminTokenFile, projectId, workspaceNamespace] =
  process.argv.slice(2);
if (!baseUrl || !privateKeyFile || !adminTokenFile || !projectId || !workspaceNamespace) {
  throw new Error(
    "usage: probe-next-app-publish-workflow.mjs <base-url> <private-key> <admin-token-file> <project-id> <workspace-namespace>",
  );
}

const privateKey = crypto.createPrivateKey(fs.readFileSync(privateKeyFile));
const adminToken = fs.readFileSync(adminTokenFile, "utf8").trim();

await request(`/internal/projects/${encodeURIComponent(projectId)}/access`, {
  method: "PUT",
  internal: true,
  body: {
    ownerPrincipalId: "generation-real-provider-suite",
    workspaceNamespace,
  },
});

const history = await request(`/projects/${encodeURIComponent(projectId)}/history`);
const snapshots = (history.body.items || [])
  .filter((item) => item.kind === "draft_snapshot")
  .map((item) => item.snapshot);
const snapshot = snapshots[0];
if (!snapshot) throw new Error("project history contains no DraftSnapshot");

const publish = await request(
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
      idempotencyKey: `real-next-app-api-probe-20260719-${projectId}`,
      expectedGeneration: 0,
      visualReviewMode: "advisory",
      runtimeProfileId: "static-web-v1",
    },
    allowError: true,
  },
);

const observations = [];
let workflow = publish.body.workflow;
if (publish.status < 400 && workflow?.id) {
  const deadline = Date.now() + 20 * 60_000;
  while (Date.now() < deadline) {
    const observation = {
      observedAt: new Date().toISOString(),
      status: workflow.status,
      checkpoint: workflow.checkpoint,
      publicUrl: workflow.publicUrl || null,
      error: workflow.error || null,
    };
    const previous = observations.at(-1);
    if (!previous || previous.status !== observation.status || previous.checkpoint !== observation.checkpoint) {
      observations.push(observation);
    }
    if (["completed", "failed", "cancelled", "rolled_back", "rollback_failed"].includes(workflow.status)) break;
    await new Promise((resolve) => setTimeout(resolve, 2_000));
    workflow = (
      await request(
        `/publish-workflows/${encodeURIComponent(workflow.id)}`,
      )
    ).body.workflow;
  }
}

process.stdout.write(
  `${JSON.stringify(
    {
      projectId,
      history: snapshots.map((candidate) => ({
        snapshotId: candidate.snapshotId,
        templateId: candidate.templateId,
        templateVersion: candidate.templateVersion,
        sourceHash: candidate.sourceHash,
        createdByRunId: candidate.createdByRunId,
      })),
      publishWorkflow: {
        httpStatus: publish.status,
        body: workflow ? { workflow } : publish.body,
        observations,
      },
    },
    null,
    2,
  )}\n`,
);

async function request(target, options = {}) {
  const response = await fetch(new URL(target, baseUrl), {
    method: options.method || "GET",
    headers: {
      ...(options.body ? { "content-type": "application/json" } : {}),
      ...(options.internal
        ? {
            "x-anydesign-internal": "true",
            "x-runtime-admin-token": adminToken,
          }
        : { authorization: `Bearer ${issuePrincipalToken()}` }),
    },
    body: options.body ? JSON.stringify(options.body) : undefined,
    signal: AbortSignal.timeout(120_000),
  });
  const text = await response.text();
  const body = text ? JSON.parse(text) : {};
  if (!response.ok && !options.allowError) {
    throw new Error(
      `${options.method || "GET"} ${target} returned ${response.status}: ${text.slice(0, 500)}`,
    );
  }
  return { status: response.status, body };
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
    sub: "generation-real-provider-suite",
    jti: crypto.randomBytes(16).toString("hex"),
    iat: now,
    exp: now + 120,
    projectId,
    operations: [
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
