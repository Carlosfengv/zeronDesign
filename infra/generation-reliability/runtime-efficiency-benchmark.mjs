#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const INPUT_SCHEMA = "runtime-efficiency-benchmark-cohort@1";
const CALCULATOR_VERSION = "runtime-efficiency-benchmark-calculator@1";
const OUTPUT_SCHEMA = "runtime-efficiency-benchmark-evaluation@1";
const LEDGER_SCHEMA = "runtime-efficiency-benchmark-ledger@1";
const MIN_ACCEPTED_ATTEMPTS = 30;
const MIN_PROMPTS = 10;
const SHA256 = /^[a-f0-9]{64}$/;
const WORKLOADS = new Set(["greenfield_build", "style_token_edit"]);
const VARIANTS = ["baseline", "candidate"];
const STATUSES = new Set(["accepted", "failed", "partial", "timeout", "rejected"]);
const CACHE_CAPABILITIES = new Set(["reported", "unsupported"]);
const FORBIDDEN_KEY = /(?:api_?key|authorization|credential|secret|prompt_?(?:text|body|content)|source_?content|provider_?(?:response|body)|request_?body|response_?body|signed_?url|temporary_?url)/i;
const SECRET_VALUE = /(?:\bBearer\s+[A-Za-z0-9._~+/=-]{8,}|\bsk-[A-Za-z0-9_-]{8,}|data:image\/)/i;

const PROFILE_KEYS = [
  "profileId",
  "workload",
  "designProfileHash",
  "templateId",
  "templateVersion",
  "modelResourceId",
  "providerResourceRevision",
  "modelVersion",
  "providerParametersHash",
  "cacheUsageCapability",
  "attempts",
];
const ATTEMPT_KEYS = [
  "sequence",
  "attemptId",
  "variant",
  "promptId",
  "status",
  "terminalEvidenceSha256",
  "metrics",
];
const METRIC_KEYS = [
  "modelTurns",
  "grossInputTokens",
  "uncachedInputTokens",
  "maxPromptTokensPerTurn",
  "cacheHitRateBasisPoints",
  "firstSourceMutationTurn",
  "generationContextBytes",
  "duplicateFullContextReads",
  "outOfScopeMutations",
  "requiredFidelityPassed",
];
const NUMERIC_METRICS = METRIC_KEYS.filter(key => key !== "requiredFidelityPassed");

const THRESHOLDS = {
  greenfield_build: {
    modelTurns: { p50: 8, p95: 12 },
    grossInputTokens: { p50: 140_000, p95: 180_000 },
    uncachedInputTokens: { p50: 90_000 },
    maxPromptTokensPerTurn: { max: 16_000 },
    cacheHitRateBasisPoints: { minP50: 6_000 },
    firstSourceMutationTurn: { max: 2 },
  },
  style_token_edit: {
    modelTurns: { p50: 6, p95: 9 },
    grossInputTokens: { p50: 100_000, p95: 140_000 },
    uncachedInputTokens: { p50: 65_000 },
    maxPromptTokensPerTurn: { max: 14_000 },
    cacheHitRateBasisPoints: { minP50: 6_000 },
    firstSourceMutationTurn: { max: 3 },
  },
};

function exactKeys(value, keys, location, errors) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    errors.push(`${location} must be an object`);
    return false;
  }
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    errors.push(`${location} keys must be exactly ${expected.join(",")}`);
    return false;
  }
  return true;
}

function sensitiveMaterialErrors(value, location = "cohort", errors = []) {
  if (typeof value === "string") {
    if (SECRET_VALUE.test(value)) errors.push(`${location} contains credential or payload material`);
    return errors;
  }
  if (Array.isArray(value)) {
    value.forEach((entry, index) => sensitiveMaterialErrors(entry, `${location}[${index}]`, errors));
    return errors;
  }
  if (!value || typeof value !== "object") return errors;
  for (const [key, child] of Object.entries(value)) {
    const normalized = key.replaceAll(/([a-z])([A-Z])/g, "$1_$2");
    if (FORBIDDEN_KEY.test(normalized)) errors.push(`${location}.${key} is forbidden in benchmark evidence`);
    sensitiveMaterialErrors(child, `${location}.${key}`, errors);
  }
  return errors;
}

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function nonNegativeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function nullableNonNegativeInteger(value) {
  return value === null || nonNegativeInteger(value);
}

