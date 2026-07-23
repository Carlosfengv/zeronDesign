#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import {
  extractBuildEvidence,
  redactEvidenceObject,
  sanitizeModelExecutionEvent,
  sanitizePersistedRuntimeEvent,
} from "./runtime-evidence-redaction.mjs";
import { confirmSandboxRelease } from "./sandbox-release-confirmation.mjs";

const [
  casesFile,
  baseUrl,
  principalPrivateKeyFile,
  adminTokenFile,
  evidenceDirectory,
] = process.argv.slice(2);

if (
  !casesFile ||
  !baseUrl ||
  !principalPrivateKeyFile ||
  !adminTokenFile ||
  !evidenceDirectory
) {
  throw new Error(
    "usage: run-real-provider-examples.mjs <cases.json> <base-url> <principal-private-key> <admin-token-file> <evidence-directory>",
  );
}

const manifest = JSON.parse(fs.readFileSync(casesFile, "utf8"));
const privateKey = crypto.createPrivateKey(
  fs.readFileSync(principalPrivateKeyFile),
);
const adminToken = fs.readFileSync(adminTokenFile, "utf8").trim();
const principalId = "generation-real-provider-suite";
const workspaceNamespace = (
  process.env.GENERATION_REAL_WORKSPACE_NAMESPACE || ""
).trim();
if (!/^ws-[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(workspaceNamespace)) {
  throw new Error(
    "GENERATION_REAL_WORKSPACE_NAMESPACE must be a valid ws-* Kubernetes namespace",
  );
}
const suiteId = new Date().toISOString().replace(/[-:.TZ]/g, "");
const suiteStartedAt = new Date().toISOString();
const suiteRoot = path.resolve(evidenceDirectory);
let suiteDirectory = path.join(suiteRoot, `suite-${suiteId}-running`);
const requestedCaseLimit = Number.parseInt(
  process.env.GENERATION_REAL_CASE_LIMIT || String(manifest.cases.length),
  10,
);
const requestedCaseIds = (process.env.GENERATION_REAL_CASE_IDS || "")
  .split(",")
  .map((value) => value.trim())
  .filter(Boolean);
const runTimeoutMs = Number.parseInt(
  process.env.GENERATION_REAL_RUN_TIMEOUT_MS || "900000",
  10,
);
const runIdleTimeoutMs = Number.parseInt(
  process.env.GENERATION_REAL_RUN_IDLE_TIMEOUT_MS || "480000",
  10,
);
const maxCaseAttempts = Number.parseInt(
  process.env.GENERATION_REAL_MAX_CASE_ATTEMPTS || "3",
  10,
);
const caseRetryCooldownMs = Number.parseInt(
  process.env.GENERATION_REAL_CASE_RETRY_COOLDOWN_MS || "45000",
  10,
);
const sandboxReleaseMaxAttempts = Number.parseInt(
  process.env.GENERATION_REAL_SANDBOX_RELEASE_MAX_ATTEMPTS || "3",
  10,
);
const sandboxReleaseRetryCooldownMs = Number.parseInt(
  process.env.GENERATION_REAL_SANDBOX_RELEASE_RETRY_COOLDOWN_MS || "1000",
  10,
);
const designProfileFixture = (
  process.env.GENERATION_REAL_DESIGN_PROFILE_FIXTURE || ""
).trim();
const draftPreviewAcceptance =
  process.env.GENERATION_REAL_DRAFT_PREVIEW_ACCEPTANCE === "1";
const draftWarmEditCanary =
  process.env.GENERATION_REAL_DRAFT_WARM_EDIT_CANARY === "1";
const draftColdDevEditCanary =
  process.env.GENERATION_REAL_DRAFT_COLD_DEV_EDIT_CANARY === "1";
const repairCanary = process.env.GENERATION_REAL_REPAIR_CANARY === "1";
const keepSandbox = process.env.GENERATION_REAL_KEEP_SANDBOX === "true";
if ([draftWarmEditCanary, draftColdDevEditCanary, repairCanary].filter(Boolean).length > 1) {
  throw new Error("Warm Edit, Cold Dev Edit, and Repair canaries are mutually exclusive");
}
const draftLifecycleEditCanary =
  draftWarmEditCanary || draftColdDevEditCanary;

fs.mkdirSync(suiteDirectory, { recursive: true });

validateManifest(manifest);
const perRunSafetyCeiling =
  manifest.budget.perRun.maxInputTokens +
  manifest.budget.perRun.maxOutputTokens;
const suiteTokenCeiling = Number.parseInt(
  process.env.GENERATION_REAL_TOTAL_TOKEN_CEILING ||
    String(manifest.budget.totalTokens),
  10,
);
if (
  !Number.isSafeInteger(suiteTokenCeiling) ||
  suiteTokenCeiling <= 0 ||
  suiteTokenCeiling > manifest.budget.totalTokens ||
  suiteTokenCeiling < perRunSafetyCeiling
) {
  throw new Error(
    `GENERATION_REAL_TOTAL_TOKEN_CEILING must be between the per-run safety ceiling ${perRunSafetyCeiling} and manifest ceiling ${manifest.budget.totalTokens}`,
  );
}
if (
  !Number.isSafeInteger(requestedCaseLimit) ||
  requestedCaseLimit < 1 ||
  requestedCaseLimit > manifest.cases.length
) {
  throw new Error(
    `GENERATION_REAL_CASE_LIMIT must be between 1 and ${manifest.cases.length}`,
  );
}
if (requestedCaseIds.length > 0 && process.env.GENERATION_REAL_CASE_LIMIT) {
  throw new Error(
    "GENERATION_REAL_CASE_IDS and GENERATION_REAL_CASE_LIMIT are mutually exclusive",
  );
}
if (!Number.isSafeInteger(runTimeoutMs) || runTimeoutMs < 60_000) {
  throw new Error("GENERATION_REAL_RUN_TIMEOUT_MS must be at least 60000");
}
if (
  !Number.isSafeInteger(runIdleTimeoutMs) ||
  runIdleTimeoutMs < 30_000 ||
  runIdleTimeoutMs >= runTimeoutMs
) {
  throw new Error(
    "GENERATION_REAL_RUN_IDLE_TIMEOUT_MS must be at least 30000 and below the total Run timeout",
  );
}
if (!Number.isSafeInteger(maxCaseAttempts) || maxCaseAttempts < 1 || maxCaseAttempts > 3) {
  throw new Error("GENERATION_REAL_MAX_CASE_ATTEMPTS must be between 1 and 3");
}
if (
  !Number.isSafeInteger(caseRetryCooldownMs) ||
  caseRetryCooldownMs < 0 ||
  caseRetryCooldownMs > 300_000
) {
  throw new Error(
    "GENERATION_REAL_CASE_RETRY_COOLDOWN_MS must be between 0 and 300000",
  );
}
if (
  !Number.isSafeInteger(sandboxReleaseMaxAttempts) ||
  sandboxReleaseMaxAttempts < 1 ||
  sandboxReleaseMaxAttempts > 3
) {
  throw new Error(
    "GENERATION_REAL_SANDBOX_RELEASE_MAX_ATTEMPTS must be between 1 and 3",
  );
}
if (
  !Number.isSafeInteger(sandboxReleaseRetryCooldownMs) ||
  sandboxReleaseRetryCooldownMs < 0 ||
  sandboxReleaseRetryCooldownMs > 30_000
) {
  throw new Error(
    "GENERATION_REAL_SANDBOX_RELEASE_RETRY_COOLDOWN_MS must be between 0 and 30000",
  );
}
const selectedCases =
  requestedCaseIds.length > 0
    ? requestedCaseIds.map((id) => {
        const testCase = manifest.cases.find((candidate) => candidate.id === id);
        if (!testCase) throw new Error(`unknown real-provider case id: ${id}`);
        return testCase;
      })
    : manifest.cases.slice(0, requestedCaseLimit);
if (repairCanary && selectedCases.some((testCase) => testCase.kind !== "docs")) {
  throw new Error("Repair canary currently requires docs fixtures with a Version lifecycle");
}
if (new Set(selectedCases.map((testCase) => testCase.id)).size !== selectedCases.length) {
  throw new Error("GENERATION_REAL_CASE_IDS must not contain duplicates");
}

let consumedTokens = 0;
let consumedInputTokens = 0;
let consumedOutputTokens = 0;
let consumedCachedInputTokens = 0;
const caseResults = [];

for (const [index, testCase] of selectedCases.entries()) {
  const caseNumber = index + 1;
  const baseProjectId =
    `real-${suiteId.slice(0, 14)}-${testCase.id}`.toLowerCase();
  const caseStartedAt = new Date().toISOString();
  process.stdout.write(
    `[${caseNumber}/${selectedCases.length}] ${testCase.id}: starting\n`,
  );

  const result = {
    schemaVersion: "generation-real-provider-case-evidence@2",
    id: testCase.id,
    title: testCase.title,
    kind: testCase.kind,
    locale: testCase.locale,
    projectId: baseProjectId,
    workspaceNamespace,
    expectedRoute: testCase.expectedRoute,
    expectedText: testCase.expectedText,
    promptSha256: sha256(testCase.prompt),
    promptBytes: Buffer.byteLength(testCase.prompt),
    startedAt: caseStartedAt,
    finishedAt: null,
    status: "failed",
    acceptance: acceptanceProbe(testCase),
    runs: [],
    attempts: [],
    artifact: null,
    contentPlan: null,
    designProfile: null,
    draftPreview: null,
    warmEdit: null,
    coldDevEdit: null,
    repair: null,
    releaseEvidence: null,
    sandboxRelease: null,
    cleanupError: null,
    error: null,
  };

  for (let attempt = 1; attempt <= maxCaseAttempts; attempt += 1) {
    const projectId =
      attempt === 1 ? baseProjectId : `${baseProjectId}-retry-${attempt}`;
    const attemptStartedAt = new Date().toISOString();
    const attemptRunStart = result.runs.length;
    result.projectId = projectId;
    result.status = "failed";
    result.artifact = null;
    result.warmEdit = null;
    result.coldDevEdit = null;
    result.repair = null;
    result.releaseEvidence = null;
    result.sandboxRelease = null;
    result.cleanupError = null;
    result.error = null;

    try {
      await grantProjectAccess(projectId);
      const executionPrompt =
        draftLifecycleEditCanary && caseNumber === 1
          ? [
              testCase.prompt,
              "完成 Build 时必须使用 preview.dev_start 和 preview.dev_status 建立 ready 且 durable 的 DraftPreviewSession；不要使用 preview.start 静态候选预览。",
            ].join(" ")
          : testCase.prompt;
      const contentPlan = await prepareApprovedContentPlan(
        projectId,
        testCase,
        executionPrompt,
      );
      result.contentPlan = contentPlan.evidence;
      if (designProfileFixture) {
        result.designProfile = configureAndBindDesignProfile(projectId);
      }

      let brief;
      try {
        assertRunReservation("brief", testCase.id);
        brief = await runBrief(projectId, executionPrompt);
        result.runs.push(brief.evidence);
        addUsage(brief.evidence.usage);
        assertActualBudget();
      } catch (error) {
        for (const run of error.runEvidence || []) {
          result.runs.push(run);
          addUsage(run.usage);
        }
        assertActualBudget();
        throw error;
      }

      let buildResult;
      try {
        assertRunReservation("build", testCase.id);
        buildResult = await runBuild(projectId, brief.briefId, contentPlan.identity);
        result.runs.push(...buildResult.evidence);
        for (const run of buildResult.evidence) addUsage(run.usage);
        assertActualBudget();
      } catch (error) {
        for (const run of error.runEvidence || []) {
          result.runs.push(run);
          addUsage(run.usage);
        }
        assertActualBudget();
        throw error;
      }

      if (draftLifecycleEditCanary && caseNumber === 1) {
        const lifecycleEdit = runDraftLifecycleEditCanary(
          projectId,
          brief.briefId,
          contentPlan.identity,
        );
        if (draftColdDevEditCanary) {
          result.coldDevEdit = lifecycleEdit;
        } else {
          result.warmEdit = lifecycleEdit;
        }
        if (lifecycleEdit.status !== "accepted") {
          throw classifiedError(
            draftColdDevEditCanary
              ? "cold_dev_edit_canary_failed"
              : "warm_edit_canary_failed",
            `Draft lifecycle Edit canary failed: ${lifecycleEdit.error?.message || lifecycleEdit.status}`,
          );
        }
      }

      if (repairCanary && caseNumber === 1) {
        const buildRunId = buildResult.evidence.at(-1)?.runId;
        if (!buildRunId) throw new Error("Repair canary base Build omitted its run id");
        const repair = await runRepairCanary(
          projectId,
          buildRunId,
          buildResult.candidateVersionId,
          contentPlan.identity,
          testCase,
        );
        result.repair = repair;
        for (const run of [repair.setupEdit?.run, repair.reviewRun, repair.run].filter(Boolean)) {
          result.runs.push(run);
        }
        assertActualBudget();
        if (repair.status !== "accepted") {
          throw classifiedError(
            "repair_canary_failed",
            `Repair canary failed: ${repair.error?.message || repair.status}`,
          );
        }
      }

      if (buildResult.canaryRecoveredFromTerminalFailure) {
        throw classifiedError(
          "warm_edit_canary_base_build_terminal_failure",
          "Warm Edit canary completed from a ready durable Draft, but its base Build retained the original terminal failure",
        );
      }

      if (draftPreviewAcceptance) {
        result.draftPreview = await verifyDraftPreview(
          projectId,
          testCase,
          buildResult.staticPreview,
        );
      } else {
        const principalToken = issuePrincipalToken(projectId);
        const artifactUrl = new URL(
          `/artifacts/${encodeURIComponent(projectId)}/current${testCase.expectedRoute}`,
          baseUrl,
        );
        const artifactResponse = await fetchWithTimeout(
          artifactUrl,
          {
            headers: { authorization: `Bearer ${principalToken}` },
          },
          120_000,
        );
        const artifactBody = await artifactResponse.text();
        if (!artifactResponse.ok) {
          throw new Error(
            `artifact route returned ${artifactResponse.status}: ${artifactBody.slice(0, 500)}`,
          );
        }
        if (!artifactBody.includes(testCase.expectedText)) {
          throw classifiedError(
            "acceptance_rejected",
            `artifact route does not contain expected text: ${testCase.expectedText}`,
            "rejected",
          );
        }
        result.artifact = {
          url: artifactUrl.toString(),
          route: testCase.expectedRoute,
          httpStatus: artifactResponse.status,
          expectedText: testCase.expectedText,
          expectedTextFound: true,
          bodySha256: sha256(artifactBody),
          bodyBytes: Buffer.byteLength(artifactBody),
        };
      }

      const releaseEvidenceResponse = await fetchWithTimeout(
        new URL(
          `/internal/projects/${encodeURIComponent(projectId)}/release-evidence`,
          baseUrl,
        ),
        {
          headers: {
            "x-anydesign-internal": "true",
            "x-runtime-admin-token": adminToken,
          },
        },
        120_000,
      );
      const releaseEvidenceBody = await releaseEvidenceResponse.text();
      if (
        draftPreviewAcceptance &&
        releaseEvidenceResponse.status === 404 &&
        releaseEvidenceBody.includes("current version not found")
      ) {
        result.releaseEvidence = {
          available: false,
          reason: "draft-only-suite-does-not-create-current-version",
        };
      } else if (
        releaseEvidenceResponse.status === 409 &&
        releaseEvidenceBody.includes(
          "release evidence requires an Edit promotion with a base version",
        )
      ) {
        result.releaseEvidence = {
          available: false,
          reason: "build-only-suite-does-not-create-edit-baseline",
        };
      } else if (!releaseEvidenceResponse.ok) {
        throw new Error(
          `release evidence returned ${releaseEvidenceResponse.status}: ${releaseEvidenceBody.slice(0, 500)}`,
        );
      } else {
        const releaseEvidence = JSON.parse(releaseEvidenceBody);
        if (releaseEvidence.terminalToolFailureCount !== 0) {
          throw new Error(
            `release evidence has terminal tool failures: ${releaseEvidence.terminalToolFailureCount}`,
          );
        }
        result.releaseEvidence = sanitizeReleaseEvidence(releaseEvidence);
      }
      result.status = "accepted";
    } catch (error) {
      result.status = error?.resultStatus || "failed";
      result.error = {
        name: error?.name || "Error",
        classification: error?.classification || "unclassified_failure",
        message: String(error?.message || error),
      };
    } finally {
      result.sandboxRelease = keepSandbox
        ? {
            required: false,
            released: false,
            attempts: [],
            maxAttempts: sandboxReleaseMaxAttempts,
            requiredSuccessfulResponses: 2,
          }
        : await releaseSandboxWithRetry(projectId);
      if (!keepSandbox && !result.sandboxRelease.released) {
        const cleanupError = {
          name: "Error",
          classification: "sandbox_release_failed",
          message: `sandbox release failed after ${result.sandboxRelease.attempts.length} attempts`,
        };
        result.cleanupError = cleanupError;
        if (result.status === "accepted") {
          result.status = "failed";
          result.error = cleanupError;
        }
      }
      const attemptRuns = result.runs.slice(attemptRunStart);
      result.attempts.push({
        attempt,
        projectId,
        startedAt: attemptStartedAt,
        finishedAt: new Date().toISOString(),
        status: result.status,
        runIds: attemptRuns.map((run) => run.runId),
        totalTokens: attemptRuns.reduce(
          (total, run) => total + run.usage.totalTokens,
          0,
        ),
        sandboxRelease: result.sandboxRelease,
        cleanupError: result.cleanupError,
        error: result.error,
      });
    }

    if (result.status === "accepted") break;
    if (!isRetryableCaseFailure(result.error) || attempt >= maxCaseAttempts) {
      break;
    }
    process.stdout.write(
      `[${caseNumber}/${selectedCases.length}] ${testCase.id}: retryable case failure (${result.error.classification}); retrying attempt ${attempt + 1}/${maxCaseAttempts} after ${caseRetryCooldownMs}ms\n`,
    );
    await delay(caseRetryCooldownMs);
  }

  result.finishedAt = new Date().toISOString();
  const persistedResult = redactEvidenceObject(result);
  fs.writeFileSync(
    path.join(suiteDirectory, `real-provider-case-${testCase.id}.json`),
    `${JSON.stringify(persistedResult, null, 2)}\n`,
  );
  caseResults.push(persistedResult);
  process.stdout.write(
    `[${caseNumber}/${selectedCases.length}] ${testCase.id}: ${result.status}\n`,
  );
}

const providerVerificationRuns = caseResults.flatMap((result) => {
  const finalAttemptRunIds = new Set(result.attempts.at(-1)?.runIds || []);
  return result.runs.filter((run) => finalAttemptRunIds.has(run.runId));
});
const realProviderVerified =
  providerVerificationRuns.length > 0 &&
  providerVerificationRuns.every((run) =>
    run.modelExecutions.some((snapshot) =>
      JSON.stringify(snapshot).includes(manifest.provider.modelResourceId),
    ),
  );
const suiteStatus = !realProviderVerified
  ? "failed"
  : caseResults.every((result) => result.status === "accepted")
    ? "accepted"
    : caseResults.every((result) => ["accepted", "rejected"].includes(result.status))
      ? "rejected"
      : "failed";
const summary = {
  schemaVersion: "generation-real-provider-suite-evidence@2",
  suiteId,
  startedAt: suiteStartedAt,
  finishedAt: new Date().toISOString(),
  status: suiteStatus,
  provenance: {
    gitCommit: process.env.GENERATION_SOURCE_COMMIT || null,
    gitDirty: process.env.GENERATION_SOURCE_DIRTY === "true",
    providerConfigSha256:
      process.env.GENERATION_PROVIDER_CONFIG_DIGEST || null,
    providerResourceRevision: Number.parseInt(
      process.env.GENERATION_PROVIDER_CONFIG_REVISION || "0",
      10,
    ) || null,
    manifestSha256: sha256(fs.readFileSync(casesFile)),
  },
  execution: {
    generatedCaseCount: manifest.cases.length,
    executedCaseCount: selectedCases.length,
    partial: selectedCases.length !== manifest.cases.length,
  },
  provider: {
    gatewayMode: manifest.provider.gatewayMode,
    modelResourceId: manifest.provider.modelResourceId,
    realProviderVerified,
    verificationError: realProviderVerified
      ? null
      : "every executed run must include model.execution evidence for the configured model resource",
  },
  approval: {
    required: false,
    manualApprovalCount: 0,
    briefProtocolConfirmation: "automated",
    contentPlanApprovalMode: "automated_fixture_exact_identity",
    verifiedContentPlanCount: caseResults.filter(
      (result) => result.contentPlan?.state === "verified",
    ).length,
  },
  budget: {
    configuredTotalTokens: suiteTokenCeiling,
    manifestTotalTokens: manifest.budget.totalTokens,
    perRunSafetyCeiling,
    actualInputTokens: consumedInputTokens,
    actualOutputTokens: consumedOutputTokens,
    actualCachedInputTokens: consumedCachedInputTokens,
    actualTotalTokens: consumedTokens,
    remainingTokens: suiteTokenCeiling - consumedTokens,
    exceeded: consumedTokens > suiteTokenCeiling,
  },
  cases: caseResults.map((result) => ({
    id: result.id,
    title: result.title,
    kind: result.kind,
    projectId: result.projectId,
    workspaceNamespace: result.workspaceNamespace,
    status: result.status,
    expectedRoute: result.expectedRoute,
    expectedText: result.expectedText,
    acceptanceContractSha256: result.acceptance.sha256,
    contentPlan: result.contentPlan,
    attemptCount: result.attempts.length,
    attempts: result.attempts,
    runIds: result.runs.map((run) => run.runId),
    totalTokens: result.runs.reduce(
      (total, run) => total + run.usage.totalTokens,
      0,
    ),
    artifact: result.artifact,
    warmEdit: result.warmEdit
      ? {
          status: result.warmEdit.status,
          runId: result.warmEdit.run?.runId || null,
          totalTokens: result.warmEdit.run?.usage?.totalTokens || 0,
          providerVerified: result.warmEdit.providerVerified === true,
          hmrMetricRecorded: result.warmEdit.hmrMetricRecorded === true,
          evidencePath: result.warmEdit.evidencePath,
      }
      : null,
    coldDevEdit: result.coldDevEdit
      ? {
          status: result.coldDevEdit.status,
          runId: result.coldDevEdit.run?.runId || null,
          totalTokens: result.coldDevEdit.run?.usage?.totalTokens || 0,
          providerVerified: result.coldDevEdit.providerVerified === true,
          coldDevMetricRecorded:
            result.coldDevEdit.coldDevMetricRecorded === true,
          durableSnapshotMetricRecorded:
            result.coldDevEdit.durableSnapshotMetricRecorded === true,
          evidencePath: result.coldDevEdit.evidencePath,
      }
      : null,
    repair: result.repair
      ? {
          status: result.repair.status,
          reviewRunId: result.repair.reviewRun?.runId || null,
          repairRunId: result.repair.run?.runId || null,
          findingId: result.repair.reviewFinding?.findingId || null,
          totalTokens:
            (result.repair.reviewRun?.usage?.totalTokens || 0) +
            (result.repair.run?.usage?.totalTokens || 0),
          providerVerified: result.repair.providerVerified === true,
          repairVerification: result.repair.repairVerification,
      }
      : null,
    sandboxRelease: result.sandboxRelease,
    cleanupError: result.cleanupError,
    error: result.error,
  })),
};

const workingSummaryFile = path.join(
  suiteDirectory,
  "real-provider-examples-summary.json",
);
fs.writeFileSync(workingSummaryFile, `${JSON.stringify(summary, null, 2)}\n`);
const finalSuiteDirectory = path.join(suiteRoot, `suite-${suiteId}-${suiteStatus}`);
fs.renameSync(suiteDirectory, finalSuiteDirectory);
suiteDirectory = finalSuiteDirectory;
const summaryFile = path.join(suiteDirectory, "real-provider-examples-summary.json");
process.stdout.write(`Real-provider suite evidence: ${summaryFile}\n`);

if (suiteStatus !== "accepted") process.exitCode = 1;

function validateManifest(value) {
  if (value.schemaVersion !== "generation-real-provider-suite@1") {
    throw new Error("unsupported real-provider suite schema");
  }
  if (value.provider?.gatewayMode !== "internal_gateway") {
    throw new Error("real-provider suite must use the governed internal Gateway");
  }
  if (!/^[a-z0-9][a-z0-9._-]{0,127}$/.test(value.provider?.modelResourceId || "")) {
    throw new Error("real-provider suite must target a valid frozen model resource id");
  }
  if (value.approval?.required !== false) {
    throw new Error("real-provider suite must not require human approval");
  }
  if (!Number.isSafeInteger(value.budget?.totalTokens)) {
    throw new Error("suite total token budget must be an integer");
  }
  if (value.budget.totalTokens > 25_000_000) {
    throw new Error("suite total token budget exceeds 25,000,000");
  }
  if (value.budget.maxRunsPerCase !== 2) {
    throw new Error("each real example must reserve exactly Brief + Build");
  }
  for (const field of [
    "maxTurns",
    "maxToolCalls",
    "maxInputTokens",
    "maxOutputTokens",
  ]) {
    if (!Number.isSafeInteger(value.budget?.perRun?.[field])) {
      throw new Error(`per-run budget ${field} must be an integer`);
    }
  }
  const suiteKind = value.suiteKind ?? "functional_canary";
  if (!new Set(["functional_canary", "efficiency_benchmark"]).has(suiteKind)) {
    throw new Error("real-provider suiteKind is unsupported");
  }
  const expectedCaseCount = suiteKind === "efficiency_benchmark" ? 10 : 5;
  if (!Array.isArray(value.cases) || value.cases.length !== expectedCaseCount) {
    throw new Error(`real-provider ${suiteKind} suite must contain exactly ${expectedCaseCount} cases`);
  }
  if (suiteKind === "efficiency_benchmark"
    && value.cases.some((testCase) => testCase.kind !== "website")) {
    throw new Error("efficiency Benchmark suite currently requires Website cases");
  }
  const ids = new Set();
  for (const testCase of value.cases) {
    if (ids.has(testCase.id)) throw new Error(`duplicate case id: ${testCase.id}`);
    ids.add(testCase.id);
    if (!["website", "docs"].includes(testCase.kind)) {
      throw new Error(`unsupported case kind: ${testCase.kind}`);
    }
    if (!testCase.prompt || !testCase.expectedText || !testCase.expectedRoute) {
      throw new Error(`case ${testCase.id} is missing an acceptance field`);
    }
  }
  const perRunSafetyCeiling =
    value.budget.perRun.maxInputTokens + value.budget.perRun.maxOutputTokens;
  if (perRunSafetyCeiling > value.budget.totalTokens) {
    throw new Error(
      `per-run safety ceiling ${perRunSafetyCeiling} exceeds suite maximum ${value.budget.totalTokens}`,
    );
  }
}

function configureAndBindDesignProfile(projectId) {
  const fixture = path.resolve(designProfileFixture);
  if (!fs.statSync(fixture).isFile()) {
    throw new Error(`Design Profile fixture is not a file: ${fixture}`);
  }
  const selectionEvidence = path.join(
    suiteDirectory,
    `design-profile-selection-${projectId}.json`,
  );
  execFileSync(
    process.execPath,
    [
      path.resolve("infra/generation-reliability/configure-real-design-profile.mjs"),
      baseUrl,
      principalPrivateKeyFile,
      projectId,
      fixture,
      selectionEvidence,
    ],
    { stdio: "inherit" },
  );
  const selection = JSON.parse(fs.readFileSync(selectionEvidence, "utf8"));
  const designProfileId = selection.selectedDesignProfile?.id;
  if (!designProfileId) {
    throw new Error("Design Profile selection evidence omitted the selected id");
  }
  const adaptationEvidence = path.join(
    suiteDirectory,
    `design-profile-adaptation-${projectId}.json`,
  );
  execFileSync(
    process.execPath,
    [
      path.resolve("infra/generation-reliability/adapt-real-design-profile-style-only.mjs"),
      baseUrl,
      principalPrivateKeyFile,
      projectId,
      designProfileId,
      adaptationEvidence,
    ],
    { stdio: "inherit" },
  );
  const adaptation = JSON.parse(fs.readFileSync(adaptationEvidence, "utf8"));
  return {
    id: designProfileId,
    name: adaptation.name,
    version: adaptation.toVersion,
    selectedFixture: selection.randomSelection.selectedFixture,
    selectedFixtureSha256: selection.randomSelection.selectedFixtureSha256,
    allowedTemplates: ["next-app"],
    preferredWebsiteTemplate: "next-app",
    bindingVerified: selection.bindingVerified && adaptation.bindingVerified,
  };
}

function repairMarker() {
  const marker = (
    process.env.GENERATION_REAL_REPAIR_MARKER ||
    `REPAIR_CANARY_${suiteId.slice(-12)}`
  ).trim();
  if (!/^[A-Za-z0-9][A-Za-z0-9_-]{7,80}$/.test(marker)) {
    throw new Error("GENERATION_REAL_REPAIR_MARKER has an invalid format");
  }
  return marker;
}

async function runRepairCanary(
  projectId,
  buildRunId,
  baseVersionId,
  contentPlan,
  testCase,
) {
  const marker = repairMarker();
  const startedAt = new Date().toISOString();
  const evidence = {
    schemaVersion: "generation-real-provider-repair-evidence@2",
    startedAt,
    finishedAt: null,
    status: "failed",
    projectId,
    promptSha256: sha256([
      `Review the deliberate inaccessible contrast defect on ${marker}.`,
      "Create a repairable blocking finding through the read-only Review run, then repair only that scoped finding and publish a fresh validated Version.",
    ].join(" ")),
    lifecycleProfile: "repair_warm",
    repairMarker: marker,
    initialBuildRunId: buildRunId,
    initialBuildVersionId: baseVersionId,
    baseVersionId: null,
    setupEdit: null,
    reviewRun: null,
    reviewFinding: null,
    run: null,
    generationContextStatus: null,
    repairedVersionId: null,
    repairVerification: null,
    providerVerified: false,
    secretMaterialPersisted: false,
    error: null,
  };
  let reviewEvents = [];
  let repairEvents = [];
  try {
    if (!baseVersionId) {
      throw new Error("Repair canary base Build did not produce a candidate Version");
    }
    assertRunReservation("repair_fixture_edit", testCase.id);
    evidence.setupEdit = runRepairFixtureEdit(projectId, marker, contentPlan);
    if (evidence.setupEdit.run?.usage) {
      addUsage(evidence.setupEdit.run.usage);
      assertActualBudget();
    }
    if (
      evidence.setupEdit.status !== "accepted" ||
      !evidence.setupEdit.versionId ||
      evidence.setupEdit.versionId === baseVersionId ||
      evidence.setupEdit.run?.status !== "completed"
    ) {
      throw new Error(
        `Repair fixture Edit failed: ${evidence.setupEdit.error?.message || evidence.setupEdit.status}`,
      );
    }
    evidence.baseVersionId = evidence.setupEdit.versionId;
    assertRunReservation("review", testCase.id);
    const reviewStarted = await startRun(projectId, "review", "visual-review", {
      parentRunId: evidence.setupEdit.run.runId,
    });
    const reviewStream = await readRunEvents(projectId, reviewStarted.runId, "review");
    reviewEvents = reviewStream.events;
    evidence.reviewRun = summarizeRun(
      "review",
      reviewStarted.runId,
      reviewEvents,
      reviewStream.evidence,
    );
    evidence.reviewRun.efficiency = await fetchRunEfficiencyMetrics(
      projectId,
      reviewStarted.runId,
    );
    evidence.reviewRun.promptEfficiency = await fetchRunPromptEfficiency(
      projectId,
      reviewStarted.runId,
    );
    evidence.reviewRun.generationContextStatus = await fetchGenerationContextStatus(
      projectId,
      reviewStarted.runId,
    );
    evidence.reviewRun.budgetProfile = await fetchRunBudgetProfile(
      projectId,
      reviewStarted.runId,
      "review",
    );
    addUsage(evidence.reviewRun.usage);
    assertActualBudget();
    if (evidence.reviewRun.status !== "completed") {
      throw new Error(
        `Review did not complete: ${evidence.reviewRun.summary || evidence.reviewRun.status}`,
      );
    }
    const findings = reviewEvents
      .filter(
        (event) =>
          event.type === "review.finding" &&
          typeof event.findingId === "string" &&
          event.findingId.length > 0,
      )
      .sort((left, right) => {
        const matchesTarget = (event) => {
          const summary = String(event.summary || "").toLowerCase();
          return summary.includes(marker.toLowerCase()) || summary.includes("contrast");
        };
        return Number(matchesTarget(right)) - Number(matchesTarget(left));
      });
    if (findings.length === 0) {
      throw new Error("read-only Review completed without recording a scoped finding");
    }

    assertRunReservation("repair", testCase.id);
    let repairStarted = null;
    let finding = null;
    let lastStartError = null;
    for (const candidateFinding of findings) {
      try {
        repairStarted = await startRun(projectId, "repair", "repair", {
          parentRunId: reviewStarted.runId,
          findingIds: [candidateFinding.findingId],
          contentPlan,
        });
        finding = candidateFinding;
        break;
      } catch (error) {
        lastStartError = error;
      }
    }
    if (!repairStarted || !finding) {
      throw lastStartError || new Error("Review produced no repairable finding");
    }
    evidence.reviewFinding = {
      findingId: finding.findingId,
      severity: finding.severity || null,
      summary: finding.summary || null,
      source: "real-provider-review.report_finding",
    };
    evidence.generationContextStatus = await fetchGenerationContextStatus(
      projectId,
      repairStarted.runId,
    );
    if (evidence.generationContextStatus.executionProfile !== "repair_warm") {
      throw new Error(
        `Repair run used unexpected execution profile: ${evidence.generationContextStatus.executionProfile || "missing"}`,
      );
    }
    const repairStream = await readRunEvents(projectId, repairStarted.runId, "repair");
    repairEvents = repairStream.events;
    evidence.run = summarizeRun(
      "repair",
      repairStarted.runId,
      repairEvents,
      repairStream.evidence,
    );
    evidence.run.generationContextStatus = evidence.generationContextStatus;
    evidence.run.designProfileIdentity = await fetchRunDesignProfileIdentity(
      projectId,
      repairStarted.runId,
    );
    evidence.run.efficiency = await fetchRunEfficiencyMetrics(
      projectId,
      repairStarted.runId,
    );
    evidence.run.promptEfficiency = await fetchRunPromptEfficiency(
      projectId,
      repairStarted.runId,
    );
    evidence.run.budgetProfile = await fetchRunBudgetProfile(
      projectId,
      repairStarted.runId,
      "repair",
    );
    addUsage(evidence.run.usage);
    assertActualBudget();
    if (evidence.run.status !== "completed") {
      throw new Error(
        `Repair did not complete: ${evidence.run.summary || evidence.run.status}`,
      );
    }
    evidence.repairedVersionId = extractCandidateVersionId(repairEvents);
    const artifactProbe = await readCurrentArtifactProbe(
      projectId,
      testCase.expectedRoute,
    );
    evidence.repairVerification = {
      reviewFindingRecorded: true,
      findingFixedByCompletedRepair: true,
      freshVersionCreated:
        Boolean(evidence.repairedVersionId) &&
        evidence.repairedVersionId !== evidence.baseVersionId,
      sourceMutationRecorded:
        Number.isFinite(evidence.run.efficiency.timeToFirstSourceMutationMs) ||
        Number.isFinite(evidence.run.efficiency.modelTurnAtFirstSourceMutation),
      previewPublishRecorded: repairEvents.some(
        (event) => event.type === "tool.completed" && event.tool === "preview.publish",
      ),
      markerPreserved: artifactProbe.body.includes(marker),
      artifactRoute: testCase.expectedRoute,
      artifactHttpStatus: artifactProbe.httpStatus,
      artifactBodySha256: artifactProbe.bodySha256,
      artifactBodyBytes: artifactProbe.bodyBytes,
    };
    evidence.providerVerified = [evidence.reviewRun, evidence.run].every(
      (run) =>
        run.modelExecutions.length > 0 &&
        run.modelExecutions.every(
          (execution) =>
            execution.modelResourceId === manifest.provider.modelResourceId &&
            execution.providerRequestIdPresent === true,
        ),
    );
    if (
      !evidence.providerVerified ||
      Object.entries(evidence.repairVerification)
        .filter(([key]) => !new Set([
          "artifactRoute",
          "artifactHttpStatus",
          "artifactBodySha256",
          "artifactBodyBytes",
        ]).has(key))
        .some(([, value]) => value !== true) ||
      evidence.repairVerification.artifactHttpStatus !== 200 ||
      !/^[a-f0-9]{64}$/.test(evidence.repairVerification.artifactBodySha256) ||
      !Number.isSafeInteger(evidence.repairVerification.artifactBodyBytes) ||
      evidence.repairVerification.artifactBodyBytes <= 0
    ) {
      throw new Error("Repair lifecycle evidence did not satisfy its acceptance contract");
    }
    evidence.status = "accepted";
  } catch (error) {
    if (!evidence.reviewRun && reviewEvents.length > 0) {
      evidence.reviewRun = summarizeRun("review", "unknown", reviewEvents);
    }
    if (!evidence.run && repairEvents.length > 0) {
      evidence.run = summarizeRun("repair", "unknown", repairEvents);
    }
    evidence.error = {
      name: error?.name || "Error",
      message: String(error?.message || error),
    };
  }
  evidence.finishedAt = new Date().toISOString();
  return evidence;
}

function runRepairFixtureEdit(projectId, marker, contentPlan) {
  const evidenceRoot = path.join(suiteDirectory, `repair-fixture-edit-${projectId}`);
  const prompt = [
    "Make one isolated setup change for a later accessibility Review/Repair canary.",
    `Add the exact visible text ${marker} to the main documentation content in a small span.`,
    "Use valid JSX with a React style object whose color is #ffffff and backgroundColor is #ffffff, intentionally making only that marker unreadable due to zero contrast.",
    "Preserve every existing route and required text, publish the updated candidate, and do not fix the deliberate marker contrast in this setup Edit.",
  ].join(" ");
  let executionError = null;
  try {
    execFileSync(
      process.execPath,
      [
        path.resolve("infra/generation-reliability/run-real-provider-edit.mjs"),
        baseUrl,
        principalPrivateKeyFile,
        adminTokenFile,
        projectId,
        prompt,
        evidenceRoot,
      ],
      {
        cwd: process.cwd(),
        env: {
          ...process.env,
          GENERATION_REAL_MODEL_RESOURCE_ID: manifest.provider.modelResourceId,
          GENERATION_REAL_DRAFT_WARM_EDIT: "false",
          GENERATION_REAL_DRAFT_COLD_DEV_EDIT: "false",
          GENERATION_REAL_KEEP_SANDBOX: "true",
          GENERATION_REAL_CONTENT_PLAN_JSON: JSON.stringify(contentPlan),
          GENERATION_REAL_ARTIFACT_KIND: "docs",
          GENERATION_REAL_EXPECTED_ARTIFACT_TEXT: marker,
        },
        stdio: "inherit",
      },
    );
  } catch (error) {
    executionError = error;
  }
  const evidenceDirectory = fs
    .readdirSync(evidenceRoot, { withFileTypes: true })
    .filter(
      (entry) =>
        entry.isDirectory() &&
        (entry.name.endsWith("-accepted") || entry.name.endsWith("-failed")),
    )
    .map((entry) => path.join(evidenceRoot, entry.name))
    .sort()
    .at(-1);
  if (!evidenceDirectory) {
    throw executionError || new Error("Repair fixture Edit did not produce evidence");
  }
  const evidenceFile = path.join(
    evidenceDirectory,
    "real-provider-edit-summary.json",
  );
  const evidence = JSON.parse(fs.readFileSync(evidenceFile, "utf8"));
  return {
    ...evidence,
    evidencePath: path.relative(suiteDirectory, evidenceFile),
  };
}

async function fetchGenerationContextStatus(projectId, runId) {
  const response = await fetchWithTimeout(
    new URL(
      `/runs/${encodeURIComponent(runId)}/generation-context-status`,
      baseUrl,
    ),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken(projectId)}` },
    },
    120_000,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(
      `generation context status returned ${response.status}: ${body.slice(0, 500)}`,
    );
  }
  return JSON.parse(body);
}

async function fetchRunDesignProfileIdentity(projectId, runId) {
  const response = await fetchWithTimeout(
    new URL(`/runs/${encodeURIComponent(runId)}/design-context-manifest`, baseUrl),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken(projectId)}` },
    },
    120_000,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`run Design Context Manifest returned ${response.status}: ${body.slice(0, 500)}`);
  }
  const manifest = JSON.parse(body);
  if (manifest.runId !== runId
    || !/^[a-f0-9]{64}$/.test(manifest.package?.effectiveProfileHash || "")) {
    throw new Error("run Design Profile identity or schema mismatch");
  }
  return {
    schemaVersion: "run-design-profile-identity@1",
    runId,
    designProfileId: manifest.package.designProfileId ?? null,
    designProfileVersion: manifest.package.designProfileVersion ?? null,
    effectiveProfileHash: manifest.package.effectiveProfileHash,
  };
}

