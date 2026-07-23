#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { generationContextProtocol, validateGenerationContextRunEvidence } from "./generation-context-evidence.mjs";
import { replayEvidence } from "../../../infra/generation-reliability/replay-evidence.mjs";
import {
  enforceReleaseBudgetPolicy,
  validateReleaseBudgetPolicy,
} from "../../../infra/generation-reliability/runtime-release-budget-policy.mjs";
import { evaluateRuntimeEfficiencyBenchmark } from "../../../infra/generation-reliability/runtime-efficiency-benchmark.mjs";
import {
  assembleBenchmarkCohort,
  evaluateBenchmarkLedger,
} from "../../../infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs";
import { validateRuntimeEfficiencyBenchmarkSourceBinding } from "../../../infra/generation-reliability/collect-runtime-efficiency-benchmark.mjs";

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    if (!key?.startsWith("--") || argv[index + 1] === undefined) throw new Error(`invalid argument: ${key ?? "<missing>"}`);
    args[key.slice(2)] = argv[index + 1];
  }
  for (const required of ["runtime", "preflight", "channel", "npm", "recovery", "lock", "out", "mode"]) {
    if (!args[required]) throw new Error(`--${required} is required`);
  }
  if (!new Set(["audit", "release"]).has(args.mode)) throw new Error("--mode must be audit or release");
  if (args.mode === "release" && !args["provider-cache"]) {
    throw new Error("--provider-cache is required in release mode");
  }
  if (args.mode === "release" && !args["terminal-bundles"]) {
    throw new Error("--terminal-bundles is required in release mode");
  }
  if (args.mode === "release" && !args["budget-policy"]) {
    throw new Error("--budget-policy is required in release mode");
  }
  if (args.mode === "release" && !args["efficiency-benchmark-ledger"]) {
    throw new Error("--efficiency-benchmark-ledger is required in release mode");
  }
  if (args.mode === "release" && !args["efficiency-source-ledger"]) {
    throw new Error("--efficiency-source-ledger is required in release mode");
  }
  if (args.mode === "release" && !args["efficiency-import-mapping"]) {
    throw new Error("--efficiency-import-mapping is required in release mode");
  }
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

function dcpStagePassed(stage, label) {
  if (generationContextProtocol(stage) === "generation-context") {
    if (validateGenerationContextRunEvidence(stage, label).length > 0) return false;
  } else if (stage?.package?.effectiveCompatibilityMode !== "enforced"
    || !Array.isArray(stage?.missingRequiredReads)
    || stage.missingRequiredReads.length !== 0) {
    return false;
  }
  return stage?.gate === "ready"
    && stage?.materialization?.ready === true
    && stage?.styleContract?.verified === true
    && ["computed-style", "a11y", "viewport"].every(
      capability => stage?.verification?.capabilities?.[capability]?.available === true,
    );
}

function stageContextHash(stage) {
  return generationContextProtocol(stage) === "generation-context"
    ? stage?.generationContext?.contextContentHash
    : stage?.package?.contentHash;
}