function quantile(values, percentile) {
  if (!values.length) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const index = (sorted.length - 1) * percentile;
  const lower = Math.floor(index);
  const upper = Math.ceil(index);
  if (lower === upper) return sorted[lower];
  return sorted[lower] + (sorted[upper] - sorted[lower]) * (index - lower);
}

function seededRandom(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 0x1_0000_0000;
  };
}

function bootstrap(values, iterations, seed, statistic) {
  if (!values.length) return [null, null];
  const random = seededRandom(seed);
  const samples = [];
  for (let iteration = 0; iteration < iterations; iteration += 1) {
    const sample = Array.from(
      { length: values.length },
      () => values[Math.floor(random() * values.length)],
    );
    const result = statistic(sample);
    if (typeof result === "number" && Number.isFinite(result)) samples.push(result);
  }
  return [quantile(samples, 0.025), quantile(samples, 0.975)];
}

function statistic(values, percentile, ready, iterations, seed) {
  if (!ready) {
    return { status: "insufficient_sample", value: null, interval95: [null, null] };
  }
  return {
    status: "ready",
    value: quantile(values, percentile),
    interval95: bootstrap(values, iterations, seed, sample => quantile(sample, percentile)),
  };
}

function distributions(attempts, ready, iterations, seed, cacheCapability) {
  return Object.fromEntries(NUMERIC_METRICS.map((metric, index) => {
    const values = attempts.map(attempt => attempt.metrics[metric]).filter(value => value !== null);
    if (metric === "cacheHitRateBasisPoints" && cacheCapability === "unsupported") {
      return [metric, {
        sampleCount: 0,
        p50: { status: "not_applicable", value: null, interval95: [null, null] },
        p95: { status: "not_applicable", value: null, interval95: [null, null] },
      }];
    }
    return [metric, {
      sampleCount: values.length,
      p50: statistic(values, 0.5, ready, iterations, seed + index * 17),
      p95: statistic(values, 0.95, ready, iterations, seed + index * 17 + 1),
    }];
  }));
}

function gate(name, value, comparison, threshold) {
  return {
    name,
    value,
    threshold,
    status: comparison(value, threshold) ? "pass" : "fail",
  };
}

function candidateGates(profile, accepted, summary) {
  const thresholds = THRESHOLDS[profile.workload];
  const metric = name => accepted.map(attempt => attempt.metrics[name]);
  const gates = [];
  for (const [name, limits] of Object.entries(thresholds)) {
    if (name === "cacheHitRateBasisPoints" && profile.cacheUsageCapability === "unsupported") {
      gates.push({
        name: `${name}.p50`,
        value: null,
        threshold: limits.minP50,
        status: "not_applicable",
      });
      continue;
    }
    if (limits.p50 !== undefined) {
      gates.push(gate(`${name}.p50`, summary[name].p50.value, (value, maximum) => value <= maximum, limits.p50));
    }
    if (limits.p95 !== undefined) {
      gates.push(gate(`${name}.p95`, summary[name].p95.value, (value, maximum) => value <= maximum, limits.p95));
    }
    if (limits.max !== undefined) {
      gates.push(gate(`${name}.max`, Math.max(...metric(name)), (value, maximum) => value <= maximum, limits.max));
    }
    if (limits.minP50 !== undefined) {
      gates.push(gate(`${name}.p50`, summary[name].p50.value, (value, minimum) => value >= minimum, limits.minP50));
    }
  }
  gates.push(gate(
    "generationContextBytes.max",
    Math.max(...metric("generationContextBytes")),
    (value, maximum) => value <= maximum,
    65_536,
  ));
  gates.push(gate(
    "duplicateFullContextReads.sum",
    metric("duplicateFullContextReads").reduce((sum, value) => sum + value, 0),
    (value, expected) => value === expected,
    0,
  ));
  gates.push(gate(
    "outOfScopeMutations.sum",
    metric("outOfScopeMutations").reduce((sum, value) => sum + value, 0),
    (value, expected) => value === expected,
    0,
  ));
  const fidelityPassed = accepted.filter(attempt => attempt.metrics.requiredFidelityPassed).length;
  gates.push(gate(
    "requiredFidelityPassed.rateBasisPoints",
    Math.floor(fidelityPassed * 10_000 / accepted.length),
    (value, expected) => value === expected,
    10_000,
  ));
  return gates;
}

