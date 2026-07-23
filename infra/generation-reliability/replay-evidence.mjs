#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { validateRuntimeRestartEvidence } from "../../services/runtime/scripts/generation-context-runtime-restart-evidence.mjs";
import {
  calculateEvidenceUsage,
  canonicalJson,
  createRunModelUsageEvidence,
  validateBudgetConformance,
} from "./runtime-budget-evidence.mjs";

const REQUIRED_REPLAY_FILES = [
  "manifest.json",
  "events.ndjson",
  "usage.json",
  "run-model-usage.json",
  "artifact-route-manifest.json",
  "validation-summary.json",
];
const REQUIRED_REAL_PROVIDER_TERMINAL_FILES = [
  "manifest.json",
  "events.ndjson",
  "usage.json",
  "run-model-usage.json",
  "budget-profiles.json",
  "case-summary.json",
  "artifact-route-identity.json",
  "validation-summary.json",
  "provider-cache-summary.json",
];
const REQUIRED_RUNTIME_RESTART_TERMINAL_FILES = [
  ...REQUIRED_REAL_PROVIDER_TERMINAL_FILES,
  "runtime-restart-evidence.json",
];
const HASH = /^[a-f0-9]{64}$/;

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function invariant(condition, message) {
  if (!condition) throw new Error(message);
}

function nonEmptyString(value) {
  return typeof value === "string" && value.length > 0;
}

function nonNegativeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function calculateLegacyEvidenceUsage(events) {
  const turnsByNumber = new Map();
  for (const event of events) {
    if (event.type !== "model.usage") continue;
    invariant(nonNegativeInteger(event.turn), "model usage turn is invalid");
    invariant(nonNegativeInteger(event.inputTokens), "model usage inputTokens is invalid");
    invariant(nonNegativeInteger(event.cachedInputTokens), "model usage cachedInputTokens is invalid");
    invariant(event.cachedInputTokens <= event.inputTokens,
      "model usage cachedInputTokens exceeds inputTokens");
    invariant(nonNegativeInteger(event.outputTokens), "model usage outputTokens is invalid");
    turnsByNumber.set(event.turn, {
      turn: event.turn,
      inputTokens: event.inputTokens,
      cachedInputTokens: event.cachedInputTokens,
      uncachedInputTokens: event.inputTokens - event.cachedInputTokens,
      outputTokens: event.outputTokens,
    });
  }
  const turns = [...turnsByNumber.values()].sort((left, right) => left.turn - right.turn);
  const aggregate = turns.reduce(
    (total, turn) => ({
      modelCalls: total.modelCalls + 1,
      inputTokens: total.inputTokens + turn.inputTokens,
      cachedInputTokens: total.cachedInputTokens + turn.cachedInputTokens,
      uncachedInputTokens: total.uncachedInputTokens + turn.uncachedInputTokens,
      outputTokens: total.outputTokens + turn.outputTokens,
    }),
    { modelCalls: 0, inputTokens: 0, cachedInputTokens: 0, uncachedInputTokens: 0, outputTokens: 0 },
  );
  return { schemaVersion: "runtime-evidence-usage@1", turns, aggregate };
}

function calculateLegacyRunModelUsage(manifest, events, usage) {
  const latestByTurn = new Map();
  for (const event of events) {
    if (event.type !== "model.usage") continue;
    invariant(typeof event.estimated === "boolean", "model usage estimated flag is invalid");
    latestByTurn.set(event.turn, event);
  }
  return {
    schemaVersion: "run-model-usage@1",
    runId: manifest.runId,
    modelServiceId: manifest.modelResourceId,
    modelDisplayName: manifest.modelDisplayName ?? manifest.modelResourceId,
    inputTokens: usage.aggregate.inputTokens,
    outputTokens: usage.aggregate.outputTokens,
    cachedInputTokens: usage.aggregate.cachedInputTokens,
    totalTokens: usage.aggregate.inputTokens + usage.aggregate.outputTokens,
    estimated: [...latestByTurn.values()].some((event) => event.estimated),
    turnCount: latestByTurn.size,
  };
}

function parseJson(bytes, label) {
  try {
    return JSON.parse(bytes.toString("utf8"));
  } catch (error) {
    throw new Error(`${label} is not valid JSON: ${error.message}`);
  }
}

