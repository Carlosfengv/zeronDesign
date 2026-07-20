#!/usr/bin/env node
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createServer } from "node:net";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createWriteStream } from "node:fs";
import path from "node:path";

const rootDir = process.cwd();
const timestamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\..+/, "").replace("T", "-");
const baselinePreset = process.env.DESIGN_BASELINE_PRESET || "authkit";
const baseline = baselineConfig(baselinePreset);
const fidelityVariant = (process.env.DESIGN_FIDELITY_VARIANT || "A").toUpperCase();
const recordFidelityFailures = process.env.DESIGN_FIDELITY_RECORD_FAILURES === "1";
if (!["A", "B", "C"].includes(fidelityVariant)) {
  throw new Error(`Unsupported DESIGN_FIDELITY_VARIANT: ${fidelityVariant}`);
}
const evidenceDir = process.env.RUNTIME_E2E_LOG_DIR || path.join(rootDir, ".runtime-evidence", `design-${baselinePreset}-${fidelityVariant}-website-http-${timestamp}`);
const designPath = process.env.DESIGN_MD_PATH || baseline.designPath;
const projectId = process.env.RUNTIME_E2E_PROJECT_ID || `${baselinePreset}-design-website-api-${Date.now()}`;
const npmRegistry = process.env.RUNTIME_NPM_REGISTRY || "https://registry.npmjs.org/";

if (!process.env.DEEPSEEK_API_KEY) {
  throw new Error("DEEPSEEK_API_KEY is required");
}

await mkdir(evidenceDir, { recursive: true });
const workspaceRoot = path.join(evidenceDir, "workspace");
const storageDir = path.join(evidenceDir, "storage");
await mkdir(workspaceRoot, { recursive: true });
await mkdir(storageDir, { recursive: true });

const port = Number(process.env.RUNTIME_PORT || await getFreePort());
const baseUrl = `http://127.0.0.1:${port}`;
const designMd = await readFile(designPath, "utf8");
const designSourceHash = createHash("sha256").update(designMd, "utf8").digest("hex");
const generationPrompt = process.env.DESIGN_GENERATION_PROMPT || baseline.prompt;
const internalAdminToken = process.env.RUNTIME_INTERNAL_ADMIN_TOKEN || `design-e2e-${Date.now()}`;
const streamLog = createWriteStream(path.join(evidenceDir, "http-stream.log"), { flags: "a" });

await writeFile(path.join(evidenceDir, "run-metadata.env"), [
  `timestampUtc=${new Date().toISOString()}`,
  `projectId=${projectId}`,
  `runtimeBaseUrl=${baseUrl}`,
  `baselinePreset=${baselinePreset}`,
  `fidelityVariant=${fidelityVariant}`,
  `designPath=${designPath}`,
  `designSourceHash=${designSourceHash}`,
  `expectedVisibleText=${baseline.expectedVisibleText}`,
  `expectedPrimaryToken=${baseline.expectedPrimaryToken}`,
  `deepseekApiKeyPresent=true`,
  `deepseekModel=${process.env.DEEPSEEK_MODEL || process.env.AGENT_MODEL || "deepseek-chat"}`,
  `deepseekProviderReportedModel=${process.env.DEEPSEEK_RESOLVED_MODEL || "unknown"}`,
  `npmRegistry=${npmRegistry}`,
  `workspaceRoot=${workspaceRoot}`,
  "",
].join("\n"));

