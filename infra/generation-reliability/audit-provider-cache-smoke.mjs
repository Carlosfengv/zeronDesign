import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { evidenceRedactionViolations } from "./runtime-evidence-redaction.mjs";

const [evidenceRootArg, outputArg] = process.argv.slice(2);
if (!evidenceRootArg) {
  throw new Error(
    "usage: audit-provider-cache-smoke.mjs <real-provider-suite-directory> [output-file]",
  );
}

const evidenceRoot = path.resolve(evidenceRootArg);
if (!fs.statSync(evidenceRoot, { throwIfNoEntry: false })?.isDirectory()) {
  throw new Error(`real-provider suite directory does not exist: ${evidenceRoot}`);
}
const outputFile = path.resolve(
  outputArg || path.join(evidenceRoot, "provider-cache-smoke-audit.json"),
);
const summaryFile = path.join(evidenceRoot, "real-provider-examples-summary.json");
if (!fs.statSync(summaryFile, { throwIfNoEntry: false })?.isFile()) {
  throw new Error("provider cache smoke requires real-provider suite summary evidence");
}
const summary = JSON.parse(fs.readFileSync(summaryFile, "utf8"));
if (summary.schemaVersion !== "generation-real-provider-suite-evidence@2") {
  throw new Error("real-provider suite summary has an unsupported evidence schema");
}
const hashPattern = /^[a-fA-F0-9]{64}$/;
const TOOL_SET_HASH_VERSION = "tool-definition-set@1";
const providerResourceRevision = summary.provenance?.providerResourceRevision;
const providerConfigSha256 = summary.provenance?.providerConfigSha256;
const modelResourceId = summary.provider?.modelResourceId;
const sourceCommit = summary.provenance?.gitCommit;
const sourceDirty = summary.provenance?.gitDirty !== false;
if (summary.status !== "accepted" || summary.provider?.realProviderVerified !== true) {
  throw new Error("provider cache smoke requires an accepted, real-provider-verified suite");
}
if (typeof modelResourceId !== "string" || modelResourceId.trim() === "") {
  throw new Error("real-provider suite summary is missing modelResourceId");
}
if (!Number.isSafeInteger(providerResourceRevision) || providerResourceRevision <= 0) {
  throw new Error("real-provider suite summary is missing providerResourceRevision");
}
if (!hashPattern.test(providerConfigSha256 || "")) {
  throw new Error("real-provider suite summary is missing providerConfigSha256");
}
const summaryCases = new Map((summary.cases || []).map((item) => [item.id, item]));

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function validateRedactedEventStream(run) {
  const stream = run.eventStream;
  if (stream?.schemaVersion !== "generation-run-event-stream@1"
    || stream?.format !== "ndjson"
    || !Number.isSafeInteger(stream?.eventCount)
    || stream.eventCount <= 0
    || !hashPattern.test(stream?.sha256 || "")
    || typeof stream?.path !== "string") {
    return { valid: false, violations: ["eventStream.contract"] };
  }
  const eventFile = path.resolve(evidenceRoot, stream.path);
  if (!eventFile.startsWith(`${evidenceRoot}${path.sep}`)
    || !fs.statSync(eventFile, { throwIfNoEntry: false })?.isFile()) {
    return { valid: false, violations: ["eventStream.path"] };
  }
  const bytes = fs.readFileSync(eventFile);
  if (sha256(bytes) !== stream.sha256) {
    return { valid: false, violations: ["eventStream.sha256"] };
  }
  const lines = bytes.toString("utf8").trim().split(/\r?\n/).filter(Boolean);
  if (lines.length !== stream.eventCount) {
    return { valid: false, violations: ["eventStream.eventCount"] };
  }
  const events = [];
  try {
    for (const line of lines) events.push(JSON.parse(line));
  } catch {
    return { valid: false, violations: ["eventStream.json"] };
  }
  const violations = evidenceRedactionViolations(events, "eventStream");
  return { valid: violations.length === 0, violations };
}

