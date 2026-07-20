#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [
  baseUrl,
  privateKeyFile,
  adminTokenFile,
  projectId,
  designProfileId,
  designFile,
  evidenceRoot,
] = process.argv.slice(2);

if (
  !baseUrl ||
  !privateKeyFile ||
  !adminTokenFile ||
  !projectId ||
  !designProfileId ||
  !designFile ||
  !evidenceRoot
) {
  throw new Error(
    "usage: apply-mintlify-docs-design.mjs <base-url> <private-key> <admin-token-file> <project-id> <design-profile-id> <design-file> <evidence-root>",
  );
}

const privateKey = crypto.createPrivateKey(fs.readFileSync(privateKeyFile));
const adminToken = fs.readFileSync(adminTokenFile, "utf8").trim();
const designBytes = fs.readFileSync(designFile);
const designSha256 = sha256(designBytes);
const startedAt = new Date().toISOString();
const operationId = startedAt.replace(/[-:.TZ]/g, "");
const evidenceDirectory = path.resolve(
  evidenceRoot,
  `mintlify-docs-${operationId}-running`,
);
const eventFile = path.join(evidenceDirectory, "run-edit.events.ndjson");
fs.mkdirSync(evidenceDirectory, { recursive: true, mode: 0o700 });

let runId = null;
let result = null;

