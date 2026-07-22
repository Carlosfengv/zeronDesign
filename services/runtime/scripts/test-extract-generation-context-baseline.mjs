#!/usr/bin/env node

import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { createBaselineManifest } from "./extract-generation-context-baseline.mjs";

const directory = await mkdtemp(path.join(os.tmpdir(), "generation-context-baseline-"));
try {
  await writeFile(path.join(directory, "real-provider-examples-summary.json"), JSON.stringify({
    schemaVersion: "generation-real-provider-suite-evidence@2",
    suiteId: "suite-fixture",
    status: "failed",
    startedAt: "2026-07-21T00:00:00Z",
    finishedAt: "2026-07-21T00:01:00Z",
    provider: { gatewayMode: "internal_gateway", modelResourceId: "deepseek-v4-pro", realProviderVerified: true },
    budget: { exceeded: false },
  }));
  const events = [
    { type: "run.workflow_progress", nextAction: { tool: "project.ensure_dependencies" } },
    { type: "model.turn_started" },
    { type: "tool.started", tool: "fs.list" },
    { type: "run.workflow_progress", nextAction: { tool: "project.ensure_dependencies" } },
    { type: "tool.started", tool: "project.ensure_dependencies" },
  ];
  await writeFile(path.join(directory, "run-build.events.ndjson"), `${events.map(JSON.stringify).join("\n")}\n`);
  await writeFile(path.join(directory, "real-provider-case-fixture.json"), JSON.stringify({
    id: "fixture",
    status: "failed",
    error: { classification: "no_progress" },
    runs: [{
      phase: "build",
      runId: "run-build",
      status: "partial",
      turns: 2,
      toolCalls: 2,
      modelExecutions: [{ modelResourceId: "deepseek-v4-pro", modelResourceRevision: 4 }],
      eventStream: {
        path: "run-build.events.ndjson",
        sha256: "a".repeat(64),
      },
      efficiency: {
        template: "next-app",
        totalDurationMs: 1000,
        timeToFirstModelTurnMs: 10,
        timeToFirstSourceMutationMs: 500,
        modelTurnAtFirstSourceMutation: 2,
        timeToFirstGreenfieldStaticBuildMs: null,
        timeToDurableSnapshotMs: null,
        prebuildFsReadCount: 1,
        prebuildFsListCount: 2,
        prebuildFsSearchCount: 3,
        inputTokens: 100,
        outputTokens: 10,
        cachedInputTokens: 20,
        duplicateReadEstimatedTokens: 0,
        duplicateFullReadRateBasisPoints: 0,
        firstBuildSucceeded: false,
      },
    }],
  }));

  const manifest = await createBaselineManifest(directory);
  assert.equal(manifest.samples.length, 1);
  assert.equal(manifest.cohort.realProviderVerified, true);
  assert.equal(manifest.samples[0].metrics.prebuildObservationCallCount, 6);
  assert.equal(manifest.samples[0].metrics.nextActionViolationCount, 1);
  assert.equal(manifest.samples[0].metrics.nextActionMatchCount, 1);
  assert.equal(manifest.samples[0].metrics.noProgressFailure, 1);
  assert.equal(manifest.samples[0].identity.modelResource, "deepseek-v4-pro@4");
  console.log("generation-context baseline extractor tests passed");
} finally {
  await rm(directory, { recursive: true, force: true });
}