const runtimeLog = createWriteStream(path.join(evidenceDir, "runtime-server.log"), { flags: "a" });
const runtime = spawn("cargo", ["run", "--manifest-path", "services/runtime/Cargo.toml"], {
  cwd: rootDir,
  env: {
    ...process.env,
    RUNTIME_HOST: "127.0.0.1",
    RUNTIME_PORT: String(port),
    RUNTIME_WORKSPACE_ROOT: workspaceRoot,
    RUNTIME_STORAGE_DIR: storageDir,
    SANDBOX_BACKEND_MODE: "phase_a_contract",
    RUNTIME_POLICY_PROFILE: "local-e2e",
    MODEL_PROVIDER: "deepseek",
    DEEPSEEK_BASE_URL: process.env.DEEPSEEK_BASE_URL || "https://api.deepseek.com",
    DEEPSEEK_MODEL: process.env.DEEPSEEK_MODEL || process.env.AGENT_MODEL || "deepseek-chat",
    RUNTIME_NPM_REGISTRY: npmRegistry,
    MODEL_STREAMING: process.env.MODEL_STREAMING || "1",
    RUNTIME_INTERNAL_ADMIN_TOKEN: internalAdminToken,
  },
  stdio: ["ignore", "pipe", "pipe"],
});
runtime.stdout.pipe(runtimeLog);
runtime.stderr.pipe(runtimeLog);

let shuttingDown = false;
const shutdown = () => {
  if (shuttingDown) return;
  shuttingDown = true;
  runtime.kill("SIGTERM");
};
process.on("SIGINT", () => {
  shutdown();
  process.exit(130);
});
process.on("SIGTERM", () => {
  shutdown();
  process.exit(143);
});