async function fetchRunBudgetProfile(projectId, runId, phase) {
  const response = await fetchWithTimeout(
    new URL(`/runs/${encodeURIComponent(runId)}/budget-profile`, baseUrl),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken(projectId)}` },
    },
    120_000,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`run budget profile returned ${response.status}: ${body.slice(0, 500)}`);
  }
  const profile = JSON.parse(body);
  if (profile.schemaVersion !== "run-budget-profile@1"
    || profile.phase !== phase
    || typeof profile.profileId !== "string"
    || !/^[a-f0-9]{64}$/.test(profile.profileHash || "")) {
    throw new Error("run budget profile identity or schema mismatch");
  }
  return profile;
}

async function readCurrentArtifactProbe(projectId, route) {
  const response = await fetchWithTimeout(
    new URL(
      `/artifacts/${encodeURIComponent(projectId)}/current${route}`,
      baseUrl,
    ),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken(projectId)}` },
    },
    120_000,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(
      `repaired artifact route returned ${response.status}: ${body.slice(0, 500)}`,
    );
  }
  return {
    body,
    httpStatus: response.status,
    bodySha256: sha256(body),
    bodyBytes: Buffer.byteLength(body),
  };
}

function runDraftLifecycleEditCanary(projectId, briefId, contentPlan) {
  const lifecycleKind = draftColdDevEditCanary
    ? "cold_dev"
    : (process.env.GENERATION_REAL_DRAFT_WARM_EDIT_KIND || "copy_css").trim();
  if (!new Set(["copy_css", "structural", "cold_dev"]).has(lifecycleKind)) {
    throw new Error("Draft lifecycle Edit kind is invalid");
  }
  const marker = (
    process.env.GENERATION_REAL_WARM_EDIT_MARKER ||
    `WARM-HMR-VERIFIED-${suiteId.slice(-8)}`
  ).trim();
  if (!/^[A-Za-z0-9][A-Za-z0-9_-]{7,80}$/.test(marker)) {
    throw new Error("GENERATION_REAL_WARM_EDIT_MARKER has an invalid format");
  }
  const evidenceRoot = path.join(
    suiteDirectory,
    `${draftColdDevEditCanary ? "cold-dev-edit" : "warm-edit"}-${projectId}`,
  );
  const prompt = lifecycleKind === "cold_dev"
    ? [
        "Update only project/app/page.tsx.",
        `Add the exact visible text ${marker} near the existing page heading without removing existing content.`,
        "Do not modify package.json or any Runtime-owned contract file. Restore the existing dependency graph with project.ensure_dependencies mode restore.",
        "Follow Runtime Workflow Progress exactly: stop the prior managed Dev process once, restart Dev once, wait for the current revision to become ready and durable, then complete the run. Do not inspect ports or processes with shell.run.",
        "Do not run a Production Build.",
      ].join(" ")
    : lifecycleKind === "structural"
    ? [
        "Update only project/app/page.tsx.",
        `Add a new visible section headed by the exact text ${marker} without removing existing content.`,
        "Change the page structure with a semantic section and reuse the existing visual language.",
        "Keep the current managed Dev session, wait for preview.dev_status to report the current revision as ready and durable, create the durable DraftSnapshot, then complete the run.",
      ].join(" ")
    : [
        "Update only project/app/page.tsx.",
        `Add the exact visible text ${marker} near the existing page heading without removing existing content, and make only a small CSS presentation adjustment to that text.`,
        "Keep the current managed Dev session, wait for preview.dev_status to report the current revision as ready and durable, create the durable DraftSnapshot, then complete the run.",
      ].join(" ");
  let executionError = null;
  try {
    execFileSync(
      process.execPath,
      [
        path.resolve("infra/generation-reliability/run-real-provider-edit.mjs"),
        baseUrl,
        principalPrivateKeyFile,
        adminTokenFile,
        projectId,
        prompt,
        evidenceRoot,
      ],
      {
        cwd: process.cwd(),
        env: {
          ...process.env,
          GENERATION_REAL_MODEL_RESOURCE_ID: manifest.provider.modelResourceId,
          GENERATION_REAL_DRAFT_WARM_EDIT: draftColdDevEditCanary ? "false" : "true",
          GENERATION_REAL_DRAFT_COLD_DEV_EDIT: draftColdDevEditCanary ? "true" : "false",
          GENERATION_REAL_DRAFT_WARM_EDIT_KIND:
            lifecycleKind === "cold_dev" ? "copy_css" : lifecycleKind,
          GENERATION_REAL_KEEP_SANDBOX: "true",
          GENERATION_REAL_EXPECTED_DRAFT_TEXT: marker,
          GENERATION_REAL_BRIEF_ID: briefId,
          GENERATION_REAL_CONTENT_PLAN_JSON: JSON.stringify(contentPlan),
        },
        stdio: "inherit",
      },
    );
  } catch (error) {
    executionError = error;
  }
  const evidenceDirectories = fs
    .readdirSync(evidenceRoot, { withFileTypes: true })
    .filter(
      (entry) =>
        entry.isDirectory() &&
        (entry.name.endsWith("-accepted") || entry.name.endsWith("-failed")),
    )
    .map((entry) => path.join(evidenceRoot, entry.name))
    .sort();
  const evidenceDirectory = evidenceDirectories.at(-1);
  if (!evidenceDirectory) {
    throw executionError || new Error("Draft lifecycle Edit canary did not produce evidence");
  }
  const evidenceFile = path.join(
    evidenceDirectory,
    "real-provider-edit-summary.json",
  );
  const evidence = JSON.parse(fs.readFileSync(evidenceFile, "utf8"));
  if (
    evidence.status === "accepted" &&
    (evidence.providerVerified !== true ||
      (!draftColdDevEditCanary && evidence.hmrMetricRecorded !== true) ||
      (draftColdDevEditCanary &&
        (evidence.coldDevMetricRecorded !== true ||
          evidence.durableSnapshotMetricRecorded !== true)))
  ) {
    throw new Error(
      `Draft lifecycle Edit canary evidence is incomplete: ${JSON.stringify({
        status: evidence.status,
        providerVerified: evidence.providerVerified,
        hmrMetricRecorded: evidence.hmrMetricRecorded,
        coldDevMetricRecorded: evidence.coldDevMetricRecorded,
        durableSnapshotMetricRecorded: evidence.durableSnapshotMetricRecorded,
      })}`,
    );
  }
  return {
    ...evidence,
    lifecycleProfile: draftColdDevEditCanary ? "cold_dev" : "warm_hmr",
    warmEditKind: draftColdDevEditCanary ? null : lifecycleKind,
    evidencePath: path.relative(suiteDirectory, evidenceFile),
  };
}

