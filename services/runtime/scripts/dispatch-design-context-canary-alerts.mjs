#!/usr/bin/env node

import { createHash, createHmac } from "node:crypto";
import { open, readFile, rm } from "node:fs/promises";
import { pathToFileURL } from "node:url";

const ALERT_CODES = new Set([
  "publish_failure_rate_delta",
  "verifier_unavailable",
  "verifier_runtime_lost",
  "unexpected_read_gate_block",
  "profile_sync_recovery_over_24h",
  "required_finding_repair_rate",
]);

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function isoTimestamp(value) {
  return hasText(value) && Number.isFinite(Date.parse(value));
}

function assertNoCredentialLikeContent(raw) {
  if (/\b(?:sk|api)[-_][A-Za-z0-9_-]{16,}\b/i.test(raw)
    || /\bBearer\s+[A-Za-z0-9._~+\/-]+=*\b/i.test(raw)
    || /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/.test(raw)
    || /"(?:apiKey|authorization|token|secret|password)"\s*:/i.test(raw)
    || /[?&](?:token|access_token|api_key|sig|signature)=[^&\s]+/i.test(raw)) {
    throw new Error("canary operational export contains credential-like content");
  }
}

function validateOperationalExport(document) {
  const cohort = document?.cohort;
  const window = document?.window;
  if (document?.schemaVersion !== "design-context-canary-operational-export@1"
    || document?.source?.kind !== "runtime-durable-store"
    || !isoTimestamp(document?.generatedAt)
    || !hasText(document?.conclusionRecordedBy)
    || !hasText(cohort?.projectId) || !hasText(cohort?.designProfileId)
    || !Number.isInteger(cohort?.designProfileVersion) || cohort.designProfileVersion <= 0
    || !Number.isInteger(cohort?.observePolicyRevision) || cohort.observePolicyRevision <= 0
    || !Number.isInteger(cohort?.policyRevision) || cohort.policyRevision <= cohort.observePolicyRevision
    || !isoTimestamp(window?.observationStartedAt) || !isoTimestamp(window?.observationEndedAt)
    || !Array.isArray(document?.alerts) || typeof document?.alertsTriggered !== "boolean") {
    throw new Error("invalid design-context canary operational export");
  }
  const triggered = [];
  const seen = new Set();
  for (const alert of document.alerts) {
    if (!ALERT_CODES.has(alert?.code) || seen.has(alert.code)
      || typeof alert?.triggered !== "boolean" || !["ok", "page"].includes(alert?.severity)
      || alert.severity !== (alert.triggered ? "page" : "ok")
      || typeof alert?.actual !== "number" || !Number.isFinite(alert.actual)
      || !hasText(alert?.threshold) || !hasText(alert?.action)) {
      throw new Error(`invalid or duplicate Runtime canary alert: ${alert?.code ?? "unknown"}`);
    }
    seen.add(alert.code);
    if (alert.triggered) triggered.push(alert);
  }
  if (document.alertsTriggered !== (triggered.length > 0)) {
    throw new Error("Runtime canary alertsTriggered does not match the alert decisions");
  }
  return triggered;
}

function webhookUrl(value) {
  if (!hasText(value)) throw new Error("CANARY_ALERT_WEBHOOK_URL is required for webhook delivery");
  const parsed = new URL(value);
  if (parsed.protocol !== "https:" || parsed.username || parsed.password) {
    throw new Error("canary alert webhook must be HTTPS and must not contain URL credentials");
  }
  return parsed;
}

function notificationFor(document, triggeredAlerts) {
  const notification = {
    schemaVersion: "design-context-canary-alert-notification@1",
    operationalExportGeneratedAt: document.generatedAt,
    cohort: document.cohort,
    window: {
      observationStartedAt: document.window.observationStartedAt,
      observationEndedAt: document.window.observationEndedAt,
    },
    triggeredAlerts: triggeredAlerts.map(alert => ({
      code: alert.code,
      severity: alert.severity,
      actual: alert.actual,
      threshold: alert.threshold,
      action: alert.action,
    })),
    conclusionRecordedBy: document.conclusionRecordedBy,
  };
  const canonical = JSON.stringify(notification);
  return {
    ...notification,
    eventId: createHash("sha256").update(canonical).digest("hex"),
  };
}

