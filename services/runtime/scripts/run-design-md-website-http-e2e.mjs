#!/usr/bin/env node
import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createWriteStream } from "node:fs";
import path from "node:path";

const rootDir = process.cwd();
const timestamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\..+/, "").replace("T", "-");
const evidenceDir = process.env.RUNTIME_E2E_LOG_DIR || path.join(rootDir, ".runtime-evidence", `design-md-website-http-${timestamp}`);
const designPath = process.env.DESIGN_MD_PATH || "/Users/carlos/Downloads/DESIGN (1).md";
const projectId = process.env.RUNTIME_E2E_PROJECT_ID || `authkit-design-website-api-${Date.now()}`;
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
const streamLog = createWriteStream(path.join(evidenceDir, "http-stream.log"), { flags: "a" });

await writeFile(path.join(evidenceDir, "run-metadata.env"), [
  `timestampUtc=${new Date().toISOString()}`,
  `projectId=${projectId}`,
  `runtimeBaseUrl=${baseUrl}`,
  `designPath=${designPath}`,
  `deepseekApiKeyPresent=true`,
  `deepseekModel=${process.env.DEEPSEEK_MODEL || process.env.AGENT_MODEL || "deepseek-chat"}`,
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

  const briefRun = await postJson(`${baseUrl}/runs`, {
    projectId,
    phase: "brief",
    agentProfile: "brief",
    inputContext: {
      contentSources: [
        {
          id: "design-md-authkit",
          kind: "design_md",
          text: designMd,
        },
        {
          id: "website-generation-prompt",
          kind: "prompt",
          text: [
            "Generate a single-page Astro website for a fictional authentication product called AuthKit.",
            "Use the attached design markdown as the visual source of truth.",
            "The page content can be invented, but it must visibly follow the midnight frosted-glass cathedral direction.",
            "The build must produce a promoted artifact whose visible text includes AuthKit.",
            "Use #05060f as the midnight canvas and set the runtime primary token to #663af3.",
          ].join(" "),
        },
      ],
    },
  });
  logLine("HTTP_RUN_START", { stage: "brief", runId: briefRun.runId });
  await pollRunEvents(baseUrl, briefRun.runId, "brief", (events) =>
    events.some((event) => event.type === "state.changed" && String(event.state).includes("needs_user_input"))
      || events.some((event) => event.type === "run.completed")
  );

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
    },
  });
  logLine("HTTP_RUN_START", { stage: "build", runId: buildRun.runId, briefId });
  const buildEvents = await pollRunEvents(baseUrl, buildRun.runId, "build", (events) =>
    events.some((event) => event.type === "run.completed")
  , 540_000);
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
    artifactContainsAuthKit: artifactHtml.includes("AuthKit"),
    artifactMentionsGlass: /glass|frost/i.test(artifactHtml),
    artifactMentionsMidnight: /midnight|05060f/i.test(artifactHtml),
    runtimeStateHasStyleContract: Boolean(runtimeState.styleContract),
    previewPromoted: currentPreview.status === "promoted",
  };
  await writeFile(path.join(evidenceDir, "checks.json"), JSON.stringify(checks, null, 2));
  if (!Object.values(checks).every(Boolean)) {
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
    "#663af3",
  ], path.join(evidenceDir, "computed-style.log"));

  const summary = {
    ok: true,
    projectId,
    baseUrl,
    artifactUrl,
    streamLog: path.join(evidenceDir, "http-stream.log"),
    runtimeLog: path.join(evidenceDir, "runtime-server.log"),
    runtimeStatePath: path.join(evidenceDir, "runtime-state.json"),
    currentPreviewPath: path.join(evidenceDir, "current-preview.json"),
    checks,
    computedStyleExitCode: computed.status,
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
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${url} failed ${response.status}: ${text}`);
  }
  return JSON.parse(text);
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
  let events = [];
  while (Date.now() < deadline) {
    events = await readRunEvents(`${baseUrl}/runs/${runId}/events`);
    for (let i = 0; i < events.length; i += 1) {
      const key = `${runId}/${i + 1}`;
      if (seen.has(key)) continue;
      seen.add(key);
      logLine("HTTP_EVENT", { stage, sequence: i + 1, event: events[i] });
    }
    if (done(events)) return events;
    await sleep(1500);
  }
  throw new Error(`run ${runId} timed out in stage ${stage}`);
}

async function readRunEvents(url) {
  const response = await fetch(url);
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`${url} failed ${response.status}: ${text}`);
  }
  return text
    .split("\n")
    .filter((line) => line.startsWith("data:"))
    .map((line) => JSON.parse(line.slice("data:".length).trim()));
}

function logLine(label, value) {
  streamLog.write(`${label} ${JSON.stringify(value)}\n`);
}

async function runNode(args, logPath) {
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
      if (status !== 0) {
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