async function verifyDraftPreview(projectId, testCase, staticPreview) {
  if (staticPreview) {
    return verifyStaticDraftPreview(projectId, testCase, staticPreview);
  }
  const deadline = Date.now() + 180_000;
  let session;
  while (Date.now() < deadline) {
    const token = issuePrincipalToken(projectId);
    const response = await fetchWithTimeout(
      new URL(`/projects/${encodeURIComponent(projectId)}/draft-preview`, baseUrl),
      { headers: { authorization: `Bearer ${token}` } },
      30_000,
    );
    if (response.ok) {
      session = await response.json();
      if (
        session.status === "ready" &&
        session.lastReadyRevision >= session.durableRevision
      ) {
        break;
      }
    } else if (response.status !== 404) {
      throw new Error(
        `draft preview lookup returned ${response.status}: ${(await response.text()).slice(0, 500)}`,
      );
    }
    await delay(1_000);
  }
  if (!session || session.status !== "ready") {
    throw new Error(
      `Draft Preview did not become ready: ${JSON.stringify(session || null)}`,
    );
  }
  if (session.templateId !== "next-app") {
    throw new Error(`Draft Preview used unexpected template: ${session.templateId}`);
  }
  const proxyPath = new URL(session.proxyUrl).pathname;
  const leaseId = proxyPath.split("/").filter(Boolean).at(-1);
  if (!leaseId) throw new Error("Draft Preview proxy URL omitted a lease id");
  const prefix = `/projects/${projectId}/previews/${leaseId}`;
  const previewUrl = new URL(
    `/previews/${encodeURIComponent(leaseId)}${testCase.expectedRoute}`,
    baseUrl,
  );
  const previewResponse = await fetchWithTimeout(
    previewUrl,
    {
      headers: {
        authorization: `Bearer ${issuePrincipalToken(projectId)}`,
        "x-anydesign-preview-prefix": prefix,
      },
    },
    120_000,
  );
  const previewBody = await previewResponse.text();
  if (!previewResponse.ok) {
    throw new Error(
      `Draft Preview route returned ${previewResponse.status}: ${previewBody.slice(0, 500)}`,
    );
  }
  if (!previewBody.includes(testCase.expectedText)) {
    throw classifiedError(
      "acceptance_rejected",
      `Draft Preview does not contain expected text: ${testCase.expectedText}`,
      "rejected",
    );
  }
  return {
    sessionId: session.sessionId,
    sessionEpoch: session.sessionEpoch,
    leaseId,
    templateId: session.templateId,
    status: session.status,
    workspaceRevision: session.workspaceRevision,
    lastReadyRevision: session.lastReadyRevision,
    durableRevision: session.durableRevision,
    durableSnapshotId: session.durableSnapshotId,
    hmr: true,
    url: previewUrl.toString(),
    route: testCase.expectedRoute,
    httpStatus: previewResponse.status,
    expectedText: testCase.expectedText,
    expectedTextFound: true,
    bodySha256: sha256(previewBody),
    bodyBytes: Buffer.byteLength(previewBody),
  };
}

