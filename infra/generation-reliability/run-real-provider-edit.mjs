#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import {
  extractBuildEvidence,
  redactEvidenceObject,
  sanitizeModelExecutionEvent,
  sanitizePersistedRuntimeEvent,
} from "./runtime-evidence-redaction.mjs";
import {
  attachSandboxReleaseEvidence,
  confirmSandboxRelease,
} from "./sandbox-release-confirmation.mjs";

const [baseUrl, privateKeyFile, adminTokenFile, projectId, prompt, evidenceRoot] =
  process.argv.slice(2);
if (!baseUrl || !privateKeyFile || !adminTokenFile || !projectId || !prompt || !evidenceRoot) {
  throw new Error(
    "usage: run-real-provider-edit.mjs <base-url> <private-key> <admin-token-file> <project-id> <prompt> <evidence-root>",
  );
}

const principalId = "generation-real-provider-suite";
const workspaceNamespace = (process.env.GENERATION_REAL_WORKSPACE_NAMESPACE || "").trim();
const configuredBaseVersionId = (process.env.GENERATION_REAL_BASE_VERSION_ID || "").trim();
const nextAppDraftMode = process.env.GENERATION_REAL_NEXT_APP_DRAFT === "true";
const draftWarmEditMode = process.env.GENERATION_REAL_DRAFT_WARM_EDIT === "true";
const draftColdDevEditMode =
  process.env.GENERATION_REAL_DRAFT_COLD_DEV_EDIT === "true";
if (draftWarmEditMode && draftColdDevEditMode) {
  throw new Error("Draft Warm Edit and Cold Dev Edit modes are mutually exclusive");
}
const draftLifecycleEditMode = draftWarmEditMode || draftColdDevEditMode;
const draftWarmEditKind = (
  process.env.GENERATION_REAL_DRAFT_WARM_EDIT_KIND || "copy_css"
).trim();
const expectedDraftText = (
  process.env.GENERATION_REAL_EXPECTED_DRAFT_TEXT || "WARM-HMR-VERIFIED"
).trim();
const artifactKind = (process.env.GENERATION_REAL_ARTIFACT_KIND || "website").trim();
const expectedArtifactText = (
  process.env.GENERATION_REAL_EXPECTED_ARTIFACT_TEXT || ""
).trim();
if (!new Set(["website", "docs"]).has(artifactKind)) {
  throw new Error("GENERATION_REAL_ARTIFACT_KIND must be website or docs");
}
if (artifactKind === "docs" && !expectedArtifactText) {
  throw new Error("Docs Edit requires GENERATION_REAL_EXPECTED_ARTIFACT_TEXT");
}
if (
  draftWarmEditMode &&
  !new Set(["copy_css", "structural"]).has(draftWarmEditKind)
) {
  throw new Error(
    "GENERATION_REAL_DRAFT_WARM_EDIT_KIND must be copy_css or structural",
  );
}
const contentPlan = process.env.GENERATION_REAL_CONTENT_PLAN_JSON
  ? JSON.parse(process.env.GENERATION_REAL_CONTENT_PLAN_JSON)
  : null;
const draftBriefId = (process.env.GENERATION_REAL_BRIEF_ID || "").trim();
const keepSandbox = process.env.GENERATION_REAL_KEEP_SANDBOX === "true";
const existingRunId = (process.env.GENERATION_REAL_EXISTING_RUN_ID || "").trim();
const sandboxReleaseMaxAttempts = Number.parseInt(
  process.env.GENERATION_REAL_SANDBOX_RELEASE_MAX_ATTEMPTS || "4",
  10,
);
const sandboxReleaseRetryCooldownMs = Number.parseInt(
  process.env.GENERATION_REAL_SANDBOX_RELEASE_RETRY_COOLDOWN_MS || "1000",
  10,
);
if (!Number.isSafeInteger(sandboxReleaseMaxAttempts) || sandboxReleaseMaxAttempts < 2) {
  throw new Error("GENERATION_REAL_SANDBOX_RELEASE_MAX_ATTEMPTS must be an integer >= 2");
}
if (!Number.isSafeInteger(sandboxReleaseRetryCooldownMs) || sandboxReleaseRetryCooldownMs < 0) {
  throw new Error("GENERATION_REAL_SANDBOX_RELEASE_RETRY_COOLDOWN_MS must be an integer >= 0");
}
const modelResourceId = (
  process.env.GENERATION_REAL_MODEL_RESOURCE_ID || "deepseek-v4-pro"
).trim();
if (!/^[a-z0-9][a-z0-9._-]{0,127}$/.test(modelResourceId)) {
  throw new Error("GENERATION_REAL_MODEL_RESOURCE_ID is invalid");
}
const nonVisualReferenceMode =
  process.env.GENERATION_REAL_NONVISUAL_REFERENCE === "true";
const multimodalReferenceMode =
  process.env.GENERATION_REAL_MULTIMODAL_REFERENCE === "true";
const expectNonVisualUnavailable =
  process.env.GENERATION_REAL_EXPECT_NONVISUAL_UNAVAILABLE === "true";
const expectMultimodalDelivered =
  process.env.GENERATION_REAL_EXPECT_MULTIMODAL_DELIVERED === "true";
