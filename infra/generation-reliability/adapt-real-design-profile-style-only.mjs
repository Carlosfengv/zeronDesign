#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [baseUrl, privateKeyFile, projectId, designProfileId, evidenceFile] =
  process.argv.slice(2);
if (!baseUrl || !privateKeyFile || !projectId || !designProfileId || !evidenceFile) {
  throw new Error(
    "usage: adapt-real-design-profile-style-only.mjs <base-url> <private-key> <project-id> <design-profile-id> <evidence-file>",
  );
}

const privateKey = crypto.createPrivateKey(fs.readFileSync(privateKeyFile));
const currentResponse = await signedJson(
  `/design-profiles/${encodeURIComponent(designProfileId)}`,
);
const current = currentResponse.designProfile;
if (!current || current.scope?.projectId !== projectId || current.status !== "active") {
  throw new Error("target Design Profile is not an active profile in the project scope");
}

const profile = structuredClone(current);
for (const field of ["id", "name", "version", "createdAt", "updatedAt"]) delete profile[field];
profile.websiteContext = {
  enforcementMode: "enforced",
  craftPacks: ["accessibility-baseline", "responsive-layout"],
};
profile.technical = {
  ...profile.technical,
  allowedTemplates: ["next-app"],
  preferredTemplates: {
    ...profile.technical?.preferredTemplates,
    website: "next-app",
  },
};
profile.product = {
  ...profile.product,
  name: "ZStack Zenova Visual Style Reference",
  category: "enterprise AI agent platform website",
  primaryUseCases: ["enterprise AI product website", "agent platform product story"],
};
profile.brand.messaging = {
  ...profile.brand.messaging,
  headlineStyle: "luminous enterprise product capability",
  bodyStyle: "short technical enterprise AI support copy",
  ctaStyle: "one violet primary action",
  proofStyle: "platform capability and architecture evidence",
};
profile.components = {
  ...profile.components,
  primitives: {
    ...profile.components?.primitives,
    button: {
      role: "navigation and conversion actions",
      usage: ["Use violet only for the primary conversion action."],
      avoid: ["extra colored CTAs"],
    },
    card: {
      role: "frosted enterprise capability surface",
      usage: ["Use inset frost edges."],
      avoid: ["paper cards"],
    },
  },
  patterns: {
    hero: {
      role: "centered enterprise AI product signal",
      requiredElements: ["product wordmark", "capability proof", "blueprint grid"],
    },
  },
};
profile.content = {
  ...profile.content,
  website: {
    requiredSections: ["navigation", "hero", "capability cards", "architecture", "footer"],
  },
};
profile.signatureRules = (profile.signatureRules || [])
  .filter(
    (rule) =>
      ![
        "authkit-auth-forms",
        "authkit-auth-submit-violet",
        "authkit-violet-scope",
      ].includes(rule.id),
  )
  .map((rule) => {
    const websiteComputedStyleVerifications = {
      "authkit-canvas": {
        kind: "computed-style",
        route: "/",
        selector: "html",
        property: "background-color",
        expected: "#05060f",
        comparator: { kind: "color-equivalent" },
        minMatches: 1,
      },
      "authkit-display-font": {
        kind: "computed-style",
        route: "/",
        selector: ".runtime-hero h1, h1",
        property: "font-family",
        expected: "Space Grotesk",
        comparator: { kind: "contains" },
        minMatches: 1,
        matchPolicy: "any",
      },
      "authkit-violet-action": {
        kind: "computed-style",
        route: "/",
        selector: ".runtime-button-primary, [data-primary-action]",
        property: "background-color",
        expected: "#663af3",
        comparator: { kind: "color-equivalent" },
        minMatches: 1,
        matchPolicy: "any",
      },
      "authkit-frost-elevation": {
        kind: "computed-style",
        route: "/",
        selector: ".runtime-card, [data-frost-card]",
        property: "box-shadow",
        expected: "inset",
        comparator: { kind: "contains" },
        minMatches: 1,
        matchPolicy: "any",
      },
    };
    const verification = websiteComputedStyleVerifications[rule.id];
    return {
      ...rule,
      ...(rule.id === "authkit-violet-action"
        ? { statement: "The primary conversion action uses the #663af3 violet fill." }
        : {}),
      ...(verification ? { verification } : {}),
    };
  });
profile.overrides = {
  ...profile.overrides,
  templates: {
    ...profile.overrides?.templates,
    "next-app": {
      content: { requiredDataHooks: ["data-blueprint-grid", "data-spotlight"] },
    },
  },
};

const updatedResponse = await signedJson(
  `/design-profiles/${encodeURIComponent(designProfileId)}`,
  {
    method: "PUT",
    body: {
      expectedVersion: current.version,
      name: `${current.name.replace(/ — Website (?:Random Style Test|Style Only)$/, "")} — Website Style Only`,
      profile,
    },
  },
);
const updated = updatedResponse.designProfile;
if (updated?.id !== designProfileId || updated.version !== current.version + 1) {
  throw new Error("Style-only adaptation did not create the expected Profile version");
}
const binding = await signedJson(`/projects/${encodeURIComponent(projectId)}/design-profile`);
if (binding.designProfile?.id !== designProfileId || binding.designProfile.version !== updated.version) {
  throw new Error("project binding did not advance to the adapted Profile version");
}

const evidence = {
  schemaVersion: "generation-real-design-profile-adaptation@1",
  recordedAt: new Date().toISOString(),
  projectId,
  designProfileId,
  fromVersion: current.version,
  toVersion: updated.version,
  name: updated.name,
  adaptation: "style-only",
  preservedVisualRuleIds: updated.signatureRules.map((rule) => rule.id),
  removedProductSpecificRuleIds: [
    "authkit-auth-forms",
    "authkit-auth-submit-violet",
    "authkit-violet-scope",
  ],
  migratedToRenderedStyleVerificationRuleIds: [
    "authkit-canvas",
    "authkit-display-font",
    "authkit-violet-action",
    "authkit-frost-elevation",
  ],
  bindingVerified: true,
  secretMaterialPersisted: false,
};
fs.mkdirSync(path.dirname(path.resolve(evidenceFile)), { recursive: true, mode: 0o700 });
fs.writeFileSync(path.resolve(evidenceFile), `${JSON.stringify(evidence, null, 2)}\n`, {
  mode: 0o600,
});
process.stdout.write(`Adapted and bound ${updated.name} (${updated.id}@${updated.version})\n`);

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