async function verifyStaticDraftPreview(projectId, testCase, staticPreview) {
  const prefix = `/projects/${projectId}/previews/${staticPreview.leaseId}`;
  const previewUrl = new URL(
    `/previews/${encodeURIComponent(staticPreview.leaseId)}${testCase.expectedRoute}`,
    baseUrl,
  );
  const previewResponse = await fetchWithTimeout(
    previewUrl,
    {
      headers: {
        authorization: `Bearer ${issuePrincipalToken(projectId)}`,
        "x-anydesign-preview-prefix": prefix,
      },
    },
    120_000,
  );
  const previewBody = await previewResponse.text();
  if (!previewResponse.ok) {
    throw new Error(
      `Static Draft Preview route returned ${previewResponse.status}: ${previewBody.slice(0, 500)}`,
    );
  }
  if (!previewBody.includes(testCase.expectedText)) {
    throw classifiedError(
      "acceptance_rejected",
      `Static Draft Preview does not contain expected text: ${testCase.expectedText}`,
      "rejected",
    );
  }
  return {
    snapshotId: staticPreview.snapshotId,
    leaseId: staticPreview.leaseId,
    hmr: false,
    route: testCase.expectedRoute,
    httpStatus: previewResponse.status,
    expectedText: testCase.expectedText,
    expectedTextFound: true,
    bodySha256: sha256(previewBody),
    bodyBytes: Buffer.byteLength(previewBody),
  };
}

