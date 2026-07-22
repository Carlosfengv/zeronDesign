#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const REQUIRED_BUCKETS = ["greenfield", "warm_copy_css", "warm_structural", "cold_dev", "repair"];
const IDENTITY_FIELDS = [
  "fixtureId", "modelResource", "modelVersion", "providerParametersHash",
  "templateVersion", "capabilitySnapshotHash", "phase",
];
const SHA256 = /^[a-f0-9]{64}$/;
const FORBIDDEN_KEY = /(?:^|_)(?:api_?key|authorization|credential|secret|prompt|text|source_?content|image_?(?:bytes|url)|provider_?(?:response|body)|request_?body|response_?body|signed_?url|temporary_?url)(?:$|_)/i;
const SECRET_VALUE = /(?:\bBearer\s+[A-Za-z0-9._~+/=-]{8,}|\bsk-[A-Za-z0-9_-]{8,}|data:image\/)/i;

function sensitiveMaterialErrors(value, location = "evidence", errors = []) {
  if (typeof value === "string") {
    if (SECRET_VALUE.test(value)) errors.push(`${location} contains credential, bearer, or image payload material`);
    return errors;
  }
  if (Array.isArray(value)) {
    value.forEach((entry, index) => sensitiveMaterialErrors(entry, `${location}[${index}]`, errors));
    return errors;
  }
  if (!value || typeof value !== "object") return errors;
  for (const [key, child] of Object.entries(value)) {
    const normalized = key.replaceAll(/([a-z])([A-Z])/g, "$1_$2");
    if (FORBIDDEN_KEY.test(normalized)) errors.push(`${location}.${key} is forbidden in hashes-only rollout evidence`);
    sensitiveMaterialErrors(child, `${location}.${key}`, errors);
  }
  return errors;
}

function finite(value) {
  return typeof value === "number" && Number.isFinite(value);
}

function quantile(values, percentile) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const index = (sorted.length - 1) * percentile;
  const lower = Math.floor(index);
  const upper = Math.ceil(index);
  if (lower === upper) return sorted[lower];
  return sorted[lower] + (sorted[upper] - sorted[lower]) * (index - lower);
}

function median(values) {
  return quantile(values, 0.5);
}

function seededRandom(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return state / 0x1_0000_0000;
  };
}

function interval(values) {
  return values.length ? [quantile(values, 0.025), quantile(values, 0.975)] : [null, null];
}

function bootstrap(pairs, iterations, seed, statistic) {
  if (pairs.length === 0) return [null, null];
  const random = seededRandom(seed);
  const values = [];
  for (let iteration = 0; iteration < iterations; iteration += 1) {
    const sample = Array.from({ length: pairs.length }, () => pairs[Math.floor(random() * pairs.length)]);
    const value = statistic(sample);
    if (finite(value)) values.push(value);
  }
  return interval(values);
}

function metricValues(pairs, side, metric) {
  return pairs.map(pair => pair[side]?.metrics?.[metric]).filter(finite);
}

function relativeMedianReduction(pairs, metric) {
  const control = median(metricValues(pairs, "control", metric));
  const candidate = median(metricValues(pairs, "candidate", metric));
  if (!finite(control) || !finite(candidate) || control <= 0) return null;
  return 1 - candidate / control;
}

function rateDifference(pairs, field) {
  if (!pairs.length || pairs.some(pair => typeof pair.control?.[field] !== "boolean"
    || typeof pair.candidate?.[field] !== "boolean")) return null;
  const rate = side => pairs.filter(pair => pair[side][field]).length / pairs.length;
  return rate("candidate") - rate("control");
}

function gate(name, value, passed, interval95 = null) {
  return { name, value, interval95, status: value === null ? "insufficient_evidence" : passed ? "pass" : "fail" };
}

