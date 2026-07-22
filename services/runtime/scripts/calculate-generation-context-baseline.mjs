#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const SHA256 = /^[0-9a-f]{64}$/;

export function canonicalJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

export function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function calculateBaselineSummary(manifest) {
  const errors = [];
  if (manifest?.schemaVersion !== "baseline-evidence@1") errors.push("schemaVersion must be baseline-evidence@1");
  if (manifest?.calculatorVersion !== "generation-context-baseline-calculator@1") {
    errors.push("calculatorVersion must be generation-context-baseline-calculator@1");
  }
  if (!Array.isArray(manifest?.samples) || manifest.samples.length === 0) errors.push("samples must not be empty");
  const ids = new Set();
  for (const [index, sample] of (manifest?.samples ?? []).entries()) {
    if (!sample?.id || ids.has(sample.id)) errors.push(`samples[${index}].id must be unique`);
    ids.add(sample?.id);
    const available = sample?.source?.availability === "available";
    const sourceHash = sample?.source?.contentSha256;
    if (available && !SHA256.test(sourceHash ?? "")) errors.push(`samples[${index}] available source requires contentSha256`);
    if (available && sample?.inclusion?.includedInImprovementDenominator !== true) {
      errors.push(`samples[${index}] available evidence must explicitly state its denominator decision`);
    }
    if (!available && sample?.inclusion?.includedInImprovementDenominator !== false) {
      errors.push(`samples[${index}] unavailable evidence cannot enter improvement denominator`);
    }
    if (!available && !(sample?.inclusion?.exclusionReasons?.length > 0)) {
      errors.push(`samples[${index}] unavailable evidence requires an exclusion reason`);
    }
  }
  const eligible = (manifest?.samples ?? []).filter(sample => sample.inclusion.includedInImprovementDenominator);
  const values = name => eligible.map(sample => sample.metrics?.[name]).filter(Number.isFinite);
  const sum = name => values(name).reduce((total, value) => total + value, 0);
  const median = numbers => {
    if (numbers.length === 0) return null;
    const sorted = [...numbers].sort((a, b) => a - b);
    const middle = Math.floor(sorted.length / 2);
    return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
  };
  const rateBasisPoints = name => eligible.length === 0
    ? null
    : Math.round(sum(name) * 10_000 / eligible.length);
  const nextActionDecisionCount = sum("nextActionMatchCount") + sum("nextActionViolationCount");
  return {
    errors,
    summary: {
      schemaVersion: "baseline-evidence-summary@1",
      calculatorVersion: manifest?.calculatorVersion,
      sampleCount: manifest?.samples?.length ?? 0,
      eligibleSampleCount: eligible.length,
      excludedSampleCount: (manifest?.samples?.length ?? 0) - eligible.length,
      metrics: {
        inputTokensMedian: median(values("inputTokens")),
        timeToFirstBuildMsMedian: median(values("timeToFirstBuildMs")),
        timeToFirstSourceMutationMsMedian: median(values("timeToFirstSourceMutationMs")),
        modelTurnAtFirstSourceMutationMedian: median(values("modelTurnAtFirstSourceMutation")),
        prebuildObservationCallCountMedian: median(values("prebuildObservationCallCount")),
        duplicateReadTokensMedian: median(values("duplicateReadTokens")),
        nextActionViolationCountMedian: median(values("nextActionViolationCount")),
        nextActionViolationRateBasisPoints: nextActionDecisionCount === 0
          ? null
          : Math.round(sum("nextActionViolationCount") * 10_000 / nextActionDecisionCount),
        completionMissingDurableSnapshotCount: sum("completionMissingDurableSnapshot"),
        noProgressFailureCount: sum("noProgressFailure"),
        firstBuildSuccessRateBasisPoints: rateBasisPoints("firstBuildSucceeded"),
        artifactAcceptanceRateBasisPoints: rateBasisPoints("artifactAccepted"),
      },
      evidenceState: eligible.length === 0 ? "insufficient_evidence" : "ready",
    },
  };
}

async function main() {
  const manifestPath = process.argv[2];
  if (!manifestPath) throw new Error("usage: calculate-generation-context-baseline.mjs <manifest.json>");
  const bytes = await readFile(manifestPath);
  const manifest = JSON.parse(bytes);
  const result = calculateBaselineSummary(manifest);
  const payloadHash = sha256(canonicalJson(manifest));
  if (result.errors.length > 0) {
    for (const error of result.errors) console.error(error);
    process.exitCode = 1;
    return;
  }
  console.log(JSON.stringify({ ...result.summary, manifestPayloadSha256: payloadHash }, null, 2));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