async function grantProjectAccess(projectId) {
  const response = await fetchWithTimeout(
    new URL(`/internal/projects/${encodeURIComponent(projectId)}/access`, baseUrl),
    {
      method: "PUT",
      headers: {
        "content-type": "application/json",
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": adminToken,
      },
      body: JSON.stringify({
        ownerPrincipalId: principalId,
        workspaceNamespace,
      }),
    },
    120_000,
  );
  if (!response.ok) {
    throw new Error(
      `project access failed ${response.status}: ${await response.text()}`,
    );
  }
}

async function prepareApprovedContentPlan(projectId, testCase, executionPrompt = testCase.prompt) {
  const eventIdentity = sha256(projectId).slice(0, 16);
  const planPayload = {
    schemaVersion: "generation-real-provider-content-plan@1",
    fixtureId: testCase.id,
    intentSha256: sha256(executionPrompt),
    expectedRoute: testCase.expectedRoute,
    expectedTextSha256: sha256(testCase.expectedText),
    kind: testCase.kind,
    locale: testCase.locale,
  };
  const identity = {
    planId: `real-provider-${testCase.id}-plan`,
    revision: 1,
    contentHash: sha256(JSON.stringify(planPayload)),
  };
  const changeRequest = {
    ...identity,
    changeEventId: `real-provider-${testCase.id}-${eventIdentity}-plan-created`,
  };
  const changeResponse = await fetchWithTimeout(
    new URL(`/projects/${encodeURIComponent(projectId)}/content-plan-changes`, baseUrl),
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": adminToken,
      },
      body: JSON.stringify(changeRequest),
    },
    120_000,
  );
  if (!changeResponse.ok) {
    throw new Error(`content plan change failed ${changeResponse.status}: ${(await changeResponse.text()).slice(0, 500)}`);
  }
  const approvalRequest = {
    ...identity,
    confirmationEventId: `real-provider-${testCase.id}-${eventIdentity}-plan-approved`,
  };
  const approvalResponse = await fetchWithTimeout(
    new URL(`/projects/${encodeURIComponent(projectId)}/content-plan-approvals`, baseUrl),
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": adminToken,
      },
      body: JSON.stringify(approvalRequest),
    },
    120_000,
  );
  if (!approvalResponse.ok) {
    throw new Error(`content plan approval failed ${approvalResponse.status}: ${(await approvalResponse.text()).slice(0, 500)}`);
  }
  return {
    identity,
    evidence: {
      ...identity,
      fixtureId: testCase.id,
      intentSha256: planPayload.intentSha256,
      approvalEvidenceSha256: sha256(JSON.stringify(approvalRequest)),
      state: "verified",
    },
  };
}

