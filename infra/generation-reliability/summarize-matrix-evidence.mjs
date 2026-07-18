#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || value === undefined) {
      throw new Error(`invalid argument: ${key ?? "<missing>"}`);
    }
    args[key.slice(2)] = value;
  }
  for (const key of ["runtime", "release", "deployment", "out", "mode", "cluster"]) {
    if (!args[key]) throw new Error(`--${key} is required`);
  }
  if (!new Set(["fixture", "real"]).has(args.mode)) {
    throw new Error("--mode must be fixture or real");
  }
  return args;
}

async function readJson(file) {
  return JSON.parse(await readFile(file, "utf8"));
}

async function fileIdentity(file) {
  const bytes = await readFile(file);
  return {
    file: path.basename(file),
    sha256: createHash("sha256").update(bytes).digest("hex"),
    bytes: bytes.length,
  };
}

function requireText(value, label) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${label} is required`);
  }
  return value;
}

function validateProject(project, kind) {
  if (!project || project.kind !== kind) throw new Error(`${kind} project evidence is missing`);
  for (const key of [
    "projectId",
    "buildRunId",
    "editRunId",
    "podUid",
    "candidateManifestHash",
    "sourceSnapshotUri",
    "artifactManifestHash",
    "artifactUrl",
  ]) {
    requireText(project[key], `${kind}.${key}`);
  }
  if (project.buildRunId === project.editRunId) {
    throw new Error(`${kind} Build and Edit run IDs must differ`);
  }
  if (project.artifactAssertions?.content?.matched !== true) {
    throw new Error(`${kind} artifact content assertion failed`);
  }
  if (project.artifactAssertions?.computedStyle?.passed !== true) {
    throw new Error(`${kind} computed-style assertion failed`);
  }
  if (project.cancelCleanup?.passed !== true) {
    throw new Error(`${kind} cancellation cleanup assertion failed`);
  }
  if (project.dependencyEvidence?.passed !== true) {
    throw new Error(`${kind} dependency evidence failed`);
  }
  if (project.events?.sequenceValid !== true) {
    throw new Error(`${kind} event sequence failed`);
  }
  if (project.terminalToolFailureCount !== 0) {
    throw new Error(`${kind} has terminal tool failures`);
  }
  if (project.artifactHttpStatusAfterRelease !== 200) {
    throw new Error(`${kind} artifact is unavailable after Sandbox release`);
  }
  return {
    kind,
    projectId: project.projectId,
    buildRunId: project.buildRunId,
    editRunId: project.editRunId,
    podUid: project.podUid,
    candidateManifestHash: project.candidateManifestHash,
    sourceSnapshotUri: project.sourceSnapshotUri,
    artifactManifestHash: project.artifactManifestHash,
    artifactUrl: project.artifactUrl,
    contentMatched: true,
    computedStylePassed: true,
    cancellationCleanupPassed: true,
    dependencyEvidencePassed: true,
    eventSequenceValid: true,
    artifactAvailableAfterRelease: true,
  };
}

function runtimeBudgets(deployment) {
  const container = deployment?.spec?.template?.spec?.containers?.find(
    (entry) => entry.name === "runtime",
  );
  if (!container) throw new Error("Runtime container is missing from deployment evidence");
  const values = Object.fromEntries(
    (container.env ?? [])
      .filter((entry) => entry.name?.startsWith("RUNTIME_AGENT_MAX_"))
      .map((entry) => [entry.name, entry.value]),
  );
  const required = [
    "RUNTIME_AGENT_MAX_TURNS",
    "RUNTIME_AGENT_MAX_TOOL_CALLS",
    "RUNTIME_AGENT_MAX_INPUT_TOKENS",
    "RUNTIME_AGENT_MAX_OUTPUT_TOKENS",
    "RUNTIME_AGENT_MAX_CONSECUTIVE_PROTOCOL_ERRORS",
  ];
  for (const key of required) requireText(values[key], `deployment.${key}`);
  return values;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const [runtime, release, deployment] = await Promise.all([
    readJson(args.runtime),
    readJson(args.release),
    readJson(args.deployment),
  ]);
  if (runtime.cluster?.name !== args.cluster) {
    throw new Error(`cluster mismatch: expected=${args.cluster} actual=${runtime.cluster?.name}`);
  }
  const projects = [
    validateProject(runtime.projects?.find((item) => item.kind === "website"), "website"),
    validateProject(runtime.projects?.find((item) => item.kind === "docs"), "docs"),
  ];
  if (projects[0].podUid === projects[1].podUid) {
    throw new Error("Website and Docs must use isolated Sandbox Pods");
  }
  if (projects[0].artifactManifestHash === projects[1].artifactManifestHash) {
    throw new Error("Website and Docs artifact manifests must differ");
  }
  const expectedProviderMode = args.mode === "real"
    ? new Set(["real"])
    : new Set(["fixture"]);
  if (!expectedProviderMode.has(runtime.provider?.mode)) {
    throw new Error(
      `provider mode mismatch: matrix=${args.mode} evidence=${runtime.provider?.mode}`,
    );
  }
  if (release.result !== "pass") {
    throw new Error("aggregated release evidence did not pass its audit checks");
  }
  const evidenceFiles = await Promise.all([
    fileIdentity(args.runtime),
    fileIdentity(args.release),
    fileIdentity(args.deployment),
  ]);
  const summary = {
    schemaVersion: "generation-reliability-matrix@1",
    recordedAt: new Date().toISOString(),
    result: "pass",
    mode: args.mode,
    cluster: runtime.cluster,
    repository: runtime.repository,
    runtime: {
      image: runtime.runtimeImage,
      imageId: runtime.runtimeImageId,
      version: runtime.runtimeVersion,
      budgets: runtimeBudgets(deployment),
    },
    provider: {
      mode: runtime.provider.mode,
      model: runtime.provider.model,
      credentialPresent: runtime.provider.credentialPresent === true,
    },
    matrix: {
      surfaces: ["website", "docs"],
      execution: runtime.fixture?.execution ?? null,
      isolated: true,
      projects,
    },
    checks: {
      websitePassed: true,
      docsPassed: true,
      sandboxIsolationPassed: true,
      artifactIsolationPassed: true,
      tokenBudgetsConfigured: true,
      protocolFuseConfigured: true,
      releaseAuditPassed: true,
    },
    evidenceFiles,
  };
  await writeFile(args.out, `${JSON.stringify(summary, null, 2)}\n`);
  process.stdout.write(`${args.out}\n`);
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