try {
  await waitForHealth(`${baseUrl}/health`, 120_000);

  const designProfileId = fidelityVariant === "A"
    ? undefined
    : await importAndActivateDesignProfile({
        baseUrl,
        internalAdminToken,
        baseline,
        designMd,
        designSourceHash,
        projectId,
      });

  const briefRun = await postJson(`${baseUrl}/runs`, {
    projectId,
    phase: "brief",
    agentProfile: "brief",
    inputContext: {
      contentSources: [
        ...(fidelityVariant === "A" ? [{
          id: `design-md-${baselinePreset}`,
          kind: "design_md",
          text: designMd,
        }] : []),
        {
          id: "website-generation-prompt",
          kind: "prompt",
          text: generationPrompt,
        },
      ],
      ...(designProfileId ? { designProfileId } : {}),
    },
  });
  logLine("HTTP_RUN_START", { stage: "brief", runId: briefRun.runId });
  const briefEvents = await pollRunEvents(baseUrl, briefRun.runId, "brief", (events) =>
    events.some((event) => event.type === "state.changed" && String(event.state).includes("needs_user_input"))
      || events.some((event) => event.type === "run.completed")
  );
  const prematureBriefCompletion = briefEvents.find((event) => event.type === "run.completed");
  if (prematureBriefCompletion) {
    throw new Error(
      `brief run terminated before confirmation: ${JSON.stringify(prematureBriefCompletion)}`,
    );
  }

  await postJson(`${baseUrl}/runs/${briefRun.runId}/continue`, {
    userMessage: "确认这个 brief，可以开始生成 website",
  });
  await pollRunEvents(baseUrl, briefRun.runId, "brief-confirm", (events) =>
    events.some((event) => event.type === "run.completed")
  );

  const conversation = await getJson(`${baseUrl}/projects/${projectId}/conversation?includeDebug=true`);
  const briefId = conversation.items
    .map((item) => item?.metadata?.briefId)
    .filter(Boolean)
    .at(-1);
  if (!briefId) {
    throw new Error("briefId not found in project conversation after confirmation");
  }

  const buildRun = await postJson(`${baseUrl}/runs`, {
    projectId,
    phase: "build",
    agentProfile: "build",
    inputContext: {
      briefId,
      ...(designProfileId ? {
        designProfileId,
        designFidelityMode: fidelityVariant === "B" ? "profile_only" : "source_fallback",
      } : {}),
    },
  });
  logLine("HTTP_RUN_START", { stage: "build", runId: buildRun.runId, briefId });
  const buildEvents = await pollRunEvents(baseUrl, buildRun.runId, "build", (events) =>
    events.some((event) => event.type === "run.completed")
      || events.some((event) => event.type === "state.changed" && String(event.state).includes("needs_user_input"))
  , 540_000);
  const blockedBuild = buildEvents.find(
    (event) => event.type === "state.changed" && String(event.state).includes("needs_user_input"),
  );
  if (blockedBuild) {
    throw new Error(`build run requires input: ${JSON.stringify(blockedBuild)}`);
  }
  const completed = buildEvents.find((event) => event.type === "run.completed");
  if (!completed || completed.status !== "completed") {
    throw new Error(`build run did not complete successfully: ${JSON.stringify(completed)}`);
  }

  const runtimeState = await getJson(`${baseUrl}/projects/${projectId}/runtime-state`);
  const currentPreview = await getJson(`${baseUrl}/preview/${projectId}/current`);
  const artifactUrl = `${baseUrl}/artifacts/${projectId}/current`;
  const artifactHtml = await getText(artifactUrl);
  await writeFile(path.join(evidenceDir, "artifact.html"), artifactHtml);
  await writeFile(path.join(evidenceDir, "runtime-state.json"), JSON.stringify(runtimeState, null, 2));
  await writeFile(path.join(evidenceDir, "current-preview.json"), JSON.stringify(currentPreview, null, 2));

  const checks = {
    artifactContainsExpectedText: artifactHtml.includes(baseline.expectedVisibleText),
    artifactContainsStyleLanguageHint: baseline.stylePatterns.every((pattern) => pattern.test(artifactHtml)),
    runtimeStateHasStyleContract: Boolean(runtimeState.styleContract),
    previewPromoted: currentPreview.status === "promoted",
  };
  await writeFile(path.join(evidenceDir, "checks.json"), JSON.stringify(checks, null, 2));
  const blockingChecks = [
    checks.artifactContainsExpectedText,
    checks.runtimeStateHasStyleContract,
    checks.previewPromoted,
  ];
  if (!blockingChecks.every(Boolean)) {
    throw new Error(`artifact checks failed: ${JSON.stringify(checks)}`);
  }

  const computed = await runNode([
    "services/runtime/scripts/verify-computed-style.mjs",
    "--url",
    artifactUrl,
    "--selector",
    ":root",
    "--property",
    "--runtime-primary",
    "--expected",
    baseline.expectedPrimaryToken,
  ], path.join(evidenceDir, "computed-style.log"), { allowFailure: recordFidelityFailures });
  const fidelity = await runNode([
    "services/runtime/scripts/verify-design-profile-fidelity.mjs",
    "--url",
    artifactUrl,
    "--preset",
    baselinePreset,
  ], path.join(evidenceDir, "fidelity-assertions.log"), { allowFailure: recordFidelityFailures });

  const summary = {
    ok: fidelity.status === 0,
    baselinePreset,
    fidelityVariant,
    designProfileId,
    projectId,
    designPath,
    designSourceHash,
    generationPrompt,
    model: process.env.DEEPSEEK_MODEL || process.env.AGENT_MODEL || "deepseek-chat",
    providerReportedModel: process.env.DEEPSEEK_RESOLVED_MODEL || null,
    template: "next-app",
    baseUrl,
    artifactUrl,
    streamLog: path.join(evidenceDir, "http-stream.log"),
    runtimeLog: path.join(evidenceDir, "runtime-server.log"),
    runtimeStatePath: path.join(evidenceDir, "runtime-state.json"),
    currentPreviewPath: path.join(evidenceDir, "current-preview.json"),
    checks,
    computedStyleExitCode: computed.status,
    fidelityExitCode: fidelity.status,
  };
  await writeFile(path.join(evidenceDir, "summary.json"), JSON.stringify(summary, null, 2));
  logLine("HTTP_E2E_SUMMARY", summary);
  console.log(JSON.stringify(summary, null, 2));
} finally {
  streamLog.end();
  shutdown();
}