function effectSizes(baseline, candidate, ready, iterations, seed) {
  const result = {};
  for (const [index, metric] of NUMERIC_METRICS.entries()) {
    if (!ready) {
      result[metric] = {
        kind: metric === "cacheHitRateBasisPoints" ? "median_difference" : "relative_median_reduction",
        status: "insufficient_sample",
        value: null,
        interval95: [null, null],
      };
      continue;
    }
    const baselineValues = baseline.map(attempt => attempt.metrics[metric]).filter(value => value !== null);
    const candidateValues = candidate.map(attempt => attempt.metrics[metric]).filter(value => value !== null);
    if (!baselineValues.length || !candidateValues.length) {
      result[metric] = {
        kind: metric === "cacheHitRateBasisPoints" ? "median_difference" : "relative_median_reduction",
        status: "not_applicable",
        value: null,
        interval95: [null, null],
      };
      continue;
    }
    const calculate = (left, right) => {
      const leftMedian = quantile(left, 0.5);
      const rightMedian = quantile(right, 0.5);
      if (metric === "cacheHitRateBasisPoints") return rightMedian - leftMedian;
      return leftMedian > 0 ? 1 - rightMedian / leftMedian : null;
    };
    const value = calculate(baselineValues, candidateValues);
    if (value === null) {
      result[metric] = {
        kind: metric === "cacheHitRateBasisPoints" ? "median_difference" : "relative_median_reduction",
        status: "not_applicable",
        value: null,
        interval95: [null, null],
      };
      continue;
    }
    const random = seededRandom(seed + index * 31);
    const bootstrapped = [];
    for (let iteration = 0; iteration < iterations; iteration += 1) {
      const resample = values => Array.from(
        { length: values.length },
        () => values[Math.floor(random() * values.length)],
      );
      const sampleValue = calculate(resample(baselineValues), resample(candidateValues));
      if (typeof sampleValue === "number" && Number.isFinite(sampleValue)) bootstrapped.push(sampleValue);
    }
    result[metric] = {
      kind: metric === "cacheHitRateBasisPoints" ? "median_difference" : "relative_median_reduction",
      status: "ready",
      value,
      interval95: [quantile(bootstrapped, 0.025), quantile(bootstrapped, 0.975)],
    };
  }
  return result;
}