const caseFiles = fs
  .readdirSync(evidenceRoot)
  .filter((name) => /^real-provider-case-.+\.json$/.test(name))
  .sort();
if (caseFiles.length === 0) {
  throw new Error("provider cache smoke requires real-provider case evidence");
}

const runs = [];
let excludedRunCount = 0;
for (const fileName of caseFiles) {
  const evidence = JSON.parse(fs.readFileSync(path.join(evidenceRoot, fileName), "utf8"));
  if (evidence.schemaVersion !== "generation-real-provider-case-evidence@2") {
    throw new Error(`${fileName} has an unsupported evidence schema`);
  }
  const summaryCase = summaryCases.get(evidence.id);
  if (!summaryCase) {
    throw new Error(`${fileName} is not bound to the suite summary`);
  }
  const caseRedactionViolations = evidenceRedactionViolations(evidence, fileName);
  const finalAttemptRunIds = new Set(evidence.attempts?.at(-1)?.runIds || []);
  for (const run of evidence.runs || []) {
    if (!run?.runId) continue;
    const finalAttempt = finalAttemptRunIds.size === 0 || finalAttemptRunIds.has(run.runId);
    const cacheSmokePhase = run.phase === "brief" || run.phase === "build";
    if (!finalAttempt || !cacheSmokePhase || run.status !== "completed") {
      excludedRunCount += 1;
      continue;
    }
    if (!(summaryCase.runIds || []).includes(run.runId)) {
      throw new Error(`${fileName} run ${run.runId} is not bound to the suite summary`);
    }
    runs.push({ caseId: evidence.id, ...run, caseRedactionViolations });
  }
}

const auditedRuns = runs.map((run) => {
  const groups = new Map();
  let compositionValid = true;
  const promptCompositions = Array.isArray(run.promptCompositions)
    ? run.promptCompositions
    : [];
  if (promptCompositions.length === 0) compositionValid = false;
  for (const composition of promptCompositions) {
    const staticPrefixHash = composition?.staticPrefixHash;
    const toolSetHash = composition?.toolSetHash;
    if (composition?.toolSetHashVersion !== TOOL_SET_HASH_VERSION
      || !hashPattern.test(staticPrefixHash || "") || !hashPattern.test(toolSetHash || "")) {
      compositionValid = false;
      continue;
    }
    const key = `${staticPrefixHash}:${toolSetHash}`;
    groups.set(key, (groups.get(key) || 0) + 1);
  }
  const repeatedStableTurns = Math.max(0, ...groups.values());
  const metrics = run.promptEfficiency || {};
  const redaction = validateRedactedEventStream(run);
  const redactionViolationsForRun = [
    ...(run.caseRedactionViolations || []),
    ...redaction.violations,
  ];
  const metricsValid =
    metrics.schemaVersion === "run-prompt-efficiency@1" &&
    metrics.runId === run.runId &&
    Number.isSafeInteger(metrics.grossInputTokens) &&
    Number.isSafeInteger(metrics.cachedInputTokens) &&
    metrics.grossInputTokens >= metrics.cachedInputTokens &&
    metrics.estimated === false;
  const providerIdentityValid = Array.isArray(run.modelExecutions)
    && run.modelExecutions.length > 0
    && run.modelExecutions.every((execution) =>
      execution?.modelResourceId === modelResourceId
      && execution?.modelResourceRevision === providerResourceRevision
      && execution?.providerRequestIdPresent === true);
  const buildIdentityValid = run.phase !== "build" || (
    typeof run.buildEvidence?.buildId === "string"
    && run.buildEvidence.buildId.length > 0
    && run.buildEvidence?.artifactRouteManifestPath === ".anydesign-artifact-routes.json"
    && hashPattern.test(run.buildEvidence?.sourceFingerprint || "")
    && hashPattern.test(run.buildEvidence?.candidateManifestHash || "")
    && hashPattern.test(run.buildEvidence?.artifactRouteManifestHash || "")
  );
  const contextStatus = run.generationContextStatus;
  const generationContextIdentityValid = run.phase !== "build" || (
    contextStatus?.schemaVersion === "generation-context-status@1"
    && contextStatus?.runId === run.runId
    && typeof contextStatus?.budgetProfileId === "string"
    && contextStatus.budgetProfileId.length > 0
    && hashPattern.test(contextStatus?.budgetProfileHash || "")
    && new Set(["off", "shadow", "enforced"]).has(contextStatus?.budgetProfileRolloutMode)
    && (
      contextStatus?.runContractVersion === "legacy@1"
      || (
        contextStatus?.runContractVersion === "generation-context@1"
        && contextStatus?.status === "compiled"
        && hashPattern.test(contextStatus?.contextContentHash || "")
        && hashPattern.test(contextStatus?.runContextBindingHash || "")
      )
    )
  );
  return {
    caseId: run.caseId,
    runId: run.runId,
    phase: run.phase,
    compositionValid,
    repeatedStableTurns,
    metricsValid,
    providerIdentityValid,
    buildIdentityValid,
    generationContextIdentityValid,
    redactionValid: redaction.valid && redactionViolationsForRun.length === 0,
    redactionViolations: redactionViolationsForRun,
    grossInputTokens: Number(metrics.grossInputTokens || 0),
    cachedInputTokens: Number(metrics.cachedInputTokens || 0),
    cacheHitRateBasisPoints: Number(metrics.cacheHitRateBasisPoints || 0),
  };
});

