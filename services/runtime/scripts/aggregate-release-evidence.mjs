#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    if (!key?.startsWith("--") || argv[index + 1] === undefined) throw new Error(`invalid argument: ${key ?? "<missing>"}`);
    args[key.slice(2)] = argv[index + 1];
  }
  for (const required of ["runtime", "channel", "npm", "recovery", "lock", "out", "mode"]) {
    if (!args[required]) throw new Error(`--${required} is required`);
  }
  if (!new Set(["audit", "release"]).has(args.mode)) throw new Error("--mode must be audit or release");
  return args;
}

async function json(file) {
  return JSON.parse(await readFile(file, "utf8"));
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function digestRef(image) {
  return `${image.ref}@${image.digest}`;
}

function normalizeProject(project) {
  const screenshot = project.screenshot ?? {};
  return {
    kind: project.kind,
    projectId: project.projectId,
    buildRunId: project.buildRunId,
    editRunId: project.editRunId,
    bindingId: project.bindingId ?? project.sandboxBindingId,
    podUid: project.podUid,
    buildId: project.buildId,
    candidateManifestHash: project.candidateManifestHash,
    sourceSnapshotUri: project.sourceSnapshotUri,
    previewLeaseId: project.previewLeaseId,
    screenshotId: project.screenshotId ?? screenshot.screenshotId,
    nonblankPixelRatio: project.nonblankPixelRatio ?? screenshot.nonblankPixelRatio,
    versionBeforeCas: project.versionBeforeCas ?? project.currentVersionBeforeCas ?? null,
    versionAfterCas: project.versionAfterCas ?? project.currentVersionAfterCas,
    artifactManifestHash: project.artifactManifestHash,
    artifactUrl: project.artifactUrl,
    artifactAssertions: project.artifactAssertions,
    cancelCleanup: project.cancelCleanup,
    dependencyEvidence: project.dependencyEvidence,
    events: project.events,
    recoverableToolFailureCount: project.recoverableToolFailureCount ?? 0,
    terminalToolFailureCount: project.terminalToolFailureCount ?? 0,
    sandboxReleasedAt: project.sandboxReleasedAt,
    artifactHttpStatusAfterRelease: project.artifactHttpStatusAfterRelease,
  };
}

function secretScan(rawFiles) {
  const patterns = [
    /\b(?:sk|api)[-_][A-Za-z0-9]{16,}\b/g,
    /authorization\s*[:=]\s*(?:bearer\s+)?\S+/gi,
    /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g,
    /(?:client[_-]?key|private[_-]?key)\s*[:=]\s*\S+/gi,
    /eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g,
  ];
  const matches = [];
  for (const { file, text } of rawFiles) {
    for (const pattern of patterns) {
      pattern.lastIndex = 0;
      if (pattern.test(text)) matches.push({ file: path.basename(file), pattern: pattern.source });
    }
  }
  return { patternSet: "runtime-credentials@1", filesScanned: rawFiles.length, matches };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const inputFiles = [args.runtime, args.channel, args.npm, args.recovery, args.lock];
  if (args.provider) inputFiles.push(args.provider);
  const rawFiles = await Promise.all(inputFiles.map(async file => ({ file, text: await readFile(file, "utf8") })));
  const [runtime, channel, npm, recovery, lock] = await Promise.all([
    json(args.runtime), json(args.channel), json(args.npm), json(args.recovery), json(args.lock),
  ]);
  const provider = args.provider ? await json(args.provider) : runtime.provider;
  const repository = runtime.repository ?? {};
  const releaseCandidate = args.mode === "release"
    && repository.dirty === false
    && provider?.mode === "approved-real"
    && Boolean(provider?.approvalReference);
  const sandboxRef = channel.sandbox?.imageRef ?? runtime.images?.sandbox?.ref ?? "";
  const sandboxDigest = channel.sandbox?.imageId ?? runtime.images?.sandbox?.configDigest ?? "";
  const projects = (runtime.projects ?? []).map(normalizeProject);
  const scan = secretScan(rawFiles);
  const evidence = {
    schemaVersion: "release-evidence@1",
    releaseEligible: false,
    recordedAt: new Date().toISOString(),
    repository: {
      commit: repository.commit ?? runtime.runtimeVersion?.repositoryCommit ?? "",
      dirty: repository.dirty ?? true,
      lockHash: repository.lockHash ?? sha256(rawFiles.find(item => item.file === args.lock).text),
    },
    cluster: runtime.cluster ?? {
      name: channel.cluster?.name ?? channel.cluster ?? "",
      kubeContext: channel.cluster?.kubeContext ?? channel.kubeContext ?? "",
      createdAt: "",
      nodeUid: "",
    },
    images: {
      runtime: runtime.images?.runtime ?? {
        ref: runtime.runtimeImage ?? "",
        configDigest: runtime.runtimeImageId ?? "",
        manifestDigest: runtime.runtimeManifestDigest ?? "",
        reportedCommit: runtime.runtimeVersion?.repositoryCommit ?? "",
      },
      sandbox: { ref: sandboxRef, configDigest: sandboxDigest },
      controller: digestRef(lock.images.agentSandboxController),
      npmProxy: digestRef(lock.images.npmProxy),
      dockerfileFrontend: digestRef(lock.images.dockerfileFrontend),
    },
    transport: runtime.transport ?? {
      mode: "mtls",
      mtlsVerified: channel.checks?.authenticatedWorkspaceChannel === true,
      runtimeSanHash: sha256("spiffe://zerondesign.dev/runtime/anydesign-runtime"),
      sandboxSanHash: sha256("spiffe://zerondesign.dev/sandbox/anydesign-sandboxes"),
    },
    auth: runtime.auth ?? {
      principalMode: "disabled",
      projectOwnershipVerified: false,
      channelJwtVerified: channel.checks?.authenticatedWorkspaceChannel === true,
    },
    provider: {
      mode: provider?.mode ?? "fixture",
      model: provider?.model ?? "unknown",
      approvalReference: provider?.approvalReference ?? args["approval-reference"] ?? null,
      credentialPresent: provider?.credentialPresent === true,
    },
    fixture: {
      deployed: runtime.fixture?.execution?.passed === true,
      concurrent: runtime.fixture?.execution?.mode === "concurrent",
      projects: (runtime.fixture?.projects ?? []).map(normalizeProject),
    },
    projects,
    recoveryScenarios: Array.isArray(recovery.scenarios) ? recovery.scenarios : [],
    networkChecks: {
      directRegistryDenied: npm.checks?.directNpmjsDenied === true,
      npmProxyInstallPassed: npm.checks?.proxyReachable === true
        && npm.checks?.accessLogObserved === true
        && npm.checks?.runtimeInstallObserved === true
        && npm.checks?.lockfilesPresent === true
        && npm.checks?.projectIsolation === true
        && npm.checks?.upstreamFailureTyped === "infrastructure.registry_unavailable",
    },
    secretScan: scan,
    result: "fail",
  };
  const projectKinds = new Set(evidence.projects.map(project => project.kind));
  const fixtureKinds = new Set(evidence.fixture.projects.map(project => project.kind));
  const requiredRecovery = new Set([
    "runtime-restart", "port-forward-kill", "sandbox-pod-replacement",
    "channel-lease-pod-uid-change", "checkpoint-runtime-restart",
    "artifact-staged-before-cas", "cas-before-event", "run-cancel",
  ]);
  const releaseEligible = releaseCandidate
    && evidence.fixture.deployed
    && evidence.fixture.concurrent
    && fixtureKinds.has("website")
    && fixtureKinds.has("docs")
    && projectKinds.has("website")
    && projectKinds.has("docs")
    && evidence.transport.mtlsVerified === true
    && evidence.transport.rotationWindowVerified === true
    && evidence.auth.principalMode === "required"
    && evidence.auth.projectOwnershipVerified === true
    && evidence.auth.channelJwtVerified === true
    && evidence.networkChecks.directRegistryDenied === true
    && evidence.networkChecks.npmProxyInstallPassed === true
    && /^sha256:[a-f0-9]{64}$/.test(evidence.images.runtime.manifestDigest ?? "")
    && [...requiredRecovery].every(scenario => evidence.recoveryScenarios.some(item => item.scenario === scenario && item.result === "pass" && item.orphanCount === 0))
    && evidence.projects.every(project => project.artifactAssertions?.content?.matched === true
      && project.artifactAssertions?.computedStyle?.passed === true
      && project.cancelCleanup?.passed === true
      && project.dependencyEvidence?.passed === true
      && project.terminalToolFailureCount === 0
      && project.artifactHttpStatusAfterRelease === 200)
    && scan.matches.length === 0;
  evidence.releaseEligible = releaseEligible;
  evidence.result = releaseEligible ? "pass" : "fail";
  await writeFile(args.out, `${JSON.stringify(evidence, null, 2)}\n`);
  process.stdout.write(`Release evidence aggregated: ${args.out} (eligible=${releaseEligible})\n`);
}

await main();
