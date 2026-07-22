#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const SHA256 = /^[a-f0-9]{64}$/;

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function optionalMetric(value) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
}

function compact(value) {
  return Object.fromEntries(Object.entries(value).filter(([, entry]) => entry !== undefined));
}

function mappedStatus(metadata, efficiency) {
  if (["failed", "timeout", "fallback"].includes(metadata.status)) return metadata.status;
  if (metadata.status === "completed" && efficiency.status !== "completed") {
    throw new Error("metadata cannot mark a non-completed Runtime Run as completed");
  }
  if (metadata.status === "completed" || efficiency.status === "completed") return "completed";
  if (["failed", "partial", "cancelled"].includes(efficiency.status)) return "failed";
  throw new Error(`metadata.status is required for non-terminal Runtime status: ${efficiency.status}`);
}

export function createPairedCohortSample(metadata, efficiency) {
  if (metadata?.schemaVersion !== "generation-context-paired-cohort-sample-metadata@1") {
    throw new Error("unsupported paired-cohort sample metadata schema");
  }
  if (efficiency?.schemaVersion !== "run-efficiency-metrics@1"
    || efficiency.calculatorVersion !== "run-efficiency-calculator@1") {
    throw new Error("efficiency input must be Runtime run-efficiency-metrics@1");
  }
  const expectedModelResource = metadata.identity?.modelResource;
  if (
    efficiency.model !== expectedModelResource &&
    efficiency.model !== `resource:${expectedModelResource}`
  ) {
    throw new Error("Runtime efficiency model must select metadata.identity.modelResource");
  }
  if (efficiency.phase !== metadata.identity?.phase) {
    throw new Error("Runtime efficiency phase must equal metadata.identity.phase");
  }
  if (!SHA256.test(metadata.acceptanceEvidenceSha256 || "")) {
    throw new Error("metadata.acceptanceEvidenceSha256 must be sha256");
  }
  const requiredFidelityPassed = efficiency.requiredFidelityPassed
    ?? metadata.requiredFidelityPassed;
  if (typeof requiredFidelityPassed !== "boolean") {
    throw new Error("required fidelity must come from Runtime metrics or an explicitly hashed acceptance result");
  }
  return {
    schemaVersion: "generation-context-paired-cohort-sample@1",
    pairId: metadata.pairId,
    batchId: metadata.batchId,
    bucket: metadata.bucket,
    side: metadata.side,
    flag: metadata.side === "control" ? "legacy" : "generation_context",
    status: mappedStatus(metadata, efficiency),
    recordedAt: metadata.recordedAt,
    identity: metadata.identity,
    execution: metadata.execution,
    source: metadata.source,
    acceptanceEvidenceSha256: metadata.acceptanceEvidenceSha256,
    firstBuildSucceeded: efficiency.firstBuildSucceeded,
    requiredFidelityPassed,
    coverage: metadata.coverage ?? [],
    metrics: compact({
      duplicateReadTokens: optionalMetric(efficiency.duplicateReadEstimatedTokens),
      inputTokens: optionalMetric(efficiency.inputTokens),
      timeToFirstGreenfieldBuildMs: optionalMetric(efficiency.timeToFirstGreenfieldStaticBuildMs),
      coldDevReadyMs: optionalMetric(efficiency.coldDevReadyMs),
      timeToIframeAppliedMs: optionalMetric(efficiency.timeToIframeAppliedMs),
      timeToDurableSnapshotMs: optionalMetric(efficiency.timeToDurableSnapshotMs),
      timeToFirstSourceMutationMs: optionalMetric(efficiency.timeToFirstSourceMutationMs),
      modelTurnAtFirstSourceMutation: optionalMetric(efficiency.modelTurnAtFirstSourceMutation),
      prebuildFsListCount: optionalMetric(efficiency.prebuildFsListCount),
      prebuildFsSearchCount: optionalMetric(efficiency.prebuildFsSearchCount),
      duplicateFullReadRateBasisPoints: optionalMetric(efficiency.duplicateFullReadRateBasisPoints),
      outOfScopeMutationCount: optionalMetric(efficiency.outOfScopeMutationCount),
    }),
  };
}

function writeExclusive(file, value) {
  fs.mkdirSync(path.dirname(path.resolve(file)), { recursive: true });
  const descriptor = fs.openSync(file, "wx", 0o600);
  try {
    fs.writeFileSync(descriptor, `${JSON.stringify(value, null, 2)}\n`);
  } finally {
    fs.closeSync(descriptor);
  }
}

async function main() {
  const [metadataFile, efficiencyFile, outputFile] = process.argv.slice(2);
  if (!metadataFile || !efficiencyFile || !outputFile) {
    throw new Error("usage: create-generation-context-paired-sample.mjs <metadata.json> <run-efficiency-metrics.json> <sample.json>");
  }
  const metadata = JSON.parse(fs.readFileSync(metadataFile, "utf8"));
  const efficiency = JSON.parse(fs.readFileSync(efficiencyFile, "utf8"));
  writeExclusive(outputFile, createPairedCohortSample(metadata, efficiency));
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`${error?.stack || error}\n`);
    process.exitCode = 1;
  });
}