async function writeReservedJson(handle, value) {
  await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`);
  await handle.close();
}

async function sendSignedWebhook({ webhookUrlValue, webhookSecret, notification, fetchImpl }) {
  const destination = webhookUrl(webhookUrlValue);
  if (!hasText(webhookSecret)) throw new Error("CANARY_ALERT_WEBHOOK_SECRET is required");
  const body = JSON.stringify(notification);
  const signature = createHmac("sha256", webhookSecret).update(body).digest("hex");
  const response = await fetchImpl(destination, {
    method: "POST",
    redirect: "error",
    signal: AbortSignal.timeout(10_000),
    headers: {
      "content-type": "application/json",
      "x-anydesign-canary-event-id": notification.eventId,
      "x-anydesign-canary-signature": `sha256=${signature}`,
    },
    body,
  });
  if (!response.ok) throw new Error(`canary alert webhook failed with HTTP ${response.status}`);
  return response.status;
}

export async function dispatchDesignContextCanaryAlerts({
  inputPath,
  outputPath,
  sourceUri,
  destinationId,
  webhookUrlValue,
  webhookSecret,
  fetchImpl = fetch,
  now = () => new Date(),
}) {
  if (!hasText(inputPath) || !hasText(outputPath) || !hasText(sourceUri) || !hasText(destinationId)) {
    throw new Error("input, output, source URI and destination ID are required");
  }
  const raw = await readFile(inputPath, "utf8");
  assertNoCredentialLikeContent(raw);
  const document = JSON.parse(raw);
  const triggeredAlerts = validateOperationalExport(document);
  const notification = notificationFor(document, triggeredAlerts);
  const reserved = await open(outputPath, "wx", 0o600);
  try {
    let responseStatus = null;
    let deliveryStatus = "not-required";
    if (triggeredAlerts.length > 0) {
      responseStatus = await sendSignedWebhook({
        webhookUrlValue,
        webhookSecret,
        notification,
        fetchImpl,
      });
      deliveryStatus = "delivered";
    }
    const event = {
      schemaVersion: "design-context-canary-event@1",
      type: "alert.delivery",
      recordedAt: now().toISOString(),
      sourceUri,
      payload: {
        operationalExportGeneratedAt: document.generatedAt,
        observationEndedAt: document.window.observationEndedAt,
        cohort: document.cohort,
        eventId: notification.eventId,
        destinationId,
        deliveryRequired: triggeredAlerts.length > 0,
        deliveryStatus,
        responseStatus,
        attemptCount: triggeredAlerts.length > 0 ? 1 : 0,
        triggeredAlertCodes: triggeredAlerts.map(alert => alert.code),
      },
    };
    assertNoCredentialLikeContent(JSON.stringify(event));
    await writeReservedJson(reserved, event);
    return { event, notification };
  } catch (error) {
    await reserved.close().catch(() => {});
    await rm(outputPath, { force: true }).catch(() => {});
    throw error;
  }
}

export async function probeDesignContextCanaryAlertDestination({
  outputPath,
  sourceUri,
  destinationId,
  operatorId,
  webhookUrlValue,
  webhookSecret,
  fetchImpl = fetch,
  now = () => new Date(),
}) {
  if (!hasText(outputPath) || !hasText(sourceUri) || !hasText(destinationId) || !hasText(operatorId)) {
    throw new Error("output, source URI, destination ID and operator ID are required");
  }
  const recordedAt = now().toISOString();
  if (!isoTimestamp(recordedAt)) throw new Error("alert destination probe timestamp is invalid");
  const unsigned = {
    schemaVersion: "design-context-canary-alert-probe@1",
    recordedAt,
    destinationId,
    operatorId,
    purpose: "pre-canary-destination-readiness",
  };
  const notification = {
    ...unsigned,
    eventId: createHash("sha256").update(JSON.stringify(unsigned)).digest("hex"),
  };
  const reserved = await open(outputPath, "wx", 0o600);
  try {
    const responseStatus = await sendSignedWebhook({
      webhookUrlValue,
      webhookSecret,
      notification,
      fetchImpl,
    });
    const event = {
      schemaVersion: "design-context-canary-event@1",
      type: "alert.destination-probe",
      recordedAt,
      sourceUri,
      payload: {
        destinationId,
        operatorId,
        eventId: notification.eventId,
        probeStatus: "delivered",
        responseStatus,
        attemptCount: 1,
      },
    };
    assertNoCredentialLikeContent(JSON.stringify(event));
    await writeReservedJson(reserved, event);
    return { event, notification };
  } catch (error) {
    await reserved.close().catch(() => {});
    await rm(outputPath, { force: true }).catch(() => {});
    throw error;
  }
}

function parseCli(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || value === undefined) throw new Error(`invalid argument: ${key ?? ""}`);
    options[key.slice(2)] = value;
  }
  return options;
}

async function main() {
  const [command, ...args] = process.argv.slice(2);
  const options = parseCli(args);
  if (command === "dispatch") {
    const { event } = await dispatchDesignContextCanaryAlerts({
      inputPath: options.input,
      outputPath: options.output,
      sourceUri: options["source-uri"],
      destinationId: options["destination-id"],
      webhookUrlValue: process.env.CANARY_ALERT_WEBHOOK_URL,
      webhookSecret: process.env.CANARY_ALERT_WEBHOOK_SECRET,
    });
    process.stdout.write(`Design-context canary alert decision recorded: ${event.payload.deliveryStatus}\n`);
    return;
  }
  if (command === "probe") {
    await probeDesignContextCanaryAlertDestination({
      outputPath: options.output,
      sourceUri: options["source-uri"],
      destinationId: options["destination-id"],
      operatorId: options["operator-id"],
      webhookUrlValue: process.env.CANARY_ALERT_WEBHOOK_URL,
      webhookSecret: process.env.CANARY_ALERT_WEBHOOK_SECRET,
    });
    process.stdout.write("Design-context canary alert destination probe delivered\n");
    return;
  }
  throw new Error("usage: dispatch-design-context-canary-alerts.mjs dispatch --input <operational-export.json> --output <event.json> --source-uri <uri> --destination-id <id> | probe --output <event.json> --source-uri <uri> --destination-id <id> --operator-id <id>");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