try {
  const beforeBinding = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/design-profile`,
  );
  const beforeProfileResponse = await signedJson(
    `/design-profiles/${encodeURIComponent(designProfileId)}`,
  );
  const beforeProfile = beforeProfileResponse.designProfile;
  if (
    beforeBinding.designProfile?.id !== designProfileId ||
    beforeProfile?.id !== designProfileId ||
    beforeProfile.scope?.projectId !== projectId ||
    beforeProfile.status !== "active"
  ) {
    throw new Error("target Design Profile is not the active Profile bound to the project");
  }

  const sourceAlreadyUploaded =
    beforeProfile.source?.kind === "imported" &&
    beforeProfile.source?.sourceHash === designSha256 &&
    beforeProfile.source?.primarySourceArtifactId;
  const alreadyIntegrated =
    sourceAlreadyUploaded &&
    beforeProfile.source?.converterVersion === "mintlify-design-md-docs@2";
  let sourceArtifact;
  let updatedProfile;
  if (sourceAlreadyUploaded) {
    const sourceResponse = await signedJson(
      `/design-source-artifacts/${encodeURIComponent(beforeProfile.source.primarySourceArtifactId)}`,
    );
    sourceArtifact = sourceResponse.artifact;
    updatedProfile = beforeProfile;
  } else {
    const sourceResponse = await signedJson("/design-source-artifacts", {
      method: "POST",
      body: {
        scope: { projectId },
        fileName: path.basename(designFile),
        mediaType: "text/markdown",
        contentBase64: designBytes.toString("base64"),
        clientSha256: designSha256,
      },
    });
    sourceArtifact = sourceResponse.artifact;
  }
  if (
    !sourceArtifact?.id ||
    sourceArtifact.sha256 !== designSha256 ||
    sourceArtifact.scope?.projectId !== projectId
  ) {
    throw new Error("Design Source API did not preserve the supplied source and digest");
  }

  if (!alreadyIntegrated) {
    const profilePayload = buildMintlifyProfile(beforeProfile, sourceArtifact, startedAt);
    const updatedResponse = await signedJson(
      `/design-profiles/${encodeURIComponent(designProfileId)}`,
      {
        method: "PUT",
        body: {
          expectedVersion: beforeProfile.version,
          name: "Mintlify Monochrome Mint Docs — Light + Dark",
          profile: profilePayload,
        },
      },
    );
    updatedProfile = updatedResponse.designProfile;
  }
  if (
    updatedProfile?.id !== designProfileId ||
    updatedProfile.version !== beforeProfile.version + (alreadyIntegrated ? 0 : 1) ||
    updatedProfile.source?.primarySourceArtifactId !== sourceArtifact.id ||
    updatedProfile.source?.sourceHash !== designSha256
  ) {
    throw new Error("current Design Profile was not advanced with the imported source");
  }

  const rebound = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/design-profile`,
    {
      method: "POST",
      body: { designProfileId },
    },
  );
  if (
    rebound.designProfile?.id !== designProfileId ||
    rebound.designProfile?.version !== updatedProfile.version
  ) {
    throw new Error("project binding did not resolve the updated Design Profile version");
  }

  const fidelity = await signedJson(
    `/design-profiles/${encodeURIComponent(designProfileId)}/versions/${updatedProfile.version}/fidelity-report?surface=docs&template=fumadocs-docs`,
  );
  if (
    fidelity.sourceIntegrity !== "verified" ||
    fidelity.sourceHashMatches !== true ||
    fidelity.capsuleMissingRuleIds?.length
  ) {
    throw new Error("Design Profile fidelity report did not verify the imported source and rules");
  }

  const beforeState = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/runtime-state`,
  );
  if (!beforeState.currentVersionId) {
    throw new Error("project has no current Version to edit");
  }

  const prompt = buildEditPrompt(updatedProfile, sourceArtifact);
  const started = await signedJson("/runs", {
    method: "POST",
    body: {
      projectId,
      phase: "edit",
      agentProfile: "edit",
      inputContext: { baseVersionId: beforeState.currentVersionId },
    },
  });
  runId = started.runId;
  if (!runId) throw new Error("Edit start response has no runId");

  const continued = await signedJson(`/runs/${encodeURIComponent(runId)}/continue`, {
    method: "POST",
    body: { userMessage: prompt },
  });
  if (continued.status === "needs_user_input") {
    await signedJson(`/runs/${encodeURIComponent(runId)}/continue`, {
      method: "POST",
      body: {
        userMessage:
          "继续执行本次 DesignProfile Edit。此消息只覆盖关键字误判；仍须严格遵守 Profile 的全部禁止项与 light/dark 要求。",
      },
    });
  }

  const stream = await readRunEvents(runId, eventFile);
  const run = summarizeRun(runId, stream.events, stream.evidence);
  if (run.status !== "completed") {
    throw new Error(`Edit did not complete: ${run.summary || run.status}`);
  }
  if (
    run.modelExecutions.length === 0 ||
    run.modelExecutions.some(
      (execution) =>
        execution.modelResourceId !== "deepseek-v4-pro" ||
        execution.providerRequestIdPresent !== true,
    )
  ) {
    throw new Error("Edit evidence does not prove real deepseek-v4-pro Provider execution");
  }

  const afterState = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/runtime-state`,
  );
  if (
    !afterState.currentVersionId ||
    afterState.currentVersionId === beforeState.currentVersionId
  ) {
    throw new Error("Edit completed without promoting a new Version");
  }

  const docsArtifact = await readDocsArtifact();
  const releaseEvidence = await readReleaseEvidence();
  result = {
    schemaVersion: "mintlify-docs-real-provider-application@1",
    status: "accepted",
    startedAt,
    finishedAt: new Date().toISOString(),
    projectId,
    designSource: {
      localFile: path.resolve(designFile),
      artifactId: sourceArtifact.id,
      sha256: designSha256,
      sizeBytes: sourceArtifact.sizeBytes,
      integrity: "verified",
    },
    designProfile: {
      id: designProfileId,
      fromVersion: alreadyIntegrated ? Math.max(1, beforeProfile.version - 1) : beforeProfile.version,
      toVersion: updatedProfile.version,
      name: updatedProfile.name,
      sourceKind: updatedProfile.source.kind,
      fidelity,
    },
    edit: {
      baseVersionId: beforeState.currentVersionId,
      versionId: afterState.currentVersionId,
      prompt,
      run,
      docsArtifact,
      releaseEvidence,
    },
    providerVerified: true,
    secretMaterialPersisted: false,
  };
} catch (error) {
  result = {
    schemaVersion: "mintlify-docs-real-provider-application@1",
    status: "failed",
    startedAt,
    finishedAt: new Date().toISOString(),
    projectId,
    designProfileId,
    designSha256,
    runId,
    error: { name: error?.name || "Error", message: String(error?.message || error) },
    secretMaterialPersisted: false,
  };
} finally {
  await releaseSandbox();
}

