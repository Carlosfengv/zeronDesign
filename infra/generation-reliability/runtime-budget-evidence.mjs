import crypto from "node:crypto";

const HASH = /^[a-f0-9]{64}$/;
const TOKEN_LIMIT_FIELDS = [
  "maxTurns",
  "maxToolCalls",
  "maxInputTokens",
  "maxGrossInputTokens",
  "maxUncachedInputTokens",
  "maxPromptTokensPerTurn",
  "maxOutputTokens",
];
const OPERATION_LIMIT_FIELDS = [
  "maxGrossInputTokens",
  "maxUncachedInputTokens",
  "maxOutputTokens",
  "maxTurns",
  "maxToolCalls",
];

export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) =>
      `${JSON.stringify(key)}:${canonicalJson(value[key])}`
    ).join(",")}}`;
  }
  return JSON.stringify(value);
}

export function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function invariant(condition, message) {
  if (!condition) throw new Error(message);
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function nonNegativeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function positiveInteger(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function exactKeys(value, expected, label) {
  invariant(value && typeof value === "object" && !Array.isArray(value), `${label} must be an object`);
  invariant(
    canonicalJson(Object.keys(value).sort()) === canonicalJson([...expected].sort()),
    `${label} fields do not match the frozen schema`,
  );
}

export function validateRunBudgetProfile(profile, expected = {}) {
  exactKeys(profile, [
    "schemaVersion",
    "profileId",
    "phase",
    "rolloutMode",
    "tokenBudgetMode",
    "operationBudgetMode",
    "enforcedLimits",
    "phaseTargetLimits",
    "operationLimits",
    "profileHash",
  ], "RunBudgetProfile");
  invariant(profile.schemaVersion === "run-budget-profile@1", "unsupported RunBudgetProfile schema");
  invariant(hasText(profile.profileId), "RunBudgetProfile profileId is required");
  invariant(hasText(profile.phase), "RunBudgetProfile phase is required");
  invariant(new Set(["off", "shadow", "enforced"]).has(profile.rolloutMode), "RunBudgetProfile rolloutMode is invalid");
  invariant(new Set(["legacy", "split_shadow", "split_enforced"]).has(profile.tokenBudgetMode), "RunBudgetProfile tokenBudgetMode is invalid");
  invariant(new Set(["shadow", "enforced"]).has(profile.operationBudgetMode), "RunBudgetProfile operationBudgetMode is invalid");
  for (const [label, limits] of [
    ["enforcedLimits", profile.enforcedLimits],
    ["phaseTargetLimits", profile.phaseTargetLimits],
  ]) {
    exactKeys(limits, TOKEN_LIMIT_FIELDS, `RunBudgetProfile.${label}`);
    invariant(TOKEN_LIMIT_FIELDS.every((field) => positiveInteger(limits[field])),
      `RunBudgetProfile.${label} limits must be positive integers`);
  }
  exactKeys(profile.operationLimits, OPERATION_LIMIT_FIELDS, "RunBudgetProfile.operationLimits");
  invariant(OPERATION_LIMIT_FIELDS.every((field) => positiveInteger(profile.operationLimits[field])),
    "RunBudgetProfile.operationLimits must be positive integers");
  const { profileHash, ...identity } = profile;
  invariant(HASH.test(profileHash || "") && sha256(canonicalJson(identity)) === profileHash,
    "RunBudgetProfile canonical hash mismatch");
  if (expected.phase !== undefined) invariant(profile.phase === expected.phase, "RunBudgetProfile phase mismatch");
  if (expected.profileId !== undefined) invariant(profile.profileId === expected.profileId, "RunBudgetProfile ID mismatch");
  if (expected.profileHash !== undefined) invariant(profile.profileHash === expected.profileHash, "RunBudgetProfile hash mismatch");
  if (expected.rolloutMode !== undefined) invariant(profile.rolloutMode === expected.rolloutMode, "RunBudgetProfile rollout mode mismatch");
  return profile;
}

export function calculateEvidenceUsage(events) {
  const turnsByKey = new Map();
  const toolCallsByRun = new Map();
  for (const event of events) {
    if (event.type === "tool.started") {
      invariant(hasText(event.runId), "tool.started event must carry runId");
      toolCallsByRun.set(event.runId, (toolCallsByRun.get(event.runId) || 0) + 1);
    }
    if (event.type !== "model.usage") continue;
    invariant(hasText(event.runId), "model.usage event must carry runId");
    const turn = Number(event.turn);
    const inputTokens = Number(event.inputTokens);
    const cachedInputTokens = Number(event.cachedInputTokens);
    const outputTokens = Number(event.outputTokens);
    invariant(positiveInteger(turn), "model usage turn is invalid");
    invariant(nonNegativeInteger(inputTokens), "model usage inputTokens is invalid");
    invariant(nonNegativeInteger(cachedInputTokens), "model usage cachedInputTokens is invalid");
    invariant(cachedInputTokens <= inputTokens, "model usage cachedInputTokens exceeds inputTokens");
    invariant(nonNegativeInteger(outputTokens), "model usage outputTokens is invalid");
    turnsByKey.set(`${event.runId}\u0000${turn}`, {
      runId: event.runId,
      turn,
      inputTokens,
      cachedInputTokens,
      uncachedInputTokens: inputTokens - cachedInputTokens,
      outputTokens,
    });
  }
  const turns = [...turnsByKey.values()].sort((left, right) =>
    left.runId.localeCompare(right.runId) || left.turn - right.turn);
  const empty = () => ({
    modelCalls: 0,
    toolCalls: 0,
    inputTokens: 0,
    cachedInputTokens: 0,
    uncachedInputTokens: 0,
    outputTokens: 0,
    maxTurnInputTokens: 0,
  });
  const aggregate = empty();
  const byRunMap = new Map();
  for (const usage of turns) {
    const run = byRunMap.get(usage.runId) || { runId: usage.runId, ...empty() };
    for (const target of [aggregate, run]) {
      target.modelCalls += 1;
      target.inputTokens += usage.inputTokens;
      target.cachedInputTokens += usage.cachedInputTokens;
      target.uncachedInputTokens += usage.uncachedInputTokens;
      target.outputTokens += usage.outputTokens;
      target.maxTurnInputTokens = Math.max(target.maxTurnInputTokens, usage.inputTokens);
    }
    byRunMap.set(usage.runId, run);
  }
  for (const [runId, toolCalls] of toolCallsByRun) {
    const run = byRunMap.get(runId) || { runId, ...empty() };
    run.toolCalls = toolCalls;
    aggregate.toolCalls += toolCalls;
    byRunMap.set(runId, run);
  }
  return {
    schemaVersion: "runtime-evidence-usage@1",
    turns,
    byRun: [...byRunMap.values()].sort((left, right) => left.runId.localeCompare(right.runId)),
    aggregate,
  };
}

export function createRunModelUsageEvidence(events, modelResourceId) {
  invariant(hasText(modelResourceId), "RunModelUsage evidence requires a Model Resource ID");
  const turnsByKey = new Map();
  const displayNames = new Map();
  for (const event of events) {
    if (event.type === "model.execution"
      && hasText(event.runId)
      && hasText(event.snapshot?.displayName)) {
      displayNames.set(event.runId, event.snapshot.displayName);
    }
    if (event.type !== "model.usage") continue;
    invariant(hasText(event.runId), "RunModelUsage event must carry runId");
    invariant(positiveInteger(event.turn), "RunModelUsage turn is invalid");
    invariant(nonNegativeInteger(event.inputTokens), "RunModelUsage inputTokens is invalid");
    invariant(nonNegativeInteger(event.cachedInputTokens), "RunModelUsage cachedInputTokens is invalid");
    invariant(event.cachedInputTokens <= event.inputTokens,
      "RunModelUsage cachedInputTokens exceeds inputTokens");
    invariant(nonNegativeInteger(event.outputTokens), "RunModelUsage outputTokens is invalid");
    invariant(typeof event.estimated === "boolean", "RunModelUsage estimated flag is invalid");
    turnsByKey.set(`${event.runId}\u0000${event.turn}`, event);
  }
  const byRun = new Map();
  for (const event of turnsByKey.values()) {
    const usage = byRun.get(event.runId) || {
      schemaVersion: "run-model-usage@1",
      runId: event.runId,
      modelServiceId: modelResourceId,
      modelDisplayName: displayNames.get(event.runId) || modelResourceId,
      inputTokens: 0,
      outputTokens: 0,
      cachedInputTokens: 0,
      totalTokens: 0,
      estimated: false,
      turnCount: 0,
    };
    usage.inputTokens += event.inputTokens;
    usage.outputTokens += event.outputTokens;
    usage.cachedInputTokens += event.cachedInputTokens;
    usage.totalTokens += event.inputTokens + event.outputTokens;
    usage.estimated ||= event.estimated;
    usage.turnCount += 1;
    byRun.set(event.runId, usage);
  }
  invariant(byRun.size > 0, "RunModelUsage evidence requires at least one model.usage event");
  return {
    schemaVersion: "runtime-evidence-run-model-usage@1",
    runs: [...byRun.values()].sort((left, right) => left.runId.localeCompare(right.runId)),
  };
}

export function createBudgetProfilesEvidence(runs, usage) {
  invariant(Array.isArray(runs) && runs.length > 0, "Budget evidence requires at least one Run");
  const entries = runs.map((run) => {
    invariant(hasText(run?.runId) && hasText(run?.phase), "Budget evidence Run identity is incomplete");
    const context = run.generationContextStatus;
    invariant(hasText(context?.operationId), `${run.runId} Operation identity is missing`);
    invariant(positiveInteger(context?.operationAttempt), `${run.runId} Operation attempt is missing`);
    const profile = structuredClone(run.budgetProfile);
    validateRunBudgetProfile(profile, {
      phase: run.phase,
      profileId: context.budgetProfileId,
      profileHash: context.budgetProfileHash,
      rolloutMode: context.budgetProfileRolloutMode,
    });
    return {
      runId: run.runId,
      operationId: context.operationId,
      operationAttempt: context.operationAttempt,
      phase: run.phase,
      profile,
    };
  });
  const evidence = {
    schemaVersion: "runtime-evidence-budget-profiles@1",
    profiles: entries.sort((left, right) => left.runId.localeCompare(right.runId)),
  };
  return { evidence, conformance: validateBudgetConformance(evidence, usage) };
}

export function validateBudgetConformance(evidence, usage) {
  invariant(evidence?.schemaVersion === "runtime-evidence-budget-profiles@1"
    && Array.isArray(evidence.profiles)
    && evidence.profiles.length > 0, "Budget Profiles evidence is incomplete");
  invariant(usage?.schemaVersion === "runtime-evidence-usage@1"
    && Array.isArray(usage.turns)
    && Array.isArray(usage.byRun), "Budget conformance requires replayed Usage");
  const profiles = new Map();
  for (const entry of evidence.profiles) {
    exactKeys(entry, ["runId", "operationId", "operationAttempt", "phase", "profile"],
      "Budget Profile entry");
    invariant(hasText(entry.runId) && hasText(entry.operationId), "Budget Profile Run/Operation identity is incomplete");
    invariant(positiveInteger(entry.operationAttempt), "Budget Profile Operation attempt is invalid");
    invariant(!profiles.has(entry.runId), `duplicate Budget Profile Run: ${entry.runId}`);
    validateRunBudgetProfile(entry.profile, { phase: entry.phase });
    profiles.set(entry.runId, entry);
  }
  invariant(usage.byRun.length === profiles.size, "Usage and Budget Profile Run sets differ");
  const attemptsByOperation = new Map();
  for (const entry of profiles.values()) {
    const attempts = attemptsByOperation.get(entry.operationId) || [];
    attempts.push(entry.operationAttempt);
    attemptsByOperation.set(entry.operationId, attempts);
  }
  for (const [operationId, attempts] of attemptsByOperation) {
    const sorted = [...attempts].sort((left, right) => left - right);
    invariant(new Set(sorted).size === sorted.length
      && sorted.every((attempt, index) => attempt === index + 1),
    `${operationId} Budget evidence is missing or duplicates an Operation attempt`);
  }
  const runResults = [];
  for (const runUsage of usage.byRun) {
    const entry = profiles.get(runUsage.runId);
    invariant(entry, `Usage references unknown Budget Profile Run: ${runUsage.runId}`);
    const limits = entry.profile.rolloutMode === "enforced"
      ? entry.profile.phaseTargetLimits
      : entry.profile.enforcedLimits;
    const grossLimit = entry.profile.tokenBudgetMode === "legacy"
      ? limits.maxInputTokens
      : limits.maxGrossInputTokens;
    invariant(runUsage.modelCalls <= limits.maxTurns, `${runUsage.runId} exceeds maxTurns`);
    invariant(runUsage.toolCalls <= limits.maxToolCalls, `${runUsage.runId} exceeds maxToolCalls`);
    invariant(runUsage.inputTokens <= grossLimit, `${runUsage.runId} exceeds Gross Input limit`);
    invariant(runUsage.uncachedInputTokens <= limits.maxUncachedInputTokens,
      `${runUsage.runId} exceeds Uncached Input limit`);
    invariant(runUsage.outputTokens <= limits.maxOutputTokens, `${runUsage.runId} exceeds Output limit`);
    invariant(runUsage.maxTurnInputTokens <= limits.maxPromptTokensPerTurn,
      `${runUsage.runId} exceeds per-turn Prompt limit`);
    runResults.push({
      runId: runUsage.runId,
      operationId: entry.operationId,
      operationAttempt: entry.operationAttempt,
      phase: entry.phase,
      profileId: entry.profile.profileId,
      profileHash: entry.profile.profileHash,
      modelCalls: runUsage.modelCalls,
      toolCalls: runUsage.toolCalls,
      grossInputTokens: runUsage.inputTokens,
      uncachedInputTokens: runUsage.uncachedInputTokens,
      outputTokens: runUsage.outputTokens,
      maxTurnInputTokens: runUsage.maxTurnInputTokens,
      passed: true,
    });
  }
  const operationGroups = new Map();
  for (const result of runResults) {
    const entry = profiles.get(result.runId);
    const group = operationGroups.get(result.operationId) || {
      operationId: result.operationId,
      limits: entry.profile.operationLimits,
      operationBudgetMode: entry.profile.operationBudgetMode,
      modelCalls: 0,
      toolCalls: 0,
      grossInputTokens: 0,
      uncachedInputTokens: 0,
      outputTokens: 0,
    };
    invariant(canonicalJson(group.limits) === canonicalJson(entry.profile.operationLimits)
      && group.operationBudgetMode === entry.profile.operationBudgetMode,
    `${result.operationId} has inconsistent Operation Budget Profiles`);
    group.modelCalls += result.modelCalls;
    group.toolCalls += result.toolCalls;
    group.grossInputTokens += result.grossInputTokens;
    group.uncachedInputTokens += result.uncachedInputTokens;
    group.outputTokens += result.outputTokens;
    operationGroups.set(result.operationId, group);
  }
  const operationResults = [...operationGroups.values()].map((group) => {
    invariant(group.modelCalls <= group.limits.maxTurns, `${group.operationId} exceeds Operation maxTurns`);
    invariant(group.toolCalls <= group.limits.maxToolCalls, `${group.operationId} exceeds Operation maxToolCalls`);
    invariant(group.grossInputTokens <= group.limits.maxGrossInputTokens,
      `${group.operationId} exceeds Operation Gross Input limit`);
    invariant(group.uncachedInputTokens <= group.limits.maxUncachedInputTokens,
      `${group.operationId} exceeds Operation Uncached Input limit`);
    invariant(group.outputTokens <= group.limits.maxOutputTokens,
      `${group.operationId} exceeds Operation Output limit`);
    const { limits: _limits, ...result } = group;
    return { ...result, passed: true };
  });
  return {
    schemaVersion: "runtime-budget-conformance@1",
    status: "passed",
    runs: runResults,
    operations: operationResults,
  };
}
