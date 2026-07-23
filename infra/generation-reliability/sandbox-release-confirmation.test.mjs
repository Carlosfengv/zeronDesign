import assert from "node:assert/strict";
import {
  attachSandboxReleaseEvidence,
  confirmSandboxRelease,
} from "./sandbox-release-confirmation.mjs";

const responses = [
  { ok: false, status: 503, error: "http_503" },
  { ok: true, status: 204 },
  { ok: true, status: 204 },
];
const delays = [];
const accepted = await confirmSandboxRelease({
  requestRelease: async () => responses.shift(),
  maxAttempts: 4,
  retryCooldownMs: 25,
  delay: async (milliseconds) => delays.push(milliseconds),
});
assert.equal(accepted.released, true);
assert.equal(accepted.attempts.length, 3);
assert.deepEqual(delays, [25, 25]);

const rejected = await confirmSandboxRelease({
  requestRelease: async () => ({ ok: false, status: 500, error: "http_500" }),
  maxAttempts: 2,
  retryCooldownMs: 0,
});
assert.equal(rejected.released, false);
assert.equal(rejected.attempts.length, 2);
const failedEvidence = attachSandboxReleaseEvidence(
  { schemaVersion: "fixture@1", status: "accepted" },
  rejected,
);
assert.equal(failedEvidence.status, "failed");
assert.equal(failedEvidence.cleanupError.classification, "sandbox_release_failed");

const retainedEvidence = attachSandboxReleaseEvidence(
  { schemaVersion: "fixture@1", status: "accepted" },
  accepted,
);
assert.equal(retainedEvidence.status, "accepted");
assert.equal(retainedEvidence.cleanupError, undefined);

await assert.rejects(
  () => confirmSandboxRelease({
    requestRelease: async () => ({ ok: true, status: 204 }),
    maxAttempts: 1,
    retryCooldownMs: 0,
    requiredSuccessfulResponses: 2,
  }),
  /maxAttempts must cover/,
);

process.stdout.write("Sandbox release confirmation tests passed\n");