function assertRedacted(value, location = "$") {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertRedacted(item, `${location}[${index}]`));
    return;
  }
  if (!value || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    invariant(
      !["authorization", "apiKey", "providerKey", "prompt", "sourceCode", "screenshotPixels"].includes(key),
      `${location}.${key} is forbidden in a redacted evidence bundle`,
    );
    assertRedacted(child, `${location}.${key}`);
  }
}

function canonicalizeRoute(route, policy) {
  if (route === "/") return route;
  if (policy === "trailing_slash") return `${route.replace(/\/+$/, "")}/`;
  if (policy === "clean_html") return route.replace(/\/+$/, "");
  if (policy === "root") return route;
  throw new Error(`unsupported canonical policy: ${policy}`);
}

function routeForHtmlFile(file) {
  const normalized = file.replace(/^\/+/, "");
  if (normalized === "index.html") return "/";
  if (normalized.endsWith("/index.html")) {
    return `/${normalized.slice(0, -"/index.html".length).replace(/^\/+|\/+$/g, "")}/`;
  }
  if (normalized.endsWith(".html")) {
    return `/${normalized.slice(0, -".html".length).replace(/^\/+|\/+$/g, "")}`;
  }
  return null;
}

export function buildRouteProjection(contract, files) {
  const routes = {};
  for (const file of files) {
    const rawRoute = routeForHtmlFile(file.path);
    if (!rawRoute) continue;
    const route = canonicalizeRoute(rawRoute, contract.canonicalPolicy);
    invariant(!routes[route], `artifact.route_ambiguous:${route}`);
    routes[route] = file.path;
  }
  invariant(routes[contract.entryRoute], `artifact.entry_route_missing:${contract.entryRoute}`);
  const aliases = {};
  for (const route of Object.keys(routes).sort()) {
    if (route === "/") continue;
    if (contract.canonicalPolicy === "trailing_slash") aliases[route.replace(/\/$/, "")] = route;
    if (contract.canonicalPolicy === "clean_html") aliases[`${route}/`] = route;
  }
  return { routes, aliases };
}

export async function runRouteConformance(corpusPath) {
  const corpus = parseJson(await readFile(corpusPath), corpusPath);
  invariant(corpus.schemaVersion === "artifact-route-conformance@1", "unsupported route corpus schema");
  const results = [];
  for (const testCase of corpus.cases) {
    try {
      const projection = buildRouteProjection(testCase.contract, testCase.files);
      invariant(!testCase.expectedErrorKind, `${testCase.id} expected ${testCase.expectedErrorKind}`);
      invariant(JSON.stringify(projection.routes) === JSON.stringify(testCase.expectedRoutes), `${testCase.id} routes differ`);
      invariant(JSON.stringify(projection.aliases) === JSON.stringify(testCase.expectedAliases), `${testCase.id} aliases differ`);
      results.push({ id: testCase.id, status: "passed" });
    } catch (error) {
      const errorKind = String(error.message).split(":", 1)[0];
      invariant(errorKind === testCase.expectedErrorKind, `${testCase.id} failed with ${errorKind}`);
      results.push({ id: testCase.id, status: "passed", expectedErrorKind: errorKind });
    }
  }
  return { schemaVersion: corpus.schemaVersion, cases: results };
}

function validateRouteManifest(routeManifest) {
  invariant(routeManifest.schemaVersion === "artifact-route-manifest@1", "unsupported artifact route manifest schema");
  invariant(routeManifest.routes?.[routeManifest.entryRoute], "artifact route manifest is missing its entry route");
  for (const [alias, canonical] of Object.entries(routeManifest.aliases ?? {})) {
    invariant(alias !== canonical && routeManifest.routes[canonical], `invalid route alias ${alias}`);
  }
}

function resolveRoute(routeManifest, requestPath) {
  const canonical = routeManifest.aliases?.[requestPath] ?? requestPath;
  return routeManifest.routes?.[canonical] ?? null;
}