function validateAttempt(attempt, location, promptIds, cacheCapability, errors) {
  if (!exactKeys(attempt, ATTEMPT_KEYS, location, errors)) return;
  if (!Number.isSafeInteger(attempt.sequence) || attempt.sequence <= 0) errors.push(`${location}.sequence must be positive`);
  if (!nonEmptyString(attempt.attemptId)) errors.push(`${location}.attemptId is required`);
  if (!VARIANTS.includes(attempt.variant)) errors.push(`${location}.variant is invalid`);
  if (!promptIds.has(attempt.promptId)) errors.push(`${location}.promptId is not in the frozen Prompt Set`);
  if (!STATUSES.has(attempt.status)) errors.push(`${location}.status is invalid`);
  if (!SHA256.test(attempt.terminalEvidenceSha256 ?? "")) errors.push(`${location}.terminalEvidenceSha256 must be sha256`);
  if (!exactKeys(attempt.metrics, METRIC_KEYS, `${location}.metrics`, errors)) return;
  for (const metric of NUMERIC_METRICS) {
    if (!nullableNonNegativeInteger(attempt.metrics[metric])) errors.push(`${location}.metrics.${metric} must be a non-negative integer or null`);
  }
  if (![true, false, null].includes(attempt.metrics.requiredFidelityPassed)) {
    errors.push(`${location}.metrics.requiredFidelityPassed must be boolean or null`);
  }
  if (attempt.status === "accepted") {
    for (const metric of NUMERIC_METRICS) {
      if (metric === "cacheHitRateBasisPoints" && cacheCapability === "unsupported") continue;
      if (!nonNegativeInteger(attempt.metrics[metric])) errors.push(`${location}.metrics.${metric} is required for accepted Attempts`);
    }
    if (cacheCapability === "unsupported" && attempt.metrics.cacheHitRateBasisPoints !== null) {
      errors.push(`${location}.metrics.cacheHitRateBasisPoints must be null when cache usage is unsupported`);
    }
    for (const metric of [
      "modelTurns",
      "grossInputTokens",
      "maxPromptTokensPerTurn",
      "firstSourceMutationTurn",
    ]) {
      if (attempt.metrics[metric] <= 0) errors.push(`${location}.metrics.${metric} must be positive for accepted Attempts`);
    }
    if (attempt.metrics.cacheHitRateBasisPoints !== null
      && attempt.metrics.cacheHitRateBasisPoints > 10_000) {
      errors.push(`${location}.metrics.cacheHitRateBasisPoints must be <= 10000`);
    }
    if (typeof attempt.metrics.requiredFidelityPassed !== "boolean") {
      errors.push(`${location}.metrics.requiredFidelityPassed is required for accepted Attempts`);
    }
  }
}

export function validateRuntimeEfficiencyBenchmarkAttempt(attempt, profile, promptSet) {
  const errors = sensitiveMaterialErrors(attempt, "attempt");
  const promptIds = new Set(Array.isArray(promptSet?.promptIds) ? promptSet.promptIds : []);
  validateAttempt(attempt, "attempt", promptIds, profile?.cacheUsageCapability, errors);
  return errors;
}

