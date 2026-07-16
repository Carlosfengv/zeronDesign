#!/usr/bin/env node
import { readFile, writeFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function assertNoCredentialLikeContent(raw) {
  if (/\b(?:sk|api)[-_][A-Za-z0-9_-]{16,}\b/i.test(raw)
    || /\bBearer\s+[A-Za-z0-9._~+\/-]+=*\b/i.test(raw)
    || /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/.test(raw)
    || /"(?:apiKey|authorization|token|secret|password)"\s*:/i.test(raw)) {
    throw new Error("canary operational export contains credential-like content");
  }
}

function markdownCell(value) {
  return String(value ?? "").replaceAll("|", "\\|").replaceAll("\n", " ");
}

function percent(value) {
  return `${(Number(value) * 100).toFixed(2)}%`;
}

export function renderDesignContextCanaryReport(document) {
  const cohort = document?.cohort;
  const window = document?.window;
  const publish = document?.publish;
  const metrics = document?.metrics;
  if (document?.schemaVersion !== "design-context-canary-operational-export@1"
    || document?.source?.kind !== "runtime-durable-store"
    || !hasText(document?.generatedAt) || !hasText(cohort?.projectId)
    || !hasText(cohort?.designProfileId) || !hasText(window?.observationStartedAt)
    || !hasText(window?.observationEndedAt) || !Array.isArray(document?.alerts)
    || typeof document?.alertsTriggered !== "boolean") {
    throw new Error("invalid design-context canary operational export");
  }
  const observationMinutes = (Date.parse(window.observationEndedAt) - Date.parse(window.observationStartedAt)) / 60_000;
  if (!Number.isFinite(observationMinutes) || observationMinutes < 0) {
    throw new Error("invalid canary observation window");
  }
  const minimumWindowReached = observationMinutes >= 7 * 24 * 60;
  const minimumSamplesReached = Number(publish?.enforcedPublishCount) >= 30;
  const decision = document.alertsTriggered
    ? "STOP / ROLLBACK REQUIRED"
    : minimumWindowReached && minimumSamplesReached
      ? "READY FOR REQUIRED ROLLBACK DRILL"
      : "OBSERVATION IN PROGRESS";
  const alerts = document.alerts.map(alert => `| ${markdownCell(alert.code)} | ${alert.triggered ? "TRIGGERED" : "OK"} | ${markdownCell(alert.actual)} | ${markdownCell(alert.threshold)} | ${markdownCell(alert.action)} |`).join("\n");
  return `# Website DCP Canary Operational Report

> Decision: **${decision}**
> This operational report never grants \`canary-verified\` by itself. Final status also requires the immutable 7-day ledger, at least 30 enforced publishes, a successful alert-destination probe plus delivery coverage for every metrics snapshot, the exact-policy rollback drill, compatibility evidence, and final validator pass.

## Cohort and provenance

| Field | Value |
|---|---|
| Project | ${markdownCell(cohort.projectId)} |
| Design Profile | ${markdownCell(cohort.designProfileId)} v${markdownCell(cohort.designProfileVersion)} |
| Observe policy revision | ${markdownCell(cohort.observePolicyRevision)} |
| Enforced policy revision | ${markdownCell(cohort.policyRevision)} |
| Source | ${markdownCell(document.source.kind)} |
| Generated at | ${markdownCell(document.generatedAt)} |
| Recorded by | ${markdownCell(document.conclusionRecordedBy)} |

## Observation window and publish samples

| Field | Value | Gate |
|---|---:|---|
| Baseline window | ${markdownCell(window.baselineStartedAt)} → ${markdownCell(window.baselineEndedAt)} | immutable |
| Observation window | ${markdownCell(window.observationStartedAt)} → ${markdownCell(window.observationEndedAt)} | ${minimumWindowReached ? "PASS" : "IN PROGRESS"} (${observationMinutes.toFixed(0)} / 10080 min) |
| Baseline publishes | ${markdownCell(publish.baselinePublishCount)} | > 0 |
| Enforced publishes | ${markdownCell(publish.enforcedPublishCount)} | ${minimumSamplesReached ? "PASS" : "IN PROGRESS"} (minimum 30) |
| Baseline failure rate | ${percent(publish.baselineFailureRate)} | reference |
| Enforced failure rate | ${percent(publish.enforcedFailureRate)} | reference |
| Failure-rate delta | ${Number(publish.publishFailureRateDeltaPp).toFixed(2)} pp | ≤ 2 pp |

## Safety metrics

| Metric | Actual | Required |
|---|---:|---:|
| Verifier unavailable | ${markdownCell(metrics.verifierUnavailableCount)} | 0 |
| Verifier runtime lost | ${markdownCell(metrics.verifierRuntimeLostCount)} | 0 |
| Unexpected read-gate blocks | ${markdownCell(metrics.unexpectedReadGateBlockCount)} | 0 |
| Profile Sync recovery over 24h | ${markdownCell(metrics.recoveryRequiredOver24hCount)} | 0 |
| Required finding repair rate | ${percent(metrics.requiredFindingRepairRate)} | 100% |

## Alert decisions

| Alert | Status | Actual | Threshold | Required action |
|---|---|---:|---|---|
${alerts || "| none | OK | 0 | n/a | continue observation |"}

## Remaining release evidence

- Append this export's derived \`metrics.snapshot\` to the immutable hash-chain ledger.
- Dispatch this export's alert decision and append its matching \`alert.delivery\` record.
- At final collection, append the Runtime-derived \`publish.samples\` fragment once.
- Execute and verify the exact \`enabled=false\` rollback after the observation window.
- Append compatibility and rollback records, then run ledger \`finalize\` and the final evidence validator.
`;
}

export async function renderReportFile({ inputPath, outputPath }) {
  if (!hasText(inputPath) || !hasText(outputPath)) throw new Error("input and output paths are required");
  const raw = await readFile(inputPath, "utf8");
  assertNoCredentialLikeContent(raw);
  const report = renderDesignContextCanaryReport(JSON.parse(raw));
  await writeFile(outputPath, report, { flag: "wx", mode: 0o600 });
  return report;
}

async function main() {
  const [inputPath, outputPath] = process.argv.slice(2);
  if (!inputPath || !outputPath) {
    throw new Error("usage: render-design-context-canary-report.mjs <operational-export.json> <report.md>");
  }
  await renderReportFile({ inputPath, outputPath });
  process.stdout.write(`Design-context canary report rendered: ${outputPath}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