async function getFreePort() {
  return await new Promise((resolve, reject) => {
    const server = createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => resolve(address.port));
    });
  });
}

function baselineConfig(preset) {
  if (preset === "authkit") {
    return {
      designPath: "/Users/carlos/Downloads/DESIGN (1).md",
      fixturePath: "services/runtime/fixtures/design-profiles/authkit-v2.json",
      profileName: "AuthKit Frosted Glass Cathedral V2",
      expectedVisibleText: "AuthKit",
      expectedPrimaryToken: "#663af3",
      stylePatterns: [/glass|frost/i, /midnight|05060f/i],
      prompt: [
        "Generate a single-page Next.js website for a fictional authentication product called AuthKit.",
        "Use the attached design markdown as the visual source of truth.",
        "The page content can be invented, but it must visibly follow the midnight frosted-glass cathedral direction.",
        "The build must produce a promoted artifact whose visible text includes AuthKit.",
        "Use #05060f as the midnight canvas and set the runtime primary token to #663af3.",
      ].join(" "),
    };
  }

  if (preset === "elevenlabs") {
    return {
      designPath: "/Users/carlos/Downloads/DESIGN (2).md",
      fixturePath: "services/runtime/fixtures/design-profiles/elevenlabs-v2.json",
      profileName: "ElevenLabs Warm Editorial V2",
      expectedVisibleText: "Voxora",
      expectedPrimaryToken: "#000000",
      stylePatterns: [/warm|editorial|taupe|fdfcfc/i, /voice|audio|sphere/i],
      prompt: [
        "Generate a single-page Next.js website for a fictional AI voice platform called Voxora.",
        "Use the attached design markdown as the visual source of truth.",
        "The page must visibly follow the warm editorial paper direction and show concrete voice or audio product evidence.",
        "The build must produce a promoted artifact whose visible text includes Voxora.",
        "Use #fdfcfc as the canvas and set the runtime primary token to #000000.",
      ].join(" "),
    };
  }

  throw new Error(`Unsupported DESIGN_BASELINE_PRESET: ${preset}`);
}

async function waitForHealth(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // keep polling while cargo builds and the server starts
    }
    await sleep(1000);
  }
  throw new Error(`runtime did not become healthy: ${url}`);
}

async function postJson(url, body) {
  return await requestJson(url, "POST", body);
}

async function requestJson(url, method, body, headers = {}) {
  const response = await fetch(url, {
    method,
    headers: { "content-type": "application/json", ...headers },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${url} failed ${response.status}: ${text}`);
  }
  return JSON.parse(text);
}

async function importAndActivateDesignProfile({
  baseUrl,
  internalAdminToken,
  baseline,
  designMd,
  designSourceHash,
  projectId,
}) {
  const serviceHeaders = {
    "x-anydesign-internal": "true",
    "x-runtime-admin-token": internalAdminToken,
  };
  const sourceResponse = await requestJson(
    `${baseUrl}/design-source-artifacts`,
    "POST",
    {
      scope: { projectId },
      fileName: path.basename(baseline.designPath),
      mediaType: "text/markdown",
      contentBase64: Buffer.from(designMd, "utf8").toString("base64"),
      clientSha256: designSourceHash,
    },
    serviceHeaders,
  );
  const imported = await requestJson(
    `${baseUrl}/design-profiles/import`,
    "POST",
    {
      name: baseline.profileName,
      scope: { projectId },
      sourceArtifactId: sourceResponse.artifact.id,
    },
    serviceHeaders,
  );
  const fixture = JSON.parse(
    await readFile(path.join(rootDir, baseline.fixturePath), "utf8"),
  );
  if (fixture.source.sourceHash !== designSourceHash) {
    throw new Error(
      `fixture source hash mismatch for ${baseline.profileName}: ${fixture.source.sourceHash} != ${designSourceHash}`,
    );
  }
  const {
    id: _id,
    name: _name,
    status: _status,
    version: _version,
    scope: _scope,
    source: _source,
    createdAt: _createdAt,
    updatedAt: _updatedAt,
    ...candidate
  } = fixture;
  const updated = await requestJson(
    `${baseUrl}/design-profiles/${encodeURIComponent(imported.designProfileDraft.id)}`,
    "PUT",
    {
      expectedVersion: imported.designProfileDraft.version,
      name: baseline.profileName,
      profile: candidate,
    },
  );
  const activated = await requestJson(
    `${baseUrl}/design-profiles/${encodeURIComponent(imported.designProfileDraft.id)}/activate`,
    "POST",
    { expectedVersion: updated.designProfile.version },
    serviceHeaders,
  );
  await postJson(`${baseUrl}/projects/${encodeURIComponent(projectId)}/design-profile`, {
    designProfileId: activated.designProfile.id,
  });
  return activated.designProfile.id;
}

async function getJson(url) {
  const response = await fetch(url);
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${url} failed ${response.status}: ${text}`);
  }
  return JSON.parse(text);
}

