#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import { pathToFileURL } from "node:url";
import {
  assemblePairedCohortEvidence,
  verifyPairedCohortLedger,
} from "../../services/runtime/scripts/generation-context-paired-cohort-ledger.mjs";
import {
  appendBenchmarkAttempts,
  getBenchmarkLedgerAppendContext,
  verifyBenchmarkLedger,
} from "./runtime-efficiency-benchmark-ledger.mjs";

const MAPPING_SCHEMA = "runtime-efficiency-benchmark-import@1";
const HASH = /^[a-f0-9]{64}$/;
const WORKLOAD_BY_BUCKET = new Map([
  ["greenfield", "greenfield_build"],
  ["warm_copy_css", "style_token_edit"],
]);

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function exactKeys(value, expected, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  const actual = Object.keys(value).sort();
  const keys = [...expected].sort();
  if (actual.length !== keys.length || actual.some((key, index) => key !== keys[index])) {
    throw new Error(`${label} keys must be exactly ${keys.join(",")}`);
  }
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function nonNegativeInteger(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function validateMapping(mapping) {
  exactKeys(mapping, ["schemaVersion", "buckets", "promptIdByFixtureId"], "mapping");
  if (mapping.schemaVersion !== MAPPING_SCHEMA) {
    throw new Error(`mapping.schemaVersion must be ${MAPPING_SCHEMA}`);
  }
  exactKeys(mapping.buckets, ["greenfield", "warm_copy_css"], "mapping.buckets");
  for (const [bucket, profileId] of Object.entries(mapping.buckets)) {
    if (!WORKLOAD_BY_BUCKET.has(bucket) || !hasText(profileId)) {
      throw new Error(`mapping.buckets.${bucket} must name a Profile ID`);
    }
  }
  if (!mapping.promptIdByFixtureId || typeof mapping.promptIdByFixtureId !== "object"
    || Array.isArray(mapping.promptIdByFixtureId)) {
    throw new Error("mapping.promptIdByFixtureId must be an object");
  }
  for (const [fixtureId, promptId] of Object.entries(mapping.promptIdByFixtureId)) {
    if (!hasText(fixtureId) || !hasText(promptId)) {
      throw new Error("mapping.promptIdByFixtureId must map non-empty Fixture IDs to Prompt IDs");
    }
  }
}

function mappedStatus(sample) {
  if (sample.status === "completed") return sample.requiredFidelityPassed ? "accepted" : "rejected";
  if (sample.status === "timeout") return "timeout";
  if (sample.status === "fallback") return "rejected";
  return "failed";
}

function metric(sample, name, accepted) {
  const value = sample.metrics?.[name];
  if (nonNegativeInteger(value)) return value;
  if (accepted) throw new Error(`${sample.pairId}/${sample.side} is accepted but metrics.${name} is missing`);
  return null;
}

function assertProfileIdentity(pair, profile, workload) {
  const identity = pair.identity;
  if (profile.workload !== workload
    || identity.designProfileHash !== profile.designProfileHash
    || identity.modelResource !== profile.modelResourceId
    || identity.providerResourceRevision !== profile.providerResourceRevision
    || identity.modelVersion !== profile.modelVersion
    || identity.providerParametersHash !== profile.providerParametersHash
    || identity.templateVersion !== `${profile.templateId}@${profile.templateVersion}`) {
    throw new Error(`pair ${pair.id} does not match frozen Benchmark Profile ${profile.profileId}`);
  }
}

function attemptFromSample({ pair, sample, profile, promptId, sequence }) {
  const status = mappedStatus(sample);
  const accepted = status === "accepted";
  if (sample.metrics?.caseAttemptCount !== 1) {
    throw new Error(`${pair.id}/${sample === pair.control ? "control" : "candidate"} must come from exactly one case Attempt`);
  }
  const terminalEvidenceSha256 = sha256(canonical({
    pairId: pair.id,
    side: sample === pair.control ? "control" : "candidate",
    contentSha256: sample.source?.contentSha256,
    acceptanceEvidenceSha256: sample.acceptanceEvidenceSha256,
    modelExecutionEvidenceSha256: sample.execution?.modelExecutionEvidenceSha256,
  }));
  const cacheHitRateBasisPoints = profile.cacheUsageCapability === "unsupported"
    ? null
    : metric(sample, "cacheHitRateBasisPoints", accepted);
  return {
    profileId: profile.profileId,
    attempt: {
      sequence,
      attemptId: `${pair.id}:${sample === pair.control ? "control" : "candidate"}`,
      variant: sample === pair.control ? "baseline" : "candidate",
      promptId,
      status,
      terminalEvidenceSha256,
      metrics: {
        modelTurns: metric(sample, "modelTurns", accepted),
        grossInputTokens: metric(sample, "grossInputTokens", accepted),
        uncachedInputTokens: metric(sample, "uncachedInputTokens", accepted),
        maxPromptTokensPerTurn: metric(sample, "maxTurnInputTokens", accepted),
        cacheHitRateBasisPoints,
        firstSourceMutationTurn: metric(sample, "modelTurnAtFirstSourceMutation", accepted),
        generationContextBytes: metric(sample, "generationContextBytes", accepted),
        duplicateFullContextReads: metric(sample, "duplicateFullReadDeliveries", accepted),
        outOfScopeMutations: metric(sample, "outOfScopeMutationCount", accepted),
        requiredFidelityPassed: typeof sample.requiredFidelityPassed === "boolean"
          ? sample.requiredFidelityPassed
          : null,
      },
    },
  };
}

function deriveRuntimeEfficiencyBenchmarkAttempts(
  pairedLedgerFile,
  benchmarkLedgerFile,
  mapping,
  { skipExisting = false } = {},
) {
  validateMapping(mapping);
  const sourceVerification = verifyPairedCohortLedger(pairedLedgerFile);
  if (sourceVerification.pendingPairs.length > 0) {
    throw new Error(`paired cohort contains incomplete pairs: ${sourceVerification.pendingPairs.join(",")}`);
  }
  const source = assemblePairedCohortEvidence(pairedLedgerFile);
  if (!HASH.test(source.provenance.ledgerSha256 || "")) {
    throw new Error("paired cohort ledger SHA is missing");
  }
  const context = getBenchmarkLedgerAppendContext(benchmarkLedgerFile);
  if (canonical(context.session.source) !== canonical(source.provenance.source)) {
    throw new Error("Benchmark Session source does not match the Paired Cohort source");
  }
  if (context.session.promptSet.sha256 !== source.provenance.fixtureManifestSha256) {
    throw new Error("Benchmark Prompt Set SHA does not match the Paired Cohort Fixture Manifest");
  }
  const profiles = new Map(context.session.profiles.map(profile => [profile.profileId, profile]));
  const existingAttemptIds = skipExisting ? new Set(context.attemptIds) : new Set();
  const records = [];
  let sequence = skipExisting ? context.nextSequence : 1;
  for (const pair of source.pairs) {
    const workload = WORKLOAD_BY_BUCKET.get(pair.bucket);
    if (!workload) continue;
    const profileId = mapping.buckets[pair.bucket];
    const profile = profiles.get(profileId);
    if (!profile) throw new Error(`Benchmark Profile is not frozen: ${profileId}`);
    assertProfileIdentity(pair, profile, workload);
    const promptId = mapping.promptIdByFixtureId[pair.identity.fixtureId];
    if (!hasText(promptId)) {
      throw new Error(`Fixture ${pair.identity.fixtureId} has no frozen Prompt ID mapping`);
    }
    for (const sample of [pair.control, pair.candidate]) {
      const attemptId = `${pair.id}:${sample === pair.control ? "control" : "candidate"}`;
      if (existingAttemptIds.has(attemptId)) continue;
      records.push(attemptFromSample({
        pair,
        sample,
        profile,
        promptId,
        sequence,
      }));
      sequence += 1;
    }
  }
  return { source, sourceVerification, context, records };
}

export function validateRuntimeEfficiencyBenchmarkSourceBinding(
  pairedLedgerFile,
  benchmarkLedgerFile,
  mapping,
) {
  const { source, sourceVerification, context, records } = deriveRuntimeEfficiencyBenchmarkAttempts(
    pairedLedgerFile,
    benchmarkLedgerFile,
    mapping,
  );
  if (canonical(records) !== canonical(context.attempts)) {
    throw new Error("Benchmark Ledger Attempts do not exactly match the verified Paired Cohort source");
  }
  return {
    schemaVersion: "runtime-efficiency-benchmark-source-binding@1",
    status: "passed",
    pairedSessionId: source.provenance.sessionId,
    source: source.provenance.source,
    pairedLedgerSha256: source.provenance.ledgerSha256,
    pairedLedgerHeadRecordHash: sourceVerification.headRecordHash,
    mappingSha256: sha256(canonical(mapping)),
    attemptCount: records.length,
  };
}

export function collectRuntimeEfficiencyBenchmarkAttempts(
  pairedLedgerFile,
  benchmarkLedgerFile,
  mapping,
) {
  const { source, records } = deriveRuntimeEfficiencyBenchmarkAttempts(
    pairedLedgerFile,
    benchmarkLedgerFile,
    mapping,
    { skipExisting: true },
  );
  if (!records.length) throw new Error("no new eligible paired-cohort Attempts to append");
  const verification = appendBenchmarkAttempts(benchmarkLedgerFile, records);
  return {
    schemaVersion: "runtime-efficiency-benchmark-import-result@1",
    sourceLedgerSha256: source.provenance.ledgerSha256,
    importedAttemptCount: records.length,
    firstSequence: records[0].attempt.sequence,
    lastSequence: records.at(-1).attempt.sequence,
    benchmarkLedger: verification,
  };
}

export function synchronizeRuntimeEfficiencyBenchmarkAttempts(
  pairedLedgerFile,
  benchmarkLedgerFile,
  mapping,
) {
  const { source, records } = deriveRuntimeEfficiencyBenchmarkAttempts(
    pairedLedgerFile,
    benchmarkLedgerFile,
    mapping,
    { skipExisting: true },
  );
  const verification = records.length
    ? appendBenchmarkAttempts(benchmarkLedgerFile, records)
    : verifyBenchmarkLedger(benchmarkLedgerFile);
  const sourceBinding = validateRuntimeEfficiencyBenchmarkSourceBinding(
    pairedLedgerFile,
    benchmarkLedgerFile,
    mapping,
  );
  return {
    schemaVersion: "runtime-efficiency-benchmark-sync-result@1",
    status: "passed",
    sourceLedgerSha256: source.provenance.ledgerSha256,
    importedAttemptCount: records.length,
    firstImportedSequence: records.length ? records[0].attempt.sequence : null,
    lastImportedSequence: records.length ? records.at(-1).attempt.sequence : null,
    benchmarkLedger: verification,
    sourceBinding,
  };
}

function main() {
  const args = process.argv.slice(2);
  const mode = args[0] === "sync" ? args.shift() : "collect";
  const [pairedLedgerFile, benchmarkLedgerFile, mappingFile] = args;
  if (!pairedLedgerFile || !benchmarkLedgerFile || !mappingFile) {
    throw new Error("usage: collect-runtime-efficiency-benchmark.mjs [sync] <paired-ledger.ndjson> <benchmark-ledger.ndjson> <mapping.json>");
  }
  const mapping = JSON.parse(fs.readFileSync(mappingFile, "utf8"));
  const result = mode === "sync"
    ? synchronizeRuntimeEfficiencyBenchmarkAttempts(pairedLedgerFile, benchmarkLedgerFile, mapping)
    : collectRuntimeEfficiencyBenchmarkAttempts(pairedLedgerFile, benchmarkLedgerFile, mapping);
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error?.stack || error}\n`);
    process.exitCode = 1;
  }
}
