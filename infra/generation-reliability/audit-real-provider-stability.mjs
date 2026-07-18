#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

const options = parseOptions(process.argv.slice(2));
const evidenceRoot = path.resolve(options.evidenceRoot);
const outFile = path.resolve(
  options.out || path.join(evidenceRoot, "real-provider-stability-audit.json"),
);
const requiredConsecutive = Number.parseInt(options.requiredConsecutive || "3", 10);
if (!Number.isSafeInteger(requiredConsecutive) || requiredConsecutive < 1) {
  throw new Error("--required-consecutive must be a positive integer");
}
if (!fs.existsSync(evidenceRoot)) {
  throw new Error(`real-provider evidence root does not exist: ${evidenceRoot}`);
}

const malformedSummaries = [];
const suites = [];
for (const entry of fs.readdirSync(evidenceRoot, { withFileTypes: true })) {
  if (!entry.isDirectory() || !entry.name.startsWith("suite-")) continue;
  const summaryFile = path.join(
    evidenceRoot,
    entry.name,
    "real-provider-examples-summary.json",
  );
  if (!fs.existsSync(summaryFile)) continue;
  try {
    const summary = JSON.parse(fs.readFileSync(summaryFile, "utf8"));
    suites.push(classifySuite(summary, path.relative(evidenceRoot, summaryFile)));
  } catch (error) {
    malformedSummaries.push({
      path: path.relative(evidenceRoot, summaryFile),
      error: String(error?.message || error),
    });
  }
}

suites.sort((left, right) => {
  const byTime = Date.parse(left.finishedAt) - Date.parse(right.finishedAt);
  return Number.isFinite(byTime) && byTime !== 0
    ? byTime
    : left.suiteId.localeCompare(right.suiteId);
});

const fullBatches = suites.filter((suite) => suite.fullBatch);
let currentConsecutivePasses = 0;
const currentPassingSuiteIds = [];
for (let index = fullBatches.length - 1; index >= 0; index -= 1) {
  const suite = fullBatches[index];
  if (!suite.qualified) break;
  currentConsecutivePasses += 1;
  currentPassingSuiteIds.unshift(suite.suiteId);
}

const falseSuccesses = suites.filter((suite) => suite.falseSuccess);
const status =
  malformedSummaries.length > 0 || falseSuccesses.length > 0
    ? "failed_evidence"
    : currentConsecutivePasses >= requiredConsecutive
      ? "passed"
      : "incomplete";
const audit = {
  schemaVersion: "generation-real-provider-stability-audit@1",
  recordedAt: new Date().toISOString(),
  status,
  requiredConsecutiveFullPasses: requiredConsecutive,
  currentConsecutiveFullPasses: currentConsecutivePasses,
  currentPassingSuiteIds,
  counts: {
    summaries: suites.length,
    fullBatches: fullBatches.length,
    qualifiedFullBatches: fullBatches.filter((suite) => suite.qualified).length,
    partialSuites: suites.filter((suite) => !suite.fullBatch).length,
    falseSuccesses: falseSuccesses.length,
    malformedSummaries: malformedSummaries.length,
  },
  latestFullBatch: fullBatches.at(-1) || null,
  falseSuccesses,
  malformedSummaries,
  suites,
};

fs.mkdirSync(path.dirname(outFile), { recursive: true });
fs.writeFileSync(outFile, `${JSON.stringify(audit, null, 2)}\n`);
process.stdout.write(
  `Real-provider stability audit: status=${status} consecutive=${currentConsecutivePasses}/${requiredConsecutive} evidence=${outFile}\n`,
);
if (status === "failed_evidence" || (status === "incomplete" && !options.allowIncomplete)) {
  process.exitCode = 1;
}

function classifySuite(summary, summaryPath) {
  if (summary.schemaVersion !== "generation-real-provider-suite-evidence@2") {
    throw new Error(`unsupported summary schema in ${summaryPath}`);
  }
  const execution = summary.execution || {};
  const cases = Array.isArray(summary.cases) ? summary.cases : [];
  const fullBatch =
    execution.generatedCaseCount === 5 &&
    execution.executedCaseCount === 5 &&
    execution.partial === false &&
    cases.length === 5;
  const providerVerified = summary.provider?.realProviderVerified === true;
  const budgetBounded = summary.budget?.exceeded === false;
  const acceptedCases = cases.filter((item) => item.status === "accepted").length;
  const artifactsVerified =
    cases.length > 0 &&
    cases.every(
      (item) =>
        item.status === "accepted" &&
        item.artifact?.httpStatus === 200 &&
        item.artifact?.expectedTextFound === true,
    );
  const reasons = [];
  if (!fullBatch) reasons.push("not_a_complete_five_case_batch");
  if (!providerVerified) reasons.push("real_provider_not_verified");
  if (summary.status !== "accepted") reasons.push(`suite_status_${summary.status || "missing"}`);
  if (!budgetBounded) reasons.push("budget_exceeded_or_missing");
  if (fullBatch && !artifactsVerified) reasons.push("accepted_artifact_probe_incomplete");
  const qualified =
    fullBatch &&
    providerVerified &&
    summary.status === "accepted" &&
    budgetBounded &&
    artifactsVerified;
  const falseSuccess =
    summary.status === "accepted" &&
    (!providerVerified || (fullBatch && (!budgetBounded || !artifactsVerified)));
  return {
    suiteId: String(summary.suiteId || ""),
    finishedAt: String(summary.finishedAt || ""),
    summaryPath,
    status: summary.status || null,
    fullBatch,
    providerVerified,
    budgetBounded,
    acceptedCases,
    artifactsVerified,
    qualified,
    falseSuccess,
    reasons,
  };
}

function parseOptions(args) {
  const result = { allowIncomplete: false };
  for (let index = 0; index < args.length; index += 1) {
    const value = args[index];
    if (value === "--allow-incomplete") {
      result.allowIncomplete = true;
      continue;
    }
    const key = {
      "--evidence-root": "evidenceRoot",
      "--out": "out",
      "--required-consecutive": "requiredConsecutive",
    }[value];
    if (!key || !args[index + 1]) throw new Error(`unknown or incomplete option: ${value}`);
    result[key] = args[index + 1];
    index += 1;
  }
  if (!result.evidenceRoot) throw new Error("--evidence-root is required");
  return result;
}
