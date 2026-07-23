#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  evaluateRuntimeEfficiencyBenchmark,
  validateRuntimeEfficiencyBenchmarkAttempt,
} from "./runtime-efficiency-benchmark.mjs";

const SESSION_SCHEMA = "runtime-efficiency-benchmark-session@1";
const RECORD_SCHEMA = "runtime-efficiency-benchmark-ledger-record@1";
const LEDGER_SCHEMA = "runtime-efficiency-benchmark-ledger@1";
const COHORT_SCHEMA = "runtime-efficiency-benchmark-cohort@1";
const CALCULATOR_VERSION = "runtime-efficiency-benchmark-calculator@1";
const SHA256 = /^[a-f0-9]{64}$/;
const ISO_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/;
const FORBIDDEN_KEY = /(?:api_?key|authorization|credential|secret|prompt_?(?:text|body|content)|source_?content|provider_?(?:response|body)|request_?body|response_?body|signed_?url|temporary_?url)/i;
const SECRET_VALUE = /(?:\bBearer\s+[A-Za-z0-9._~+/=-]{8,}|\bsk-[A-Za-z0-9_-]{8,}|data:image\/)/i;
const PROFILE_KEYS = [
  "profileId",
  "workload",
  "designProfileHash",
  "templateId",
  "templateVersion",
  "modelResourceId",
  "providerResourceRevision",
  "modelVersion",
  "providerParametersHash",
  "cacheUsageCapability",
];
const SESSION_KEYS = [
  "schemaVersion",
  "sessionId",
  "createdAt",
  "calculatorVersion",
  "source",
  "bootstrap",
  "promptSet",
  "profiles",
];

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

function exactKeys(value, keys, location) {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${location} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index])) {
    throw new Error(`${location} keys must be exactly ${expected.join(",")}`);
  }
}