async function getText(url) {
  const response = await fetch(url);
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${url} failed ${response.status}: ${text}`);
  }
  return text;
}

async function pollRunEvents(baseUrl, runId, stage, done, timeoutMs = 420_000) {
  const deadline = Date.now() + timeoutMs;
  const seen = new Set();
  const events = [];
  let lastEventId;
  while (Date.now() < deadline) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), Math.max(1, deadline - Date.now()));
    try {
      const response = await fetch(`${baseUrl}/runs/${runId}/events`, {
        headers: lastEventId ? { "last-event-id": lastEventId } : {},
        signal: controller.signal,
      });
      if (!response.ok) {
        throw new Error(
          `${baseUrl}/runs/${runId}/events failed ${response.status}: ${await response.text()}`,
        );
      }
      if (!response.body) throw new Error("SSE response body is not readable");
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      while (Date.now() < deadline) {
        const chunk = await reader.read();
        if (chunk.done) break;
        buffer += decoder.decode(chunk.value, { stream: true }).replaceAll("\r\n", "\n");
        let boundary;
        while ((boundary = buffer.indexOf("\n\n")) >= 0) {
          const block = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          const id = block
            .split("\n")
            .find((line) => line.startsWith("id:"))
            ?.slice(3)
            .trim();
          const data = block
            .split("\n")
            .filter((line) => line.startsWith("data:"))
            .map((line) => line.slice(5).trimStart())
            .join("\n");
          if (!data || (id && seen.has(id))) continue;
          const event = JSON.parse(data);
          if (id) {
            seen.add(id);
            lastEventId = id;
          }
          events.push(event);
          logLine("HTTP_EVENT", {
            stage,
            sequence: events.length,
            eventId: id,
            event,
          });
          if (done(events)) {
            await reader.cancel();
            return events;
          }
        }
      }
    } catch (error) {
      if (error?.name !== "AbortError") throw error;
    } finally {
      clearTimeout(timeout);
    }
  }
  throw new Error(`run ${runId} timed out in stage ${stage}`);
}

function logLine(label, value) {
  streamLog.write(`${label} ${JSON.stringify(value)}\n`);
}

async function runNode(args, logPath, { allowFailure = false } = {}) {
  return await new Promise((resolve, reject) => {
    const log = createWriteStream(logPath, { flags: "a" });
    const child = spawn(process.execPath, args, {
      cwd: rootDir,
      env: {
        ...process.env,
        NODE_PATH: process.env.NODE_PATH || path.join(process.env.HOME || "", ".cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules"),
      },
    });
    child.stdout.pipe(log);
    child.stderr.pipe(log);
    child.on("error", reject);
    child.on("close", (status) => {
      log.end();
      if (status !== 0 && !allowFailure) {
        reject(new Error(`${process.execPath} ${args.join(" ")} failed with exit code ${status}`));
        return;
      }
      resolve({ status });
    });
  });
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
