#!/usr/bin/env node

import { open, readFile, rm, writeFile } from "node:fs/promises";
import { readCanaryLedger } from "./design-context-canary-ledger.mjs";

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function positiveInteger(value) {
  return Number.isInteger(value) && value > 0;
}

function sha256(value) {
  return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
}

export function rollbackDiagnosticsReady(diagnostics) {
  if (diagnostics?.gate !== "ready") return false;
  const context = diagnostics?.generationContext;
  if (context) {
    return context.status === "compiled"
      && sha256(context.contextContentHash)
      && sha256(context.runContextBindingHash)
      && sha256(context.runtimeAttestationHash);
  }
  return Array.isArray(diagnostics?.missingRequiredReads)
    && diagnostics.missingRequiredReads.length === 0;
}

function parsePositiveInteger(value, label) {
  const parsed = Number(value);
  if (!positiveInteger(parsed)) throw new Error(`${label} must be a positive integer`);
  return parsed;
}

function requiredToken(value, name) {
  if (!hasText(value)) throw new Error(`${name} is required and must be supplied via environment`);
  return value;
}

async function runtimeJson(url, init) {
  const response = await fetch(url, { ...init, signal: AbortSignal.timeout(120_000) });
  const text = await response.text();
  if (!response.ok) throw new Error(`Runtime ${init?.method ?? "GET"} ${new URL(url).pathname} failed ${response.status}`);
  return JSON.parse(text);
}

export async function disableCanaryPolicy({
  baseUrl,
  projectId,
  designProfileId,
  designProfileVersion,
  expectedRevision,
  updatedBy,
  statePath,
  adminToken,
  recordedAt = new Date().toISOString(),
}) {
  requiredToken(adminToken, "RUNTIME_ADMIN_TOKEN");
  for (const [name, value] of Object.entries({ baseUrl, projectId, designProfileId, updatedBy, statePath })) {
    if (!hasText(value)) throw new Error(`${name} is required`);
  }
  if (!positiveInteger(designProfileVersion) || !positiveInteger(expectedRevision)) {
    throw new Error("designProfileVersion and expectedRevision must be positive integers");
  }
  // Reserve the evidence path before mutating Runtime. A stale/existing path must
  // fail before the CAS request, otherwise the policy can be disabled without a
  // writable audit state for the mandatory post-rollback verification.
  const stateFile = await open(statePath, "wx", 0o600);
  try {
    const response = await runtimeJson(
      new URL(`/internal/projects/${encodeURIComponent(projectId)}/design-context-enforcement`, baseUrl),
      {
        method: "PUT",
        headers: {
          "content-type": "application/json",
          "x-anydesign-internal": "true",
          "x-runtime-admin-token": adminToken,
        },
        body: JSON.stringify({
          designProfileId,
          designProfileVersion,
          enabled: false,
          expectedRevision,
          updatedBy,
        }),
      },
    );
    const policy = response?.policy;
    if (policy?.projectId !== projectId || policy?.designProfileId !== designProfileId
      || policy?.designProfileVersion !== designProfileVersion || policy?.enabled !== false
      || policy?.revision !== expectedRevision + 1 || policy?.updatedBy !== updatedBy) {
      throw new Error("Runtime returned an unexpected disabled policy identity");
    }
    const state = {
      schemaVersion: "design-context-canary-rollback-state@1",
      recordedAt,
      projectId,
      designProfileId,
      designProfileVersion,
      enforcedPolicyRevision: expectedRevision,
      policyRevisionAfterRollback: policy.revision,
      updatedBy,
    };
    await stateFile.writeFile(`${JSON.stringify(state, null, 2)}\n`, { encoding: "utf8" });
    await stateFile.sync();
    return state;
  } catch (error) {
    await stateFile.close().catch(() => {});
    await rm(statePath, { force: true }).catch(() => {});
    throw error;
  } finally {
    await stateFile.close().catch(() => {});
  }
}

function parseSseEvents(text) {
  return text.split(/\r?\n/)
    .filter(line => line.startsWith("data: "))
    .map(line => JSON.parse(line.slice(6)));
}