function bucketGates(bucket, pairs, iterations, seed) {
  const gates = [];
  const reductions = [
    ["duplicateReadTokens", 0.8],
    ["inputTokens", 0.5],
  ];
  if (bucket === "greenfield") reductions.push(["timeToFirstGreenfieldBuildMs", 0.4]);
  for (const [metric, threshold] of reductions) {
    const value = relativeMedianReduction(pairs, metric);
    const interval95 = bootstrap(pairs, iterations, seed + gates.length, sample => relativeMedianReduction(sample, metric));
    gates.push(gate(`${metric}.medianReduction`, value, value >= threshold && interval95[0] >= threshold, interval95));
  }

  const candidate = metric => metricValues(pairs, "candidate", metric);
  const absolute = (metric, percentile, maximum) => {
    const value = quantile(candidate(metric), percentile);
    gates.push(gate(`${metric}.p${percentile * 100}`, value, value <= maximum));
  };
  if (bucket === "cold_dev") {
    absolute("coldDevReadyMs", 0.5, 15_000);
    absolute("coldDevReadyMs", 0.95, 30_000);
  }
  if (bucket === "warm_copy_css") {
    absolute("timeToIframeAppliedMs", 0.5, 1_000);
    absolute("timeToIframeAppliedMs", 0.95, 3_000);
  }
  if (bucket === "warm_structural") {
    absolute("timeToIframeAppliedMs", 0.5, 2_000);
    absolute("timeToIframeAppliedMs", 0.95, 5_000);
  }
  if (["warm_copy_css", "warm_structural", "cold_dev", "repair"].includes(bucket)) {
    absolute("timeToDurableSnapshotMs", 0.95, 5_000);
  }

  const firstMutationTurns = candidate("modelTurnAtFirstSourceMutation");
  gates.push(gate(
    "modelTurnAtFirstSourceMutation.max",
    firstMutationTurns.length === pairs.length ? Math.max(...firstMutationTurns) : null,
    firstMutationTurns.length === pairs.length && Math.max(...firstMutationTurns) <= 2,
  ));
  const prebuildExploration = pairs.map(pair => {
    const metrics = pair.candidate?.metrics;
    return finite(metrics?.prebuildFsListCount) && finite(metrics?.prebuildFsSearchCount)
      ? metrics.prebuildFsListCount + metrics.prebuildFsSearchCount
      : null;
  });
  gates.push(gate(
    "prebuildListAndSearch.max",
    prebuildExploration.every(finite) ? Math.max(...prebuildExploration) : null,
    prebuildExploration.every(finite) && Math.max(...prebuildExploration) <= 2,
  ));
  const duplicateRates = candidate("duplicateFullReadRateBasisPoints");
  gates.push(gate(
    "duplicateFullReadRate.maxBasisPoints",
    duplicateRates.length === pairs.length ? Math.max(...duplicateRates) : null,
    duplicateRates.length === pairs.length && Math.max(...duplicateRates) < 500,
  ));
  const outOfScope = candidate("outOfScopeMutationCount");
  gates.push(gate(
    "outOfScopeMutationCount.sum",
    outOfScope.length === pairs.length ? outOfScope.reduce((sum, value) => sum + value, 0) : null,
    outOfScope.length === pairs.length && outOfScope.every(value => value === 0),
  ));

  for (const [field, boundary] of [["firstBuildSucceeded", -0.05], ["requiredFidelityPassed", -0.02]]) {
    const value = rateDifference(pairs, field);
    const interval95 = bootstrap(pairs, iterations, seed + gates.length, sample => rateDifference(sample, field));
    gates.push(gate(`${field}.rateDifference`, value, value >= boundary && interval95[0] >= boundary, interval95));
  }
  return gates;
}

function bucketDistributions(pairs, iterations, seed) {
  const metrics = [
    "duplicateReadTokens",
    "inputTokens",
    "timeToFirstGreenfieldBuildMs",
    "coldDevReadyMs",
    "timeToIframeAppliedMs",
    "timeToDurableSnapshotMs",
  ];
  return Object.fromEntries(metrics.map((metric, metricIndex) => {
    const summarize = (side, sideIndex) => {
      const values = metricValues(pairs, side, metric);
      const statistic = percentile => sample => quantile(metricValues(sample, side, metric), percentile);
      return {
        sampleCount: values.length,
        median: {
          value: median(values),
          interval95: bootstrap(pairs, iterations, seed + metricIndex * 11 + sideIndex, statistic(0.5)),
        },
        p95: {
          value: quantile(values, 0.95),
          interval95: bootstrap(pairs, iterations, seed + metricIndex * 11 + sideIndex + 2, statistic(0.95)),
        },
      };
    };
    return [metric, { control: summarize("control", 0), candidate: summarize("candidate", 1) }];
  }));
}

