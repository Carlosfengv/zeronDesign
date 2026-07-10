#!/usr/bin/env node
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

if (!process.env.DEEPSEEK_API_KEY) throw new Error("DEEPSEEK_API_KEY is required");

const rootDir = process.cwd();
const requestedModel = process.env.DEEPSEEK_MODEL || process.env.AGENT_MODEL || "deepseek-chat";
const providerReportedModel = await resolveProviderModel(requestedModel);
const repetitions = Number.parseInt(process.env.DESIGN_FIDELITY_REPETITIONS || "3", 10);
if (!Number.isInteger(repetitions) || repetitions < 1) {
  throw new Error("DESIGN_FIDELITY_REPETITIONS must be a positive integer");
}
const timestamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\..+/, "").replace("T", "-");
const matrixDir = process.env.RUNTIME_E2E_LOG_DIR || path.join(rootDir, ".runtime-evidence", `design-fidelity-matrix-${timestamp}`);
await mkdir(matrixDir, { recursive: true });

const runs = [];
for (const preset of ["authkit", "elevenlabs"]) {
  for (const variant of ["A", "B", "C"]) {
    for (let repetition = 1; repetition <= repetitions; repetition += 1) {
      const runDir = path.join(matrixDir, `${preset}-${variant}-${repetition}`);
      await mkdir(runDir, { recursive: true });
      const startedAt = new Date().toISOString();
      const status = await runChild(
        process.execPath,
        ["services/runtime/scripts/run-design-md-website-http-e2e.mjs"],
        runDir,
        {
          ...process.env,
          DESIGN_BASELINE_PRESET: preset,
          DESIGN_FIDELITY_VARIANT: variant,
          DESIGN_FIDELITY_RECORD_FAILURES: "1",
          DEEPSEEK_RESOLVED_MODEL: providerReportedModel,
          RUNTIME_E2E_LOG_DIR: runDir,
        },
      );
      const record = {
        preset,
        variant,
        repetition,
        startedAt,
        completedAt: new Date().toISOString(),
        exitCode: status,
        evidenceDir: runDir,
      };
      try {
        record.summary = JSON.parse(await readFile(path.join(runDir, "summary.json"), "utf8"));
      } catch {
        record.summary = null;
      }
      try {
        const fidelity = JSON.parse(await readFile(path.join(runDir, "fidelity-assertions.log"), "utf8"));
        record.fidelity = fidelity;
        record.score = fidelity.assertions.filter((item) => item.passed).length / fidelity.assertions.length;
      } catch {
        record.fidelity = null;
        record.score = 0;
      }
      try {
        const stream = await readFile(path.join(runDir, "http-stream.log"), "utf8");
        const streamMetrics = analyzeStream(stream);
        record.repairCount = streamMetrics.repairCount;
        record.capabilityGapCount = streamMetrics.capabilityGapCount;
      } catch {
        record.repairCount = 0;
        record.capabilityGapCount = 0;
      }
      runs.push(record);
      await writeFile(path.join(matrixDir, "runs.json"), JSON.stringify(runs, null, 2));
    }
  }
}

