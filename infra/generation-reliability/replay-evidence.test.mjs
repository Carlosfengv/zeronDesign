import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { test } from "node:test";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { replayEvidence, runRouteConformance } from "./replay-evidence.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

test("Node route projection passes the shared Rust conformance corpus", async () => {
  const result = await runRouteConformance(
    resolve(repositoryRoot, "services/runtime/evidence/replay/contracts/artifact-route-conformance@1.json"),
  );
  assert.equal(result.cases.length, 6);
  assert.ok(result.cases.every((testCase) => testCase.status === "passed"));
});

test("sanitized fixture evidence replays without network or provider access", async () => {
  const result = await replayEvidence(
    resolve(repositoryRoot, "services/runtime/evidence/replay/fixture-docs-route-failure"),
  );
  assert.equal(result.status, "passed");
  assert.equal(result.replay.usage.uncachedInputTokens, 1100);
  assert.equal(result.replay.runModelUsage.schemaVersion, "run-model-usage@1");
  assert.equal(result.replay.runModelUsage.totalTokens, 1700);
  assert.equal(result.replay.runModelUsage.turnCount, 2);
  assert.deepEqual(result.replay.failureOwners, { metadata: "source" });
  assert.equal(result.replay.finalStage, "validating");
});

test("legacy replay rejects a self-checksummed RunModelUsage projection that differs from events", async () => {
  const source = resolve(
    repositoryRoot,
    "services/runtime/evidence/replay/fixture-docs-route-failure",
  );
  const root = await mkdtemp(join(tmpdir(), "runtime-replay-model-usage-"));
  const bundle = join(root, "bundle");
  try {
    await cp(source, bundle, { recursive: true });
    const usagePath = join(bundle, "run-model-usage.json");
    const usage = JSON.parse(await readFile(usagePath, "utf8"));
    usage.inputTokens += 1;
    usage.totalTokens += 1;
    const usageText = `${JSON.stringify(usage, null, 2)}\n`;
    await writeFile(usagePath, usageText);

    const checksumsPath = join(bundle, "checksums.sha256");
    const usageSha256 = createHash("sha256").update(usageText).digest("hex");
    const checksums = (await readFile(checksumsPath, "utf8"))
      .replace(
        /^[a-f0-9]{64}  run-model-usage\.json$/m,
        `${usageSha256}  run-model-usage.json`,
      );
    await writeFile(checksumsPath, checksums);

    await assert.rejects(
      () => replayEvidence(bundle),
      /RunModelUsage evidence does not match events/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("legacy replay rejects changed per-turn usage even when aggregate totals are unchanged", async () => {
  const source = resolve(
    repositoryRoot,
    "services/runtime/evidence/replay/fixture-docs-route-failure",
  );
  const root = await mkdtemp(join(tmpdir(), "runtime-replay-turns-"));
  const bundle = join(root, "bundle");
  try {
    await cp(source, bundle, { recursive: true });
    const usagePath = join(bundle, "usage.json");
    const usage = JSON.parse(await readFile(usagePath, "utf8"));
    usage.turns[0].inputTokens += 100;
    usage.turns[0].uncachedInputTokens += 100;
    usage.turns[1].inputTokens -= 100;
    usage.turns[1].uncachedInputTokens -= 100;
    const usageText = `${JSON.stringify(usage, null, 2)}\n`;
    await writeFile(usagePath, usageText);

    const checksumsPath = join(bundle, "checksums.sha256");
    const usageSha256 = createHash("sha256").update(usageText).digest("hex");
    const checksums = (await readFile(checksumsPath, "utf8"))
      .replace(/^[a-f0-9]{64}  usage\.json$/m, `${usageSha256}  usage.json`);
    await writeFile(checksumsPath, checksums);

    await assert.rejects(() => replayEvidence(bundle), /usage evidence does not match events/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
