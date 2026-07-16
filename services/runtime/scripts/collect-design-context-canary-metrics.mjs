#!/usr/bin/env node
import { open, readFile, rm } from "node:fs/promises";
import { pathToFileURL } from "node:url";

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function positiveInteger(value) {
  return Number.isInteger(value) && value > 0;
}

function isoTimestamp(value) {
  return hasText(value) && Number.isFinite(Date.parse(value));
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

function validateSession(session) {
  const cohort = session?.cohort;
  const window = session?.window;
  if (session?.schemaVersion !== "design-context-canary-session-config@1"
    || !hasText(cohort?.projectId) || !hasText(cohort?.designProfileId)
    || !positiveInteger(cohort?.designProfileVersion)
    || !positiveInteger(cohort?.observePolicyRevision)
    || !positiveInteger(cohort?.policyRevision)
    || cohort.policyRevision <= cohort.observePolicyRevision
    || !isoTimestamp(window?.baselineStartedAt) || !isoTimestamp(window?.baselineEndedAt)
    || !isoTimestamp(window?.observationStartedAt)) {
    throw new Error("invalid design-context canary session config");
  }
  return { cohort, window };
}

function assertOperationalExport(document, cohort, window, observationEndedAt) {
  const exportedCohort = document?.cohort;
  const exportedWindow = document?.window;
  const metrics = document?.metrics;
  if (document?.schemaVersion !== "design-context-canary-operational-export@1"
    || document?.source?.kind !== "runtime-durable-store"
    || exportedCohort?.projectId !== cohort.projectId
    || exportedCohort?.designProfileId !== cohort.designProfileId
    || exportedCohort?.designProfileVersion !== cohort.designProfileVersion
    || exportedCohort?.observePolicyRevision !== cohort.observePolicyRevision
    || exportedCohort?.policyRevision !== cohort.policyRevision
    || Date.parse(exportedWindow?.baselineStartedAt) !== Date.parse(window.baselineStartedAt)
    || Date.parse(exportedWindow?.baselineEndedAt) !== Date.parse(window.baselineEndedAt)
    || Date.parse(exportedWindow?.observationStartedAt) !== Date.parse(window.observationStartedAt)
    || Date.parse(exportedWindow?.observationEndedAt) !== Date.parse(observationEndedAt)
    || !Array.isArray(document?.publish?.samples)
    || !Number.isInteger(metrics?.verifierUnavailableCount)
    || !Number.isInteger(metrics?.verifierRuntimeLostCount)
    || !Number.isInteger(metrics?.unexpectedReadGateBlockCount)
    || !Number.isInteger(metrics?.recoveryRequiredOver24hCount)
    || typeof metrics?.requiredFindingRepairRate !== "number"
    || typeof document?.alertsTriggered !== "boolean"
    || !hasText(document?.conclusionRecordedBy)) {
    throw new Error("Runtime returned an invalid or mismatched canary operational export");
  }
  const sampleIds = new Set();
  for (const sample of document.publish.samples) {
    const expectedRevision = sample?.mode === "baseline"
      ? cohort.observePolicyRevision
      : cohort.policyRevision;
    if (!hasText(sample?.sampleId) || sampleIds.has(sample.sampleId) || !hasText(sample?.runId)
      || !isoTimestamp(sample?.observedAt) || !["baseline", "enforced"].includes(sample?.mode)
      || !["pass", "fail"].includes(sample?.publishVerdict)
      || typeof sample?.dcpCausedFailure !== "boolean"
      || sample?.projectId !== cohort.projectId
      || sample?.designProfileId !== cohort.designProfileId
      || sample?.designProfileVersion !== cohort.designProfileVersion
      || sample?.policyRevision !== expectedRevision) {
      throw new Error(`invalid Runtime publish sample: ${sample?.sampleId ?? "unknown"}`);
    }
    sampleIds.add(sample.sampleId);
  }
}

function assertNoCredentialLikeOutput(document) {
  const raw = JSON.stringify(document);
  if (/\b(?:sk|api)[-_][A-Za-z0-9_-]{16,}\b/i.test(raw)
    || /\bBearer\s+[A-Za-z0-9._~+\/-]+=*\b/i.test(raw)
    || /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/.test(raw)
    || /"(?:apiKey|authorization|token|secret|password)"\s*:/i.test(raw)) {
    throw new Error("Runtime canary export contains credential-like content");
  }
}

async function writeNewJsonSet(entries) {
  const opened = [];
  try {
    for (const entry of entries) {
      opened.push({ ...entry, handle: await open(entry.path, "wx", 0o600) });
    }
    for (const entry of opened) {
      await entry.handle.writeFile(`${JSON.stringify(entry.value, null, 2)}\n`);
    }
  } catch (error) {
    for (const entry of opened) await entry.handle.close().catch(() => {});
    for (const entry of opened) await rm(entry.path, { force: true }).catch(() => {});
    throw error;
  }
  for (const entry of opened) await entry.handle.close();
}

export async function collectDesignContextCanaryMetrics({
  baseUrl,
  sessionPath,
  observationEndedAt,
  conclusionRecordedBy,
  exportOutput,
  metricsOutput,
  metricsSourceUri,
  samplesOutput,
  samplesSourceUri,
  adminToken,
  fetchImpl = fetch,
}) {
  if (!hasText(baseUrl) || !hasText(sessionPath) || !isoTimestamp(observationEndedAt)
    || !hasText(conclusionRecordedBy) || !hasText(exportOutput) || !hasText(metricsOutput)
    || !hasText(metricsSourceUri) || !hasText(adminToken)
    || (samplesOutput && !hasText(samplesSourceUri))) {
    throw new Error("missing required canary metrics collection input");
  }
  const session = JSON.parse(await readFile(sessionPath, "utf8"));
  const { cohort, window } = validateSession(session);
  if (Date.parse(observationEndedAt) <= Date.parse(window.observationStartedAt)) {
    throw new Error("observation-ended-at must follow the session observation start");
  }
  const url = new URL(
    `/internal/projects/${encodeURIComponent(cohort.projectId)}/design-context-canary-metrics`,
    baseUrl,
  );
  for (const [key, value] of Object.entries({
    designProfileId: cohort.designProfileId,
    designProfileVersion: cohort.designProfileVersion,
    observePolicyRevision: cohort.observePolicyRevision,
    policyRevision: cohort.policyRevision,
    baselineStartedAt: window.baselineStartedAt,
    baselineEndedAt: window.baselineEndedAt,
    observationStartedAt: window.observationStartedAt,
    observationEndedAt,
    conclusionRecordedBy,
  })) url.searchParams.set(key, String(value));
  const response = await fetchImpl(url, {
    headers: {
      "x-anydesign-internal": "true",
      "x-runtime-admin-token": adminToken,
    },
  });
  const body = await response.text();
  if (!response.ok) throw new Error(`Runtime canary metrics export failed with HTTP ${response.status}`);
  const document = JSON.parse(body);
  assertOperationalExport(document, cohort, window, observationEndedAt);
  assertNoCredentialLikeOutput(document);

  const recordedAt = document.generatedAt;
  if (!isoTimestamp(recordedAt)) throw new Error("Runtime canary export generatedAt is invalid");
  const metricsEvent = {
    schemaVersion: "design-context-canary-event@1",
    type: "metrics.snapshot",
    recordedAt,
    sourceUri: metricsSourceUri,
    payload: {
      observationEndedAt: document.window.observationEndedAt,
      conclusionRecordedBy: document.conclusionRecordedBy,
      verifierUnavailableCount: document.metrics.verifierUnavailableCount,
      verifierRuntimeLostCount: document.metrics.verifierRuntimeLostCount,
      unexpectedReadGateBlockCount: document.metrics.unexpectedReadGateBlockCount,
      recoveryRequiredOver24hCount: document.metrics.recoveryRequiredOver24hCount,
      requiredFindingRepairRate: document.metrics.requiredFindingRepairRate,
      alertsTriggered: document.alertsTriggered,
    },
  };
  if (samplesOutput && document.publish.samples.length === 0) {
    throw new Error("cannot emit an empty publish.samples fragment");
  }
  let samplesEvent = null;
  if (samplesOutput) {
    samplesEvent = {
      schemaVersion: "design-context-canary-event@1",
      type: "publish.samples",
      recordedAt,
      sourceUri: samplesSourceUri,
      payload: { samples: document.publish.samples },
    };
  }
  await writeNewJsonSet([
    { path: exportOutput, value: document },
    { path: metricsOutput, value: metricsEvent },
    ...(samplesEvent ? [{ path: samplesOutput, value: samplesEvent }] : []),
  ]);
  return { document, metricsEvent, samplesEvent };
}

async function main() {
  const options = parseCli(process.argv.slice(2));
  await collectDesignContextCanaryMetrics({
    baseUrl: options["base-url"],
    sessionPath: options.session,
    observationEndedAt: options["observation-ended-at"],
    conclusionRecordedBy: options["conclusion-recorded-by"],
    exportOutput: options["export-output"],
    metricsOutput: options["metrics-output"],
    metricsSourceUri: options["metrics-source-uri"],
    samplesOutput: options["samples-output"],
    samplesSourceUri: options["samples-source-uri"],
    adminToken: process.env.RUNTIME_ADMIN_TOKEN,
  });
  process.stdout.write(`Design-context canary metrics collected: ${options["export-output"]}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