export function evaluateRolloutEvidence(evidence) {
  const errors = sensitiveMaterialErrors(evidence);
  if (evidence?.schemaVersion !== "generation-context-rollout-evidence@1") errors.push("unsupported schemaVersion");
  if (evidence?.calculatorVersion !== "generation-context-rollout-calculator@1") errors.push("unsupported calculatorVersion");
  const iterations = evidence?.bootstrap?.iterations;
  const seed = evidence?.bootstrap?.seed;
  if (!Number.isInteger(iterations) || iterations < 100 || !Number.isInteger(seed)) {
    errors.push("bootstrap iterations must be >= 100 and seed must be an integer");
  }
  const pairs = Array.isArray(evidence?.pairs) ? evidence.pairs : [];
  const ids = new Set();
  for (const [index, pair] of pairs.entries()) {
    if (!pair?.id || ids.has(pair.id)) errors.push(`pairs[${index}].id must be unique`);
    ids.add(pair?.id);
    if (!REQUIRED_BUCKETS.includes(pair?.bucket)) errors.push(`pairs[${index}].bucket is invalid`);
    if (!pair?.batchId) errors.push(`pairs[${index}].batchId is required`);
    for (const field of IDENTITY_FIELDS) {
      if (!pair?.identity?.[field]) errors.push(`pairs[${index}].identity.${field} is required`);
    }
    if (!Number.isSafeInteger(pair?.identity?.providerResourceRevision)
      || pair.identity.providerResourceRevision <= 0) {
      errors.push(`pairs[${index}].identity.providerResourceRevision must be a positive integer`);
    }
    for (const field of ["providerParametersHash", "capabilitySnapshotHash"]) {
      if (!SHA256.test(pair?.identity?.[field] ?? "")) errors.push(`pairs[${index}].identity.${field} must be sha256`);
    }
    if (pair?.control?.flag !== "legacy" || pair?.candidate?.flag !== "generation_context") {
      errors.push(`pairs[${index}] must compare legacy control with generation_context candidate`);
    }
    for (const side of ["control", "candidate"]) {
      if (!["completed", "failed", "timeout", "fallback"].includes(pair?.[side]?.status)) {
        errors.push(`pairs[${index}].${side}.status is invalid`);
      }
      if (!pair?.[side]?.source?.storageRef || !SHA256.test(pair?.[side]?.source?.contentSha256 ?? "")) {
        errors.push(`pairs[${index}].${side}.source must contain storageRef and contentSha256`);
      }
      if (!SHA256.test(pair?.[side]?.acceptanceEvidenceSha256 ?? "")) {
        errors.push(`pairs[${index}].${side}.acceptanceEvidenceSha256 must be sha256`);
      }
      const execution = pair?.[side]?.execution;
      if (execution?.gatewayMode !== "internal_gateway"
        || execution?.modelResourceId !== pair?.identity?.modelResource
        || execution?.providerResourceRevision !== pair?.identity?.providerResourceRevision
        || !SHA256.test(execution?.modelExecutionEvidenceSha256 ?? "")) {
        errors.push(`pairs[${index}].${side}.execution must prove the matching internal Gateway Model Resource revision`);
      }
      for (const field of ["firstBuildSucceeded", "requiredFidelityPassed"]) {
        if (typeof pair?.[side]?.[field] !== "boolean") {
          errors.push(`pairs[${index}].${side}.${field} must be boolean`);
        }
      }
    }
  }

  const buckets = {};
  for (const bucket of REQUIRED_BUCKETS) {
    const samples = pairs.filter(pair => pair.bucket === bucket);
    const minimum = ["cold_dev", "repair"].includes(bucket) ? 10 : 20;
    const batches = new Set(samples.map(pair => pair.batchId)).size;
    const sampleReady = samples.length >= minimum && batches >= 3;
    const gates = sampleReady ? bucketGates(bucket, samples, iterations, seed + REQUIRED_BUCKETS.indexOf(bucket) * 101) : [];
    const status = !sampleReady
      ? "insufficient_evidence"
      : gates.some(item => item.status === "insufficient_evidence")
        ? "insufficient_evidence"
        : gates.every(item => item.status === "pass") ? "pass" : "fail";
    buckets[bucket] = {
      pairCount: samples.length,
      batchCount: batches,
      minimumPairCount: minimum,
      distributions: bucketDistributions(samples, iterations, seed + REQUIRED_BUCKETS.indexOf(bucket) * 1_003),
      gates,
      status,
    };
  }
  const coverage = evidence?.coverage ?? {};
  const coveragePassed = [
    "nextTemplate", "fumadocsTemplate", "multimodalVisualDelivered",
    "nonVisualUnavailableMainTaskPassed", "runtimeRestart",
  ].every(field => coverage[field] === true);
  const result = errors.length
    ? "invalid"
    : !coveragePassed || Object.values(buckets).some(bucket => bucket.status === "insufficient_evidence")
      ? "insufficient_evidence"
      : Object.values(buckets).every(bucket => bucket.status === "pass") ? "pass" : "fail";
  return {
    schemaVersion: "generation-context-rollout-evaluation@1",
    calculatorVersion: evidence?.calculatorVersion,
    errors,
    coveragePassed,
    buckets,
    result,
  };
}

async function main() {
  const path = process.argv[2];
  if (!path) throw new Error("usage: evaluate-generation-context-rollout.mjs <rollout-evidence.json>");
  const result = evaluateRolloutEvidence(JSON.parse(await readFile(path, "utf8")));
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  if (result.result !== "pass") process.exitCode = 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