export function enforcedDcpLifecyclePassed(fixture) {
  const dcp = fixture?.designContextEnforced;
  const lifecycle = dcp?.lifecycle;
  const stages = [dcp?.build, dcp?.edit, dcp?.repair];
  const runIds = [lifecycle?.buildRunId, lifecycle?.editRunId, lifecycle?.reviewRunId, lifecycle?.repairRunId];
  if (runIds.some(value => typeof value !== "string" || value.length === 0) || new Set(runIds).size !== 4) return false;
  if (lifecycle?.findingStatus !== "fixed" || !lifecycle?.findingId || !lifecycle?.candidateVersionId) return false;
  if (stages.some((stage, index) => !dcpStagePassed(stage, `designContextEnforced.stage${index + 1}`))) return false;
  if (dcp.build.runId !== lifecycle.buildRunId || dcp.edit.runId !== lifecycle.editRunId || dcp.repair.runId !== lifecycle.repairRunId) return false;
  const protocols = stages.map(generationContextProtocol);
  if (new Set(protocols).size !== 1) return false;
  if (protocols[0] === "legacy" && new Set(stages.map(stageContextHash)).size !== 1) return false;
  if (protocols[0] === "generation-context"
    && (new Set(stages.map(stage => stage.templateVersion)).size !== 1
      || new Set(stages.map(stage => stage.generationContext.runContextBindingHash)).size !== stages.length)) return false;
  if (new Set(stages.map(stage => stage.materialization.hash)).size !== 1) return false;
  const repairFinding = fixture?.reviewRepair?.findings?.find(item => item?.findingId === lifecycle.findingId);
  return fixture?.reviewRepair?.reviewRunId === lifecycle.reviewRunId
    && fixture?.reviewRepair?.repairRunId === lifecycle.repairRunId
    && repairFinding?.versionId === lifecycle.candidateVersionId
    && repairFinding?.severity === "blocking"
    && repairFinding?.repairable === true
    && repairFinding?.status === "fixed";
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
    visualReview: project.visualReview,
    publication: project.publication,
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
    artifactAccessAfterRelease: project.artifactAccessAfterRelease,
    reviewRepair: project.reviewRepair,
    designContextEnforced: project.designContextEnforced,
  };
}

function artifactAvailableAfterRelease(project) {
  return project?.artifactHttpStatusAfterRelease === 200
    && project?.artifactAccessAfterRelease?.authentication === "project-principal"
    && project?.artifactAccessAfterRelease?.authenticated === true
    && project?.artifactAccessAfterRelease?.projectId === project?.projectId
    && project?.artifactAccessAfterRelease?.httpStatus === 200;
}

function providerDcpProjectPassed(project) {
  const provenance = project?.designContextEnforced?.reviewProvenance;
  return project?.kind === "website"
    && enforcedDcpLifecyclePassed(project)
    && project?.artifactAssertions?.content?.matched === true
    && project?.artifactAssertions?.computedStyle?.passed === true
    && project?.artifactAssertions?.route === "/"
    && project?.cancelCleanup?.passed === true
    && project?.dependencyEvidence?.passed === true
    && project?.terminalToolFailureCount === 0
    && artifactAvailableAfterRelease(project)
    && provenance?.source === "fixture-seeded"
    && provenance?.mutationProvider === "deepseek"
    && provenance?.reviewProvider === "deterministic-tool-sequence"
    && provenance?.repairProvider === "deepseek";
}

export function providerCacheSmokePassed(evidence, provider, repositoryCommit) {
  const runs = Array.isArray(evidence?.runs) ? evidence.runs : [];
  const stableRuns = runs.filter((run) =>
    run?.compositionValid === true
    && run?.metricsValid === true
    && run?.providerIdentityValid === true
    && run?.buildIdentityValid === true
    && run?.generationContextIdentityValid === true
    && run?.redactionValid === true
    && run?.repeatedStableTurns >= 2);
  const grossInputTokens = stableRuns.reduce(
    (total, run) => total + Number(run.grossInputTokens || 0),
    0,
  );
  const cachedInputTokens = stableRuns.reduce(
    (total, run) => total + Number(run.cachedInputTokens || 0),
    0,
  );
  return evidence?.schemaVersion === "provider-cache-smoke-audit@1"
    && evidence?.toolSetHashVersion === "tool-definition-set@1"
    && evidence?.status === "passed"
    && evidence?.releaseEligible === true
    && evidence?.stableRunCount > 0
    && evidence?.cachedInputTokens > 0
    && evidence?.invalidRunCount === 0
    && evidence?.auditedRunCount === runs.length
    && evidence?.stableRunCount === stableRuns.length
    && evidence?.grossInputTokens === grossInputTokens
    && evidence?.cachedInputTokens === cachedInputTokens
    && evidence?.sourceDirty === false
    && evidence?.sourceCommit === repositoryCommit
    && evidence?.modelResourceId === provider?.modelResourceId
    && evidence?.providerResourceRevision === provider?.providerResourceRevision
    && evidence?.providerConfigSha256 === provider?.providerConfigSha256;
}