export async function replayEvidence(bundleDirectory) {
  const bundle = resolve(bundleDirectory);
  const unverifiedManifest = parseJson(
    await readFile(resolve(bundle, "manifest.json")),
    "manifest.json",
  );
  const requiredFiles = unverifiedManifest.bundleKind === "runtime_restart_terminal"
    ? REQUIRED_RUNTIME_RESTART_TERMINAL_FILES
    : unverifiedManifest.bundleKind === "real_provider_terminal"
      ? REQUIRED_REAL_PROVIDER_TERMINAL_FILES
      : REQUIRED_REPLAY_FILES;
  const checksumBytes = await readFile(resolve(bundle, "checksums.sha256"));
  const checksumEntries = checksumBytes
    .toString("utf8")
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const match = /^([a-f0-9]{64})  ([A-Za-z0-9._-]+)$/.exec(line);
      invariant(match, `invalid checksum line: ${line}`);
      return { expected: match[1], file: match[2] };
    });
  invariant(
    JSON.stringify(checksumEntries.map(({ file }) => file).sort()) === JSON.stringify([...requiredFiles].sort()),
    "checksums.sha256 must cover exactly the required replay inputs",
  );

  const bytesByFile = new Map();
  for (const { expected, file } of checksumEntries) {
    const bytes = await readFile(resolve(bundle, file));
    invariant(sha256(bytes) === expected, `checksum mismatch for ${file}`);
    bytesByFile.set(file, bytes);
  }

  const manifest = parseJson(bytesByFile.get("manifest.json"), "manifest.json");
  const usage = parseJson(bytesByFile.get("usage.json"), "usage.json");
  invariant(manifest.schemaVersion === "runtime-evidence-bundle@1", "unsupported evidence bundle schema");
  if (manifest.bundleKind === "runtime_restart_terminal") {
    return replayRuntimeRestartTerminalBundle(manifest, usage, bytesByFile);
  }
  if (manifest.bundleKind === "real_provider_terminal") {
    return replayRealProviderTerminalBundle(manifest, usage, bytesByFile);
  }
  const routeManifest = parseJson(bytesByFile.get("artifact-route-manifest.json"), "artifact-route-manifest.json");
  const validation = parseJson(bytesByFile.get("validation-summary.json"), "validation-summary.json");
  const runModelUsage = parseJson(bytesByFile.get("run-model-usage.json"), "run-model-usage.json");
  const events = bytesByFile
    .get("events.ndjson")
    .toString("utf8")
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line, index) => parseJson(Buffer.from(line), `events.ndjson:${index + 1}`));

  [manifest, usage, runModelUsage, routeManifest, validation, events]
    .forEach((value) => assertRedacted(value));
  invariant(sha256(bytesByFile.get("events.ndjson")) === manifest.streamSha256, "streamSha256 mismatch");
  invariant(sha256(bytesByFile.get("artifact-route-manifest.json")) === manifest.artifactRouteManifestHash, "artifactRouteManifestHash mismatch");
  validateRouteManifest(routeManifest);

  const replayedUsage = calculateLegacyEvidenceUsage(events);
  invariant(canonicalJson(replayedUsage) === canonicalJson(usage),
    "usage evidence does not match events");
  const replayedRunModelUsage = calculateLegacyRunModelUsage(manifest, events, replayedUsage);
  invariant(canonicalJson(replayedRunModelUsage) === canonicalJson(runModelUsage),
    "RunModelUsage evidence does not match events");

  for (const probe of events.filter((event) => event.type === "route.probe")) {
    const target = resolveRoute(routeManifest, probe.requestPath);
    invariant(target, `route probe ${probe.requestPath} is unresolved`);
    invariant(target.file === probe.resolvedFile && target.sha256 === probe.resolvedSha256, `route probe ${probe.requestPath} differs from the manifest`);
  }

  const failureOwners = Object.fromEntries(
    validation.checks
      .filter((check) => check.status !== "passed")
      .map((check) => [check.id, check.owner]),
  );
  const progressEvents = events.filter((event) => event.type === "workflow.progress");
  const replay = {
    usage: replayedUsage.aggregate,
    runModelUsage: replayedRunModelUsage,
    failureOwners,
    substantiveProgressCount: progressEvents.filter((event) => event.substantive === true).length,
    finalStage: progressEvents.at(-1)?.stage ?? null,
  };
  invariant(JSON.stringify(replay) === JSON.stringify(manifest.replayExpectations), "replayed result differs from manifest expectations");
  return { schemaVersion: "runtime-evidence-replay@1", evidenceId: manifest.evidenceId, status: "passed", replay };
}

