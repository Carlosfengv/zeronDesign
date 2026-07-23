#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { assemblePairedCohortEvidence } from "../../services/runtime/scripts/generation-context-paired-cohort-ledger.mjs";
import { initializeBenchmarkLedger } from "./runtime-efficiency-benchmark-ledger.mjs";

const BUCKETS = {
  greenfield: "greenfield_build",
  warm_copy_css: "style_token_edit",
};

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

function splitTemplate(value) {
  const separator = value.lastIndexOf("@");
  if (separator <= 0 || separator === value.length - 1) {
    throw new Error(`paired Template identity must be <template-id>@<version>: ${value}`);
  }
  return { templateId: value.slice(0, separator), templateVersion: value.slice(separator + 1) };
}

function profileForBucket(source, bucket, workload) {
  const pairs = source.pairs.filter(pair => pair.bucket === bucket);
  const promptIds = [...new Set(pairs.map(pair => pair.identity.fixtureId))].sort();
  if (promptIds.length < 10) {
    throw new Error(`${bucket} must cover at least 10 unique Prompt Fixtures before Benchmark preparation`);
  }
  const identities = new Map();
  for (const pair of pairs) {
    const { fixtureId: _fixtureId, ...profileIdentity } = pair.identity;
    identities.set(canonical(profileIdentity), profileIdentity);
  }
  if (identities.size !== 1) {
    throw new Error(`${bucket} must use exactly one frozen Design Profile/Template/Provider identity`);
  }
  const identity = [...identities.values()][0];
  const { templateId, templateVersion } = splitTemplate(identity.templateVersion);
  const cacheValues = pairs.flatMap(pair => [pair.control, pair.candidate])
    .map(sample => sample.metrics?.cacheHitRateBasisPoints);
  const reportedCount = cacheValues.filter(Number.isSafeInteger).length;
  if (reportedCount !== 0 && reportedCount !== cacheValues.length) {
    throw new Error(`${bucket} mixes reported and unsupported cached-usage evidence`);
  }
  const profileIdentity = {
    workload,
    designProfileHash: identity.designProfileHash,
    templateId,
    templateVersion,
    modelResourceId: identity.modelResource,
    providerResourceRevision: identity.providerResourceRevision,
    modelVersion: identity.modelVersion,
    providerParametersHash: identity.providerParametersHash,
    cacheUsageCapability: reportedCount === cacheValues.length ? "reported" : "unsupported",
  };
  return {
    profile: {
      profileId: `${workload}-${sha256(canonical(profileIdentity)).slice(0, 16)}`,
      ...profileIdentity,
    },
    promptIds,
  };
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, { flag: "wx", mode: 0o600 });
}

export function prepareRuntimeEfficiencyBenchmark(pairedLedgerFile, outputDirectory) {
  const source = assemblePairedCohortEvidence(pairedLedgerFile);
  const prepared = Object.entries(BUCKETS).map(([bucket, workload]) => ({
    bucket,
    ...profileForBucket(source, bucket, workload),
  }));
  const promptIds = [...new Set(prepared.flatMap(item => item.promptIds))].sort();
  const session = {
    schemaVersion: "runtime-efficiency-benchmark-session@1",
    sessionId: `benchmark-${source.provenance.sessionId}`,
    createdAt: new Date().toISOString(),
    calculatorVersion: "runtime-efficiency-benchmark-calculator@1",
    source: source.provenance.source,
    bootstrap: source.bootstrap,
    promptSet: {
      id: `paired-fixtures-${source.provenance.sessionId}`,
      version: source.provenance.source.commit,
      sha256: source.provenance.fixtureManifestSha256,
      promptIds,
    },
    profiles: prepared.map(item => item.profile),
  };
  const mapping = {
    schemaVersion: "runtime-efficiency-benchmark-import@1",
    buckets: Object.fromEntries(prepared.map(item => [item.bucket, item.profile.profileId])),
    promptIdByFixtureId: Object.fromEntries(promptIds.map(promptId => [promptId, promptId])),
  };
  fs.mkdirSync(outputDirectory, { mode: 0o700 });
  const sessionFile = path.join(outputDirectory, "session.json");
  const mappingFile = path.join(outputDirectory, "import-mapping.json");
  const ledgerFile = path.join(outputDirectory, "benchmark.ndjson");
  writeJson(sessionFile, session);
  writeJson(mappingFile, mapping);
  const ledger = initializeBenchmarkLedger(ledgerFile, session);
  const result = {
    schemaVersion: "runtime-efficiency-benchmark-preparation@1",
    sourcePairedLedgerSha256: source.provenance.ledgerSha256,
    sourcePairedSessionId: source.provenance.sessionId,
    promptCount: promptIds.length,
    profiles: prepared.map(item => ({
      bucket: item.bucket,
      profileId: item.profile.profileId,
      workload: item.profile.workload,
      promptCount: item.promptIds.length,
    })),
    files: {
      session: "session.json",
      mapping: "import-mapping.json",
      ledger: "benchmark.ndjson",
    },
    ledger,
  };
  writeJson(path.join(outputDirectory, "preparation.json"), result);
  return result;
}

function main() {
  const [pairedLedgerFile, outputDirectory] = process.argv.slice(2);
  if (!pairedLedgerFile || !outputDirectory) {
    throw new Error("usage: prepare-runtime-efficiency-benchmark.mjs <paired-ledger.ndjson> <new-output-directory>");
  }
  process.stdout.write(`${JSON.stringify(prepareRuntimeEfficiencyBenchmark(
    pairedLedgerFile,
    outputDirectory,
  ), null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error?.stack || error}\n`);
    process.exitCode = 1;
  }
}