const groups = [];
for (const preset of ["authkit", "elevenlabs"]) {
  for (const variant of ["A", "B", "C"]) {
    const groupRuns = runs.filter((run) => run.preset === preset && run.variant === variant);
    const scores = groupRuns.map((run) => run.score).sort((left, right) => left - right);
    groups.push({
      preset,
      variant,
      repetitions: groupRuns.length,
      successfulRuns: groupRuns.filter((run) => run.exitCode === 0).length,
      medianScore: median(scores),
      scores,
      repairCount: groupRuns.reduce((total, run) => total + run.repairCount, 0),
      capabilityGapCount: groupRuns.reduce((total, run) => total + run.capabilityGapCount, 0),
      systemicFailedRuleIds: systemicFailedRuleIds(groupRuns),
    });
  }
}
const operationalOk = runs.every((run) => run.exitCode === 0);
const thresholdChecks = [];
for (const preset of ["authkit", "elevenlabs"]) {
  const a = groups.find((group) => group.preset === preset && group.variant === "A");
  const b = groups.find((group) => group.preset === preset && group.variant === "B");
  const c = groups.find((group) => group.preset === preset && group.variant === "C");
  thresholdChecks.push({
    preset,
    bWithinFivePointsOfA: b.medianScore + 0.05 >= a.medianScore,
    cNotBelowB: c.medianScore >= b.medianScore,
    noSystemicCriticalFailure: [b, c].every((group) => group.systemicFailedRuleIds.length === 0),
  });
}
const acceptanceReady = repetitions >= 3;
const thresholdsPassed = thresholdChecks.every((check) =>
  check.bWithinFivePointsOfA && check.cNotBelowB && check.noSystemicCriticalFailure
);
const report = {
  ok: operationalOk && (!acceptanceReady || thresholdsPassed),
  operationalOk,
  acceptanceReady,
  thresholdsPassed,
  repetitions,
  model: requestedModel,
  providerReportedModel,
  parameters: {
    modelStreaming: process.env.MODEL_STREAMING || "1",
    runtimePolicy: "local-e2e",
    template: "astro-website",
    viewport: "1440x1000",
  },
  groups,
  thresholdChecks,
  runs,
};
await writeFile(path.join(matrixDir, "matrix-summary.json"), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
if (!report.ok) process.exitCode = 1;

function median(values) {
  if (values.length === 0) return 0;
  const middle = Math.floor(values.length / 2);
  return values.length % 2 === 0
    ? (values[middle - 1] + values[middle]) / 2
    : values[middle];
}

function systemicFailedRuleIds(groupRuns) {
  if (groupRuns.length < 3) return [];
  const counts = new Map();
  for (const run of groupRuns) {
    for (const assertion of run.fidelity?.assertions || []) {
      if (!assertion.passed && isCriticalRule(assertion.id)) {
        counts.set(assertion.id, (counts.get(assertion.id) || 0) + 1);
      }
    }
  }
  return [...counts.entries()]
    .filter(([, count]) => count >= 2)
    .map(([id]) => id)
    .sort();
}

function isCriticalRule(id) {
  return /background|primary|display|gradient|tracking|cta|action|accent/i.test(id);
}

function analyzeStream(stream) {
  const events = stream
    .split("\n")
    .filter((line) => line.startsWith("HTTP_EVENT "))
    .flatMap((line) => {
      try {
        return [JSON.parse(line.slice("HTTP_EVENT ".length)).event];
      } catch {
        return [];
      }
    });
  let failedPreviewCycles = 0;
  let failureRecordedForCurrentPreview = false;
  let terminalStatus;
  for (const event of events) {
    if (event.type === "preview.updated") {
      failureRecordedForCurrentPreview = false;
    } else if (
      event.type === "review.finding" &&
      /^DesignProfile rule /.test(event.summary || "") &&
      !failureRecordedForCurrentPreview
    ) {
      failedPreviewCycles += 1;
      failureRecordedForCurrentPreview = true;
    } else if (event.type === "run.completed") {
      terminalStatus = event.status;
    }
  }
  return {
    repairCount: Math.max(0, failedPreviewCycles - (terminalStatus === "partial" ? 1 : 0)),
    capabilityGapCount: events.filter((event) =>
      JSON.stringify(event).includes("design_profile_capability_gap")
    ).length,
  };
}

async function runChild(command, args, logDir, env) {
  return await new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: rootDir, env, stdio: ["ignore", "pipe", "pipe"] });
    const output = [];
    child.stdout.on("data", (chunk) => output.push(chunk));
    child.stderr.on("data", (chunk) => output.push(chunk));
    child.on("error", reject);
    child.on("close", async (status) => {
      await writeFile(path.join(logDir, "matrix-run.log"), Buffer.concat(output));
      resolve(status ?? 1);
    });
  });
}

async function resolveProviderModel(model) {
  const baseUrl = (process.env.DEEPSEEK_BASE_URL || "https://api.deepseek.com").replace(/\/$/, "");
  const endpoint = baseUrl.endsWith("/chat/completions")
    ? baseUrl
    : `${baseUrl}${baseUrl.endsWith("/v1") ? "" : "/v1"}/chat/completions`;
  const response = await fetch(endpoint, {
    method: "POST",
    headers: {
      authorization: `Bearer ${process.env.DEEPSEEK_API_KEY}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      model,
      messages: [{ role: "user", content: "Reply OK." }],
      max_tokens: 2,
      stream: false,
    }),
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(`DeepSeek model probe failed with ${response.status}: ${body.error?.message || "unknown error"}`);
  }
  return typeof body.model === "string" && body.model ? body.model : model;
}