async function runBrief(projectId, prompt) {
  const started = await startRun(projectId, "brief", "brief", {
    contentSources: [
      {
        id: "real-provider-source",
        kind: "prompt",
        text: prompt,
        readable: true,
      },
    ],
    modelResourceId: manifest.provider.modelResourceId,
  });
  const eventStreamOutcome = readRunEvents(
    projectId,
    started.runId,
    "brief",
  ).then(
    (eventStream) => ({ eventStream, error: null }),
    (error) => ({ eventStream: null, error }),
  );
  const confirmationDeadline = Date.now() + 900_000;
  let confirmationSent = false;
  let briefId = "";
  let completedWithoutBriefDeadline = null;

  while (Date.now() < confirmationDeadline && !briefId) {
    const token = issuePrincipalToken(projectId);
    const response = await fetchWithTimeout(
      new URL(
        `/projects/${encodeURIComponent(projectId)}/conversation?includeDebug=true`,
        baseUrl,
      ),
      { headers: { authorization: `Bearer ${token}` } },
      30_000,
    );
    if (!response.ok) {
      throw new Error(
        `brief conversation failed ${response.status}: ${await response.text()}`,
      );
    }
    const conversation = await response.json();
    const serialized = JSON.stringify(conversation);
    briefId =
      [...(conversation.items || [])]
        .reverse()
        .find((item) => item?.metadata?.briefId)?.metadata?.briefId || "";
    const confirmationVisible =
      serialized.includes('"kind":"approval_request"') ||
      serialized.includes("Requested brief confirmation") ||
      serialized.includes("confirmation_requested") ||
      serialized.includes("briefId");
    if (confirmationVisible && !confirmationSent) {
      const continueResponse = await fetchWithTimeout(
        new URL(`/runs/${encodeURIComponent(started.runId)}/continue`, baseUrl),
        {
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization: `Bearer ${token}`,
          },
          body: JSON.stringify({ userMessage: "confirm" }),
        },
        120_000,
      );
      if (!continueResponse.ok) {
        throw new Error(
          `automated brief confirmation failed ${continueResponse.status}: ${await continueResponse.text()}`,
        );
      }
      confirmationSent = true;
    }
    if (!briefId) {
      const outcome = await Promise.race([
        eventStreamOutcome,
        delay(1_000).then(() => null),
      ]);
      if (outcome?.error) {
        attachPartialRunEvidence(outcome.error, "brief", started.runId);
        throw outcome.error;
      }
      if (outcome?.eventStream) {
        const terminalRun = summarizeRun(
          "brief",
          started.runId,
          outcome.eventStream.events,
          outcome.eventStream.evidence,
        );
        if (terminalRun.status !== "completed") {
          const failure = classifyTerminalRunFailure(terminalRun);
          const error = classifiedError(
            failure.classification,
            `brief did not complete: ${terminalRun.summary || terminalRun.status || "unknown"}`,
            failure.resultStatus,
          );
          error.runEvidence = [terminalRun];
          throw error;
        }
        completedWithoutBriefDeadline ??= Date.now() + 5_000;
        if (Date.now() >= completedWithoutBriefDeadline) {
          const error = classifiedError(
            "brief_id_missing_after_completion",
            `brief run ${started.runId} completed without producing a briefId`,
          );
          error.runEvidence = [terminalRun];
          throw error;
        }
        await delay(250);
      }
    }
  }
  if (!briefId) {
    throw new Error(`brief did not produce briefId for project ${projectId}`);
  }

  const outcome = await eventStreamOutcome;
  if (outcome.error) {
    attachPartialRunEvidence(outcome.error, "brief", started.runId);
    throw outcome.error;
  }
  const eventStream = outcome.eventStream;
  const evidence = summarizeRun(
    "brief",
    started.runId,
    eventStream.events,
    eventStream.evidence,
  );
  evidence.efficiency = await fetchRunEfficiencyMetrics(projectId, started.runId);
  evidence.promptEfficiency = await fetchRunPromptEfficiency(projectId, started.runId);
  evidence.budgetProfile = await fetchRunBudgetProfile(projectId, started.runId, "brief");
  if (evidence.status !== "completed") {
    const error = new Error(
      `brief run ${started.runId} ended with status ${evidence.status}`,
    );
    error.runEvidence = [evidence];
    throw error;
  }
  return {
    briefId,
    evidence,
  };
}

