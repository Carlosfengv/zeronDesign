import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { canonicalJson, sha256 } from "./runtime-budget-evidence.mjs";
import {
  enforceReleaseBudgetPolicy,
  validateReleaseBudgetPolicy,
} from "./runtime-release-budget-policy.mjs";

const directory = path.dirname(fileURLToPath(import.meta.url));
const policy = JSON.parse(fs.readFileSync(path.join(directory, "release-budget-policy.json"), "utf8"));
const targets = {
  build: { turns: 16, gross: 300000, uncached: 180000, prompt: 64000 },
  edit: { turns: 12, gross: 220000, uncached: 120000, prompt: 48000 },
  review: { turns: 20, gross: 300000, uncached: 180000, prompt: 64000 },
  repair: { turns: 10, gross: 180000, uncached: 100000, prompt: 48000 },
};

function profile(phase) {
  const target = targets[phase];
  const enforcedLimits = {
    maxTurns: 20,
    maxToolCalls: 60,
    maxInputTokens: 200000,
    maxGrossInputTokens: 300000,
    maxUncachedInputTokens: 180000,
    maxPromptTokensPerTurn: 64000,
    maxOutputTokens: 40000,
  };
  const value = {
    schemaVersion: "run-budget-profile@1",
    profileId: `phase-budget-v1-${phase}`,
    phase,
    rolloutMode: "enforced",
    tokenBudgetMode: "split_enforced",
    operationBudgetMode: "enforced",
    enforcedLimits,
    phaseTargetLimits: phase === "review"
      ? { ...enforcedLimits }
      : {
          ...enforcedLimits,
          maxTurns: target.turns,
          maxInputTokens: target.gross,
          maxGrossInputTokens: target.gross,
          maxUncachedInputTokens: target.uncached,
          maxPromptTokensPerTurn: target.prompt,
        },
    operationLimits: {
      maxGrossInputTokens: 450000,
      maxUncachedInputTokens: 270000,
      maxOutputTokens: 80000,
      maxTurns: 30,
      maxToolCalls: 100,
    },
  };
  value.profileHash = sha256(canonicalJson(value));
  return value;
}

function evidence(profiles) {
  return {
    schemaVersion: "runtime-evidence-budget-profiles@1",
    profiles: profiles.map((value, index) => ({
      runId: `run-${index + 1}`,
      operationId: `operation-${index + 1}`,
      operationAttempt: 1,
      phase: value.phase,
      profile: value,
    })),
  };
}

test("repository Production Budget Policy approves exactly the frozen enforced profiles", () => {
  assert.doesNotThrow(() => validateReleaseBudgetPolicy(policy));
  const profiles = Object.keys(targets).map(profile);
  for (const value of profiles) {
    assert.deepEqual(policy.phaseProfiles[value.phase].allowedProfileHashes, [value.profileHash]);
  }
  const result = enforceReleaseBudgetPolicy(policy, evidence(profiles));
  assert.equal(result.status, "passed");
  assert.equal(result.runs.length, profiles.length);
});

test("Shadow and self-authorized wider profiles fail closed", () => {
  const shadow = profile("build");
  shadow.rolloutMode = "shadow";
  const { profileHash: _shadowHash, ...shadowIdentity } = shadow;
  shadow.profileHash = sha256(canonicalJson(shadowIdentity));
  assert.throws(() => enforceReleaseBudgetPolicy(policy, evidence([shadow])), /modes are not release-approved/);

  const wider = profile("build");
  wider.phaseTargetLimits.maxPromptTokensPerTurn += 1;
  const { profileHash: _oldHash, ...identity } = wider;
  wider.profileHash = sha256(canonicalJson(identity));
  const selfAuthorized = structuredClone(policy);
  selfAuthorized.phaseProfiles.build.allowedProfileHashes = [wider.profileHash];
  assert.throws(() => enforceReleaseBudgetPolicy(selfAuthorized, evidence([wider])), /exceeds the release-approved Phase limits/);
});