fs.writeFileSync(
  path.join(evidenceDirectory, "summary.json"),
  `${JSON.stringify(result, null, 2)}\n`,
  { mode: 0o600 },
);
const finalDirectory = evidenceDirectory.replace(
  /-running$/,
  result.status === "accepted" ? "-accepted" : "-failed",
);
fs.renameSync(evidenceDirectory, finalDirectory);
process.stdout.write(`${JSON.stringify({ ...result, evidenceDirectory: finalDirectory })}\n`);
if (result.status !== "accepted") process.exitCode = 1;

function buildMintlifyProfile(current, sourceArtifact, importedAt) {
  const profile = structuredClone(current);
  for (const field of ["id", "name", "version", "createdAt", "updatedAt"]) {
    delete profile[field];
  }
  profile.schemaVersion = "design-profile@2";
  profile.status = "active";
  profile.scope = { projectId };
  profile.source = {
    kind: "imported",
    sourceArtifactIds: [sourceArtifact.id],
    primarySourceArtifactId: sourceArtifact.id,
    sourceHash: sourceArtifact.sha256,
    converterVersion: "mintlify-design-md-docs@2",
    importedAt,
    integrity: "verified",
    notes:
      "Mintlify DESIGN.md mapped to the existing project Profile. The source is light-first; a semantic dark adaptation is included for the product's theme toggle.",
  };
  profile.product = {
    name: "Agent Cloud Quickstart Documentation",
    category: "developer documentation",
    primaryUseCases: [
      "enterprise AI agent quickstart",
      "runtime architecture reference",
      "operational developer guidance",
    ],
    audience: ["developers", "platform engineers", "enterprise AI teams"],
  };
  profile.brand = {
    tone: ["clear", "precise", "confident", "restrained"],
    personality: "technical calm with editorial discipline",
    messaging: {
      headlineStyle: "short, strong, sentence-case headings",
      bodyStyle: "concise technical explanations with generous reading rhythm",
      ctaStyle: "ink-black actions; mint is reserved for links and active state",
      proofStyle: "code, architecture and operational evidence",
    },
  };
  profile.visual = {
    direction:
      "Mintlify monochrome paper documentation with a single functional mint accent and a coherent semantic dark counterpart",
    principles: [
      "Use true white, ink black and mist gray as the dominant light surfaces.",
      "Use #0c8c5e only for links, icons, focus and active navigation; never as a large fill.",
      "Use Inter for display, body and interface text; weight creates hierarchy.",
      "Use 4px controls, 16px cards and 24px only for genuinely large containers.",
      "Dark mode must replace every light semantic surface; it must never retain paper cards or low-contrast light overrides.",
    ],
    moodKeywords: ["monochrome", "editorial", "precise", "calm", "developer-first"],
    avoidKeywords: [
      "pill controls",
      "multi-color accents",
      "heavy shadows",
      "off-white page canvas",
      "gradient decoration",
      "light cards in dark mode",
    ],
    composition: {
      maxWidth: "1200px",
      pageGutter: "32px desktop, 20px mobile",
      sectionGap: "80px",
      articleMeasure: "readable and content-first",
    },
    imagery: { style: "minimal product diagrams and code; no decorative stock imagery" },
    motion: { style: "subtle state transitions only", duration: "150-200ms" },
  };
  profile.tokens = {
    color: {
      light: {
        background: "#ffffff",
        surface: "#ffffff",
        surfaceStrong: "#f2f2f2",
        ink: "#08090a",
        text: "#000000",
        muted: "#5f6368",
        border: "#dddddd",
        accent: "#0c8c5e",
      },
      dark: {
        background: "#0b0d0c",
        surface: "#111513",
        surfaceStrong: "#181e1b",
        text: "#f5f7f6",
        muted: "#a7b0ab",
        border: "#2a332f",
        accent: "#31c48d",
      },
      accentPolicy:
        "Mint is functional only: links, icons, focus rings and active navigation. Primary buttons stay ink black in light and near-white in dark.",
    },
    typography: {
      family: "Inter, ui-sans-serif, system-ui, -apple-system, sans-serif",
      displayFamily: "Inter, ui-sans-serif, system-ui, -apple-system, sans-serif",
      codeFamily: "Inter, ui-monospace, SFMono-Regular, monospace",
      weights: { regular: 400, medium: 500, semibold: 600 },
      sizes: { caption: "13px", body: "16px", h3: "20px", h2: "24px", h1: "40px" },
      tracking: "-0.01em",
    },
    radius: { control: "4px", card: "16px", largeContainer: "24px", pill: "forbidden" },
    shadow: {
      soft: "0 2px 4px rgba(0,0,0,0.03)",
      dark: "0 2px 8px rgba(0,0,0,0.24)",
    },
    spacing: { unit: "8px", cardPadding: "24px", sectionGap: "80px" },
  };
  profile.runtimeTokenMapping = {
    "color.background": "#ffffff",
    "color.surface": "#ffffff",
    "color.surfaceStrong": "#f2f2f2",
    "color.text": "#000000",
    "color.muted": "#5f6368",
    "color.primary": "#0c8c5e",
    "color.primaryContrast": "#ffffff",
    "color.border": "#dddddd",
    "radius.card": "16px",
    "radius.control": "4px",
    "font.sans": "Inter, ui-sans-serif, system-ui, -apple-system, sans-serif",
    "shadow.soft": "0 2px 4px rgba(0,0,0,0.03)",
  };
  profile.extendedTokenMapping = {
    "font.display": "Inter, ui-sans-serif, system-ui, -apple-system, sans-serif",
    "font.mono": "Inter, ui-monospace, SFMono-Regular, monospace",
    "type.display.letterSpacing": "-0.01em",
    "type.body.letterSpacing": "-0.01em",
    "spacing.pageGutter": "32px",
    "spacing.section": "80px",
    "radius.input": "4px",
    "radius.badge": "4px",
  };
  profile.components = {
    primitives: {
      button: {
        role: "navigation and explicit actions",
        usage: ["4px radius", "ink fill for primary", "mint focus ring"],
        avoid: ["mint-filled primary buttons", "pill geometry"],
      },
      card: {
        role: "group related documentation choices",
        usage: ["16px radius", "1px quiet border", "24px padding", "3% shadow maximum"],
        avoid: ["floating glass", "heavy shadow", "light surface in dark mode"],
      },
      sidebarItem: {
        role: "documentation navigation",
        usage: ["16px", "6px vertical rhythm", "active mint text and faint mint background"],
        avoid: ["pill active state", "filled mint rail"],
      },
      search: {
        role: "documentation discovery",
        usage: ["4px radius", "semantic surface", "visible keyboard focus"],
        avoid: ["unconditional white background"],
      },
      codeBlock: {
        role: "technical examples",
        usage: ["high contrast in both themes", "subtle border", "copy action remains visible"],
        avoid: ["paper background in dark mode", "low-contrast comments"],
      },
      table: {
        role: "structured reference",
        usage: ["quiet row borders", "semantic header surface", "horizontal overflow on mobile"],
        avoid: ["fixed light backgrounds"],
      },
    },
    patterns: {
      docsShell: {
        role: "three-column Fumadocs documentation shell",
        requiredElements: ["top navigation", "left sidebar", "article", "table of contents"],
      },
      darkTheme: {
        role: "complete semantic counterpart of the light theme",
        requiredElements: ["dark canvas", "dark surfaces", "readable code", "mint active state"],
      },
    },
  };
  profile.websiteContext = {
    enforcementMode: "enforced",
    craftPacks: ["accessibility-baseline", "responsive-layout", "anti-generic-ui"],
  };
  profile.content = {
    voice: "concise developer documentation",
    docs: {
      preserve: ["all existing Chinese content", "routes", "navigation order", "code samples"],
      hierarchy: "clear h1-h3 scale using Inter 600/500",
      requiredDataHooks: ["theme toggle", "active sidebar item", "code copy action"],
    },
  };
  profile.accessibility = {
    standard: "WCAG 2.2 AA",
    contrast: "4.5:1 for body text in light and dark modes",
    keyboard: "all controls and navigation remain keyboard reachable",
    focus: "2px mint focus indicator with sufficient offset",
    reducedMotion: "honor prefers-reduced-motion",
  };
  profile.technical = {
    allowedTemplates: ["fumadocs-docs"],
    preferredTemplates: { docs: "fumadocs-docs" },
    responsiveBreakpoints: { mobile: "<768px", desktop: ">=1024px" },
    themeStrategy: "class-based .dark semantic tokens; no unconditional light surface overrides",
  };
  profile.governance = {
    conflictBehavior: "prefer-user",
    requireVisualReview: true,
    conflictPolicy: "preserve documentation semantics and accessibility; surface unresolved conflicts",
    changeScope: "style-only; content and information architecture are immutable",
  };
  profile.signatureRules = [
    {
      id: "mint-source-contract",
      statement: "The generated token layer contains the required Mintlify light and dark semantic tokens.",
      category: "governance",
      priority: "required",
      appliesTo: "all",
      verification: {
        kind: "source-pattern",
        paths: ["project/app/tokens.css"],
        pattern:
          "(?s)--runtime-bg:\\s*#ffffff;.*--runtime-primary:\\s*#0c8c5e;.*--runtime-radius-card:\\s*16px;.*--runtime-radius-control:\\s*4px;.*\\.dark\\s*\\{.*--runtime-bg:\\s*#0b0d0c;.*--runtime-primary:\\s*#31c48d;",
      },
    },
    tokenRule("mint-background", "The light Docs canvas is true paper white.", "color.background", "#ffffff", "exact", "preferred"),
    tokenRule("mint-text", "The light Docs text is true black.", "color.text", "#000000", "exact", "preferred"),
    tokenRule("mint-accent", "The only functional light accent is Mint Green #0c8c5e.", "color.primary", "#0c8c5e", "exact", "preferred"),
    tokenRule("mint-border", "Light borders use Cloud Gray #dddddd.", "color.border", "#dddddd", "exact", "preferred"),
    tokenRule("mint-card-radius", "Documentation cards use a 16px radius.", "radius.card", "16px", "exact", "preferred"),
    tokenRule("mint-control-radius", "Controls use a 4px radius and never pill geometry.", "radius.control", "4px", "exact", "preferred"),
    tokenRule("mint-inter", "Inter is the primary Docs type family.", "font.sans", "Inter", "contains", "preferred"),
    {
      id: "mint-docs-route",
      statement: "The styled documentation remains available at /docs/.",
      priority: "preferred",
      appliesTo: "all",
      verification: { kind: "dom", route: "/docs/", selector: "main", minMatches: 1 },
    },
    {
      id: "mint-dark-semantic",
      statement: "Dark mode uses dark semantic surfaces throughout and never leaves white cards, sidebars, code blocks or tables.",
      priority: "preferred",
      appliesTo: "all",
      verification: {
        kind: "visual-review",
        rubric:
          "Toggle dark mode on /docs/. Canvas, header, sidebar, cards, code, tables, search and TOC must be coherent dark surfaces with readable text and mint active/focus states.",
      },
    },
    {
      id: "mint-accent-discipline",
      statement: "Mint appears only on links, icons, focus and active states; actions remain monochrome.",
      priority: "preferred",
      appliesTo: "all",
      verification: {
        kind: "visual-review",
        rubric:
          "Review /docs/ in both themes. Mint must be a sparse functional accent, never a large surface or primary button fill.",
      },
    },
  ];
  profile.overrides = {
    surfaces: {
      docs: {
        visual: {
          direction:
            "Mintlify monochrome Docs shell: paper white light mode plus a fully semantic near-black dark mode",
        },
      },
    },
    templates: {
      "fumadocs-docs": {
        technical: {
          themeStrategy:
            "Map both :root and .dark Fumadocs variables, then style actual rendered selectors rather than invented class names.",
        },
      },
    },
  };
  return profile;
}

