#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const args = parseArgs(process.argv.slice(2));
const required = [
  "authority",
  "source-file",
  "source-revision",
  "source-updated-at",
  "recorded-at",
  "source-sha256",
];
for (const name of required) {
  if (!args[name]) fail(`missing --${name}`);
}
if (args["source-complete"] !== "true") {
  fail("--source-complete=true is required; a partial source cannot close migration readiness");
}
if (!/^\d+$/.test(args["source-revision"])) fail("--source-revision must be a non-negative integer");
if (!isIsoTimestamp(args["source-updated-at"]) || !isIsoTimestamp(args["recorded-at"])) {
  fail("source and record timestamps must be ISO-8601 values");
}
if (!isSha256(args["source-sha256"])) fail("--source-sha256 must be lowercase SHA-256");

const source = fs.readFileSync(0);
const actualSourceSha256 = sha256(source);
if (actualSourceSha256 !== args["source-sha256"]) {
  fail(`source hash mismatch: expected ${args["source-sha256"]}, got ${actualSourceSha256}`);
}

const rawRecords = parseJsonLines(source.toString("utf8"));
const latestByBrief = new Map();
for (const record of rawRecords) {
  validateLegacyBriefRecord(record);
  const previous = latestByBrief.get(record.briefId);
  if (previous && (previous.projectId !== record.projectId || previous.runId !== record.runId)) {
    fail(`brief ${record.briefId} changed project or run identity`);
  }
  latestByBrief.set(record.briefId, record);
}

const latestRecords = [...latestByBrief.values()];
const confirmed = latestRecords.filter(record => record.status === "confirmed");
const candidates = confirmed
  .map(classifyCandidate)
  .sort((left, right) => left.legacyIdentityHash.localeCompare(right.legacyIdentityHash));
const mappableCount = candidates.filter(candidate => candidate.classification === "mappable").length;
const unmappableCount = candidates.length - mappableCount;
const statusCounts = Object.fromEntries(
  [...new Set(latestRecords.map(record => record.status))]
    .sort()
    .map(status => [status, latestRecords.filter(record => record.status === status).length]),
);

const evidence = {
  schemaVersion: "content-plan-approval-migration-evidence@1",
  recordedAt: args["recorded-at"],
  source: {
    authority: args.authority,
    file: args["source-file"],
    revision: Number(args["source-revision"]),
    updatedAt: args["source-updated-at"],
    contentSha256: actualSourceSha256,
    complete: true,
  },
  mappingRule: {
    version: "content-plan-approval-legacy-mapping@1",
    requiredFields: ["planId", "revision", "contentHash", "confirmationEventId"],
    briefConfirmationAloneIsApproval: false,
    acceptanceOrBuildEvidenceAloneIsApproval: false,
    unmappableRecordsBecomeVerified: false,
  },
  inventory: {
    rawRecordCount: rawRecords.length,
    uniqueBriefCount: latestRecords.length,
    latestStatusCounts: statusCounts,
    confirmedCandidateCount: confirmed.length,
    mappableCount,
    unmappableCount,
  },
  candidates,
  migration: {
    verifiedApprovalsCreated: 0,
    requiresProducerMigration: mappableCount > 0,
    allUnmappableRecordsKeptUnverified: unmappableCount === candidates.length,
  },
  gate: {
    status: mappableCount > 0 ? "requires_producer_migration" : "complete",
    enabledOrEnforceAllowed: mappableCount === 0,
  },
};
evidence.evidenceSha256 = sha256(Buffer.from(canonicalJson(evidence)));
evidence.passed = mappableCount === 0 && evidence.migration.verifiedApprovalsCreated === 0;

const serialized = `${JSON.stringify(evidence, null, 2)}\n`;
if (args.output) {
  fs.mkdirSync(path.dirname(path.resolve(args.output)), { recursive: true });
  fs.writeFileSync(args.output, serialized, { mode: 0o600 });
} else {
  process.stdout.write(serialized);
}
if (!evidence.passed) process.exitCode = 4;

function classifyCandidate(record) {
  const contentPlan = record.contentPlan ?? record.contentPlanIdentity ?? {};
  const reasons = [];
  if (typeof contentPlan.planId !== "string" || contentPlan.planId.trim() === "") {
    reasons.push("missing_plan_id");
  }
  if (!Number.isInteger(contentPlan.revision) || contentPlan.revision <= 0) {
    reasons.push("missing_revision");
  }
  if (!isSha256(contentPlan.contentHash)) reasons.push("missing_content_hash");
  const confirmationEventId = record.confirmationEventId ?? contentPlan.confirmationEventId;
  if (typeof confirmationEventId !== "string" || confirmationEventId.trim() === "") {
    reasons.push("missing_confirmation_event_id");
  }
  return {
    legacyIdentityHash: sha256(Buffer.from(canonicalJson({
      briefId: record.briefId,
      projectId: record.projectId,
      runId: record.runId,
    }))),
    classification: reasons.length === 0 ? "mappable" : "unmappable",
    reasons,
    wouldCreateVerifiedApproval: false,
  };
}

function validateLegacyBriefRecord(record) {
  if (!record || typeof record !== "object" || Array.isArray(record)) fail("legacy record must be an object");
  for (const name of ["briefId", "projectId", "runId", "status"]) {
    if (typeof record[name] !== "string" || record[name].trim() === "") {
      fail(`legacy record ${name} must be a non-empty string`);
    }
  }
  if (!["draft", "confirmed", "superseded"].includes(record.status)) {
    fail(`unsupported legacy Brief status: ${record.status}`);
  }
}

function parseJsonLines(text) {
  return text.split("\n").flatMap((line, index) => {
    if (line.trim() === "") return [];
    try {
      return [JSON.parse(line)];
    } catch (error) {
      fail(`invalid JSON on line ${index + 1}: ${error.message}`);
    }
  });
}

function parseArgs(values) {
  const parsed = {};
  for (const value of values) {
    const match = /^--([^=]+)=(.*)$/s.exec(value);
    if (!match) fail(`arguments must use --name=value: ${value}`);
    parsed[match[1]] = match[2];
  }
  return parsed;
}

function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function isSha256(value) {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

function isIsoTimestamp(value) {
  return typeof value === "string" && !Number.isNaN(Date.parse(value));
}

function fail(message) {
  console.error(`content_plan_approval_migration.invalid: ${message}`);
  process.exit(2);
}