if (nonVisualReferenceMode && multimodalReferenceMode) {
  throw new Error("Non-visual and multimodal reference modes are mutually exclusive");
}
if (expectNonVisualUnavailable && !nonVisualReferenceMode) {
  throw new Error(
    "GENERATION_REAL_EXPECT_NONVISUAL_UNAVAILABLE requires GENERATION_REAL_NONVISUAL_REFERENCE",
  );
}
if (expectMultimodalDelivered && !multimodalReferenceMode) {
  throw new Error(
    "GENERATION_REAL_EXPECT_MULTIMODAL_DELIVERED requires GENERATION_REAL_MULTIMODAL_REFERENCE",
  );
}
const visualReferenceMode = nonVisualReferenceMode || multimodalReferenceMode;
if (!/^ws-[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(workspaceNamespace)) {
  throw new Error("GENERATION_REAL_WORKSPACE_NAMESPACE must be a valid ws-* namespace");
}
const privateKey = crypto.createPrivateKey(fs.readFileSync(privateKeyFile));
const adminToken = fs.readFileSync(adminTokenFile, "utf8").trim();
const startedAt = new Date().toISOString();
const editId = startedAt.replace(/[-:.TZ]/g, "");
const evidenceDirectory = path.resolve(evidenceRoot, `edit-${editId}-running`);
const eventFile = path.join(evidenceDirectory, "run-edit.events.ndjson");
fs.mkdirSync(evidenceDirectory, { recursive: true, mode: 0o700 });

let runId = null;
let result = null;
let editRunEvidence = null;
try {
  const draftSession = draftLifecycleEditMode ? await getDraftPreview() : null;
  let before;
  let inputContext;
  let editImpactPlan = null;
  if (draftSession) {
    if (!contentPlan) {
      throw new Error("Draft lifecycle Edit requires GENERATION_REAL_CONTENT_PLAN_JSON");
    }
    if (!draftBriefId) {
      throw new Error("Draft lifecycle Edit requires GENERATION_REAL_BRIEF_ID");
    }
    if (
      draftSession.status !== "ready" ||
      draftSession.lastReadyRevision < draftSession.workspaceRevision ||
      draftSession.durableRevision < draftSession.workspaceRevision
    ) {
      throw new Error(
        `Draft lifecycle Edit requires a ready durable session: ${JSON.stringify(draftSession)}`,
      );
    }
    const editBase = {
      kind: "draft",
      snapshotId: draftSession.durableSnapshotId,
      sessionId: draftSession.sessionId,
      expectedSessionEpoch: draftSession.sessionEpoch,
      expectedWorkspaceRevision: draftSession.workspaceRevision,
      writerLeaseId: draftSession.writerLeaseId,
    };
    editImpactPlan = await createDraftEditImpactPlan(editBase);
    inputContext = {
      editBase,
      editImpactPlanHash: editImpactPlan.planHash,
      sandboxBindingId: draftSession.sandboxBindingId,
      modelResourceId,
      briefId: draftBriefId,
      contentPlan,
    };
    before = {
      currentVersionId: null,
      sandboxBindingId: draftSession.sandboxBindingId,
      draftSnapshotId: draftSession.durableSnapshotId,
      workspaceRevision: draftSession.workspaceRevision,
    };
    if (visualReferenceMode) {
      const reference = await createVisualReference(
        draftSession.durableSnapshotId,
        nonVisualReferenceMode ? "nonvisual-provider-canary" : "multimodal-provider-canary",
      );
      inputContext.visualBindings = [reference.binding];
      before.visualReferenceArtifactId = reference.artifactId;
      before.visualReferenceSha256 = reference.sha256;
      before.visualReferenceMediaType = reference.mediaType;
    }
  } else {
    try {
      before = await getRuntimeState();
    } catch (error) {
      if (!configuredBaseVersionId) throw error;
      before = { currentVersionId: configuredBaseVersionId, sandboxBindingId: null };
    }
    if (!before.currentVersionId) {
      throw new Error("existing project has no editable currentVersionId");
    }

    // A published project may retain the identity of a released binding in its
    // runtime-state snapshot. Omit it so StartRun provisions a fresh Sandbox and
    // restores the immutable base Version into that workspace.
    inputContext = {
      baseVersionId: before.currentVersionId,
      modelResourceId,
      ...(contentPlan ? { contentPlan } : {}),
    };
  }

  if (existingRunId) {
    runId = existingRunId;
  } else {
    const started = await signedJson("/runs", {
      method: "POST",
      body: {
        projectId,
        phase: "edit",
        agentProfile: "edit",
        inputContext,
      },
    });
    runId = started.runId;
    if (!runId) throw new Error("Edit start response has no runId");

    await signedJson(`/runs/${encodeURIComponent(runId)}/continue`, {
      method: "POST",
      body: { userMessage: prompt },
    });
  }
  const stream = await readRunEvents(runId, eventFile);
  editRunEvidence = summarizeRun(runId, stream.events, stream.evidence);
  editRunEvidence.efficiency = await fetchRunEfficiencyMetrics(runId);
  editRunEvidence.promptEfficiency = await fetchRunPromptEfficiency(runId);
  editRunEvidence.budgetProfile = await fetchRunBudgetProfile(runId);
  if (editRunEvidence.status !== "completed") {
    throw new Error(
      `Edit did not complete: ${editRunEvidence.summary || editRunEvidence.status}`,
    );
  }
  const generationContextStatus = await signedJson(
    `/runs/${encodeURIComponent(runId)}/generation-context-status`,
  );
  editRunEvidence.generationContextStatus = generationContextStatus;
  editRunEvidence.designProfileIdentity = await fetchRunDesignProfileIdentity(runId);
  const visualUnavailableMetricRecorded = stream.events.some(
    (event) =>
      event.type === "metric.recorded" &&
      event.name === "generation_context_visual_delivery_unavailable_total" &&
      Number(event.value) === 1,
  );
  const visualBindingsVerified =
    /^[a-f0-9]{64}$/.test(generationContextStatus?.visualBindingSetHash || "") &&
    /^[a-f0-9]{64}$/.test(generationContextStatus?.runtimeAttestationHash || "");
  const acceptedVisualExecution = editRunEvidence.modelExecutions.find(
    (execution) =>
      execution.modelResourceId === modelResourceId &&
      execution.visualInput?.state === "verified_and_provider_accepted" &&
      Number(execution.visualInput?.imageCount) >= 1 &&
      execution.visualInput?.artifactSha256s?.includes(before.visualReferenceSha256) &&
      execution.visualInput?.mediaTypes?.includes(before.visualReferenceMediaType),
  );
  if (
    expectNonVisualUnavailable &&
    (!visualBindingsVerified ||
      generationContextStatus?.visualDeliveryState !== "unavailable" ||
      !visualUnavailableMetricRecorded)
  ) {
    throw new Error(
      `non-visual fallback evidence is incomplete: ${JSON.stringify({
        visualBindingSetHash: generationContextStatus?.visualBindingSetHash,
        runtimeAttestationHash: generationContextStatus?.runtimeAttestationHash,
        visualBindingsVerified,
        visualDeliveryState: generationContextStatus?.visualDeliveryState,
        visualUnavailableMetricRecorded,
      })}`,
    );
  }
  if (
    expectMultimodalDelivered &&
    (!visualBindingsVerified ||
      generationContextStatus?.visualDeliveryState !== "delivered" ||
      visualUnavailableMetricRecorded ||
      !acceptedVisualExecution)
  ) {
    throw new Error(
      `multimodal delivery evidence is incomplete: ${JSON.stringify({
        visualBindingSetHash: generationContextStatus?.visualBindingSetHash,
        runtimeAttestationHash: generationContextStatus?.runtimeAttestationHash,
        visualBindingsVerified,
        visualDeliveryState: generationContextStatus?.visualDeliveryState,
        visualUnavailableMetricRecorded,
        gatewayVisualInputAttested: Boolean(acceptedVisualExecution),
      })}`,
    );
  }

  const draftPreview = draftLifecycleEditMode
    ? await verifyDraftPreview(runId)
    : nextAppDraftMode
    ? existingRunId
      ? {
          status: "previously_verified",
          runId,
          note: "The completed Run's Draft route and headline were observed before Sandbox cleanup; preview base-path rewriting is regression-tested and icon availability is rechecked on the published Artifact.",
        }
      : await verifyDraftPreview(runId)
    : null;
  const publishWorkflow = !draftLifecycleEditMode && nextAppDraftMode
    ? await publishLatestDraft(runId)
    : null;

  const after = draftLifecycleEditMode
    ? { currentVersionId: null }
    : nextAppDraftMode
    ? { currentVersionId: publishWorkflow.versionId }
    : await getRuntimeState();
  if (
    !draftLifecycleEditMode &&
    (!after.currentVersionId || after.currentVersionId === before.currentVersionId)
  ) {
    throw new Error("Edit completed without promoting a new Version");
  }
  const artifact = draftLifecycleEditMode ? null : await readArtifact(after.currentVersionId);
  const releaseEvidence = draftLifecycleEditMode
    ? {
        available: false,
        reason: "draft-lifecycle-edit-does-not-promote-current-version",
      }
    : nextAppDraftMode
    ? {
        available: true,
        source: "publish-workflow",
        versionId: publishWorkflow.versionId,
        releaseId: publishWorkflow.releaseId,
        operationId: publishWorkflow.operationId,
        publicUrl: publishWorkflow.publicUrl,
      }
    : await readReleaseEvidence();
  result = {
    schemaVersion: "generation-real-provider-edit-evidence@2",
    startedAt,
    finishedAt: new Date().toISOString(),
    status: "accepted",
    projectId,
    workspaceNamespace,
    promptSha256: sha256(prompt),
    promptBytes: Buffer.byteLength(prompt),
    baseVersionId: before.currentVersionId,
    versionId: after.currentVersionId,
    baseDraftSnapshotId: before.draftSnapshotId || null,
    editImpactPlanHash: editImpactPlan?.planHash || null,
    warmEditKind: draftWarmEditMode ? draftWarmEditKind : null,
    lifecycleProfile: draftColdDevEditMode ? "cold_dev" : draftWarmEditMode ? "warm_hmr" : null,
    run: editRunEvidence,
    draftPreview,
    publishWorkflow,
    artifact,
    releaseEvidence,
    visualDelivery: visualReferenceMode
      ? {
          state: generationContextStatus.visualDeliveryState,
          visualBindingsVerified,
          visualBindingSetHash: generationContextStatus.visualBindingSetHash,
          runtimeAttestationHash: generationContextStatus.runtimeAttestationHash,
          bindingVerificationSource:
            nonVisualReferenceMode
              ? "frozen-runtime-attestation-plus-unavailable-delivery-metric"
              : "frozen-runtime-attestation-plus-gateway-visual-input-attestation",
          unavailableMetricRecorded: visualUnavailableMetricRecorded,
          gatewayVisualInputAttested: Boolean(acceptedVisualExecution),
          gatewayAcceptedImageCount:
            acceptedVisualExecution?.visualInput?.imageCount || 0,
          referenceArtifactSha256: before.visualReferenceSha256,
          referenceMediaType: before.visualReferenceMediaType,
          mainTaskCompleted: editRunEvidence.status === "completed",
          providerModelResourceId: modelResourceId,
          providerVisionCapable: multimodalReferenceMode,
          referenceArtifactId: before.visualReferenceArtifactId,
        }
      : null,
    hmrMetricRecorded: stream.events.some(
      (event) =>
        event.type === "metric.recorded" &&
        event.name === "efficiency.time_to_iframe_applied_ms",
    ),
    coldDevMetricRecorded: stream.events.some(
      (event) =>
        event.type === "metric.recorded" &&
        event.name === "efficiency.cold_dev_ready_ms",
    ),
    durableSnapshotMetricRecorded: stream.events.some(
      (event) =>
        event.type === "metric.recorded" &&
        event.name === "efficiency.time_to_durable_snapshot_ms",
    ),
    providerVerified:
      editRunEvidence.modelExecutions.length > 0 &&
      editRunEvidence.modelExecutions.every(
        (execution) =>
          execution.modelResourceId === modelResourceId &&
          execution.providerRequestIdPresent === true,
      ),
    secretMaterialPersisted: false,
  };
  if (!result.providerVerified) {
    throw new Error(`Edit evidence does not prove real ${modelResourceId} execution`);
  }
  if (draftWarmEditMode && !result.hmrMetricRecorded) {
    throw new Error("Draft Warm Edit completed without recording the HMR iframe metric");
  }
  if (
    draftColdDevEditMode &&
    (!result.coldDevMetricRecorded || !result.durableSnapshotMetricRecorded)
  ) {
    throw new Error(
      "Draft Cold Dev Edit completed without recording Cold Dev ready and durable snapshot metrics",
    );
  }
} catch (error) {
  result = {
    schemaVersion: "generation-real-provider-edit-evidence@2",
    startedAt,
    finishedAt: new Date().toISOString(),
    status: "failed",
    projectId,
    promptSha256: sha256(prompt),
    promptBytes: Buffer.byteLength(prompt),
    runId,
    warmEditKind: draftWarmEditMode ? draftWarmEditKind : null,
    lifecycleProfile: draftColdDevEditMode ? "cold_dev" : draftWarmEditMode ? "warm_hmr" : null,
    run: editRunEvidence,
    error: { name: error?.name || "Error", message: String(error?.message || error) },
    secretMaterialPersisted: false,
  };
} finally {
  const sandboxRelease = existingRunId || keepSandbox
    ? {
        required: false,
        released: false,
        reason: existingRunId ? "existing-run-owned-by-caller" : "keep-sandbox-requested",
        attempts: [],
        maxAttempts: sandboxReleaseMaxAttempts,
        requiredSuccessfulResponses: 2,
      }
    : await releaseSandboxWithRetry();
  result = attachSandboxReleaseEvidence(result, sandboxRelease);
}

const finalDirectory = evidenceDirectory.replace(
  /-running$/,
  result.status === "accepted" ? "-accepted" : "-failed",
);
fs.writeFileSync(
  path.join(evidenceDirectory, "real-provider-edit-summary.json"),
  `${JSON.stringify(redactEvidenceObject(result), null, 2)}\n`,
  { mode: 0o600 },
);
fs.renameSync(evidenceDirectory, finalDirectory);
process.stdout.write(`Real Provider Edit ${result.status}: ${finalDirectory}\n`);
if (result.status !== "accepted") process.exitCode = 1;

async function getRuntimeState() {
  return signedJson(`/projects/${encodeURIComponent(projectId)}/runtime-state`);
}

async function fetchRunEfficiencyMetrics(editRunId) {
  const metrics = await signedJson(
    `/runs/${encodeURIComponent(editRunId)}/efficiency-metrics`,
  );
  if (
    metrics.schemaVersion !== "run-efficiency-metrics@1" ||
    metrics.calculatorVersion !== "run-efficiency-calculator@1" ||
    metrics.runId !== editRunId ||
    metrics.projectId !== projectId
  ) {
    throw new Error("run efficiency metrics identity or schema mismatch");
  }
  return metrics;
}

async function fetchRunPromptEfficiency(editRunId) {
  const metrics = await signedJson(
    `/runs/${encodeURIComponent(editRunId)}/prompt-efficiency`,
  );
  if (
    metrics.schemaVersion !== "run-prompt-efficiency@1" ||
    metrics.runId !== editRunId
  ) {
    throw new Error("run prompt efficiency identity or schema mismatch");
  }
  return metrics;
}

async function fetchRunDesignProfileIdentity(editRunId) {
  const manifest = await signedJson(
    `/runs/${encodeURIComponent(editRunId)}/design-context-manifest`,
  );
  if (manifest.runId !== editRunId
    || !/^[a-f0-9]{64}$/.test(manifest.package?.effectiveProfileHash || "")) {
    throw new Error("run Design Profile identity or schema mismatch");
  }
  return {
    schemaVersion: "run-design-profile-identity@1",
    runId: editRunId,
    designProfileId: manifest.package.designProfileId ?? null,
    designProfileVersion: manifest.package.designProfileVersion ?? null,
    effectiveProfileHash: manifest.package.effectiveProfileHash,
  };
}

async function fetchRunBudgetProfile(editRunId) {
  const profile = await signedJson(
    `/runs/${encodeURIComponent(editRunId)}/budget-profile`,
  );
  if (profile.schemaVersion !== "run-budget-profile@1"
    || profile.phase !== "edit"
    || typeof profile.profileId !== "string"
    || !/^[a-f0-9]{64}$/.test(profile.profileHash || "")) {
    throw new Error("run budget profile identity or schema mismatch");
  }
  return profile;
}

async function getDraftPreview() {
  return signedJson(`/projects/${encodeURIComponent(projectId)}/draft-preview`);
}

async function createVisualReference(snapshotId, purpose) {
  const history = await signedJson(`/projects/${encodeURIComponent(projectId)}/history`);
  const snapshot = (history.items || [])
    .filter((item) => item.kind === "draft_snapshot")
    .map((item) => item.snapshot)
    .find((candidate) => candidate.snapshotId === snapshotId);
  if (!snapshot?.sourceHash) {
    throw new Error(`project history has no durable base snapshot: ${snapshotId}`);
  }
  // Opaque 1x1 PNG; evidence stores only the Runtime artifact identity and hash.
  const contentBase64 =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
  const bytes = Buffer.from(contentBase64, "base64");
  const contentSha256 = sha256(bytes);
  const response = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/visual-artifacts`,
    {
      method: "POST",
      body: {
        contentBase64,
        clientSha256: contentSha256,
        originMetadata: {
          purpose: `generation-context-${purpose}`,
          source: "real-provider-suite",
        },
      },
    },
  );
  if (!response.artifact?.id) {
    throw new Error("visual artifact response omitted artifact.id");
  }
  return {
    artifactId: response.artifact.id,
    sha256: contentSha256,
    mediaType: "image/png",
    binding: {
      artifactId: response.artifact.id,
      role: "reference",
      route: "/",
      viewport: { width: 1440, height: 900, deviceScaleFactor: 1 },
      target: {
        kind: "static-snapshot",
        snapshotId,
        sourceHash: snapshot.sourceHash,
      },
      order: 0,
    },
  };
}

async function createDraftEditImpactPlan(editBase) {
  const operations = draftColdDevEditMode
    ? ["dependency", "copy"]
    : draftWarmEditKind === "structural"
      ? ["layout", "component"]
      : ["copy", "style"];
  const targets = draftColdDevEditMode
    ? ["project/app/page.tsx"]
    : ["project/app/page.tsx"];
  let plan = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/edit-impact-plans`,
    {
      method: "POST",
      body: {
        scope: "page",
        targets,
        operations,
        risk:
          draftColdDevEditMode || draftWarmEditKind === "structural"
            ? "medium"
            : "low",
        editBase,
      },
    },
  );
  if (plan.requiresConfirmation) {
    plan = await signedJson(
      `/projects/${encodeURIComponent(projectId)}/edit-impact-plans/${encodeURIComponent(plan.planHash)}/confirm`,
      { method: "POST", body: {} },
    );
  }
  if (!plan.planHash) throw new Error("Draft EditImpactPlan response omitted planHash");
  return plan;
}