function validateCohort(cohort) {
  const errors = sensitiveMaterialErrors(cohort);
  if (!exactKeys(
    cohort,
    ["schemaVersion", "calculatorVersion", "source", "bootstrap", "promptSet", "ledger", "profiles"],
    "cohort",
    errors,
  )) return errors;
  if (cohort.schemaVersion !== INPUT_SCHEMA) errors.push(`schemaVersion must be ${INPUT_SCHEMA}`);
  if (cohort.calculatorVersion !== CALCULATOR_VERSION) errors.push(`calculatorVersion must be ${CALCULATOR_VERSION}`);
  if (exactKeys(cohort.source, ["commit", "dirty"], "cohort.source", errors)) {
    if (!nonEmptyString(cohort.source.commit)) errors.push("cohort.source.commit is required");
    if (typeof cohort.source.dirty !== "boolean") errors.push("cohort.source.dirty must be boolean");
  }
  if (exactKeys(cohort.bootstrap, ["iterations", "seed"], "cohort.bootstrap", errors)) {
    if (!Number.isSafeInteger(cohort.bootstrap.iterations) || cohort.bootstrap.iterations < 100) {
      errors.push("cohort.bootstrap.iterations must be >= 100");
    }
    if (!Number.isSafeInteger(cohort.bootstrap.seed)) errors.push("cohort.bootstrap.seed must be an integer");
  }
  let promptIds = new Set();
  if (exactKeys(cohort.promptSet, ["id", "version", "sha256", "promptIds"], "cohort.promptSet", errors)) {
    if (!nonEmptyString(cohort.promptSet.id) || !nonEmptyString(cohort.promptSet.version)) {
      errors.push("cohort.promptSet id and version are required");
    }
    if (!SHA256.test(cohort.promptSet.sha256 ?? "")) errors.push("cohort.promptSet.sha256 must be sha256");
    if (!Array.isArray(cohort.promptSet.promptIds)) {
      errors.push("cohort.promptSet.promptIds must be an array");
    } else {
      promptIds = new Set(cohort.promptSet.promptIds);
      if (promptIds.size !== cohort.promptSet.promptIds.length || [...promptIds].some(id => !nonEmptyString(id))) {
        errors.push("cohort.promptSet.promptIds must be unique non-empty strings");
      }
      if (promptIds.size < MIN_PROMPTS) errors.push(`cohort.promptSet must contain at least ${MIN_PROMPTS} Prompt IDs`);
    }
  }
  if (exactKeys(
    cohort.ledger,
    ["schemaVersion", "sha256", "firstSequence", "lastSequence", "recordCount"],
    "cohort.ledger",
    errors,
  )) {
    if (cohort.ledger.schemaVersion !== LEDGER_SCHEMA) errors.push(`cohort.ledger.schemaVersion must be ${LEDGER_SCHEMA}`);
    if (!SHA256.test(cohort.ledger.sha256 ?? "")) errors.push("cohort.ledger.sha256 must be sha256");
    for (const field of ["firstSequence", "lastSequence", "recordCount"]) {
      if (!Number.isSafeInteger(cohort.ledger[field]) || cohort.ledger[field] <= 0) {
        errors.push(`cohort.ledger.${field} must be a positive integer`);
      }
    }
  }
  if (!Array.isArray(cohort.profiles) || cohort.profiles.length === 0) {
    errors.push("cohort.profiles must be a non-empty array");
    return errors;
  }
  const profileIds = new Set();
  const attemptIds = new Set();
  const sequences = [];
  for (const [profileIndex, profile] of cohort.profiles.entries()) {
    const location = `cohort.profiles[${profileIndex}]`;
    if (!exactKeys(profile, PROFILE_KEYS, location, errors)) continue;
    if (!nonEmptyString(profile.profileId) || profileIds.has(profile.profileId)) errors.push(`${location}.profileId must be unique`);
    profileIds.add(profile.profileId);
    if (!WORKLOADS.has(profile.workload)) errors.push(`${location}.workload is invalid`);
    for (const field of ["designProfileHash", "providerParametersHash"]) {
      if (!SHA256.test(profile[field] ?? "")) errors.push(`${location}.${field} must be sha256`);
    }
    for (const field of ["templateId", "templateVersion", "modelResourceId", "modelVersion"]) {
      if (!nonEmptyString(profile[field])) errors.push(`${location}.${field} is required`);
    }
    if (!Number.isSafeInteger(profile.providerResourceRevision) || profile.providerResourceRevision <= 0) {
      errors.push(`${location}.providerResourceRevision must be positive`);
    }
    if (!CACHE_CAPABILITIES.has(profile.cacheUsageCapability)) errors.push(`${location}.cacheUsageCapability is invalid`);
    if (!Array.isArray(profile.attempts) || profile.attempts.length === 0) {
      errors.push(`${location}.attempts must be a non-empty array`);
      continue;
    }
    for (const [attemptIndex, attempt] of profile.attempts.entries()) {
      const attemptLocation = `${location}.attempts[${attemptIndex}]`;
      validateAttempt(attempt, attemptLocation, promptIds, profile.cacheUsageCapability, errors);
      if (attemptIds.has(attempt?.attemptId)) errors.push(`${attemptLocation}.attemptId must be globally unique`);
      attemptIds.add(attempt?.attemptId);
      if (Number.isSafeInteger(attempt?.sequence)) sequences.push(attempt.sequence);
    }
  }
  sequences.sort((left, right) => left - right);
  const ledger = cohort.ledger ?? {};
  if (sequences.length !== ledger.recordCount
    || sequences[0] !== ledger.firstSequence
    || sequences.at(-1) !== ledger.lastSequence
    || sequences.some((sequence, index) => sequence !== ledger.firstSequence + index)) {
    errors.push("cohort ledger sequences must be complete, continuous, unique, and match recordCount");
  }
  return errors;
}

