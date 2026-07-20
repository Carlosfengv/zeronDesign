#!/usr/bin/env node

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

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
const designProfileFixture = (
  process.env.GENERATION_REAL_DESIGN_PROFILE_FIXTURE || ""
).trim();
const draftPreviewAcceptance =
  process.env.GENERATION_REAL_DRAFT_PREVIEW_ACCEPTANCE === "1";

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
  throw new Error("GENERATION_REAL_CASE_LIMIT must be between 1 and 5");
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
const selectedCases =
  requestedCaseIds.length > 0
    ? requestedCaseIds.map((id) => {
        const testCase = manifest.cases.find((candidate) => candidate.id === id);
        if (!testCase) throw new Error(`unknown real-provider case id: ${id}`);
        return testCase;
      })
    : manifest.cases.slice(0, requestedCaseLimit);
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
    startedAt: caseStartedAt,
    finishedAt: null,
    status: "failed",
    acceptance: acceptanceProbe(testCase),
    runs: [],
    attempts: [],
    artifact: null,
    designProfile: null,
    draftPreview: null,
    releaseEvidence: null,
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
    result.releaseEvidence = null;
    result.error = null;

    try {
      await grantProjectAccess(projectId);
      if (designProfileFixture) {
        result.designProfile = configureAndBindDesignProfile(projectId);
      }

      let brief;
      try {
        assertRunReservation("brief", testCase.id);
        brief = await runBrief(projectId, testCase.prompt);
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

      try {
        assertRunReservation("build", testCase.id);
        const build = await runBuild(projectId, brief.briefId);
        result.runs.push(...build.evidence);
        for (const run of build.evidence) addUsage(run.usage);
        assertActualBudget();
      } catch (error) {
        for (const run of error.runEvidence || []) {
          result.runs.push(run);
          addUsage(run.usage);
        }
        assertActualBudget();
        throw error;
      }

      if (draftPreviewAcceptance) {
        result.draftPreview = await verifyDraftPreview(projectId, testCase);
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
      await releaseSandbox(projectId);
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
        error: result.error,
      });
    }

    if (result.status === "accepted") break;
    if (!isRetryableProviderFailure(result.error) || attempt >= maxCaseAttempts) {
      break;
    }
    process.stdout.write(
      `[${caseNumber}/${selectedCases.length}] ${testCase.id}: transient Provider failure; retrying attempt ${attempt + 1}/${maxCaseAttempts} after ${caseRetryCooldownMs}ms\n`,
    );
    await delay(caseRetryCooldownMs);
  }

  result.finishedAt = new Date().toISOString();
  fs.writeFileSync(
    path.join(suiteDirectory, `real-provider-case-${testCase.id}.json`),
    `${JSON.stringify(result, null, 2)}\n`,
  );
  caseResults.push(result);
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
    attemptCount: result.attempts.length,
    attempts: result.attempts,
    runIds: result.runs.map((run) => run.runId),
    totalTokens: result.runs.reduce(
      (total, run) => total + run.usage.totalTokens,
      0,
    ),
    artifact: result.artifact,
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
  if (value.provider?.modelResourceId !== "deepseek-v4-pro") {
    throw new Error("real-provider suite must target deepseek-v4-pro");
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
  if (!Array.isArray(value.cases) || value.cases.length !== 5) {
    throw new Error("real-provider suite must contain exactly five cases");
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

async function verifyDraftPreview(projectId, testCase) {
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

async function runBuild(projectId, briefId) {
  const started = await startRun(projectId, "build", "build", { briefId });
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
  run.attempt = 1;
  if (run.status === "completed") return { evidence: [run] };
  const failure = classifyTerminalRunFailure(run);
  const error = classifiedError(
    failure.classification,
    `build did not complete: ${run.summary || run.status || "unknown"}`,
    failure.resultStatus,
  );
  error.runEvidence = [run];
  throw error;
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

function isRetryableProviderFailure(error) {
  if (!error || error.classification !== "run_incomplete") return false;
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
      body: JSON.stringify({ projectId, phase, agentProfile, inputContext }),
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
        event = sanitizeEvidenceEvent(event);
        events.push(event);
        resetIdleTimer();
        fs.writeSync(eventFileDescriptor, `${JSON.stringify(event)}\n`);
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
  return {
    phase,
    runId,
    status: terminal?.status || "unknown",
    summary: terminal?.summary || null,
    usage,
    modelExecutions: events
      .filter((event) => event.type === "model.execution")
      .map((event) => event.snapshot),
    turns: usageEvents.length,
    toolCalls: events.filter((event) => event.type === "tool.started").length,
    terminalToolFailures: events.filter(
      (event) =>
        event.type === "tool.failed" && event.recoverable !== true,
    ).length,
    eventStream: eventEvidence,
  };
}

function sanitizeEvidenceEvent(event) {
  if (event?.type !== "model.execution" || !event.snapshot) return event;
  const { providerRequestId, ...snapshot } = event.snapshot;
  return {
    ...event,
    snapshot: {
      ...snapshot,
      providerRequestIdPresent:
        typeof providerRequestId === "string" && providerRequestId.length > 0,
    },
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

async function releaseSandbox(projectId, strict = false) {
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
    if (!response.ok && strict) {
      throw new Error(
        `release sandbox failed ${response.status}: ${await response.text()}`,
      );
    }
    return response.ok;
  } catch (error) {
    if (strict) throw error;
    // Best-effort cleanup. The failure is captured by cluster diagnostics.
    return false;
  }
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