export async function verifyCanaryRollback({
  baseUrl,
  statePath,
  postRunId,
  ledgerPath,
  outputPath,
  sourceUri,
  principalToken,
  recordedAt = new Date().toISOString(),
}) {
  requiredToken(principalToken, "RUNTIME_PRINCIPAL_TOKEN");
  for (const [name, value] of Object.entries({ baseUrl, statePath, postRunId, ledgerPath, outputPath, sourceUri })) {
    if (!hasText(value)) throw new Error(`${name} is required`);
  }
  const state = JSON.parse(await readFile(statePath, "utf8"));
  if (state?.schemaVersion !== "design-context-canary-rollback-state@1"
    || !hasText(state?.projectId) || !hasText(state?.designProfileId)
    || !positiveInteger(state?.designProfileVersion) || !positiveInteger(state?.enforcedPolicyRevision)
    || state?.policyRevisionAfterRollback !== state.enforcedPolicyRevision + 1
    || !hasText(state?.updatedBy) || !Number.isFinite(Date.parse(state?.recordedAt))) {
    throw new Error("invalid canary rollback state");
  }
  const authorization = `Bearer ${principalToken}`;
  const diagnostics = await runtimeJson(
    new URL(`/runs/${encodeURIComponent(postRunId)}/design-context-diagnostics`, baseUrl),
    { headers: { authorization } },
  );
  const eventsResponse = await fetch(
    new URL(`/runs/${encodeURIComponent(postRunId)}/events`, baseUrl),
    { headers: { authorization }, signal: AbortSignal.timeout(120_000) },
  );
  const eventsText = await eventsResponse.text();
  if (!eventsResponse.ok) throw new Error(`Runtime events failed ${eventsResponse.status}`);
  const events = parseSseEvents(eventsText);
  const started = events.find(event => event.type === "run.started");
  const completed = events.find(event => event.type === "run.completed" && event.status === "completed");
  const policy = diagnostics?.package?.enforcementPolicy;
  if (diagnostics?.runId !== postRunId || diagnostics?.package?.designProfileId !== state.designProfileId
    || diagnostics?.package?.designProfileVersion !== state.designProfileVersion
    || diagnostics?.package?.effectiveCompatibilityMode !== "observe"
    || policy?.source !== "persistent" || policy?.enabled !== false
    || policy?.policyRevision !== state.policyRevisionAfterRollback
    || policy?.policyUpdatedBy !== state.updatedBy
    || !rollbackDiagnosticsReady(diagnostics)
    || !started || Date.parse(started.timestamp) < Date.parse(state.recordedAt) || !completed) {
    throw new Error("post-rollback Run does not prove a new observe Run with ready legacy or runtime-attested protocol evidence and frozen disabled policy");
  }
  const ledger = await readCanaryLedger(ledgerPath);
  const recovery = ledger.find(record => record.type === "profile-sync.recovery")?.payload;
  if (!recovery || recovery.projectId !== state.projectId || recovery.designProfileId !== state.designProfileId
    || recovery.designProfileVersion !== state.designProfileVersion
    || recovery.policyRevision !== state.enforcedPolicyRevision
    || recovery.recoveryRequiredObserved !== true || recovery.reusedChildRun !== true) {
    throw new Error("hash-chained Profile Sync recovery evidence was not preserved across rollback");
  }
  const event = {
    schemaVersion: "design-context-canary-event@1",
    type: "rollback",
    recordedAt,
    sourceUri,
    payload: {
      projectId: state.projectId,
      designProfileId: state.designProfileId,
      designProfileVersion: state.designProfileVersion,
      policyRevision: state.enforcedPolicyRevision,
      updatedBy: state.updatedBy,
      recordedAt,
      postRollbackRunId: postRunId,
      policyEnabledAfterRollback: false,
      policyRevisionAfterRollback: state.policyRevisionAfterRollback,
      postRollbackMode: "observe",
      newRunReadGateBlocked: false,
      operationRecoveryPreserved: true,
    },
  };
  await writeFile(outputPath, `${JSON.stringify(event, null, 2)}\n`, { encoding: "utf8", flag: "wx", mode: 0o600 });
  return event;
}

function parseCli(argv) {
  const [command, ...rest] = argv;
  const options = {};
  for (let index = 0; index < rest.length; index += 2) {
    if (!rest[index]?.startsWith("--") || rest[index + 1] === undefined) throw new Error(`invalid argument: ${rest[index] ?? "missing"}`);
    options[rest[index].slice(2)] = rest[index + 1];
  }
  return { command, options };
}

async function main() {
  const { command, options } = parseCli(process.argv.slice(2));
  if (command === "disable") {
    const state = await disableCanaryPolicy({
      baseUrl: options["base-url"],
      projectId: options["project-id"],
      designProfileId: options["design-profile-id"],
      designProfileVersion: parsePositiveInteger(options["design-profile-version"], "design-profile-version"),
      expectedRevision: parsePositiveInteger(options["expected-revision"], "expected-revision"),
      updatedBy: options["updated-by"],
      statePath: options.state,
      adminToken: process.env.RUNTIME_ADMIN_TOKEN,
    });
    process.stdout.write(`Canary policy disabled: revision=${state.policyRevisionAfterRollback} state=${options.state}\n`);
    return;
  }
  if (command === "verify") {
    await verifyCanaryRollback({
      baseUrl: options["base-url"],
      statePath: options.state,
      postRunId: options["post-run-id"],
      ledgerPath: options.ledger,
      outputPath: options.output,
      sourceUri: options["source-uri"],
      principalToken: process.env.RUNTIME_PRINCIPAL_TOKEN,
    });
    process.stdout.write(`Canary rollback verified: event=${options.output}\n`);
    return;
  }
  throw new Error("usage: run-design-context-canary-rollback.mjs disable --base-url <url> --project-id <id> --design-profile-id <id> --design-profile-version <n> --expected-revision <n> --updated-by <id> --state <json> | verify --base-url <url> --state <json> --post-run-id <id> --ledger <ndjson> --output <event.json> --source-uri <uri>");
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  await main();
}