async function readArtifact(versionId) {
  const route = artifactKind === "docs" ? "/docs/" : "/";
  const response = await signedFetch(
    `/artifacts/${encodeURIComponent(projectId)}/current${route}`,
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`current Artifact returned ${response.status}: ${body.slice(0, 500)}`);
  }
  if (artifactKind === "docs") {
    if (!body.includes(expectedArtifactText)) {
      throw new Error(`edited Docs Artifact does not contain expected text: ${expectedArtifactText}`);
    }
    return {
      versionId,
      route,
      httpStatus: response.status,
      expectedText: expectedArtifactText,
      expectedTextFound: true,
      bodySha256: sha256(body),
      bodyBytes: Buffer.byteLength(body),
    };
  }
  const navPattern = /<nav(?:\s|>)/i;
  if (!navPattern.test(body)) {
    throw new Error("edited Artifact does not contain a semantic <nav> element");
  }
  if (!body.includes("让 AI 成为可治理的企业生产力")) {
    throw new Error("edited Artifact lost the original required headline");
  }
  const iconHref = declaredIconHref(body);
  if (!iconHref) {
    throw new Error("edited Artifact does not declare an application icon");
  }
  let iconStatus = 200;
  if (!iconHref.startsWith("data:")) {
    const iconUrl = new URL(iconHref, baseUrl);
    const iconPath = iconUrl.pathname.replace(/^\/+/, "");
    const iconResponse = await signedFetch(
      `/artifacts/${encodeURIComponent(projectId)}/current/${iconPath}${iconUrl.search}`,
    );
    iconStatus = iconResponse.status;
    if (!iconResponse.ok) {
      throw new Error(
        `edited Artifact icon returned ${iconResponse.status}: ${(await iconResponse.text()).slice(0, 300)}`,
      );
    }
  }
  return {
    versionId,
    route,
    httpStatus: response.status,
    semanticNavFound: true,
    originalHeadlineFound: true,
    declaredIconHref: iconHref,
    declaredIconHttpStatus: iconStatus,
    bodySha256: sha256(body),
    bodyBytes: Buffer.byteLength(body),
  };
}

