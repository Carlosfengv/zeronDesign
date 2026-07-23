import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const script = path.join(path.dirname(fileURLToPath(import.meta.url)), "audit-provider-cache-smoke.mjs");

function runAudit({
  cachedInputTokens,
  repeated = true,
  estimated = false,
  gitDirty = false,
  unsafeEvent = false,
  toolSetHashVersion = "tool-definition-set@1",
}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "provider-cache-smoke-"));
  const runId = "run-1";
  const eventStream = [
    JSON.stringify({
      type: "model.usage",
      runId,
      inputTokens: 1000,
      cachedInputTokens,
      outputTokens: 100,
      estimated: false,
    }),
    JSON.stringify(unsafeEvent
      ? { type: "tool.output", runId, tool: "fs.read", text: "raw source must not persist" }
      : { type: "run.completed", runId, status: "completed", summarySha256: "e".repeat(64), summaryBytes: 9 }),
  ].join("\n") + "\n";
  const eventFile = "run-1.events.ndjson";
  fs.writeFileSync(path.join(root, eventFile), eventStream);
  fs.writeFileSync(path.join(root, "real-provider-examples-summary.json"), JSON.stringify({
    schemaVersion: "generation-real-provider-suite-evidence@2",
    suiteId: "suite-fixture",
    status: "accepted",
    provenance: {
      gitCommit: "commit-fixture",
      gitDirty,
      providerConfigSha256: "d".repeat(64),
      providerResourceRevision: 7,
    },
    provider: {
      modelResourceId: "deepseek-v4-pro",
      realProviderVerified: true,
    },
    cases: [{ id: "fixture", runIds: [runId] }],
  }));
  fs.writeFileSync(path.join(root, "real-provider-case-fixture.json"), JSON.stringify({
    schemaVersion: "generation-real-provider-case-evidence@2",
    id: "fixture",
    runs: [{
      runId,
      phase: "build",
      status: "completed",
      modelExecutions: [{
        modelResourceId: "deepseek-v4-pro",
        modelResourceRevision: 7,
        providerRequestIdPresent: true,
      }],
      buildEvidence: {
        buildId: "build-1",
        sourceFingerprint: "a".repeat(64),
        candidateManifestHash: "b".repeat(64),
        artifactRouteManifestPath: ".anydesign-artifact-routes.json",
        artifactRouteManifestHash: "c".repeat(64),
      },
      generationContextStatus: {
        schemaVersion: "generation-context-status@1",
        runId,
        runContractVersion: "generation-context@1",
        status: "compiled",
        contextContentHash: "d".repeat(64),
        runContextBindingHash: "e".repeat(64),
        budgetProfileId: "phase-default",
        budgetProfileHash: "f".repeat(64),
        budgetProfileRolloutMode: "shadow",
      },
      eventStream: {
        schemaVersion: "generation-run-event-stream@1",
        path: eventFile,
        format: "ndjson",
        eventCount: 2,
        sha256: crypto.createHash("sha256").update(eventStream).digest("hex"),
      },
      promptEfficiency: {
        schemaVersion: "run-prompt-efficiency@1",
        runId,
        grossInputTokens: 1000,
        cachedInputTokens,
        cacheHitRateBasisPoints: cachedInputTokens * 10,
        estimated,
      },
      promptCompositions: [
        {
          turn: 1,
          staticPrefixHash: "a".repeat(64),
          ...(toolSetHashVersion ? { toolSetHashVersion } : {}),
          toolSetHash: "b".repeat(64),
        },
        {
          turn: 2,
          staticPrefixHash: (repeated ? "a" : "c").repeat(64),
          ...(toolSetHashVersion ? { toolSetHashVersion } : {}),
          toolSetHash: "b".repeat(64),
        },
      ],
    }],
  }));
  const output = path.join(root, "audit.json");
  const execution = spawnSync(process.execPath, [script, root, output], { encoding: "utf8" });
  return { execution, audit: JSON.parse(fs.readFileSync(output, "utf8")) };
}

const passed = runAudit({ cachedInputTokens: 400 });
assert.equal(passed.execution.status, 0);
assert.equal(passed.audit.status, "passed");
assert.equal(passed.audit.releaseEligible, true);
assert.equal(passed.audit.toolSetHashVersion, "tool-definition-set@1");
assert.equal(passed.audit.modelResourceId, "deepseek-v4-pro");
assert.equal(passed.audit.providerResourceRevision, 7);
assert.equal(passed.audit.providerConfigSha256, "d".repeat(64));

const unsupported = runAudit({ cachedInputTokens: 0 });
assert.equal(unsupported.execution.status, 0);
assert.equal(unsupported.audit.status, "provider_not_reporting_cached_usage");
assert.equal(unsupported.audit.releaseEligible, false);

const unstable = runAudit({ cachedInputTokens: 400, repeated: false });
assert.equal(unstable.execution.status, 1);
assert.equal(unstable.audit.status, "failed");

const estimated = runAudit({ cachedInputTokens: 400, estimated: true });
assert.equal(estimated.execution.status, 1);
assert.equal(estimated.audit.status, "failed");

const dirty = runAudit({ cachedInputTokens: 400, gitDirty: true });
assert.equal(dirty.execution.status, 0);
assert.equal(dirty.audit.status, "passed");
assert.equal(dirty.audit.releaseEligible, false);

const unsafe = runAudit({ cachedInputTokens: 400, unsafeEvent: true });
assert.equal(unsafe.execution.status, 1);
assert.equal(unsafe.audit.status, "failed");
assert.equal(unsafe.audit.runs[0].redactionValid, false);

const legacyToolHash = runAudit({ cachedInputTokens: 400, toolSetHashVersion: null });
assert.equal(legacyToolHash.execution.status, 1);
assert.equal(legacyToolHash.audit.status, "failed");
assert.equal(legacyToolHash.audit.runs[0].compositionValid, false);

process.stdout.write("Provider cache smoke audit tests passed\n");