async function runBuild(projectId, briefId, contentPlan) {
  const started = await startRun(projectId, "build", "build", {
    briefId,
    contentPlan,
    modelResourceId: manifest.provider.modelResourceId,
  });
  let eventStream;
  try {
    eventStream = await readRunEvents(projectId, started.runId, "build");
  } catch (error) {
    attachPartialRunEvidence(error, "build", started.runId);
    throw error;
  }
  const run = summarizeRun(
    "build",
    started.runId,
    eventStream.events,
    eventStream.evidence,
  );
  run.efficiency = await fetchRunEfficiencyMetrics(projectId, started.runId);
  run.promptEfficiency = await fetchRunPromptEfficiency(projectId, started.runId);
  run.budgetProfile = await fetchRunBudgetProfile(projectId, started.runId, "build");
  run.generationContextStatus = await fetchGenerationContextStatus(
    projectId,
    started.runId,
  );
  run.designProfileIdentity = await fetchRunDesignProfileIdentity(
    projectId,
    started.runId,
  );
  run.attempt = 1;
  if (run.status === "completed") {
    return {
      evidence: [run],
      staticPreview: extractStaticDraftPreview(eventStream.events),
      candidateVersionId: extractCandidateVersionId(eventStream.events),
    };
  }
  const readyDurableDraft =
    eventStream.events.some(
      (event) =>
        event.type === "tool.completed" && event.tool === "preview.dev_status",
    ) &&
    eventStream.events.some(
      (event) =>
        event.type === "tool.completed" && event.tool === "draft.snapshot_create",
    );
  if (
    draftLifecycleEditCanary &&
    readyDurableDraft &&
    String(run.summary || "").includes("DraftSnapshot conflict")
  ) {
    return {
      evidence: [run],
      staticPreview: extractStaticDraftPreview(eventStream.events),
      canaryRecoveredFromTerminalFailure: true,
    };
  }
  const failure = classifyTerminalRunFailure(run);
  const error = classifiedError(
    failure.classification,
    `build did not complete: ${run.summary || run.status || "unknown"}`,
    failure.resultStatus,
  );
  error.runEvidence = [run];
  throw error;
}

function extractStaticDraftPreview(events) {
  const completed = events.filter((event) => event.type === "tool.completed");
  const preview = completed.findLast((event) => event.tool === "preview.start");
  const snapshot = completed.findLast(
    (event) => event.tool === "draft.snapshot_create",
  );
  // A Dev Preview snapshot also carries previewUrl/snapshotId metadata. It is
  // not a static preview and must be verified through the authoritative
  // DraftPreviewSession so the current ready/durable revision is selected.
  if (!preview || !snapshot) return null;
  const previewUrl =
    snapshot?.metadata?.postToolUseSuccess?.previewUrl ||
    preview?.metadata?.postToolUseSuccess?.url;
  const snapshotId = snapshot?.metadata?.postToolUseSuccess?.snapshotId;
  if (!previewUrl || !snapshotId) return null;
  let leaseId;
  try {
    leaseId = new URL(previewUrl).pathname.split("/").filter(Boolean).at(-1);
  } catch {
    return null;
  }
  if (!leaseId) return null;
  return { leaseId, snapshotId };
}

function extractCandidateVersionId(events) {
  return [...events]
    .reverse()
    .find((event) => event.type === "preview.candidate" && event.versionId)
    ?.versionId || null;
}

function classifyTerminalRunFailure(run) {
  const summary = String(run.summary || "");
  if (summary.includes("Run stopped for no_progress")) {
    return { classification: "no_progress", resultStatus: "failed" };
  }
  if (summary.includes("Run token budget exhausted")) {
    return { classification: "run_token_budget_exhausted", resultStatus: "failed" };
  }
  if (summary.includes("Run watchdog stopped execution: kind=idle")) {
    return { classification: "runtime_idle_timeout", resultStatus: "timeout" };
  }
  if (summary.includes("Run watchdog stopped execution: kind=total")) {
    return { classification: "runtime_total_timeout", resultStatus: "timeout" };
  }
  if (summary.includes("acceptance.repair_exhausted")) {
    return { classification: "acceptance_repair_exhausted", resultStatus: "rejected" };
  }
  if (summary.includes("exhausted the frozen Brief acceptance repair budget")) {
    return { classification: "acceptance_repair_exhausted", resultStatus: "rejected" };
  }
  if (summary.includes("Reached model-turn budget")) {
    return { classification: "model_turn_budget_exhausted", resultStatus: "failed" };
  }
  return { classification: "run_incomplete", resultStatus: "failed" };
}

function isRetryableCaseFailure(error) {
  if (!error) return false;
  if (error.classification === "no_progress") return true;
  if (error.classification !== "run_incomplete") return false;
  const message = String(error.message || "");
  return [
    "code=provider_unavailable",
    "code=provider_response_invalid",
    "code=provider_timeout",
    "code=gateway_draining",
    "code=gateway_storage_unavailable",
    "model gateway turn timed out",
  ].some((marker) => message.includes(marker));
}

async function startRun(projectId, phase, agentProfile, inputContext) {
  const runReservation =
    manifest.budget.perRun.maxInputTokens +
    manifest.budget.perRun.maxOutputTokens;
  if (consumedTokens + runReservation > manifest.budget.totalTokens) {
    throw new Error(`insufficient token budget to start ${phase} for ${projectId}`);
  }
  const token = issuePrincipalToken(projectId);
  const response = await fetchWithTimeout(
    new URL("/runs", baseUrl),
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${token}`,
      },
      body: JSON.stringify({
        projectId,
        phase,
        agentProfile,
        inputContext: {
          ...inputContext,
          modelResourceId: manifest.provider.modelResourceId,
        },
      }),
    },
    120_000,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`start ${phase} failed ${response.status}: ${body}`);
  }
  return JSON.parse(body);
}

async function readRunEvents(projectId, runId, phase) {
  const token = issuePrincipalToken(projectId);
  const eventFile = path.join(
    suiteDirectory,
    `run-${phase}-${runId}.events.ndjson`,
  );
  const eventFileDescriptor = fs.openSync(eventFile, "wx", 0o600);
  const events = [];
  const controller = new AbortController();
  let timeoutClassification = null;
  let idleTimer;
  const totalTimer = setTimeout(() => {
    timeoutClassification = "total_timeout";
    controller.abort();
  }, runTimeoutMs);
  const resetIdleTimer = () => {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(() => {
      timeoutClassification = "idle_timeout";
      controller.abort();
    }, runIdleTimeoutMs);
  };
  resetIdleTimer();

  let caughtError = null;
  try {
    const response = await fetch(
      new URL(`/runs/${encodeURIComponent(runId)}/events`, baseUrl),
      {
        headers: { authorization: `Bearer ${token}` },
        signal: controller.signal,
      },
    );
    if (!response.ok) {
      const body = await response.text();
      throw classifiedError(
        "event_stream_http_error",
        `run events failed ${response.status}: ${body.slice(0, 500)}`,
      );
    }
    if (!response.body) {
      throw classifiedError(
        "event_stream_body_missing",
        `run ${runId} event response has no body`,
      );
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let terminalSeen = false;
    while (!terminalSeen) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      while (true) {
        const newline = buffer.indexOf("\n");
        if (newline < 0) break;
        const rawLine = buffer.slice(0, newline).replace(/\r$/, "");
        buffer = buffer.slice(newline + 1);
        if (!rawLine.startsWith("data:")) continue;
        const payload = rawLine.slice(5).trimStart();
        if (!payload) continue;
        let event;
        try {
          event = JSON.parse(payload);
        } catch {
          throw classifiedError(
            "event_stream_invalid_json",
            `run ${runId} emitted invalid SSE JSON`,
          );
        }
        event = sanitizeModelExecutionEvent(event);
        events.push(event);
        resetIdleTimer();
        fs.writeSync(
          eventFileDescriptor,
          `${JSON.stringify(sanitizePersistedRuntimeEvent(event))}\n`,
        );
        terminalSeen = event.type === "run.completed";
        if (terminalSeen) {
          await reader.cancel();
          break;
        }
      }
    }

    const terminal = [...events]
      .reverse()
      .find((event) => event.type === "run.completed");
    if (!terminal) {
      throw classifiedError(
        "terminal_event_missing",
        `run ${runId} did not emit a terminal event: ${JSON.stringify(events.at(-1) || {})}`,
      );
    }
  } catch (error) {
    if (timeoutClassification) {
      try {
        await cancelTimedOutRun(projectId, runId);
      } catch {
        // Preserve the timeout as the primary failure; cleanup is audited separately.
      }
      caughtError = classifiedError(
        timeoutClassification,
        `run ${runId} reached ${timeoutClassification === "idle_timeout" ? `${runIdleTimeoutMs}ms idle` : `${runTimeoutMs}ms total`} timeout and was cancelled`,
        "timeout",
      );
    } else {
      caughtError = error;
    }
  } finally {
    clearTimeout(totalTimer);
    clearTimeout(idleTimer);
    fs.closeSync(eventFileDescriptor);
  }

  const evidence = eventStreamEvidence(eventFile, events.length);
  if (caughtError) {
    caughtError.partialEvents = events;
    caughtError.eventEvidence = evidence;
    throw caughtError;
  }
  return { events, evidence };
}

async function cancelTimedOutRun(projectId, runId) {
  const token = issuePrincipalToken(projectId);
  const response = await fetchWithTimeout(
    new URL(`/runs/${encodeURIComponent(runId)}/cancel`, baseUrl),
    {
      method: "POST",
      headers: { authorization: `Bearer ${token}` },
    },
    120_000,
  );
  if (!response.ok) {
    throw new Error(
      `timed-out run cancellation failed ${response.status}: ${(await response.text()).slice(0, 500)}`,
    );
  }
}

async function fetchRunEfficiencyMetrics(projectId, runId) {
  const response = await fetchWithTimeout(
    new URL(`/runs/${encodeURIComponent(runId)}/efficiency-metrics`, baseUrl),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken(projectId)}` },
    },
    120_000,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(
      `run efficiency metrics returned ${response.status}: ${body.slice(0, 500)}`,
    );
  }
  const metrics = JSON.parse(body);
  if (
    metrics.schemaVersion !== "run-efficiency-metrics@1" ||
    metrics.calculatorVersion !== "run-efficiency-calculator@1" ||
    metrics.runId !== runId ||
    metrics.projectId !== projectId
  ) {
    throw new Error("run efficiency metrics identity or schema mismatch");
  }
  return metrics;
}