async function readReleaseEvidence() {
  const response = await fetch(
    new URL(`/internal/projects/${encodeURIComponent(projectId)}/release-evidence`, baseUrl),
    {
      headers: {
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": adminToken,
      },
      signal: AbortSignal.timeout(120_000),
    },
  );
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`release evidence returned ${response.status}: ${body.slice(0, 500)}`);
  }
  const evidence = JSON.parse(body);
  return {
    available: true,
    schemaVersion: evidence.schemaVersion,
    projectId: evidence.projectId,
    baseVersionId: evidence.baseVersionId,
    currentVersionId: evidence.currentVersionId,
    versionId: evidence.currentVersionId,
    releaseId: evidence.releaseId || null,
    artifactManifestHash: evidence.artifactManifestHash,
    sourceFingerprint: evidence.sourceFingerprint,
    terminalToolFailureCount: evidence.terminalToolFailureCount,
  };
}

async function verifyDraftPreview(editRunId) {
  const deadline = Date.now() + 180_000;
  let session = null;
  while (Date.now() < deadline) {
    const response = await signedFetch(
      `/projects/${encodeURIComponent(projectId)}/draft-preview`,
    );
    if (response.ok) {
      session = await response.json();
      if (
        session.status === "ready" &&
        session.lastReadyRevision >= session.workspaceRevision &&
        session.durableRevision >= session.workspaceRevision
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
    throw new Error(`Draft Preview did not become ready: ${JSON.stringify(session)}`);
  }
  const leaseId = new URL(session.proxyUrl).pathname.split("/").filter(Boolean).at(-1);
  if (!leaseId) throw new Error("Draft Preview proxy URL omitted a lease id");
  const prefix = `/projects/${projectId}/previews/${leaseId}`;
  const response = await signedFetch(`/previews/${encodeURIComponent(leaseId)}/`, {
    headers: { "x-anydesign-preview-prefix": prefix },
  });
  const html = await response.text();
  if (!response.ok) {
    throw new Error(
      `Draft Preview route returned ${response.status}: ${html.slice(0, 500)}`,
    );
  }
  const expectedText = draftLifecycleEditMode
    ? expectedDraftText
    : "让 AI 成为可治理的企业生产力";
  if (!html.includes(expectedText)) {
    throw new Error(`Draft Preview does not contain expected text: ${expectedText}`);
  }
  if (draftLifecycleEditMode) {
    return {
      sessionId: session.sessionId,
      sessionEpoch: session.sessionEpoch,
      leaseId,
      runId: editRunId,
      status: session.status,
      workspaceRevision: session.workspaceRevision,
      lastReadyRevision: session.lastReadyRevision,
      durableRevision: session.durableRevision,
      durableSnapshotId: session.durableSnapshotId,
      httpStatus: response.status,
      expectedText,
      expectedTextFound: true,
    };
  }
  const iconHref = declaredIconHref(html);
  if (!iconHref) {
    throw new Error("Draft Preview HTML does not declare an application icon");
  }
  let iconStatus = 200;
  if (!iconHref.startsWith("data:")) {
    const iconUrl = new URL(iconHref, baseUrl);
    const publicPrefix = `/projects/${projectId}/previews/${leaseId}`;
    const upstreamPrefix = `/previews/${leaseId}`;
    const iconPath = iconUrl.pathname.startsWith(`${publicPrefix}/`)
      ? iconUrl.pathname.slice(publicPrefix.length)
      : iconUrl.pathname.startsWith(`${upstreamPrefix}/`)
        ? iconUrl.pathname.slice(upstreamPrefix.length)
        : iconUrl.pathname;
    const previewPath = `/previews/${encodeURIComponent(leaseId)}/${iconPath.replace(/^\/+/, "")}`;
    const iconResponse = await signedFetch(
      `${previewPath}${iconUrl.search}`,
      { headers: { "x-anydesign-preview-prefix": prefix } },
    );
    iconStatus = iconResponse.status;
    if (!iconResponse.ok) {
      throw new Error(
        `Draft Preview icon returned ${iconResponse.status}: ${(await iconResponse.text()).slice(0, 300)}`,
      );
    }
  }
  return {
    sessionId: session.sessionId,
    sessionEpoch: session.sessionEpoch,
    leaseId,
    runId: editRunId,
    status: session.status,
    workspaceRevision: session.workspaceRevision,
    lastReadyRevision: session.lastReadyRevision,
    durableRevision: session.durableRevision,
    durableSnapshotId: session.durableSnapshotId,
    httpStatus: response.status,
    expectedText,
    expectedTextFound: true,
    declaredIconHref: iconHref,
    declaredIconHttpStatus: iconStatus,
  };
}

async function publishLatestDraft(editRunId) {
  const history = await signedJson(`/projects/${encodeURIComponent(projectId)}/history`);
  const snapshot = (history.items || [])
    .filter((item) => item.kind === "draft_snapshot")
    .map((item) => item.snapshot)
    .find((candidate) => candidate.createdByRunId === editRunId);
  if (!snapshot) {
    throw new Error(`project history has no DraftSnapshot created by ${editRunId}`);
  }
  const existingWorkflows = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/publish-workflows`,
  );
  const completed = (existingWorkflows.workflows || []).find(
    (workflow) =>
      workflow.status === "completed" &&
      workflow.source?.kind === "static-snapshot" &&
      workflow.source?.snapshotId === snapshot.snapshotId,
  );
  if (completed?.publicUrl) {
    return publishWorkflowEvidence(completed, snapshot, []);
  }
  let expectedGeneration = 0;
  let expectedCurrentReleaseId = null;
  const deployment = await signedFetch(
    `/projects/${encodeURIComponent(projectId)}/deployment-state`,
  );
  if (deployment.ok) {
    const state = await deployment.json();
    expectedGeneration = Number(state.runtime?.desiredGeneration || 0);
    expectedCurrentReleaseId = state.runtime?.currentReleaseId || null;
  } else if (deployment.status !== 404) {
    throw new Error(
      `deployment state returned ${deployment.status}: ${(await deployment.text()).slice(0, 500)}`,
    );
  }
  const started = await signedJson(
    `/projects/${encodeURIComponent(projectId)}/publish-workflows`,
    {
      method: "POST",
      body: {
        source: {
          kind: "static-snapshot",
          projectId,
          snapshotId: snapshot.snapshotId,
          expectedSourceHash: snapshot.sourceHash,
        },
        idempotencyKey: `real-next-app-edit-${editRunId}-${expectedGeneration}-${expectedCurrentReleaseId || "initial"}`,
        expectedCurrentReleaseId,
        expectedGeneration,
        visualReviewMode: "advisory",
        runtimeProfileId: "static-web-v1",
      },
    },
  );
  let workflow = started.workflow;
  const observations = [];
  const deadline = Date.now() + 20 * 60_000;
  while (workflow && Date.now() < deadline) {
    const observation = {
      observedAt: new Date().toISOString(),
      status: workflow.status,
      checkpoint: workflow.checkpoint,
      publicUrl: workflow.publicUrl || null,
      error: workflow.error || null,
    };
    const previous = observations.at(-1);
    if (
      !previous ||
      previous.status !== observation.status ||
      previous.checkpoint !== observation.checkpoint
    ) {
      observations.push(observation);
    }
    if (["completed", "failed", "cancelled", "rolled_back", "rollback_failed"].includes(workflow.status)) {
      break;
    }
    await delay(2_000);
    workflow = (
      await signedJson(`/publish-workflows/${encodeURIComponent(workflow.id)}`)
    ).workflow;
  }
  if (workflow?.status !== "completed" || !workflow.publicUrl) {
    throw new Error(
      `PublishWorkflow did not complete: ${JSON.stringify({ workflow, observations }).slice(0, 1500)}`,
    );
  }
  return publishWorkflowEvidence(workflow, snapshot, observations);
}

function publishWorkflowEvidence(workflow, snapshot, observations) {
  return {
    workflowId: workflow.id,
    status: workflow.status,
    checkpoint: workflow.checkpoint,
    versionId: workflow.versionId,
    releaseId: workflow.releaseId,
    operationId: workflow.operationId,
    publicUrl: workflow.publicUrl,
    expectedGeneration: workflow.expectedGeneration,
    expectedCurrentReleaseId: workflow.expectedCurrentReleaseId || null,
    snapshotId: snapshot.snapshotId,
    sourceHash: snapshot.sourceHash,
    observations,
  };
}

function declaredIconHref(html) {
  for (const match of html.matchAll(/<link\b[^>]*>/gi)) {
    const tag = match[0];
    const rel = tag.match(/\brel=["']([^"']+)["']/i)?.[1] || "";
    if (!rel.split(/\s+/).includes("icon")) continue;
    const href = tag.match(/\bhref=["']([^"']+)["']/i)?.[1];
    if (href) return href.replaceAll("&amp;", "&");
  }
  return null;
}

async function releaseSandboxOnce() {
  try {
    const response = await fetch(
      new URL(`/internal/projects/${encodeURIComponent(projectId)}/release-sandbox`, baseUrl),
      {
        method: "POST",
        headers: {
          "x-anydesign-internal": "true",
          "x-runtime-admin-token": adminToken,
        },
        signal: AbortSignal.timeout(120_000),
      },
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

async function releaseSandboxWithRetry() {
  return confirmSandboxRelease({
    requestRelease: releaseSandboxOnce,
    maxAttempts: sandboxReleaseMaxAttempts,
    retryCooldownMs: sandboxReleaseRetryCooldownMs,
    requiredSuccessfulResponses: 2,
    delay,
  });
}

async function signedJson(target, options = {}) {
  const response = await signedFetch(target, options);
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`${options.method || "GET"} ${target} returned ${response.status}: ${body.slice(0, 500)}`);
  }
  return body ? JSON.parse(body) : {};
}

async function signedFetch(target, options = {}) {
  const headers = {
    authorization: `Bearer ${issuePrincipalToken()}`,
    ...(options.body ? { "content-type": "application/json" } : {}),
    ...(options.headers || {}),
  };
  return fetch(new URL(target, baseUrl), {
    ...options,
    headers,
    body: options.body ? JSON.stringify(options.body) : undefined,
    signal: AbortSignal.timeout(options.timeoutMs || 120_000),
  });
}

async function readRunEvents(editRunId, destination) {
  const response = await fetch(
    new URL(`/runs/${encodeURIComponent(editRunId)}/events`, baseUrl),
    {
      headers: { authorization: `Bearer ${issuePrincipalToken()}` },
      signal: AbortSignal.timeout(900_000),
    },
  );
  if (!response.ok || !response.body) {
    throw new Error(`Edit event stream returned ${response.status}`);
  }
  const descriptor = fs.openSync(destination, "wx", 0o600);
  const events = [];
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let terminalSeen = false;
  try {
    while (!terminalSeen) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      while (true) {
        const newline = buffer.indexOf("\n");
        if (newline < 0) break;
        const line = buffer.slice(0, newline).replace(/\r$/, "");
        buffer = buffer.slice(newline + 1);
        if (!line.startsWith("data:")) continue;
        const payload = line.slice(5).trimStart();
        if (!payload) continue;
        const event = sanitizeModelExecutionEvent(JSON.parse(payload));
        events.push(event);
        fs.writeSync(
          descriptor,
          `${JSON.stringify(sanitizePersistedRuntimeEvent(event))}\n`,
        );
        terminalSeen = event.type === "run.completed";
        if (terminalSeen) await reader.cancel();
      }
    }
  } finally {
    fs.closeSync(descriptor);
  }
  if (!terminalSeen) throw new Error("Edit stream ended without run.completed");
  const bytes = fs.readFileSync(destination);
  return {
    events,
    evidence: {
      schemaVersion: "generation-run-event-stream@1",
      path: path.basename(destination),
      format: "ndjson",
      eventCount: events.length,
      bytes: bytes.byteLength,
      sha256: sha256(bytes),
      incremental: true,
    },
  };
}

function summarizeRun(editRunId, events, eventStream) {
  const terminal = [...events].reverse().find((event) => event.type === "run.completed");
  const usageEvents = events.filter((event) => event.type === "model.usage");
  const usage = usageEvents.reduce(
    (total, event) => ({
      inputTokens: total.inputTokens + Number(event.inputTokens || 0),
      outputTokens: total.outputTokens + Number(event.outputTokens || 0),
      cachedInputTokens: total.cachedInputTokens + Number(event.cachedInputTokens || 0),
      estimated: total.estimated || event.estimated === true,
    }),
    { inputTokens: 0, outputTokens: 0, cachedInputTokens: 0, estimated: false },
  );
  usage.totalTokens = usage.inputTokens + usage.outputTokens;
  return {
    phase: "edit",
    runId: editRunId,
    status: terminal?.status || "unknown",
    summary: terminal?.summary || null,
    terminalClassification: terminal?.status === "completed" ? "completed" : "failed",
    buildEvidence: extractBuildEvidence(events),
    usage,
    turns: usageEvents.length,
    toolCalls: events.filter((event) => event.type === "tool.started").length,
    recoverableToolFailures: events.filter((event) => event.type === "tool.failed").length,
    modelExecutions: events
      .filter((event) => event.type === "model.execution")
      .map((event) => event.snapshot),
    eventStream,
  };
}

function issuePrincipalToken() {
  const publicDer = crypto
    .createPublicKey(privateKey)
    .export({ type: "spki", format: "der" });
  const now = Math.floor(Date.now() / 1_000);
  const encode = (value) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const header = encode({ alg: "EdDSA", typ: "JWT", kid: `ed25519-${sha256(publicDer).slice(0, 16)}` });
  const payload = encode({
    iss: "anydesign-bff",
    aud: "anydesign-runtime-public",
    sub: principalId,
    jti: crypto.randomBytes(16).toString("hex"),
    iat: now,
    exp: now + 120,
    projectId,
    operations: [
      "preview.read",
      "project.read",
      "project.write",
      "publication.read",
      "publication.write",
    ],
  });
  const input = `${header}.${payload}`;
  return `${input}.${crypto.sign(null, Buffer.from(input), privateKey).toString("base64url")}`;
}

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
