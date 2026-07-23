import assert from "node:assert/strict";
import test from "node:test";
import {
  calculateEvidenceUsage,
  canonicalJson,
  createBudgetProfilesEvidence,
  createRunModelUsageEvidence,
  sha256,
  validateBudgetConformance,
  validateRunBudgetProfile,
} from "./runtime-budget-evidence.mjs";

function profile(phase, overrides = {}) {
  const tokenLimits = {
    maxTurns: 4,
    maxToolCalls: 4,
    maxInputTokens: 1000,
    maxGrossInputTokens: 1000,
    maxUncachedInputTokens: 800,
    maxPromptTokensPerTurn: 600,
    maxOutputTokens: 400,
  };
  const value = {
    schemaVersion: "run-budget-profile@1",
    profileId: `profile-${phase}`,
    phase,
    rolloutMode: "enforced",
    tokenBudgetMode: "split_enforced",
    operationBudgetMode: "enforced",
    enforcedLimits: { ...tokenLimits, ...overrides.enforcedLimits },
    phaseTargetLimits: { ...tokenLimits, ...overrides.phaseTargetLimits },
    operationLimits: {
      maxGrossInputTokens: 1600,
      maxUncachedInputTokens: 1200,
      maxOutputTokens: 600,
      maxTurns: 6,
      maxToolCalls: 6,
      ...overrides.operationLimits,
    },
  };
  value.profileHash = sha256(canonicalJson(value));
  return value;
}

function run(runId, phase, operationId, frozenProfile = profile(phase), operationAttempt = 1) {
  return {
    runId,
    phase,
    budgetProfile: frozenProfile,
    generationContextStatus: {
      operationId,
      operationAttempt,
      budgetProfileId: frozenProfile.profileId,
      budgetProfileHash: frozenProfile.profileHash,
      budgetProfileRolloutMode: frozenProfile.rolloutMode,
    },
  };
}

test("canonical profile and per-Run/Operation usage replay pass", () => {
  const events = [
    { type: "model.usage", runId: "run-a", turn: 1, inputTokens: 400, cachedInputTokens: 100, outputTokens: 40 },
    { type: "model.usage", runId: "run-a", turn: 1, inputTokens: 450, cachedInputTokens: 150, outputTokens: 50 },
    { type: "tool.started", runId: "run-a" },
    { type: "model.usage", runId: "run-b", turn: 1, inputTokens: 500, cachedInputTokens: 100, outputTokens: 60 },
  ];
  const usage = calculateEvidenceUsage(events);
  assert.equal(usage.turns.length, 2, "latest event wins for a replayed (runId, turn)");
  assert.equal(usage.aggregate.inputTokens, 950);
  const result = createBudgetProfilesEvidence([
    run("run-a", "build", "operation-1"),
    run("run-b", "edit", "operation-1", profile("edit"), 2),
  ], usage);
  assert.equal(result.conformance.status, "passed");
  assert.equal(result.conformance.runs.length, 2);
  assert.equal(result.conformance.operations[0].grossInputTokens, 950);
  assert.doesNotThrow(() => validateBudgetConformance(result.evidence, usage));
});

test("RunModelUsage projections keep the latest event per Run and turn", () => {
  const evidence = createRunModelUsageEvidence([
    {
      type: "model.execution",
      runId: "run-a",
      snapshot: { displayName: "DeepSeek V4 Pro" },
    },
    {
      type: "model.usage",
      runId: "run-a",
      turn: 1,
      inputTokens: 100,
      cachedInputTokens: 20,
      outputTokens: 10,
      estimated: true,
    },
    {
      type: "model.usage",
      runId: "run-a",
      turn: 1,
      inputTokens: 120,
      cachedInputTokens: 30,
      outputTokens: 12,
      estimated: false,
    },
  ], "deepseek-v4-pro");

  assert.deepEqual(evidence.runs, [{
    schemaVersion: "run-model-usage@1",
    runId: "run-a",
    modelServiceId: "deepseek-v4-pro",
    modelDisplayName: "DeepSeek V4 Pro",
    inputTokens: 120,
    outputTokens: 12,
    cachedInputTokens: 30,
    totalTokens: 132,
    estimated: false,
    turnCount: 1,
  }]);
});

test("tampered canonical profile fails closed", () => {
  const frozen = profile("build");
  frozen.enforcedLimits.maxPromptTokensPerTurn += 1;
  assert.throws(() => validateRunBudgetProfile(frozen), /canonical hash mismatch/);
});

test("per-turn Prompt overage fails even when aggregate Gross remains below the Run limit", () => {
  const usage = calculateEvidenceUsage([
    { type: "model.usage", runId: "run-a", turn: 1, inputTokens: 601, cachedInputTokens: 500, outputTokens: 10 },
  ]);
  assert.throws(
    () => createBudgetProfilesEvidence([run("run-a", "build", "operation-1")], usage),
    /per-turn Prompt limit/,
  );
});

test("Operation aggregation fails across individually valid Runs", () => {
  const operationProfileA = profile("build", { operationLimits: { maxGrossInputTokens: 900 } });
  const operationProfileB = profile("edit", { operationLimits: { maxGrossInputTokens: 900 } });
  const usage = calculateEvidenceUsage([
    { type: "model.usage", runId: "run-a", turn: 1, inputTokens: 500, cachedInputTokens: 100, outputTokens: 10 },
    { type: "model.usage", runId: "run-b", turn: 1, inputTokens: 500, cachedInputTokens: 100, outputTokens: 10 },
  ]);
  assert.throws(
    () => createBudgetProfilesEvidence([
      run("run-a", "build", "operation-1", operationProfileA),
      run("run-b", "edit", "operation-1", operationProfileB, 2),
    ], usage),
    /Operation Gross Input limit/,
  );
});

test("invalid Provider cached usage is rejected before conformance", () => {
  assert.throws(
    () => calculateEvidenceUsage([
      { type: "model.usage", runId: "run-a", turn: 1, inputTokens: 10, cachedInputTokens: 11, outputTokens: 0 },
    ]),
    /cachedInputTokens exceeds inputTokens/,
  );
});

test("a successor without its predecessor Operation attempt fails closed", () => {
  const successor = run("run-successor", "build", "operation-1");
  successor.generationContextStatus.operationAttempt = 2;
  const usage = calculateEvidenceUsage([
    { type: "model.usage", runId: "run-successor", turn: 1, inputTokens: 100, cachedInputTokens: 0, outputTokens: 10 },
  ]);
  assert.throws(
    () => createBudgetProfilesEvidence([successor], usage),
    /missing or duplicates an Operation attempt/,
  );
});