async function fetchRunPromptEfficiency(projectId, runId) {
  const response = await fetchWithTimeout(
    new URL(`/runs/${encodeURIComponent(runId)}/prompt-efficiency`, baseUrl),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken(projectId)}` },
    },
    120_000,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(
      `run prompt efficiency returned ${response.status}: ${body.slice(0, 500)}`,
    );
  }
  const metrics = JSON.parse(body);
  if (
    metrics.schemaVersion !== "run-prompt-efficiency@1" ||
    metrics.runId !== runId
  ) {
    throw new Error("run prompt efficiency identity or schema mismatch");
  }
  return metrics;
}

function summarizeRun(phase, runId, events, eventEvidence = null) {
  const terminal = [...events]
    .reverse()
    .find((event) => event.type === "run.completed");
  const usageEvents = events.filter((event) => event.type === "model.usage");
  const usage = usageEvents.reduce(
    (total, event) => {
      total.inputTokens += Number(event.inputTokens || 0);
      total.outputTokens += Number(event.outputTokens || 0);
      total.cachedInputTokens += Number(event.cachedInputTokens || 0);
      total.estimated = total.estimated || event.estimated === true;
      return total;
    },
    {
      inputTokens: 0,
      outputTokens: 0,
      cachedInputTokens: 0,
      estimated: false,
    },
  );
  usage.totalTokens = usage.inputTokens + usage.outputTokens;
  const terminalClassification = terminal?.status === "completed"
    ? "completed"
    : classifyTerminalRunFailure({ summary: terminal?.summary }).classification;
  return {
    phase,
    runId,
    status: terminal?.status || "unknown",
    summary: terminal?.summary || null,
    terminalClassification,
    buildEvidence: extractBuildEvidence(events),
    usage,
    modelExecutions: events
      .filter((event) => event.type === "model.execution")
      .map((event) => event.snapshot),
    promptCompositions: events
      .filter((event) => event.type === "prompt.composition")
      .map((event) => ({
        turn: event.turn,
        staticPrefixHash: event.staticPrefixHash,
        toolSetHashVersion: event.toolSetHashVersion,
        toolSetHash: event.toolSetHash,
        estimatedInputTokens: event.estimatedInputTokens,
      })),
    turns: usageEvents.length,
    toolCalls: events.filter((event) => event.type === "tool.started").length,
    terminalToolFailures: events.filter(
      (event) =>
        event.type === "tool.failed" && event.recoverable !== true,
    ).length,
    eventStream: eventEvidence,
  };
}

function attachPartialRunEvidence(error, phase, runId) {
  if (!Array.isArray(error?.partialEvents)) return;
  error.runEvidence = [
    summarizeRun(
      phase,
      runId,
      error.partialEvents,
      error.eventEvidence || null,
    ),
  ];
}

function addUsage(usage) {
  consumedInputTokens += usage.inputTokens;
  consumedOutputTokens += usage.outputTokens;
  consumedCachedInputTokens += usage.cachedInputTokens;
  consumedTokens += usage.totalTokens;
}

function assertActualBudget() {
  if (consumedTokens > suiteTokenCeiling) {
    throw new Error(
      `actual token usage ${consumedTokens} exceeds suite maximum ${suiteTokenCeiling}`,
    );
  }
}

function assertRunReservation(phase, caseId) {
  const remainingTokens = suiteTokenCeiling - consumedTokens;
  if (remainingTokens >= perRunSafetyCeiling) return;
  throw classifiedError(
    "suite_budget_reservation_exhausted",
    `cannot start ${phase} for ${caseId}: remaining=${remainingTokens}, required reservation=${perRunSafetyCeiling}, suite ceiling=${suiteTokenCeiling}`,
    "failed",
  );
}

function issuePrincipalToken(projectId) {
  const publicDer = crypto
    .createPublicKey(privateKey)
    .export({ type: "spki", format: "der" });
  const kid = `ed25519-${sha256(publicDer).slice(0, 16)}`;
  const now = Math.floor(Date.now() / 1_000);
  const encode = (value) =>
    Buffer.from(JSON.stringify(value)).toString("base64url");
  const header = encode({ alg: "EdDSA", typ: "JWT", kid });
  const payload = encode({
    iss: "anydesign-bff",
    aud: "anydesign-runtime-public",
    sub: principalId,
    jti: crypto.randomBytes(16).toString("hex"),
    iat: now,
    exp: now + 120,
    projectId,
    operations: ["preview.read", "project.read", "project.write"],
  });
  const input = `${header}.${payload}`;
  return `${input}.${crypto.sign(null, Buffer.from(input), privateKey).toString("base64url")}`;
}

async function releaseSandboxOnce(projectId) {
  try {
    const response = await fetchWithTimeout(
      new URL(
        `/internal/projects/${encodeURIComponent(projectId)}/release-sandbox`,
        baseUrl,
      ),
      {
        method: "POST",
        headers: {
          "x-anydesign-internal": "true",
          "x-runtime-admin-token": adminToken,
        },
      },
      120_000,
    );
    await response.text();
    return {
      ok: response.ok,
      status: response.status,
      error: response.ok ? null : `http_${response.status}`,
    };
  } catch (error) {
    return {
      ok: false,
      status: null,
      error: error?.name || "release_request_failed",
    };
  }
}

async function releaseSandboxWithRetry(projectId) {
  return confirmSandboxRelease({
    requestRelease: () => releaseSandboxOnce(projectId),
    maxAttempts: sandboxReleaseMaxAttempts,
    retryCooldownMs: sandboxReleaseRetryCooldownMs,
    requiredSuccessfulResponses: 2,
    delay,
  });
}

function sanitizeReleaseEvidence(evidence) {
  return {
    schemaVersion: evidence.schemaVersion,
    projectId: evidence.projectId,
    currentVersionId: evidence.currentVersionId,
    artifactManifestHash: evidence.artifactManifestHash,
    previewLeaseId: evidence.previewLeaseId,
    terminalToolFailureCount: evidence.terminalToolFailureCount,
    validation: evidence.validation,
    designProfileFidelity: evidence.designProfileFidelity,
  };
}

function acceptanceProbe(testCase) {
  const contract = {
    schemaVersion: "legacy-external-acceptance@1",
    locale: testCase.locale,
    artifactType: testCase.kind,
    requiredRoute: testCase.expectedRoute,
    requiredText: testCase.expectedText,
  };
  return {
    contract,
    sha256: sha256(JSON.stringify(contract)),
    runtimeEnforced: false,
  };
}

function eventStreamEvidence(file, eventCount) {
  const bytes = fs.readFileSync(file);
  return {
    schemaVersion: "generation-run-event-stream@1",
    path: path.relative(suiteDirectory, file),
    format: "ndjson",
    eventCount,
    bytes: bytes.byteLength,
    sha256: sha256(bytes),
    incremental: true,
  };
}

function classifiedError(classification, message, resultStatus = "failed") {
  const error = new Error(message);
  error.classification = classification;
  error.resultStatus = resultStatus;
  return error;
}

async function fetchWithTimeout(url, options, timeoutMs) {
  return fetch(url, {
    ...options,
    signal: AbortSignal.timeout(timeoutMs),
  });
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
