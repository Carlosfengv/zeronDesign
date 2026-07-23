import {
  canonicalJson,
  validateRunBudgetProfile,
} from "./runtime-budget-evidence.mjs";

const HASH = /^[a-f0-9]{64}$/;
const TOKEN_FIELDS = [
  "maxTurns",
  "maxToolCalls",
  "maxInputTokens",
  "maxGrossInputTokens",
  "maxUncachedInputTokens",
  "maxPromptTokensPerTurn",
  "maxOutputTokens",
];
const OPERATION_FIELDS = [
  "maxGrossInputTokens",
  "maxUncachedInputTokens",
  "maxOutputTokens",
  "maxTurns",
  "maxToolCalls",
];

function invariant(condition, message) {
  if (!condition) throw new Error(message);
}

function positiveInteger(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function exactKeys(value, expected, label) {
  invariant(value && typeof value === "object" && !Array.isArray(value), `${label} must be an object`);
  invariant(canonicalJson(Object.keys(value).sort()) === canonicalJson([...expected].sort()),
    `${label} fields do not match the frozen schema`);
}

export function validateReleaseBudgetPolicy(policy) {
  exactKeys(policy, ["schemaVersion", "releaseStage", "requiredModes", "phaseProfiles"],
    "Release Budget Policy");
  invariant(policy.schemaVersion === "runtime-release-budget-policy@1",
    "unsupported Release Budget Policy schema");
  invariant(policy.releaseStage === "production", "Release Budget Policy must target production");
  exactKeys(policy.requiredModes, ["rolloutMode", "tokenBudgetMode", "operationBudgetMode"],
    "Release Budget Policy requiredModes");
  invariant(policy.requiredModes.rolloutMode === "enforced"
    && policy.requiredModes.tokenBudgetMode === "split_enforced"
    && policy.requiredModes.operationBudgetMode === "enforced",
  "production Release Budget Policy must require enforced modes");
  invariant(policy.phaseProfiles && typeof policy.phaseProfiles === "object"
    && !Array.isArray(policy.phaseProfiles)
    && Object.keys(policy.phaseProfiles).length > 0,
  "Release Budget Policy phaseProfiles are required");
  for (const [phase, phasePolicy] of Object.entries(policy.phaseProfiles)) {
    invariant(typeof phase === "string" && phase.length > 0, "Release Budget Policy phase is invalid");
    exactKeys(phasePolicy, ["allowedProfileHashes", "maximumLimits", "maximumOperationLimits"],
      `Release Budget Policy phaseProfiles.${phase}`);
    invariant(Array.isArray(phasePolicy.allowedProfileHashes)
      && phasePolicy.allowedProfileHashes.length > 0
      && new Set(phasePolicy.allowedProfileHashes).size === phasePolicy.allowedProfileHashes.length
      && phasePolicy.allowedProfileHashes.every((hash) => HASH.test(hash)),
    `Release Budget Policy ${phase} allowedProfileHashes are invalid`);
    exactKeys(phasePolicy.maximumLimits, TOKEN_FIELDS,
      `Release Budget Policy phaseProfiles.${phase}.maximumLimits`);
    exactKeys(phasePolicy.maximumOperationLimits, OPERATION_FIELDS,
      `Release Budget Policy phaseProfiles.${phase}.maximumOperationLimits`);
    invariant(TOKEN_FIELDS.every((field) => positiveInteger(phasePolicy.maximumLimits[field])),
      `Release Budget Policy ${phase} maximumLimits must be positive integers`);
    invariant(OPERATION_FIELDS.every((field) => positiveInteger(phasePolicy.maximumOperationLimits[field])),
      `Release Budget Policy ${phase} maximumOperationLimits must be positive integers`);
  }
  return policy;
}

export function enforceReleaseBudgetPolicy(policy, budgetEvidence) {
  validateReleaseBudgetPolicy(policy);
  invariant(budgetEvidence?.schemaVersion === "runtime-evidence-budget-profiles@1"
    && Array.isArray(budgetEvidence.profiles)
    && budgetEvidence.profiles.length > 0,
  "Release Budget Policy requires Budget Profiles evidence");
  const results = budgetEvidence.profiles.map((entry) => {
    const profile = validateRunBudgetProfile(entry.profile, { phase: entry.phase });
    const phasePolicy = policy.phaseProfiles[entry.phase];
    invariant(phasePolicy, `Release Budget Policy does not approve phase ${entry.phase}`);
    invariant(profile.rolloutMode === policy.requiredModes.rolloutMode
      && profile.tokenBudgetMode === policy.requiredModes.tokenBudgetMode
      && profile.operationBudgetMode === policy.requiredModes.operationBudgetMode,
    `${entry.runId} Budget Profile modes are not release-approved`);
    invariant(phasePolicy.allowedProfileHashes.includes(profile.profileHash),
      `${entry.runId} Budget Profile hash is not release-approved`);
    const effectiveLimits = profile.rolloutMode === "enforced"
      ? profile.phaseTargetLimits
      : profile.enforcedLimits;
    invariant(TOKEN_FIELDS.every((field) => effectiveLimits[field] <= phasePolicy.maximumLimits[field]),
      `${entry.runId} Budget Profile exceeds the release-approved Phase limits`);
    invariant(OPERATION_FIELDS.every((field) =>
      profile.operationLimits[field] <= phasePolicy.maximumOperationLimits[field]),
    `${entry.runId} Budget Profile exceeds the release-approved Operation limits`);
    return {
      runId: entry.runId,
      phase: entry.phase,
      profileHash: profile.profileHash,
      passed: true,
    };
  });
  return {
    schemaVersion: "runtime-release-budget-policy-result@1",
    status: "passed",
    releaseStage: policy.releaseStage,
    requiredModes: policy.requiredModes,
    runs: results,
  };
}