function tokenRule(
  id,
  statement,
  token,
  expected,
  comparator = "exact",
  priority = "required",
) {
  return {
    id,
    statement,
    priority,
    appliesTo: "all",
    verification: {
      kind: "token",
      token,
      expected,
      comparator: { kind: comparator },
    },
  };
}

function buildEditPrompt(profile, sourceArtifact) {
  return `Apply the currently bound Design Profile ${profile.id}@${profile.version} to the existing Fumadocs site as a strict style-only edit. The immutable source artifact is ${sourceArtifact.id} with SHA-256 ${sourceArtifact.sha256}.

Use the real Design Capsule and source-derived tokens. This is an execution request, not an exploration request. The source root is project/. Read only inputs/design.md, project/app/tokens.css and project/app/global.css. Do not call project.inspect, style.update_tokens, fs.list, or read any other file: this restored Edit workspace has no state/style-contract.json. Immediately use fs.write to replace project/app/tokens.css with the Profile token layer and project/app/global.css with the explicit dark mapping plus the actual Fumadocs selectors already present there. Then call preview.publish and, when it succeeds, call run.complete immediately. Use a bounded repair only if validation returns a concrete failure.

Required light mode outcome:
- true white #ffffff canvas and surfaces, black #000000 text, #dddddd quiet borders;
- Inter throughout, 600/500/400 hierarchy, tight -0.01em tracking;
- #0c8c5e only for links, icons, keyboard focus and active navigation;
- 4px controls, 16px cards, 24px only on genuinely large containers;
- use compact 4px control geometry, flat surfaces, very soft elevation and only one accent hue.

Required dark mode outcome:
- implement explicit .dark semantic tokens: canvas #0b0d0c, surface #111513, strong surface #181e1b, text #f5f7f6, muted #a7b0ab, border #2a332f, accent #31c48d;
- remove or override every unconditional light background previously applied to the header, sidebar, cards, code blocks, tables, search, TOC and article chrome;
- preserve readable code and controls, visible focus, and WCAG AA contrast;
- keep primary actions monochrome; dark mint remains a sparse functional accent.

Do not edit any MDX, layout, navigation or content source. Only project/app/tokens.css and project/app/global.css may change. Publish as soon as those two style sources are updated; after preview.publish succeeds, do not inspect anything else—call run.complete.`;
}