function rejectSensitiveMaterial(value, location = "document") {
  if (typeof value === "string") {
    if (SECRET_VALUE.test(value)) throw new Error(`${location} contains credential or payload material`);
    return;
  }
  if (Array.isArray(value)) {
    value.forEach((entry, index) => rejectSensitiveMaterial(entry, `${location}[${index}]`));
    return;
  }
  if (!value || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    const normalized = key.replaceAll(/([a-z])([A-Z])/g, "$1_$2");
    if (FORBIDDEN_KEY.test(normalized)) throw new Error(`${location}.${key} is forbidden in benchmark evidence`);
    rejectSensitiveMaterial(child, `${location}.${key}`);
  }
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function validateSession(session) {
  rejectSensitiveMaterial(session, "session");
  exactKeys(session, SESSION_KEYS, "session");
  if (session.schemaVersion !== SESSION_SCHEMA) throw new Error(`session.schemaVersion must be ${SESSION_SCHEMA}`);
  if (!hasText(session.sessionId)) throw new Error("session.sessionId is required");
  if (!ISO_TIMESTAMP.test(session.createdAt ?? "")) throw new Error("session.createdAt must be an ISO UTC timestamp");
  if (session.calculatorVersion !== CALCULATOR_VERSION) {
    throw new Error(`session.calculatorVersion must be ${CALCULATOR_VERSION}`);
  }
  exactKeys(session.source, ["commit", "dirty"], "session.source");
  if (!hasText(session.source.commit)) throw new Error("session.source.commit is required");
  if (typeof session.source.dirty !== "boolean") throw new Error("session.source.dirty must be boolean");
  exactKeys(session.bootstrap, ["iterations", "seed"], "session.bootstrap");
  if (!Number.isSafeInteger(session.bootstrap.iterations) || session.bootstrap.iterations < 100) {
    throw new Error("session.bootstrap.iterations must be >= 100");
  }
  if (!Number.isSafeInteger(session.bootstrap.seed)) throw new Error("session.bootstrap.seed must be an integer");
  exactKeys(session.promptSet, ["id", "version", "sha256", "promptIds"], "session.promptSet");
  if (!hasText(session.promptSet.id) || !hasText(session.promptSet.version)) {
    throw new Error("session.promptSet id and version are required");
  }
  if (!SHA256.test(session.promptSet.sha256 ?? "")) throw new Error("session.promptSet.sha256 must be sha256");
  if (!Array.isArray(session.promptSet.promptIds)
    || session.promptSet.promptIds.length < 10
    || new Set(session.promptSet.promptIds).size !== session.promptSet.promptIds.length
    || session.promptSet.promptIds.some(id => !hasText(id))) {
    throw new Error("session.promptSet.promptIds must contain at least 10 unique IDs");
  }
  if (!Array.isArray(session.profiles) || !session.profiles.length) {
    throw new Error("session.profiles must be a non-empty array");
  }
  const profileIds = new Set();
  for (const [index, profile] of session.profiles.entries()) {
    const location = `session.profiles[${index}]`;
    exactKeys(profile, PROFILE_KEYS, location);
    if (!hasText(profile.profileId) || profileIds.has(profile.profileId)) {
      throw new Error(`${location}.profileId must be unique`);
    }
    profileIds.add(profile.profileId);
    if (!["greenfield_build", "style_token_edit"].includes(profile.workload)) {
      throw new Error(`${location}.workload is invalid`);
    }
    for (const field of ["designProfileHash", "providerParametersHash"]) {
      if (!SHA256.test(profile[field] ?? "")) throw new Error(`${location}.${field} must be sha256`);
    }
    for (const field of ["templateId", "templateVersion", "modelResourceId", "modelVersion"]) {
      if (!hasText(profile[field])) throw new Error(`${location}.${field} is required`);
    }
    if (!Number.isSafeInteger(profile.providerResourceRevision) || profile.providerResourceRevision <= 0) {
      throw new Error(`${location}.providerResourceRevision must be positive`);
    }
    if (!["reported", "unsupported"].includes(profile.cacheUsageCapability)) {
      throw new Error(`${location}.cacheUsageCapability is invalid`);
    }
  }
}

function recordPayload(record) {
  const { recordHash, ...payload } = record;
  return payload;
}

function makeRecord(kind, payload, previousRecordHash) {
  const record = { schemaVersion: RECORD_SCHEMA, kind, previousRecordHash, payload };
  return { ...record, recordHash: sha256(canonical(record)) };
}

function ledgerBytes(records) {
  return `${records.map(record => JSON.stringify(record)).join("\n")}\n`;
}

function parseLedger(ledgerFile) {
  const bytes = fs.readFileSync(ledgerFile, "utf8");
  if (!bytes.endsWith("\n")) throw new Error("benchmark ledger must end with a newline");
  const lines = bytes.split(/\r?\n/).filter(Boolean);
  if (!lines.length) throw new Error("benchmark ledger is empty");
  const records = lines.map((line, index) => {
    try {
      return JSON.parse(line);
    } catch {
      throw new Error(`benchmark ledger line ${index + 1} is not JSON`);
    }
  });
  let previousRecordHash = null;
  records.forEach((record, index) => {
    exactKeys(
      record,
      ["schemaVersion", "kind", "previousRecordHash", "payload", "recordHash"],
      `ledger line ${index + 1}`,
    );
    if (record.schemaVersion !== RECORD_SCHEMA) throw new Error(`ledger line ${index + 1} schema is invalid`);
    if (record.previousRecordHash !== previousRecordHash) throw new Error(`ledger hash chain breaks at line ${index + 1}`);
    if (record.recordHash !== sha256(canonical(recordPayload(record)))) {
      throw new Error(`ledger record hash mismatch at line ${index + 1}`);
    }
    previousRecordHash = record.recordHash;
  });
  if (records[0].kind !== "session" || records.slice(1).some(record => record.kind !== "attempt")) {
    throw new Error("benchmark ledger must contain one leading session followed by Attempts");
  }
  const session = records[0].payload;
  validateSession(session);
  const attempts = records.slice(1).map(record => record.payload);
  const attemptIds = new Set();
  for (const [index, attemptRecord] of attempts.entries()) {
    exactKeys(attemptRecord, ["profileId", "attempt"], `attempt record ${index + 1}`);
    const profile = session.profiles.find(item => item.profileId === attemptRecord.profileId);
    if (!profile) throw new Error(`attempt record ${index + 1} references an unknown Profile`);
    const errors = validateRuntimeEfficiencyBenchmarkAttempt(
      attemptRecord.attempt,
      profile,
      session.promptSet,
    );
    if (errors.length) throw new Error(`attempt record ${index + 1} is invalid: ${errors.join("; ")}`);
    if (attemptRecord.attempt.sequence !== index + 1) {
      throw new Error(`attempt sequence must be continuous at record ${index + 1}`);
    }
    if (attemptIds.has(attemptRecord.attempt.attemptId)) {
      throw new Error(`duplicate Attempt ID: ${attemptRecord.attempt.attemptId}`);
    }
    attemptIds.add(attemptRecord.attempt.attemptId);
  }
  return {
    bytes,
    records,
    session,
    attempts,
    headRecordHash: previousRecordHash,
  };
}

function withLock(ledgerFile, operation) {
  const lockFile = `${ledgerFile}.lock`;
  let descriptor;
  try {
    descriptor = fs.openSync(lockFile, "wx", 0o600);
  } catch (error) {
    if (error?.code === "EEXIST") throw new Error(`benchmark ledger is locked: ${lockFile}`);
    throw error;
  }
  try {
    return operation();
  } finally {
    fs.closeSync(descriptor);
    fs.unlinkSync(lockFile);
  }
}

export function initializeBenchmarkLedger(ledgerFile, session) {
  validateSession(session);
  fs.mkdirSync(path.dirname(path.resolve(ledgerFile)), { recursive: true });
  const record = makeRecord("session", session, null);
  const descriptor = fs.openSync(ledgerFile, "wx", 0o600);
  try {
    fs.writeFileSync(descriptor, ledgerBytes([record]));
    fs.fsyncSync(descriptor);
  } finally {
    fs.closeSync(descriptor);
  }
  return verifyBenchmarkLedger(ledgerFile);
}

export function appendBenchmarkAttempt(ledgerFile, attemptRecord) {
  return appendBenchmarkAttempts(ledgerFile, [attemptRecord]);
}

export function appendBenchmarkAttempts(ledgerFile, attemptRecords) {
  if (!Array.isArray(attemptRecords) || attemptRecords.length === 0) {
    throw new Error("attemptRecords must be a non-empty array");
  }
  return withLock(ledgerFile, () => {
    const parsed = parseLedger(ledgerFile);
    const attemptIds = new Set(parsed.attempts.map(existing => existing.attempt.attemptId));
    for (const [index, attemptRecord] of attemptRecords.entries()) {
      const location = `attemptRecords[${index}]`;
      rejectSensitiveMaterial(attemptRecord, location);
      exactKeys(attemptRecord, ["profileId", "attempt"], location);
      const profile = parsed.session.profiles.find(item => item.profileId === attemptRecord.profileId);
      if (!profile) throw new Error(`${location}.profileId is not frozen in the Session`);
      const errors = validateRuntimeEfficiencyBenchmarkAttempt(
        attemptRecord.attempt,
        profile,
        parsed.session.promptSet,
      );
      if (errors.length) throw new Error(`${location}: ${errors.join("; ")}`);
      const expectedSequence = parsed.attempts.length + index + 1;
      if (attemptRecord.attempt.sequence !== expectedSequence) {
        throw new Error(`${location}.attempt.sequence must be ${expectedSequence}`);
      }
      if (attemptIds.has(attemptRecord.attempt.attemptId)) {
        throw new Error(`duplicate Attempt ID: ${attemptRecord.attempt.attemptId}`);
      }
      attemptIds.add(attemptRecord.attempt.attemptId);
    }
    let previousRecordHash = parsed.headRecordHash;
    const records = attemptRecords.map((attemptRecord) => {
      const record = makeRecord("attempt", attemptRecord, previousRecordHash);
      previousRecordHash = record.recordHash;
      return record;
    });
    const descriptor = fs.openSync(ledgerFile, "a", 0o600);
    try {
      fs.writeFileSync(descriptor, ledgerBytes(records));
      fs.fsyncSync(descriptor);
    } finally {
      fs.closeSync(descriptor);
    }
    return verifyBenchmarkLedger(ledgerFile);
  });
}

export function getBenchmarkLedgerAppendContext(ledgerFile) {
  const parsed = parseLedger(ledgerFile);
  return {
    session: structuredClone(parsed.session),
    nextSequence: parsed.attempts.length + 1,
    attemptIds: parsed.attempts.map(record => record.attempt.attemptId),
    attempts: structuredClone(parsed.attempts),
  };
}

export function verifyBenchmarkLedger(ledgerFile) {
  const parsed = parseLedger(ledgerFile);
  return {
    schemaVersion: "runtime-efficiency-benchmark-ledger-verification@1",
    sessionId: parsed.session.sessionId,
    recordCount: parsed.records.length,
    attemptCount: parsed.attempts.length,
    headRecordHash: parsed.headRecordHash,
    ledgerSha256: sha256(parsed.bytes),
    status: "passed",
  };
}

export function assembleBenchmarkCohort(ledgerFile) {
  const parsed = parseLedger(ledgerFile);
  if (!parsed.attempts.length) throw new Error("benchmark ledger has no Attempts");
  const profiles = parsed.session.profiles.map(profile => ({
    ...profile,
    attempts: parsed.attempts
      .filter(record => record.profileId === profile.profileId)
      .map(record => record.attempt),
  }));
  if (profiles.some(profile => !profile.attempts.length)) {
    throw new Error("every frozen Profile must have at least one Attempt before assembly");
  }
  const cohort = {
    schemaVersion: COHORT_SCHEMA,
    calculatorVersion: parsed.session.calculatorVersion,
    source: parsed.session.source,
    bootstrap: parsed.session.bootstrap,
    promptSet: parsed.session.promptSet,
    ledger: {
      schemaVersion: LEDGER_SCHEMA,
      sha256: sha256(parsed.bytes),
      firstSequence: 1,
      lastSequence: parsed.attempts.length,
      recordCount: parsed.attempts.length,
    },
    profiles,
  };
  const evaluation = evaluateRuntimeEfficiencyBenchmark(cohort);
  if (evaluation.result === "invalid") {
    throw new Error(`assembled benchmark cohort is invalid: ${evaluation.errors.join("; ")}`);
  }
  return cohort;
}

export function evaluateBenchmarkLedger(ledgerFile) {
  const verification = verifyBenchmarkLedger(ledgerFile);
  const cohort = assembleBenchmarkCohort(ledgerFile);
  return {
    sourceLedger: verification,
    evaluation: evaluateRuntimeEfficiencyBenchmark(cohort),
  };
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function usage() {
  throw new Error(
    "usage: runtime-efficiency-benchmark-ledger.mjs init <ledger> <session.json> | append <ledger> <attempt.json> | verify <ledger> | assemble <ledger> <cohort.json> | evaluate <ledger>",
  );
}

async function main() {
  const [command, ledgerFile, inputOrOutput] = process.argv.slice(2);
  if (!command || !ledgerFile) usage();
  let result;
  if (command === "init" && inputOrOutput) {
    result = initializeBenchmarkLedger(ledgerFile, readJson(inputOrOutput));
  } else if (command === "append" && inputOrOutput) {
    result = appendBenchmarkAttempt(ledgerFile, readJson(inputOrOutput));
  } else if (command === "verify" && !inputOrOutput) {
    result = verifyBenchmarkLedger(ledgerFile);
  } else if (command === "assemble" && inputOrOutput) {
    result = assembleBenchmarkCohort(ledgerFile);
    fs.writeFileSync(inputOrOutput, `${JSON.stringify(result, null, 2)}\n`, { flag: "wx", mode: 0o600 });
  } else if (command === "evaluate" && !inputOrOutput) {
    result = evaluateBenchmarkLedger(ledgerFile);
    if (result.evaluation.result !== "pass") process.exitCode = 1;
  } else {
    usage();
  }
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