export function normalizeProviderEvidence(input) {
  const provider = input?.provider && typeof input.provider === "object"
    ? input.provider
    : input ?? {};
  const realProviderVerified =
    input?.schemaVersion === "generation-real-provider-suite-evidence@2"
    && input?.status === "accepted"
    && provider.realProviderVerified === true;
  return {
    mode: provider.mode ?? (realProviderVerified ? "real" : "fixture"),
    model: provider.model ?? provider.modelResourceId ?? "unknown",
    modelResourceId: provider.modelResourceId ?? provider.model ?? "unknown",
    providerResourceRevision:
      provider.providerResourceRevision ?? input?.provenance?.providerResourceRevision ?? null,
    providerConfigSha256:
      provider.providerConfigSha256 ?? input?.provenance?.providerConfigSha256 ?? null,
    // A suite that proves real Provider execution is stronger evidence than
    // recording whether a credential happened to be configured.
    credentialPresent: provider.credentialPresent === true || realProviderVerified,
  };
}

const REQUIRED_TERMINAL_KINDS = new Set([
  "website",
  "docs",
  "edit",
  "repair",
  "runtime_restart",
]);

export function terminalBundleSetPassed(
  index,
  provider,
  repositoryCommit,
  providerCacheSmokeSha256,
  releaseBudgetPolicySha256,
) {
  const entries = Array.isArray(index?.entries) ? index.entries : [];
  const kinds = new Set(entries.map((entry) => entry.kind));
  const website = entries.find((entry) => entry.kind === "website");
  const docs = entries.find((entry) => entry.kind === "docs");
  return index?.schemaVersion === "runtime-terminal-bundle-index@1"
    && index?.status === "passed"
    && /^[a-f0-9]{64}$/.test(index?.setSha256 || "")
    && index?.budgetPolicyStatus === "passed"
    && index?.budgetPolicySha256 === releaseBudgetPolicySha256
    && Number.isSafeInteger(index?.filesScanned)
    && index.filesScanned > 0
    && index?.replayedCount === entries.length
    && entries.length >= REQUIRED_TERMINAL_KINDS.size
    && new Set(entries.map((entry) => entry.evidenceId)).size === entries.length
    && [...REQUIRED_TERMINAL_KINDS].every((kind) => kinds.has(kind))
    && website?.entryRoute === "/"
    && docs?.entryRoute === "/docs/"
    && website?.runId !== docs?.runId
    && website?.projectId !== docs?.projectId
    && entries.every((entry) =>
      entry.replayStatus === "passed"
      && entry.resultStatus === "accepted"
      && entry.bundleKind === (entry.kind === "runtime_restart"
        ? "runtime_restart_terminal"
        : "real_provider_terminal")
      && entry.gitSha === repositoryCommit
      && entry.modelResourceId === provider?.modelResourceId
      && entry.modelResourceRevision === provider?.providerResourceRevision
      && entry.providerConfigSha256 === provider?.providerConfigSha256
      && entry.providerCacheSourceAuditSha256 === providerCacheSmokeSha256
      && typeof entry.budgetProfileId === "string"
      && entry.budgetProfileId.length > 0
      && /^[a-f0-9]{64}$/.test(entry.budgetProfileHash || "")
      && /^[a-f0-9]{64}$/.test(entry.budgetProfilesSha256 || "")
      && /^[a-f0-9]{64}$/.test(entry.runModelUsageSha256 || "")
      && entry.budgetConformanceStatus === "passed"
      && entry.releaseBudgetPolicyStatus === "passed"
      && Number.isSafeInteger(entry.budgetProfileCount)
      && entry.budgetProfileCount > 0
      && Number.isSafeInteger(entry.maxTurnInputTokens)
      && entry.maxTurnInputTokens > 0
      && /^[a-f0-9]{64}$/.test(entry.checksumsSha256 || "")
      && /^[a-f0-9]{64}$/.test(entry.streamSha256 || ""));
}

