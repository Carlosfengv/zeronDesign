#!/usr/bin/env node

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { initializePairedCohortLedger } from "../../services/runtime/scripts/generation-context-paired-cohort-ledger.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const script = path.join(root, "infra/generation-reliability/run-runtime-efficiency-benchmark-cohort.sh");
const casesFile = path.join(root, "infra/generation-reliability/runtime-efficiency-benchmark-cases.json");
const casesSha = crypto.createHash("sha256").update(fs.readFileSync(casesFile)).digest("hex");
const directory = fs.mkdtempSync(path.join(os.tmpdir(), "runtime-efficiency-plan-"));

try {
  const session = {
    schemaVersion: "generation-context-paired-cohort-session@1",
    sessionId: "dry-run-session",
    createdAt: "2026-07-23T10:00:00.000Z",
    calculatorVersion: "generation-context-rollout-calculator@1",
    bootstrap: { iterations: 100, seed: 42 },
    source: { commit: "abc123", dirty: false },
    sourcePolicy: "hashes_only",
    fixtureManifestSha256: casesSha,
    providers: [{
      gatewayMode: "internal_gateway",
      modelResourceId: "deepseek-v4-pro",
      resourceRevision: 7,
      modelVersion: "deepseek-v4-pro-2026-07",
      providerParametersHash: "a".repeat(64),
    }],
    runtimes: {
      control: {
        generationContextMode: "off",
        deploymentRevision: "control-1",
        allowedModelResourceIds: ["deepseek-v4-pro"],
      },
      candidate: {
        generationContextMode: "enabled",
        deploymentRevision: "candidate-1",
        allowedModelResourceIds: ["deepseek-v4-pro"],
      },
    },
  };
  fs.writeFileSync(path.join(directory, "session.json"), `${JSON.stringify(session)}\n`);
  initializePairedCohortLedger(path.join(directory, "cohort.ndjson"), session);
  const result = spawnSync("bash", [script, directory, "release-benchmark"], {
    cwd: root,
    env: { ...process.env, GENERATION_EFFICIENCY_DRY_RUN: "1" },
    encoding: "utf8",
  });
  assert.equal(result.status, 0, result.stderr);
  const plan = JSON.parse(result.stdout);
  assert.equal(plan.pairCount, 60);
  assert.equal(plan.attemptCount, 120);
  assert.equal(new Set(plan.pairs.map(item => item.pairId)).size, 60);
  assert.equal(plan.pairs.filter(item => item.bucket === "greenfield").length, 30);
  assert.equal(plan.pairs.filter(item => item.bucket === "warm_copy_css").length, 30);
  assert(plan.pairs.every(item => item.status === "pending"));

  session.source.dirty = true;
  const dirtyDirectory = path.join(directory, "dirty");
  fs.mkdirSync(dirtyDirectory);
  fs.writeFileSync(path.join(dirtyDirectory, "session.json"), `${JSON.stringify(session)}\n`);
  initializePairedCohortLedger(path.join(dirtyDirectory, "cohort.ndjson"), session);
  const dirty = spawnSync("bash", [script, dirtyDirectory, "dirty-benchmark"], {
    cwd: root,
    env: { ...process.env, GENERATION_EFFICIENCY_DRY_RUN: "1" },
    encoding: "utf8",
  });
  assert.notEqual(dirty.status, 0);
  assert.match(dirty.stderr, /clean frozen Paired Session source/);

  const corruptDirectory = path.join(directory, "corrupt");
  fs.mkdirSync(corruptDirectory);
  const corruptSession = { ...session, source: { commit: "abc123", dirty: false } };
  fs.writeFileSync(
    path.join(corruptDirectory, "session.json"),
    `${JSON.stringify(corruptSession)}\n`,
  );
  const corruptLedger = path.join(corruptDirectory, "cohort.ndjson");
  initializePairedCohortLedger(corruptLedger, corruptSession);
  fs.appendFileSync(corruptLedger, "{\"tampered\":true}\n");
  const providerMarker = path.join(corruptDirectory, "provider-called");
  const fakePairRunner = path.join(corruptDirectory, "fake-pair-runner.sh");
  fs.writeFileSync(fakePairRunner, `#!/usr/bin/env bash\ntouch "${providerMarker}"\n`);
  const corrupt = spawnSync("bash", [script, corruptDirectory, "corrupt-benchmark"], {
    cwd: root,
    env: { ...process.env, GENERATION_EFFICIENCY_PAIR_RUNNER: fakePairRunner },
    encoding: "utf8",
  });
  assert.notEqual(corrupt.status, 0);
  assert.match(corrupt.stderr, /paired_ledger_preflight_failed/);
  assert.equal(fs.existsSync(providerMarker), false);

  const wrongCorpusDirectory = path.join(directory, "wrong-corpus");
  fs.mkdirSync(wrongCorpusDirectory);
  const wrongManifest = JSON.parse(fs.readFileSync(casesFile, "utf8"));
  wrongManifest.cases[0].prompt = "请创建一个普通营销首页。";
  const wrongManifestBytes = Buffer.from(`${JSON.stringify(wrongManifest, null, 2)}\n`);
  const wrongCasesFile = path.join(wrongCorpusDirectory, "cases.json");
  fs.writeFileSync(wrongCasesFile, wrongManifestBytes);
  const wrongCorpusSession = {
    ...corruptSession,
    fixtureManifestSha256: crypto.createHash("sha256").update(wrongManifestBytes).digest("hex"),
  };
  fs.writeFileSync(
    path.join(wrongCorpusDirectory, "session.json"),
    `${JSON.stringify(wrongCorpusSession)}\n`,
  );
  initializePairedCohortLedger(
    path.join(wrongCorpusDirectory, "cohort.ndjson"),
    wrongCorpusSession,
  );
  const wrongCorpusMarker = path.join(wrongCorpusDirectory, "provider-called");
  const wrongCorpusRunner = path.join(wrongCorpusDirectory, "fake-pair-runner.sh");
  fs.writeFileSync(wrongCorpusRunner, `#!/usr/bin/env bash\ntouch "${wrongCorpusMarker}"\n`);
  const wrongCorpus = spawnSync("bash", [script, wrongCorpusDirectory, "wrong-corpus"], {
    cwd: root,
    env: {
      ...process.env,
      GENERATION_EFFICIENCY_CASES_FILE: wrongCasesFile,
      GENERATION_EFFICIENCY_PAIR_RUNNER: wrongCorpusRunner,
    },
    encoding: "utf8",
  });
  assert.notEqual(wrongCorpus.status, 0);
  assert.match(wrongCorpus.stderr, /ten unique Design System Website prompts/);
  assert.equal(fs.existsSync(wrongCorpusMarker), false);
} finally {
  fs.rmSync(directory, { recursive: true, force: true });
}

process.stdout.write("Runtime efficiency Benchmark cohort planner tests passed.\n");