const stableRuns = auditedRuns.filter(
  (run) => run.compositionValid
    && run.metricsValid
    && run.providerIdentityValid
    && run.buildIdentityValid
    && run.generationContextIdentityValid
    && run.redactionValid
    && run.repeatedStableTurns >= 2,
);
const grossInputTokens = stableRuns.reduce((total, run) => total + run.grossInputTokens, 0);
const cachedInputTokens = stableRuns.reduce((total, run) => total + run.cachedInputTokens, 0);
const invalidRunCount = auditedRuns.filter(
  (run) => !run.compositionValid
    || !run.metricsValid
    || !run.providerIdentityValid
    || !run.buildIdentityValid
    || !run.generationContextIdentityValid
    || !run.redactionValid,
).length;
const status = invalidRunCount > 0 || stableRuns.length === 0
  ? "failed"
  : cachedInputTokens > 0
    ? "passed"
    : "provider_not_reporting_cached_usage";
const audit = {
  schemaVersion: "provider-cache-smoke-audit@1",
  toolSetHashVersion: TOOL_SET_HASH_VERSION,
  status,
  releaseEligible:
    status === "passed" && !sourceDirty && typeof sourceCommit === "string" && sourceCommit.length > 0,
  evidenceRoot,
  suiteId: summary.suiteId,
  sourceCommit: sourceCommit || null,
  sourceDirty,
  modelResourceId,
  providerResourceRevision,
  providerConfigSha256,
  caseFileCount: caseFiles.length,
  auditedRunCount: auditedRuns.length,
  stableRunCount: stableRuns.length,
  invalidRunCount,
  excludedRunCount,
  grossInputTokens,
  cachedInputTokens,
  uncachedInputTokens: Math.max(0, grossInputTokens - cachedInputTokens),
  cacheHitRateBasisPoints:
    grossInputTokens === 0 ? 0 : Math.floor((cachedInputTokens * 10_000) / grossInputTokens),
  runs: auditedRuns,
  generatedAt: new Date().toISOString(),
};

fs.mkdirSync(path.dirname(outputFile), { recursive: true });
fs.writeFileSync(outputFile, `${JSON.stringify(audit, null, 2)}\n`);
process.stdout.write(
  `Provider cache smoke audit: status=${status} stableRuns=${stableRuns.length}/${auditedRuns.length} cached=${cachedInputTokens}/${grossInputTokens} evidence=${outputFile}\n`,
);
if (status === "failed") process.exitCode = 1;