export function efficiencyBenchmarkPassed(benchmark, provider, repositoryCommit) {
  const cohort = benchmark?.cohort;
  const evaluation = benchmark?.evaluation;
  const sourceLedger = benchmark?.sourceLedger;
  const sourceBinding = benchmark?.sourceBinding;
  if (!cohort || !evaluation || !sourceLedger || !sourceBinding) return false;
  const recomputed = evaluateRuntimeEfficiencyBenchmark(cohort);
  const profiles = Array.isArray(cohort.profiles) ? cohort.profiles : [];
  const workloads = new Set(profiles.map(profile => profile.workload));
  return sourceLedger?.schemaVersion === "runtime-efficiency-benchmark-ledger-verification@1"
    && sourceLedger?.status === "passed"
    && /^[a-f0-9]{64}$/.test(sourceLedger?.ledgerSha256 ?? "")
    && sourceLedger.ledgerSha256 === cohort?.ledger?.sha256
    && sourceBinding.schemaVersion === "runtime-efficiency-benchmark-source-binding@1"
    && sourceBinding.status === "passed"
    && /^[a-f0-9]{64}$/.test(sourceBinding.pairedLedgerSha256 ?? "")
    && /^[a-f0-9]{64}$/.test(sourceBinding.pairedLedgerHeadRecordHash ?? "")
    && /^[a-f0-9]{64}$/.test(sourceBinding.mappingSha256 ?? "")
    && sourceBinding.attemptCount === sourceLedger.attemptCount
    && sourceBinding.source?.commit === repositoryCommit
    && sourceBinding.source?.dirty === false
    && cohort?.source?.commit === repositoryCommit
    && cohort?.source?.dirty === false
    && evaluation?.result === "pass"
    && recomputed.result === "pass"
    && JSON.stringify(evaluation) === JSON.stringify(recomputed)
    && workloads.has("greenfield_build")
    && workloads.has("style_token_edit")
    && profiles.every(profile =>
      profile.modelResourceId === provider?.modelResourceId
      && profile.providerResourceRevision === provider?.providerResourceRevision
      && profile.providerParametersHash === provider?.providerConfigSha256);
}