function replayRuntimeRestartTerminalBundle(manifest, usage, bytesByFile) {
  const baseExpectations = {
    usage: manifest.replayExpectations?.usage,
    budgetConformance: manifest.replayExpectations?.budgetConformance,
    runModelUsageSha256: manifest.replayExpectations?.runModelUsageSha256,
    routeIdentitySha256: manifest.replayExpectations?.routeIdentitySha256,
    validationPassed: manifest.replayExpectations?.validationPassed,
    providerCachePassed: manifest.replayExpectations?.providerCachePassed,
  };
  const baseReplay = replayRealProviderTerminalBundle(
    {
      ...manifest,
      bundleKind: "real_provider_terminal",
      replayExpectations: baseExpectations,
    },
    usage,
    bytesByFile,
  );
  const restart = parseJson(
    bytesByFile.get("runtime-restart-evidence.json"),
    "runtime-restart-evidence.json",
  );
  assertRedacted(restart);
  invariant(restart.schemaVersion === "generation-context-runtime-restart-evidence@2",
    "Runtime Restart terminal replay requires cleanup-aware evidence@2");
  validateRuntimeRestartEvidence(restart, {
    side: "candidate",
    projectId: manifest.projectId,
    runId: manifest.runId,
    runtimeDeploymentRevision: manifest.runtimeDeploymentRevision,
  });
  const routeIdentity = parseJson(
    bytesByFile.get("artifact-route-identity.json"),
    "artifact-route-identity.json",
  );
  invariant(restart.side === "candidate"
    && restart.runtimeDeploymentRevision === manifest.runtimeDeploymentRevision
    && restart.podBefore?.uid === manifest.podBeforeUid
    && restart.podAfter?.uid === manifest.podAfterUid
    && restart.podBefore.uid !== restart.podAfter.uid
    && restart.evidenceSha256 === manifest.restartEvidenceSha256,
  "Runtime Restart deployment or Pod identity mismatch");
  invariant(restart.before?.generationContextStatus?.contextContentHash
      === manifest.generationContextHash
    && restart.before?.generationContextStatus?.runContextBindingHash
      === manifest.runContextBindingHash
    && restart.before?.generationContextStatus?.budgetProfileId
      === manifest.budgetProfileId
    && restart.before?.generationContextStatus?.budgetProfileHash
      === manifest.budgetProfileHash,
  "Runtime Restart frozen Context/Budget identity mismatch");
  invariant(restart.before?.projectState?.sourceSnapshotRefSha256
      === routeIdentity.sourceSnapshotUriSha256
    && restart.before?.artifact?.httpStatus === routeIdentity.httpProbe?.status
    && restart.before?.artifact?.bodySha256 === routeIdentity.httpProbe?.bodySha256
    && restart.before?.artifact?.bodyBytes === routeIdentity.httpProbe?.bodyBytes,
  "Runtime Restart Source or Artifact identity mismatch");
  const replay = {
    ...baseReplay.replay,
    restartEvidenceSha256: restart.evidenceSha256,
    podUidChanged: restart.verification.podUidChanged === true,
    statePreserved: Object.values(restart.verification).every((value) => value === true),
    sandboxReleased: restart.cleanup?.released === true,
  };
  invariant(JSON.stringify(replay) === JSON.stringify(manifest.replayExpectations),
    "replayed Runtime Restart result differs from manifest expectations");
  return {
    schemaVersion: "runtime-evidence-replay@1",
    evidenceId: manifest.evidenceId,
    status: "passed",
    replay,
  };
}