export function evaluateRuntimeEfficiencyBenchmark(cohort) {
  const errors = validateCohort(cohort);
  if (errors.length) {
    return {
      schemaVersion: OUTPUT_SCHEMA,
      calculatorVersion: cohort?.calculatorVersion ?? null,
      errors,
      profiles: {},
      result: "invalid",
    };
  }
  const iterations = cohort.bootstrap.iterations;
  const seed = cohort.bootstrap.seed;
  const profiles = {};
  for (const [profileIndex, profile] of cohort.profiles.entries()) {
    const variants = {};
    let sampleReady = true;
    const acceptedByVariant = {};
    for (const [variantIndex, variant] of VARIANTS.entries()) {
      const attempts = profile.attempts.filter(attempt => attempt.variant === variant);
      const accepted = attempts.filter(attempt => attempt.status === "accepted");
      const promptCount = new Set(accepted.map(attempt => attempt.promptId)).size;
      const ready = accepted.length >= MIN_ACCEPTED_ATTEMPTS && promptCount >= MIN_PROMPTS;
      sampleReady &&= ready;
      acceptedByVariant[variant] = accepted;
      variants[variant] = {
        attemptCount: attempts.length,
        acceptedCount: accepted.length,
        failedCount: attempts.length - accepted.length,
        acceptedPromptCount: promptCount,
        minimumAcceptedAttempts: MIN_ACCEPTED_ATTEMPTS,
        minimumPromptCount: MIN_PROMPTS,
        sampleStatus: ready ? "ready" : "insufficient_sample",
        failureStatusCounts: Object.fromEntries(
          [...STATUSES]
            .filter(status => status !== "accepted")
            .map(status => [status, attempts.filter(attempt => attempt.status === status).length]),
        ),
        distributions: distributions(
          accepted,
          ready,
          iterations,
          seed + profileIndex * 10_003 + variantIndex * 1_003,
          profile.cacheUsageCapability,
        ),
      };
    }
    const gates = sampleReady
      ? candidateGates(profile, acceptedByVariant.candidate, variants.candidate.distributions)
      : [];
    const status = !sampleReady
      ? "insufficient_sample"
      : gates.some(item => item.status === "fail") ? "fail" : "pass";
    profiles[profile.profileId] = {
      workload: profile.workload,
      identity: {
        designProfileHash: profile.designProfileHash,
        templateId: profile.templateId,
        templateVersion: profile.templateVersion,
        modelResourceId: profile.modelResourceId,
        providerResourceRevision: profile.providerResourceRevision,
        modelVersion: profile.modelVersion,
        providerParametersHash: profile.providerParametersHash,
        cacheUsageCapability: profile.cacheUsageCapability,
        promptSetId: cohort.promptSet.id,
        promptSetVersion: cohort.promptSet.version,
        promptSetSha256: cohort.promptSet.sha256,
        ledgerSha256: cohort.ledger.sha256,
      },
      variants,
      effectSizes: effectSizes(
        acceptedByVariant.baseline,
        acceptedByVariant.candidate,
        sampleReady,
        iterations,
        seed + profileIndex * 100_003,
      ),
      gates,
      status,
    };
  }
  const values = Object.values(profiles);
  const result = values.some(profile => profile.status === "insufficient_sample")
    ? "insufficient_sample"
    : values.some(profile => profile.status === "fail") ? "fail" : "pass";
  return {
    schemaVersion: OUTPUT_SCHEMA,
    calculatorVersion: cohort.calculatorVersion,
    errors: [],
    source: cohort.source,
    promptSet: cohort.promptSet,
    ledger: cohort.ledger,
    profiles,
    result,
  };
}

async function main() {
  const path = process.argv[2];
  if (!path) throw new Error("usage: runtime-efficiency-benchmark.mjs <benchmark-cohort.json>");
  const result = evaluateRuntimeEfficiencyBenchmark(JSON.parse(await readFile(path, "utf8")));
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  if (result.result !== "pass") process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
