#!/usr/bin/env node

import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const SHA256 = /^[0-9a-f]{64}$/;

function firstToolAfterProgressMetrics(events) {
  let expectedTool = null;
  let awaitingFirstTool = false;
  let matched = 0;
  let violated = 0;
  for (const event of events) {
    if (event.type === "run.workflow_progress") {
      expectedTool = event.nextAction?.tool ?? null;
      awaitingFirstTool = Boolean(expectedTool);
      continue;
    }
    if (event.type !== "tool.started" || !awaitingFirstTool) continue;
    if (event.tool === expectedTool) matched += 1;
    else violated += 1;
    awaitingFirstTool = false;
  }
  return { matched, violated };
}

function numberOrNull(value) {
  return Number.isFinite(value) ? value : null;
}

export async function createBaselineManifest(suiteDirectory) {
  const summaryPath = path.join(suiteDirectory, "real-provider-examples-summary.json");
  const summary = JSON.parse(await readFile(summaryPath, "utf8"));
  if (summary.schemaVersion !== "generation-real-provider-suite-evidence@2") {
    throw new Error("unsupported real Provider suite schema");
  }
  const caseFiles = (await readdir(suiteDirectory))
    .filter(name => name.startsWith("real-provider-case-") && name.endsWith(".json"))
    .sort();
  if (caseFiles.length === 0) throw new Error("suite contains no case evidence");

  const samples = [];
  for (const caseFile of caseFiles) {
    const caseEvidence = JSON.parse(await readFile(path.join(suiteDirectory, caseFile), "utf8"));
    const run = caseEvidence.runs?.find(candidate => candidate.phase === "build");
    if (!run?.efficiency || !run?.eventStream?.path || !SHA256.test(run.eventStream.sha256 ?? "")) {
      throw new Error(`case ${caseEvidence.id ?? caseFile} has no complete Build evidence`);
    }
    const events = (await readFile(path.join(suiteDirectory, run.eventStream.path), "utf8"))
      .split("\n")
      .filter(Boolean)
      .map(line => JSON.parse(line));
    const nextAction = firstToolAfterProgressMetrics(events);
    const modelExecution = run.modelExecutions?.[0];
    if (!modelExecution?.modelResourceId || !Number.isSafeInteger(modelExecution.modelResourceRevision)) {
      throw new Error(`case ${caseEvidence.id} has no governed Model Resource identity`);
    }
    const efficiency = run.efficiency;
    samples.push({
      id: `${summary.suiteId}-${caseEvidence.id}-build`,
      source: {
        storageRef: `suite-evidence://${summary.suiteId}/${run.eventStream.path}`,
        contentSha256: run.eventStream.sha256,
        availability: "available",
      },
      identity: {
        runId: run.runId,
        modelResource: `${modelExecution.modelResourceId}@${modelExecution.modelResourceRevision}`,
        template: efficiency.template,
        phase: "build",
        fixtureIdentity: caseEvidence.id,
      },
      outcome: {
        status: run.status,
        accepted: caseEvidence.status === "accepted",
        failureClassification: caseEvidence.error?.classification ?? null,
      },
      metrics: {
        modelTurns: run.turns,
        toolCalls: run.toolCalls,
        totalDurationMs: numberOrNull(efficiency.totalDurationMs),
        timeToFirstModelTurnMs: numberOrNull(efficiency.timeToFirstModelTurnMs),
        timeToFirstSourceMutationMs: numberOrNull(efficiency.timeToFirstSourceMutationMs),
        modelTurnAtFirstSourceMutation: numberOrNull(efficiency.modelTurnAtFirstSourceMutation),
        timeToFirstBuildMs: numberOrNull(efficiency.timeToFirstGreenfieldStaticBuildMs),
        prebuildFsReadCount: efficiency.prebuildFsReadCount ?? 0,
        prebuildFsListCount: efficiency.prebuildFsListCount ?? 0,
        prebuildFsSearchCount: efficiency.prebuildFsSearchCount ?? 0,
        prebuildObservationCallCount:
          (efficiency.prebuildFsReadCount ?? 0)
          + (efficiency.prebuildFsListCount ?? 0)
          + (efficiency.prebuildFsSearchCount ?? 0),
        inputTokens: numberOrNull(efficiency.inputTokens),
        outputTokens: numberOrNull(efficiency.outputTokens),
        cachedInputTokens: numberOrNull(efficiency.cachedInputTokens),
        duplicateReadTokens: numberOrNull(efficiency.duplicateReadEstimatedTokens),
        duplicateFullReadRateBasisPoints: numberOrNull(efficiency.duplicateFullReadRateBasisPoints),
        nextActionMatchCount: nextAction.matched,
        nextActionViolationCount: nextAction.violated,
        firstBuildSucceeded: efficiency.firstBuildSucceeded ? 1 : 0,
        completionMissingDurableSnapshot:
          run.status === "completed" && efficiency.timeToDurableSnapshotMs == null ? 1 : 0,
        noProgressFailure: caseEvidence.error?.classification === "no_progress" ? 1 : 0,
        artifactAccepted: caseEvidence.status === "accepted" ? 1 : 0,
      },
      inclusion: {
        includedInImprovementDenominator: true,
        exclusionReasons: [],
      },
    });
  }

  return {
    schemaVersion: "baseline-evidence@1",
    calculatorVersion: "generation-context-baseline-calculator@1",
    recordedAt: summary.finishedAt,
    sourceWindow: {
      startedAt: summary.startedAt,
      endedAt: summary.finishedAt,
    },
    cohort: {
      suiteId: summary.suiteId,
      status: summary.status,
      gatewayMode: summary.provider?.gatewayMode ?? null,
      modelResourceId: summary.provider?.modelResourceId ?? null,
      realProviderVerified: summary.provider?.realProviderVerified === true,
      budgetExceeded: summary.budget?.exceeded === true,
    },
    samples,
  };
}

async function main() {
  const suiteDirectory = process.argv[2];
  if (!suiteDirectory) {
    throw new Error("usage: extract-generation-context-baseline.mjs <suite-evidence-directory>");
  }
  console.log(JSON.stringify(await createBaselineManifest(suiteDirectory), null, 2));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