function replayRealProviderTerminalBundle(manifest, usage, bytesByFile) {
  const caseSummary = parseJson(bytesByFile.get("case-summary.json"), "case-summary.json");
  const routeIdentity = parseJson(
    bytesByFile.get("artifact-route-identity.json"),
    "artifact-route-identity.json",
  );
  const validation = parseJson(
    bytesByFile.get("validation-summary.json"),
    "validation-summary.json",
  );
  const cache = parseJson(
    bytesByFile.get("provider-cache-summary.json"),
    "provider-cache-summary.json",
  );
  const budgetProfiles = parseJson(
    bytesByFile.get("budget-profiles.json"),
    "budget-profiles.json",
  );
  const runModelUsage = parseJson(
    bytesByFile.get("run-model-usage.json"),
    "run-model-usage.json",
  );
  const events = bytesByFile
    .get("events.ndjson")
    .toString("utf8")
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line, index) => parseJson(Buffer.from(line), `events.ndjson:${index + 1}`));
  [manifest, usage, runModelUsage, budgetProfiles, caseSummary, routeIdentity, validation, cache, events]
    .forEach((value) => assertRedacted(value));
  invariant(manifest.runtimeMode === "real_provider"
    && manifest.result?.status === "accepted"
    && nonEmptyString(manifest.evidenceId)
    && nonEmptyString(manifest.gitSha)
    && nonEmptyString(manifest.scenarioKind)
    && nonEmptyString(manifest.projectId)
    && nonEmptyString(manifest.runId)
    && nonEmptyString(manifest.operationId)
    && nonEmptyString(manifest.modelResourceId)
    && nonNegativeInteger(manifest.modelResourceRevision)
    && HASH.test(manifest.providerConfigSha256 || "")
    && HASH.test(manifest.promptSha256 || "")
    && HASH.test(manifest.generationContextHash || "")
    && HASH.test(manifest.runContextBindingHash || "")
    && nonEmptyString(manifest.budgetProfileId)
    && HASH.test(manifest.budgetProfileHash || "")
    && HASH.test(manifest.budgetProfilesSha256 || "")
    && HASH.test(manifest.runModelUsageSha256 || "")
    && nonEmptyString(manifest.templateVersion)
    && HASH.test(manifest.candidateManifestHash || "")
    && HASH.test(manifest.artifactRouteManifestHash || "")
    && HASH.test(manifest.sourceFingerprint || "")
    && HASH.test(manifest.streamSha256 || ""),
  "terminal manifest identity is incomplete");
  for (const event of events) {
    invariant(!(event.type === "agent.message" && "text" in event), "agent.message text is not redacted");
    invariant(!(event.type === "tool.output" && "text" in event), "tool.output text is not redacted");
    invariant(!(event.type === "tool.failed" && "error" in event), "tool.failed error is not redacted");
    invariant(!(event.type === "run.completed" && "summary" in event), "run.completed summary is not redacted");
  }
  invariant(sha256(bytesByFile.get("events.ndjson")) === manifest.streamSha256, "streamSha256 mismatch");
  invariant(sha256(bytesByFile.get("budget-profiles.json")) === manifest.budgetProfilesSha256,
    "budgetProfilesSha256 mismatch");
  invariant(sha256(bytesByFile.get("run-model-usage.json")) === manifest.runModelUsageSha256,
    "runModelUsageSha256 mismatch");
  invariant(caseSummary.runId === manifest.runId
    && caseSummary.projectId === manifest.projectId
    && caseSummary.operationId === manifest.operationId
    && caseSummary.kind === manifest.scenarioKind,
  "case summary Run identity mismatch");
  const budgetRunIds = budgetProfiles.profiles.map((entry) => entry.runId).sort();
  invariant(Array.isArray(caseSummary.budgetRunIds)
    && new Set(caseSummary.budgetRunIds).size === caseSummary.budgetRunIds.length
    && canonicalJson([...caseSummary.budgetRunIds].sort()) === canonicalJson(budgetRunIds),
  "case summary Budget Run set mismatch");
  if (caseSummary.operationRunIds !== undefined) {
    const operationRunIds = budgetProfiles.profiles
      .filter((entry) => entry.operationId === manifest.operationId)
      .map((entry) => entry.runId)
      .sort();
    invariant(Array.isArray(caseSummary.operationRunIds)
      && canonicalJson([...caseSummary.operationRunIds].sort()) === canonicalJson(operationRunIds),
    "case summary Operation Run set mismatch");
  }
  invariant(caseSummary.promptSha256 === manifest.promptSha256
    && HASH.test(caseSummary.acceptanceContractSha256 || "")
    && HASH.test(caseSummary.contentPlanIntentSha256 || "")
    && caseSummary.templateVersion === manifest.templateVersion
    && caseSummary.contextContentHash === manifest.generationContextHash
    && caseSummary.runContextBindingHash === manifest.runContextBindingHash
    && caseSummary.budgetProfileId === manifest.budgetProfileId
    && caseSummary.budgetProfileHash === manifest.budgetProfileHash
    && caseSummary.budgetProfilesSha256 === manifest.budgetProfilesSha256
    && caseSummary.runModelUsageSha256 === manifest.runModelUsageSha256,
  "case summary frozen context identity mismatch");
  invariant(routeIdentity.schemaVersion === "artifact-route-identity@1"
    && nonEmptyString(routeIdentity.buildId)
    && routeIdentity.artifactRouteManifestPath === ".anydesign-artifact-routes.json"
    && routeIdentity.artifactRouteManifestHash === manifest.artifactRouteManifestHash
    && routeIdentity.candidateManifestHash === manifest.candidateManifestHash
    && routeIdentity.sourceFingerprint === manifest.sourceFingerprint
    && HASH.test(routeIdentity.sourceSnapshotUriSha256 || "")
    && nonEmptyString(routeIdentity.entryRoute)
    && routeIdentity.httpProbe?.status === 200
    && routeIdentity.httpProbe?.expectedTextFound === true
    && HASH.test(routeIdentity.httpProbe?.bodySha256 || "")
    && nonNegativeInteger(routeIdentity.httpProbe?.bodyBytes),
  "artifact route identity is incomplete or mismatched");
  invariant(cache.status === "passed"
    && cache.releaseEligible === true
    && cache.toolSetHashVersion === "tool-definition-set@1"
    && cache.sourceCommit === manifest.gitSha
    && cache.modelResourceId === manifest.modelResourceId
    && cache.providerResourceRevision === manifest.modelResourceRevision
    && cache.providerConfigSha256 === manifest.providerConfigSha256
    && nonNegativeInteger(cache.auditedRunCount)
    && nonNegativeInteger(cache.stableRunCount)
    && cache.stableRunCount > 0
    && cache.stableRunCount <= cache.auditedRunCount
    && nonNegativeInteger(cache.grossInputTokens)
    && nonNegativeInteger(cache.cachedInputTokens)
    && cache.cachedInputTokens <= cache.grossInputTokens
    && HASH.test(cache.sourceAuditSha256 || ""),
  "Provider cache identity is incomplete or mismatched");
  const requiredChecks = new Set([
    "case-accepted",
    "entry-route",
    "provider-identity",
    "event-redaction",
    "sandbox-release",
  ]);
  invariant(validation.schemaVersion === "runtime-validation-summary@1"
    && Array.isArray(validation.checks)
    && validation.checks.length === requiredChecks.size
    && validation.checks.every((check) => requiredChecks.delete(check.id)
      && check.status === "passed"
      && nonEmptyString(check.owner))
    && requiredChecks.size === 0,
  "terminal validation summary is incomplete or failed");
  invariant(usage.schemaVersion === "runtime-evidence-usage@1"
    && Array.isArray(usage.turns)
    && usage.turns.length > 0,
  "usage evidence is incomplete");
  const replayedUsage = calculateEvidenceUsage(events);
  replayedUsage.budgetConformance = validateBudgetConformance(budgetProfiles, replayedUsage);
  invariant(canonicalJson(replayedUsage) === canonicalJson(usage),
    "usage evidence does not match the event replay");
  const replayedRunModelUsage = createRunModelUsageEvidence(events, manifest.modelResourceId);
  invariant(canonicalJson(replayedRunModelUsage) === canonicalJson(runModelUsage),
    "RunModelUsage evidence does not match the event replay");
  const primaryProfile = budgetProfiles.profiles.find((entry) => entry.runId === manifest.runId);
  invariant(primaryProfile?.operationId === manifest.operationId
    && primaryProfile.phase === replayedUsage.budgetConformance.runs
      .find((entry) => entry.runId === manifest.runId)?.phase
    && primaryProfile.profile.profileId === manifest.budgetProfileId
    && primaryProfile.profile.profileHash === manifest.budgetProfileHash,
  "primary Run Budget Profile identity mismatch");
  const replay = {
    usage: replayedUsage.aggregate,
    budgetConformance: replayedUsage.budgetConformance,
    runModelUsageSha256: manifest.runModelUsageSha256,
    routeIdentitySha256: sha256(bytesByFile.get("artifact-route-identity.json")),
    validationPassed: true,
    providerCachePassed: true,
  };
  invariant(JSON.stringify(replay) === JSON.stringify(manifest.replayExpectations), "replayed result differs from manifest expectations");
  return {
    schemaVersion: "runtime-evidence-replay@1",
    evidenceId: manifest.evidenceId,
    status: "passed",
    replay,
  };
}

async function main() {
  const args = process.argv.slice(2);
  if (args[0] === "--conformance") {
    invariant(args.length === 2, "usage: replay-evidence.mjs --conformance <corpus.json>");
    console.log(JSON.stringify(await runRouteConformance(resolve(args[1])), null, 2));
    return;
  }
  invariant(args.length === 1, "usage: replay-evidence.mjs <bundle-directory>");
  console.log(JSON.stringify(await replayEvidence(resolve(args[0])), null, 2));
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : null;
if (invokedPath === import.meta.url) {
  main().catch((error) => {
    console.error(`replay-evidence: ${error.message}`);
    process.exitCode = 1;
  });
}