async function readDocsArtifact() {
  const response = await signedFetch(
    `/artifacts/${encodeURIComponent(projectId)}/current/docs/`,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`Docs Artifact returned ${response.status}: ${body.slice(0, 500)}`);
  }
  if (!body.includes("10 分钟完成首个企业智能体")) {
    throw new Error("edited Docs Artifact lost the required quickstart headline");
  }
  return {
    route: "/docs/",
    httpStatus: response.status,
    requiredHeadlineFound: true,
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
  return JSON.parse(body);
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
    // Best-effort cleanup. Primary failure remains in the evidence summary.
  }
}

async function signedJson(target, options = {}) {
  const response = await signedFetch(target, options);
  const body = await response.text();
  if (!response.ok) {
    throw new Error(
      `${options.method || "GET"} ${target} returned ${response.status}: ${body.slice(0, 1000)}`,
    );
  }
  return body ? JSON.parse(body) : {};
}

async function signedFetch(target, options = {}) {
  return fetch(new URL(target, baseUrl), {
    method: options.method || "GET",
    headers: {
      authorization: `Bearer ${issuePrincipalToken()}`,
      ...(options.body ? { "content-type": "application/json" } : {}),
      ...(options.headers || {}),
    },
    body: options.body ? JSON.stringify(options.body) : undefined,
    signal: AbortSignal.timeout(options.timeoutMs || 120_000),
  });
}

