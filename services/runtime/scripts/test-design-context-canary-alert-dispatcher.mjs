#!/usr/bin/env node

import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { access, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  dispatchDesignContextCanaryAlerts,
  probeDesignContextCanaryAlertDestination,
} from "./dispatch-design-context-canary-alerts.mjs";

const secret = "test-only-webhook-signing-key";

function exportDocument(overrides = {}) {
  return {
    schemaVersion: "design-context-canary-operational-export@1",
    generatedAt: "2026-07-16T00:00:00Z",
    source: { kind: "runtime-durable-store" },
    cohort: {
      projectId: "project-1",
      designProfileId: "profile-1",
      designProfileVersion: 1,
      observePolicyRevision: 1,
      policyRevision: 2,
    },
    window: {
      observationStartedAt: "2026-07-15T00:00:00Z",
      observationEndedAt: "2026-07-16T00:00:00Z",
    },
    alerts: [
      {
        code: "verifier_unavailable",
        severity: "page",
        triggered: true,
        actual: 1,
        threshold: "must_equal_0",
        action: "page_and_disable_affected_exact_policy",
      },
    ],
    alertsTriggered: true,
    conclusionRecordedBy: "operator-1",
    ...overrides,
  };
}

const root = await mkdtemp(join(tmpdir(), "design-context-canary-alert-dispatcher-"));
try {
  const inputPath = join(root, "operational-export.json");
  const outputPath = join(root, "alert-delivery.json");
  await writeFile(inputPath, `${JSON.stringify(exportDocument())}\n`);
  let calls = 0;
  const result = await dispatchDesignContextCanaryAlerts({
    inputPath,
    outputPath,
    sourceUri: "evidence://canary/alert-delivery/1",
    destinationId: "primary-oncall",
    webhookUrlValue: "https://alerts.example.invalid/v1/canary",
    webhookSecret: secret,
    now: () => new Date("2026-07-16T00:01:00Z"),
    fetchImpl: async (url, init) => {
      calls += 1;
      assert.equal(url.href, "https://alerts.example.invalid/v1/canary");
      assert.equal(init.method, "POST");
      assert.equal(init.redirect, "error");
      const expected = createHmac("sha256", secret).update(init.body).digest("hex");
      assert.equal(init.headers["x-anydesign-canary-signature"], `sha256=${expected}`);
      assert.equal(init.headers["x-anydesign-canary-event-id"], JSON.parse(init.body).eventId);
      assert.deepEqual(JSON.parse(init.body).triggeredAlerts.map(alert => alert.code), ["verifier_unavailable"]);
      return new Response("ignored-sensitive-response-body", { status: 202 });
    },
  });
  assert.equal(calls, 1);
  assert.equal(result.event.type, "alert.delivery");
  assert.equal(result.event.payload.deliveryStatus, "delivered");
  assert.equal(result.event.payload.responseStatus, 202);
  assert.deepEqual(result.event.payload.triggeredAlertCodes, ["verifier_unavailable"]);
  const persisted = await readFile(outputPath, "utf8");
  assert.doesNotMatch(persisted, /alerts\.example|signing-key|sensitive-response/);
  assert.equal((await stat(outputPath)).mode & 0o777, 0o600);
  await assert.rejects(() => dispatchDesignContextCanaryAlerts({
    inputPath,
    outputPath,
    sourceUri: "evidence://canary/alert-delivery/duplicate",
    destinationId: "primary-oncall",
    webhookUrlValue: "https://alerts.example.invalid/v1/canary",
    webhookSecret: secret,
    fetchImpl: async () => {
      calls += 1;
      return new Response(null, { status: 202 });
    },
  }), /EEXIST/);
  assert.equal(calls, 1, "an existing evidence path must prevent duplicate delivery");

  const quietInput = join(root, "quiet-export.json");
  const quietOutput = join(root, "quiet-delivery.json");
  await writeFile(quietInput, `${JSON.stringify(exportDocument({
    alerts: [{
      code: "verifier_unavailable",
      severity: "ok",
      triggered: false,
      actual: 0,
      threshold: "must_equal_0",
      action: "page_and_disable_affected_exact_policy",
    }],
    alertsTriggered: false,
  }))}\n`);
  const quiet = await dispatchDesignContextCanaryAlerts({
    inputPath: quietInput,
    outputPath: quietOutput,
    sourceUri: "evidence://canary/alert-delivery/quiet",
    destinationId: "primary-oncall",
    now: () => new Date("2026-07-16T00:02:00Z"),
    fetchImpl: async () => assert.fail("quiet snapshots must not call the webhook"),
  });
  assert.equal(quiet.event.payload.deliveryStatus, "not-required");
  assert.equal(quiet.event.payload.attemptCount, 0);

  const probeOutput = join(root, "destination-probe.json");
  const probe = await probeDesignContextCanaryAlertDestination({
    outputPath: probeOutput,
    sourceUri: "evidence://canary/alert-destination-probe",
    destinationId: "primary-oncall",
    operatorId: "operator-1",
    webhookUrlValue: "https://alerts.example.invalid/v1/canary",
    webhookSecret: secret,
    now: () => new Date("2026-07-15T23:59:00Z"),
    fetchImpl: async (_url, init) => {
      const body = JSON.parse(init.body);
      assert.equal(body.schemaVersion, "design-context-canary-alert-probe@1");
      assert.equal(body.purpose, "pre-canary-destination-readiness");
      return new Response(null, { status: 204 });
    },
  });
  assert.equal(probe.event.type, "alert.destination-probe");
  assert.equal(probe.event.payload.probeStatus, "delivered");
  assert.equal(probe.event.payload.responseStatus, 204);
  assert.doesNotMatch(await readFile(probeOutput, "utf8"), /alerts\.example|signing-key/);

  for (const [name, options, pattern] of [
    ["missing-secret", { webhookUrlValue: "https://alerts.example.invalid/v1/canary" }, /WEBHOOK_SECRET/],
    ["insecure-url", { webhookUrlValue: "http://alerts.example.invalid/v1/canary", webhookSecret: secret }, /must be HTTPS/],
    ["failed-delivery", { webhookUrlValue: "https://alerts.example.invalid/v1/canary", webhookSecret: secret,
      fetchImpl: async () => new Response(null, { status: 503 }) }, /HTTP 503/],
  ]) {
    const failedOutput = join(root, `${name}.json`);
    await assert.rejects(() => dispatchDesignContextCanaryAlerts({
      inputPath,
      outputPath: failedOutput,
      sourceUri: `evidence://canary/alert-delivery/${name}`,
      destinationId: "primary-oncall",
      ...options,
    }), pattern);
    await assert.rejects(access(failedOutput), /ENOENT/);
  }

  const inconsistentInput = join(root, "inconsistent-export.json");
  await writeFile(inconsistentInput, `${JSON.stringify(exportDocument({ alertsTriggered: false }))}\n`);
  await assert.rejects(() => dispatchDesignContextCanaryAlerts({
    inputPath: inconsistentInput,
    outputPath: join(root, "inconsistent-delivery.json"),
    sourceUri: "evidence://canary/alert-delivery/inconsistent",
    destinationId: "primary-oncall",
  }), /does not match/);

  const secretInput = join(root, "secret-export.json");
  await writeFile(secretInput, `${JSON.stringify({ ...exportDocument(), authorization: "Bearer forbidden-value-123456" })}\n`);
  await assert.rejects(() => dispatchDesignContextCanaryAlerts({
    inputPath: secretInput,
    outputPath: join(root, "secret-delivery.json"),
    sourceUri: "evidence://canary/alert-delivery/secret",
    destinationId: "primary-oncall",
  }), /credential-like/);
} finally {
  await rm(root, { recursive: true, force: true });
}

process.stdout.write("design-context canary alert dispatcher tests passed\n");
