#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [baseUrl, privateKeyFile, projectId, fixtureFile, evidenceFile] = process.argv.slice(2);
if (!baseUrl || !privateKeyFile || !projectId || !fixtureFile || !evidenceFile) {
  throw new Error(
    "usage: configure-real-design-profile.mjs <base-url> <private-key> <project-id> <fixture-file> <evidence-file>",
  );
}

const privateKey = crypto.createPrivateKey(fs.readFileSync(privateKeyFile));
const fixtureBytes = fs.readFileSync(fixtureFile);
const fixture = JSON.parse(fixtureBytes);
const profile = structuredClone(fixture);
for (const field of ["id", "name", "version", "createdAt", "updatedAt"]) delete profile[field];
profile.scope = { projectId };
profile.status = "active";
profile.source = {
  kind: "manual",
  notes: `Project-scoped copy of randomly selected fixture ${path.basename(fixtureFile)} for a real Provider Website style Edit.`,
};

const before = await signedJson(`/design-profiles?projectId=${encodeURIComponent(projectId)}`);
const created = await signedJson("/design-profiles", {
  method: "POST",
  body: {
    projectId,
    name: `${fixture.name} — Website Random Style Test`,
    profile,
  },
});
const selected = created.designProfile;
if (!selected?.id || selected.scope?.projectId !== projectId || selected.status !== "active") {
  throw new Error("created Design Profile is not active in the target project scope");
}
const bound = await signedJson(`/projects/${encodeURIComponent(projectId)}/design-profile`, {
  method: "POST",
  body: { designProfileId: selected.id },
});
if (bound.designProfile?.id !== selected.id) {
  throw new Error("project binding did not return the selected Design Profile");
}
const after = await signedJson(`/design-profiles?projectId=${encodeURIComponent(projectId)}`);
const visible = after.designProfiles || [];
if (!visible.some((candidate) => candidate.id === selected.id)) {
  throw new Error("selected Design Profile is not visible from the target project");
}

const evidence = {
  schemaVersion: "generation-real-design-profile-selection@1",
  recordedAt: new Date().toISOString(),
  projectId,
  randomSelection: {
    algorithm: "system-shuf",
    candidateCount: 2,
    selectedFixture: path.basename(fixtureFile),
    selectedFixtureSha256: sha256(fixtureBytes),
  },
  beforeVisibleProfileCount: before.designProfiles?.length || 0,
  afterVisibleProfileCount: visible.length,
  selectedDesignProfile: {
    id: selected.id,
    name: selected.name,
    version: selected.version,
    status: selected.status,
    scope: selected.scope,
    effectiveWebsiteTokens: selected.runtimeTokenMapping,
  },
  bindingVerified: true,
  secretMaterialPersisted: false,
};
fs.mkdirSync(path.dirname(path.resolve(evidenceFile)), { recursive: true, mode: 0o700 });
fs.writeFileSync(path.resolve(evidenceFile), `${JSON.stringify(evidence, null, 2)}\n`, {
  mode: 0o600,
});
process.stdout.write(
  `Selected and bound ${selected.name} (${selected.id}@${selected.version})\n`,
);

async function signedJson(target, options = {}) {
  const response = await fetch(new URL(target, baseUrl), {
    method: options.method || "GET",
    headers: {
      authorization: `Bearer ${issuePrincipalToken()}`,
      ...(options.body ? { "content-type": "application/json" } : {}),
    },
    body: options.body ? JSON.stringify(options.body) : undefined,
    signal: AbortSignal.timeout(120_000),
  });
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`${options.method || "GET"} ${target} returned ${response.status}: ${body.slice(0, 500)}`);
  }
  return body ? JSON.parse(body) : {};
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
    operations: ["project.read", "project.write"],
  });
  const input = `${header}.${payload}`;
  return `${input}.${crypto.sign(null, Buffer.from(input), privateKey).toString("base64url")}`;
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}