async function readRunEvents(editRunId, destination) {
  const response = await fetch(
    new URL(`/runs/${encodeURIComponent(editRunId)}/events`, baseUrl),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken()}` },
      signal: AbortSignal.timeout(1_200_000),
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
  let completionNudgeSent = false;
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
        if (
          !completionNudgeSent &&
          event.type === "run.workflow_progress" &&
          event.stage === "ready_to_complete"
        ) {
          completionNudgeSent = true;
          try {
            await signedJson(`/runs/${encodeURIComponent(editRunId)}/continue`, {
              method: "POST",
              body: {
                userMessage:
                  "候选已经 ready_to_complete。现在不要读取、搜索、列目录或修改任何文件；只调用 run.complete 完成本次 Run。",
              },
            });
          } catch (error) {
            events.push({
              type: "orchestrator.completion_nudge_nonfatal",
              runId: editRunId,
              error: String(error?.message || error),
              timestamp: new Date().toISOString(),
            });
          }
        }
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
      format: "ndjson",
      eventCount: events.length,
      bytes: bytes.byteLength,
      sha256: sha256(bytes),
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
    }),
    { inputTokens: 0, outputTokens: 0, cachedInputTokens: 0 },
  );
  usage.totalTokens = usage.inputTokens + usage.outputTokens;
  return {
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
    operations: ["preview.read", "project.read", "project.write"],
  });
  const input = `${header}.${payload}`;
  return `${input}.${crypto.sign(null, Buffer.from(input), privateKey).toString("base64url")}`;
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}