export async function loadTerminalBundleIndex(setFile, budgetPolicyFile = null) {
  const raw = await readFile(setFile, "utf8");
  const set = JSON.parse(raw);
  if (set?.schemaVersion !== "runtime-terminal-bundle-set@1"
    || !Array.isArray(set.bundles)
    || set.bundles.length === 0
    || set.bundles.some((bundle) => typeof bundle !== "string" || bundle.length === 0)) {
    throw new Error("terminal bundle set must be runtime-terminal-bundle-set@1 with bundle paths");
  }
  const base = path.dirname(path.resolve(setFile));
  const budgetPolicyRaw = budgetPolicyFile === null
    ? null
    : await readFile(path.resolve(budgetPolicyFile), "utf8");
  const budgetPolicy = budgetPolicyRaw === null ? null : JSON.parse(budgetPolicyRaw);
  if (budgetPolicy !== null) validateReleaseBudgetPolicy(budgetPolicy);
  const entries = [];
  let filesScanned = 0;
  for (const bundlePath of set.bundles) {
    const directory = path.resolve(base, bundlePath);
    const replay = await replayEvidence(directory);
    if (replay.status !== "passed") throw new Error(`terminal bundle replay failed: ${bundlePath}`);
    const [manifestRaw, caseRaw, checksumsRaw, budgetProfilesRaw] = await Promise.all([
      readFile(path.join(directory, "manifest.json"), "utf8"),
      readFile(path.join(directory, "case-summary.json"), "utf8"),
      readFile(path.join(directory, "checksums.sha256")),
      readFile(path.join(directory, "budget-profiles.json"), "utf8"),
    ]);
    const manifest = JSON.parse(manifestRaw);
    const caseSummary = JSON.parse(caseRaw);
    const budgetProfiles = JSON.parse(budgetProfilesRaw);
    const budgetPolicyResult = budgetPolicy === null
      ? null
      : enforceReleaseBudgetPolicy(budgetPolicy, budgetProfiles);
    const routeIdentity = JSON.parse(
      await readFile(path.join(directory, "artifact-route-identity.json"), "utf8"),
    );
    const cacheSummary = JSON.parse(
      await readFile(path.join(directory, "provider-cache-summary.json"), "utf8"),
    );
    const checksumFiles = checksumsRaw.toString("utf8").trim().split(/\r?\n/)
      .filter(Boolean)
      .map((line) => /^([a-f0-9]{64})  ([A-Za-z0-9._-]+)$/.exec(line)?.[2])
      .filter(Boolean);
    const bundleRawFiles = await Promise.all(checksumFiles.map(async (file) => ({
      file: path.join(directory, file),
      text: await readFile(path.join(directory, file), "utf8"),
    })));
    const bundleScan = secretScan(bundleRawFiles);
    if (bundleScan.matches.length > 0) {
      throw new Error(`terminal bundle contains credential-like material: ${bundlePath}`);
    }
    filesScanned += bundleRawFiles.length;
    if (caseSummary.runId !== manifest.runId
      || caseSummary.projectId !== manifest.projectId
      || caseSummary.operationId !== manifest.operationId
      || caseSummary.kind !== manifest.scenarioKind) {
      throw new Error(`terminal bundle summary identity mismatch: ${bundlePath}`);
    }
    entries.push({
      evidenceId: manifest.evidenceId,
      bundleKind: manifest.bundleKind,
      kind: manifest.scenarioKind,
      entryRoute: routeIdentity.entryRoute,
      projectId: manifest.projectId,
      runId: manifest.runId,
      operationId: manifest.operationId,
      gitSha: manifest.gitSha,
      modelResourceId: manifest.modelResourceId,
      modelResourceRevision: manifest.modelResourceRevision,
      providerConfigSha256: manifest.providerConfigSha256,
      providerCacheSourceAuditSha256: cacheSummary.sourceAuditSha256,
      budgetProfileId: manifest.budgetProfileId,
      budgetProfileHash: manifest.budgetProfileHash,
      budgetProfilesSha256: manifest.budgetProfilesSha256,
      runModelUsageSha256: manifest.runModelUsageSha256,
      budgetProfiles,
      budgetProfileCount: budgetProfiles.profiles?.length ?? 0,
      budgetConformanceStatus: replay.replay?.budgetConformance?.status,
      releaseBudgetPolicyStatus: budgetPolicyResult?.status ?? "not_evaluated",
      maxTurnInputTokens: replay.replay?.usage?.maxTurnInputTokens,
      streamSha256: manifest.streamSha256,
      checksumsSha256: sha256(checksumsRaw),
      resultStatus: manifest.result?.status,
      replayStatus: replay.status,
    });
  }
  return {
    schemaVersion: "runtime-terminal-bundle-index@1",
    status: "passed",
    setSha256: sha256(raw),
    budgetPolicyStatus: budgetPolicy === null ? "not_evaluated" : "passed",
    budgetPolicySha256: budgetPolicyRaw === null ? null : sha256(budgetPolicyRaw),
    filesScanned,
    replayedCount: entries.length,
    entries,
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
  const inputFiles = [args.runtime, args.preflight, args.channel, args.npm, args.recovery, args.lock];
  if (args.provider) inputFiles.push(args.provider);
  if (args["provider-cache"]) inputFiles.push(args["provider-cache"]);
  if (args["terminal-bundles"]) inputFiles.push(args["terminal-bundles"]);
  if (args["budget-policy"]) inputFiles.push(args["budget-policy"]);
  if (args["efficiency-benchmark-ledger"]) inputFiles.push(args["efficiency-benchmark-ledger"]);
  if (args["efficiency-source-ledger"]) inputFiles.push(args["efficiency-source-ledger"]);
  if (args["efficiency-import-mapping"]) inputFiles.push(args["efficiency-import-mapping"]);
  const rawFiles = await Promise.all(inputFiles.map(async file => ({ file, text: await readFile(file, "utf8") })));
  const [runtime, preflight, channel, npm, recovery, lock] = await Promise.all([
    json(args.runtime), json(args.preflight), json(args.channel), json(args.npm), json(args.recovery), json(args.lock),
  ]);
  const provider = normalizeProviderEvidence(
    args.provider ? await json(args.provider) : runtime.provider,
  );
  const providerCacheSmoke = args["provider-cache"]
    ? await json(args["provider-cache"])
    : null;
  const providerCacheSmokeSha256 = args["provider-cache"]
    ? sha256(rawFiles.find((item) => item.file === args["provider-cache"]).text)
    : null;
  const terminalBundles = args["terminal-bundles"]
    ? await loadTerminalBundleIndex(args["terminal-bundles"], args["budget-policy"] ?? null)
    : null;
  const releaseBudgetPolicySha256 = args["budget-policy"]
    ? sha256(rawFiles.find((item) => item.file === args["budget-policy"]).text)
    : null;
  const releaseBudgetPolicy = args["budget-policy"]
    ? await json(args["budget-policy"])
    : null;
  const efficiencyBenchmark = args["efficiency-benchmark-ledger"]
    ? {
        cohort: assembleBenchmarkCohort(args["efficiency-benchmark-ledger"]),
        ...evaluateBenchmarkLedger(args["efficiency-benchmark-ledger"]),
        sourceBinding: args["efficiency-source-ledger"] && args["efficiency-import-mapping"]
          ? validateRuntimeEfficiencyBenchmarkSourceBinding(
              args["efficiency-source-ledger"],
              args["efficiency-benchmark-ledger"],
              await json(args["efficiency-import-mapping"]),
            )
          : null,
      }
    : null;
  const repository = runtime.repository ?? {};
  const releaseCandidate = args.mode === "release"
    && repository.dirty === false
    && provider?.mode === "real"
    && preflight?.schemaVersion === "runtime-rc-preflight@1"
    && preflight?.passed === true
    && preflight?.prefetchImages === true;
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
    provider,
    providerCacheSmoke,
    providerCacheSmokeSha256,
    terminalBundles,
    releaseBudgetPolicySha256,
    releaseBudgetPolicy,
    efficiencyBenchmark,
    preflight,
    fixture: {
      deployed: runtime.fixture?.execution?.passed === true,
      concurrent: runtime.fixture?.execution?.mode === "concurrent",
      projects: (runtime.fixture?.projects ?? []).map(normalizeProject),
    },
    enforcedDcpFixture: runtime.enforcedDcpFixture ?? null,
    providerDcpProject: runtime.providerDcpProject ? normalizeProject(runtime.providerDcpProject) : null,
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
  const auditPassed = evidence.fixture.deployed
    && evidence.fixture.concurrent
    && enforcedDcpLifecyclePassed(evidence.enforcedDcpFixture)
    && (provider?.mode === "fixture" || providerDcpProjectPassed(evidence.providerDcpProject))
    && (provider?.mode === "fixture"
      || providerCacheSmokePassed(providerCacheSmoke, evidence.provider, evidence.repository.commit))
    && (args.mode !== "release"
      || terminalBundleSetPassed(
        terminalBundles,
        evidence.provider,
        evidence.repository.commit,
        evidence.providerCacheSmokeSha256,
        evidence.releaseBudgetPolicySha256,
      ))
    && (args.mode !== "release"
      || efficiencyBenchmarkPassed(
        efficiencyBenchmark,
        evidence.provider,
        evidence.repository.commit,
      ))
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
      && project.artifactAssertions?.route === (
        project.kind === "docs" && provider?.mode !== "fixture" ? "/docs/" : "/"
      )
      && project.cancelCleanup?.passed === true
      && project.dependencyEvidence?.passed === true
      && project.terminalToolFailureCount === 0
      && artifactAvailableAfterRelease(project))
    && scan.matches.length === 0;
  const releaseEligible = releaseCandidate
    && evidence.preflight.lockHash === evidence.repository.lockHash
    && evidence.preflight.entries?.length === Object.keys(lock.images || {}).length
    && evidence.preflight.entries.every(entry => entry.lockedDigestVerified === true
      && entry.mutableTagMatchesLock === true
      && entry.pulled === true)
    && auditPassed;
  evidence.auditPassed = auditPassed;
  evidence.releaseEligible = releaseEligible;
  evidence.result = auditPassed ? "pass" : "fail";
  await writeFile(args.out, `${JSON.stringify(evidence, null, 2)}\n`);
  process.stdout.write(`Release evidence aggregated: ${args.out} (eligible=${releaseEligible})\n`);
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  await main();
}
