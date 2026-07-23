import { describe, expect, it } from "vitest";
import {
  createRuntimeClient,
  parseRunEventMessage,
  previewProxyHeaders,
  runEventsProxyHeaders,
  runEventsUrl,
  runtimePreviewPath,
  RuntimeApiError,
  subscribeRunEvents,
  type EventSourceLike,
  type RuntimeFetch,
} from "./runtime-client.js";

const timestamp = "2026-07-09T00:00:00.000Z";

const designProfile = () => ({
  id: "design-profile-1",
  name: "Harness Calm Ops",
  status: "active",
  version: 1,
  scope: { projectId: "project-1" },
  source: { kind: "manual" },
  product: {
    name: "AnyDesign Runtime",
    category: "agent harness",
    audience: ["internal builders"],
    primaryUseCases: ["generate websites"],
    productQualities: ["reliable"],
  },
  brand: {
    voice: {
      tone: ["clear"],
      sentenceStyle: "technical",
      vocabulary: { prefer: ["runtime"], avoid: ["magic"] },
      writingRules: ["Use concrete status text."],
    },
    messaging: {
      headlineStyle: "specific",
      bodyStyle: "concise",
      ctaStyle: "verb first",
      proofStyle: "evidence based",
      forbiddenClaims: ["guaranteed"],
    },
  },
  visual: {
    direction: "quiet operational interface",
    principles: ["scan friendly"],
    moodKeywords: ["calm"],
    avoidKeywords: ["flashy"],
    composition: {},
    imagery: {},
    motion: {},
  },
  tokens: { color: {}, typography: {}, radius: {}, shadow: {}, spacing: {} },
  runtimeTokenMapping: {
    "color.background": "#ffffff",
    "color.surface": "#f8fafc",
    "color.surfaceStrong": "#e2e8f0",
    "color.text": "#0f172a",
    "color.muted": "#475569",
    "color.primary": "#2563eb",
    "color.primaryContrast": "#ffffff",
    "color.border": "#cbd5e1",
    "radius.card": "8px",
    "radius.control": "6px",
    "font.sans": "Inter, sans-serif",
    "shadow.soft": "0 1px 2px rgba(15, 23, 42, 0.12)",
  },
  components: {
    primitives: {
      button: { intent: "clear action", usage: ["primary actions"], avoid: ["overuse"] },
      input: { intent: "precise entry", usage: ["forms"], avoid: ["placeholder-only labels"] },
      card: { intent: "group repeated items", usage: ["lists"], avoid: ["nested cards"] },
      badge: { intent: "show status", usage: ["statuses"], avoid: ["decorative noise"] },
    },
  },
  content: {},
  accessibility: {},
  technical: {
    allowedTemplates: ["next-app", "fumadocs-docs"],
    preferredTemplates: { website: "next-app", docs: "fumadocs-docs" },
    cssStrategy: "runtime-style-contract",
    dependencyPolicy: {},
    filePolicy: {
      designProfilePath: "/workspace/inputs/design-profile.json",
      designMarkdownPath: "/workspace/inputs/design.md",
      styleContractPath: "/workspace/state/style-contract.json",
    },
  },
  governance: { conflictBehavior: "ask" },
  createdAt: timestamp,
  updatedAt: timestamp,
});

class MockEventSource implements EventSourceLike {
  static instances: MockEventSource[] = [];
  onmessage: ((event: { data: string; lastEventId?: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  closed = false;

  constructor(
    readonly url: string,
    readonly init?: { withCredentials?: boolean },
  ) {
    MockEventSource.instances.push(this);
  }

  emit(data: unknown, lastEventId = "run-1/1") {
    this.onmessage?.({ data: JSON.stringify(data), lastEventId });
  }

  close() {
    this.closed = true;
  }
}

describe("runtime client", () => {
  it("creates and reads release packaging with idempotency and principal headers", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const release = {
      id: "release-1", projectId: "project-1", versionId: "version-1", runId: "run-1",
      templateId: "next-app", templateVersion: "next-app@1",
      artifactManifestHash: "a".repeat(64), runtimeManifestHash: "b".repeat(64),
      sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
      runtimeProfileId: "static-web-v1", runtimeImageRef: null, runtimeImageDigest: null,
      status: "packaging", createdAt: timestamp, updatedAt: timestamp,
    };
    const packaging = {
      id: "packaging-1", idempotencyKey: "content-key", projectId: "project-1",
      releaseId: "release-1", artifactManifestHash: "a".repeat(64),
      runtimeManifestHash: "b".repeat(64), baseImageDigest: `sha256:${"c".repeat(64)}`,
      packagerVersion: "packager@1", registryRepository: "registry.example/works",
      builtImageDigest: null, pushedImageDigest: null, sbomDigest: null,
      provenanceDigest: null, signatureIdentity: null, signatureDigest: null,
      scanPolicyVersion: "scan@1", scanEvidence: null, status: "prepared", attempts: 0,
      lastError: null, createdAt: timestamp, updatedAt: timestamp,
    };
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return { ok: true, status: 200, async json() { return { release, packaging }; } };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local", fetch: fetchImpl,
      publicPrincipalToken: "principal-token",
    });

    await client.createRelease(
      "project/1", "version/1", { runtimeProfileId: "static-web-v1" }, "publish-click-1",
    );
    await client.getReleasePackaging("packaging/1");

    expect(calls[0]).toEqual({
      url: "http://runtime.local/projects/project%2F1/versions/version%2F1/releases",
      init: {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "idempotency-key": "publish-click-1",
          authorization: "Bearer principal-token",
        },
        body: JSON.stringify({ runtimeProfileId: "static-web-v1" }),
      },
    });
    expect(calls[1]).toEqual({
      url: "http://runtime.local/release-packagings/packaging%2F1",
      init: { headers: { authorization: "Bearer principal-token" } },
    });
  });

  it("sends publication compare-and-swap headers for initial publish and updates", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const operation = {
      schemaVersion: "publish-operation@1",
      id: "operation-1",
      idempotencyKeyHash: "a".repeat(64),
      requestHash: "b".repeat(64),
      projectId: "project-1",
      releaseId: "release-2",
      expectedCurrentReleaseId: null,
      desiredGeneration: 1,
      kind: "publish",
      status: "requested",
      checkpoint: "requested",
      lastError: null,
      createdAt: timestamp,
      updatedAt: timestamp,
    };
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return { ok: true, status: 202, async json() { return { operation }; } };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      publicPrincipalToken: "principal-token",
    });

    await client.publishWork(
      "project-1",
      { releaseId: "release-1", expectedGeneration: 0, runtimeProfileId: "static-web-v1" },
      "publish-1",
    );
    await client.publishWork(
      "project-1",
      {
        releaseId: "release-2",
        expectedCurrentReleaseId: "release-1",
        expectedGeneration: 1,
        runtimeProfileId: "static-web-v1",
      },
      "update-1",
    );

    expect(calls[0]?.init?.headers).toMatchObject({
      "idempotency-key": "publish-1",
      "if-none-match": "*",
      authorization: "Bearer principal-token",
    });
    expect(calls[1]?.init?.headers).toMatchObject({
      "idempotency-key": "update-1",
      "if-match": '"release-1"',
      authorization: "Bearer principal-token",
    });
  });

  it("gets and confirms structured briefs using public principal authorization", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const payload = {
      briefId: "brief-1",
      projectId: "project-1",
      runId: "run-1",
      status: "draft",
      runStatus: "needs_user_input",
      brief: {
        projectType: "website",
        audience: "product teams",
        contentHierarchy: ["hero"],
        pageStructure: [
          { title: "Home", purpose: "Explain the product", keyContent: ["hero"] },
        ],
        visualDirection: "clear editorial",
        recommendedTemplate: "next-app",
        assumptions: [],
        missingInformation: [],
      },
    };
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return { ok: true, status: 200, async json() { return payload; } };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      publicPrincipalToken: "principal-token",
    });

    await client.getBrief("brief/1");
    await client.confirmBrief("brief/1");

    expect(calls).toEqual([
      {
        url: "http://runtime.local/briefs/brief%2F1",
        init: { headers: { authorization: "Bearer principal-token" } },
      },
      {
        url: "http://runtime.local/briefs/brief%2F1/confirm",
        init: {
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization: "Bearer principal-token",
          },
          body: "{}",
        },
      },
    ]);
  });

  it("reads exact Content Plan approval identity and producer state", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const contentHash = "a".repeat(64);
    const approval = {
      schemaVersion: "content-plan-approval@1",
      approvalId: "approval-1",
      projectId: "project/1",
      planId: "plan/1",
      revision: 2,
      contentHash,
      decision: "approved",
      confirmationEventId: "confirmation-2",
      approvedAt: timestamp,
      invalidatedAt: null,
      invalidationReason: null,
    } as const;
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      const target = url.toString();
      if (target.endsWith("content-plan-approval-producer")) {
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              ready: true,
              schemaVersion: "content-plan-approval-producer@1",
              transactionSchemaVersion: "content-plan-approval-transaction@1",
              lastSequence: 2,
            };
          },
        };
      }
      if (target.includes("/verify?")) {
        return {
          ok: true,
          status: 200,
          async json() {
            return { state: "verified", approval, reason: null };
          },
        };
      }
      return { ok: true, status: 200, async json() { return approval; } };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      publicPrincipalToken: "principal-token",
    });

    await client.verifyContentPlanApproval("project/1", {
      planId: "plan/1",
      revision: 2,
      contentHash,
    });
    await client.getContentPlanApprovalProducerStatus("project/1");

    expect(calls.map((call) => call.url)).toEqual([
      `http://runtime.local/projects/project%2F1/content-plan-approvals/verify?planId=plan%2F1&revision=2&contentHash=${contentHash}`,
      "http://runtime.local/projects/project%2F1/content-plan-approval-producer",
    ]);
    for (const call of calls) {
      expect(call.init?.headers).toMatchObject({ authorization: "Bearer principal-token" });
    }
  });

  it("builds BFF preview proxy requests with only the minted principal token", () => {
    expect(runtimePreviewPath("lease/1", "assets/app.js")).toBe(
      "/previews/lease%2F1/assets/app.js",
    );
    expect(
      previewProxyHeaders(
        "principal.jwt.token",
        "/projects/project-1/previews/lease-1",
      ),
    ).toEqual({
      authorization: "Bearer principal.jwt.token",
      "x-anydesign-preview-prefix": "/projects/project-1/previews/lease-1",
    });
    expect(() => previewProxyHeaders(" ", "/projects/project-1/previews/lease-1")).toThrow(
      "preview principal token is required",
    );
  });

  it("upserts project access using internal service authorization", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return {
        ok: true,
        status: 200,
        async json() {
          return {
            projectAccess: {
              projectId: "project-1",
              ownerPrincipalId: "principal-1",
              workspaceNamespace: "ws-one",
              createdAt: timestamp,
              updatedAt: timestamp,
            },
          };
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      internalAdminToken: "runtime-admin-token",
    });

    await client.upsertProjectAccess("project-1", {
      ownerPrincipalId: "principal-1",
      workspaceNamespace: "ws-one",
    });

    expect(calls[0]).toEqual({
      url: "http://runtime.local/internal/projects/project-1/access",
      init: {
        method: "PUT",
        headers: {
          "content-type": "application/json",
          "x-anydesign-internal": "true",
          "x-runtime-admin-token": "runtime-admin-token",
        },
        body: JSON.stringify({
          ownerPrincipalId: "principal-1",
          workspaceNamespace: "ws-one",
        }),
      },
    });
  });

  it("calls runtime JSON endpoints with shared response validation", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return {
        ok: true,
        status: 200,
        async json() {
          return { runId: "run-1", status: "queued" };
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local/",
      fetch: fetchImpl,
    });

    const response = await client.startRun({
      projectId: "project-1",
      phase: "brief",
      agentProfile: "brief",
      inputContext: {},
    });

    expect(response).toEqual({ runId: "run-1", status: "queued" });
    expect(calls).toEqual([
      {
        url: "http://runtime.local/runs",
        init: {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            projectId: "project-1",
            phase: "brief",
            agentProfile: "brief",
            inputContext: {},
          }),
        },
      },
    ]);
  });

  it("reads versioned run efficiency metrics", async () => {
    const calls: string[] = [];
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      publicPrincipalToken: "principal-token",
      fetch: async (url) => {
        calls.push(url.toString());
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              schemaVersion: "run-efficiency-metrics@1",
              calculatorVersion: "run-efficiency-calculator@1",
              runId: "run/1",
              projectId: "project-1",
              phase: "build",
              model: "fixture",
              template: "next-app",
              status: "running",
              totalDurationMs: null,
              timeToFirstModelTurnMs: 5,
              timeToFirstSourceMutationMs: 20,
              modelTurnAtFirstSourceMutation: 1,
              timeToFirstGreenfieldStaticBuildMs: 100,
              coldDevReadyMs: null,
              timeToIframeAppliedMs: null,
              timeToDurableSnapshotMs: null,
              timeToDraftReadyMs: null,
              prebuildFsReadCount: 2,
              prebuildFsListCount: 1,
              prebuildFsSearchCount: 0,
              inputTokens: 500,
              outputTokens: 50,
              cachedInputTokens: 0,
              contextReadDeliveries: 1,
              sourceReadDeliveries: 2,
              diagnosticReadDeliveries: 0,
              verificationReadDeliveries: 0,
              fullReadDeliveries: 3,
              duplicateFullReadDeliveries: 1,
              duplicateFullReadRateBasisPoints: 3334,
              duplicateReadEstimatedTokens: 25,
              outOfScopeMutationCount: 0,
              firstBuildSucceeded: true,
              requiredFidelityPassed: null,
            };
          },
        };
      },
    });

    expect((await client.getRunEfficiencyMetrics("run/1")).inputTokens).toBe(500);
    expect(calls).toEqual(["http://runtime.local/runs/run%2F1/efficiency-metrics"]);
  });

  it("reads the sanitized model service catalog", async () => {
    const calls: string[] = [];
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: async (url) => {
        calls.push(url.toString());
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              items: [{
                id: "model-service-1",
                displayName: "Balanced Model",
                description: "General generation model",
                capabilities: {
                  toolCalls: true,
                  strictToolSchema: false,
                  streaming: true,
                  vision: false,
                  visionInput: false,
                  supportedImageMediaTypes: [],
                  maxImageBytes: 0,
                  maxImageCount: 0,
                },
                availability: "available",
              }],
            };
          },
        };
      },
    });

    const catalog = await client.listModelServices("project/1", "build", "build");
    expect(catalog.items[0]?.displayName).toBe("Balanced Model");
    expect(calls).toEqual([
      "http://runtime.local/projects/project%2F1/model-services?phase=build&agentProfile=build",
    ]);
  });

  it("reads deduplicated known model usage", async () => {
    const calls: string[] = [];
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: async (url) => {
        calls.push(url.toString());
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              schemaVersion: "run-model-usage@1",
              runId: "run/1",
              modelServiceId: "model-service-1",
              modelDisplayName: "Balanced Model",
              inputTokens: 500,
              outputTokens: 50,
              cachedInputTokens: 25,
              totalTokens: 550,
              estimated: false,
              turnCount: 2,
            };
          },
        };
      },
    });

    expect((await client.getRunModelUsage("run/1")).totalTokens).toBe(550);
    expect(calls).toEqual(["http://runtime.local/runs/run%2F1/model-usage"]);
  });

  it("aggregates usage across one generation operation", async () => {
    const calls: string[] = [];
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: async (url) => {
        calls.push(url.toString());
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              schemaVersion: "generation-operation-usage@1",
              projectId: "project/1",
              operationId: "operation/1",
              attempts: [{
                runId: "run/1",
                attempt: 1,
                status: "completed",
                inputTokens: 500,
                outputTokens: 50,
                cachedInputTokens: 25,
                totalTokens: 550,
                turnCount: 2,
                estimated: false,
                startedAt: timestamp,
                completedAt: timestamp,
              }],
              inputTokens: 500,
              uncachedInputTokens: 475,
              outputTokens: 50,
              cachedInputTokens: 25,
              totalTokens: 550,
              turnCount: 2,
              automaticContinuationCount: 0,
              retryAmplificationBasisPoints: null,
              estimated: false,
              startedAt: timestamp,
              completedAt: timestamp,
              latencyMs: 0,
              status: "completed",
            };
          },
        };
      },
    });

    expect(
      (await client.getGenerationOperationUsage("project/1", "operation/1")).totalTokens,
    ).toBe(550);
    expect(calls).toEqual([
      "http://runtime.local/projects/project%2F1/generation-operations/operation%2F1/usage",
    ]);
  });

  it("reads prompt efficiency without exposing prompt content", async () => {
    const calls: string[] = [];
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: async (url) => {
        calls.push(url.toString());
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              schemaVersion: "run-prompt-efficiency@1",
              runId: "run/1",
              grossInputTokens: 500,
              cachedInputTokens: 200,
              uncachedInputTokens: 300,
              outputTokens: 50,
              turnCount: 2,
              maxTurnInputTokens: 300,
              averageTurnInputTokens: 250,
              cacheHitRateBasisPoints: 4000,
              generationContextEstimatedTokens: 20,
              generationContextRepeatedEstimatedTokens: 40,
              promptCompactionCount: 0,
              promptTokensRemovedByCompaction: 0,
              largeToolArgumentTokensRetainedPeak: 0,
              retryAmplificationBasisPoints: null,
              estimated: false,
            };
          },
        };
      },
    });

    expect((await client.getRunPromptEfficiency("run/1")).uncachedInputTokens).toBe(300);
    expect(calls).toEqual(["http://runtime.local/runs/run%2F1/prompt-efficiency"]);
  });

  it("reads hash-only GenerationContext status", async () => {
    const calls: string[] = [];
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: async (url) => {
        calls.push(url.toString());
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              schemaVersion: "generation-context-status@1",
              runId: "run/1",
              runContractVersion: "generation-context@1",
              status: "compiled",
              runtimeMode: "enabled",
              compilerVersion: "generation-context-compiler@1",
              contextContentHash: "a".repeat(64),
              runContextBindingHash: "b".repeat(64),
              runtimeAttestationHash: "d".repeat(64),
              visualBindingSetHash: "e".repeat(64),
              visualDeliveryState: "not_applicable",
              executionProfile: "greenfield_static",
              workflowState: "context_ready",
              contextWindowEpoch: 0,
              contextInjectedTurn: null,
              operationId: "operation-1",
              operationAttempt: 1,
              budgetProfileId: "phase-budget-v1-build",
              budgetProfileHash: "a".repeat(64),
              budgetProfileRolloutMode: "shadow",
              predecessorRunId: null,
              successorRunId: null,
              continuationSnapshotId: null,
              contentPlan: {
                planId: "plan-1",
                revision: 2,
                contentHash: "c".repeat(64),
              },
              approvalId: "approval-1",
              approvalState: "verified",
              designSourceKind: "template_default",
            };
          },
        };
      },
    });

    expect((await client.getRunGenerationContextStatus("run/1")).status).toBe("compiled");
    expect(calls).toEqual([
      "http://runtime.local/runs/run%2F1/generation-context-status",
    ]);
  });

  it("reads the complete frozen Run Budget Profile", async () => {
    const calls: string[] = [];
    const tokenLimits = {
      maxTurns: 16,
      maxToolCalls: 60,
      maxInputTokens: 300000,
      maxGrossInputTokens: 300000,
      maxUncachedInputTokens: 180000,
      maxPromptTokensPerTurn: 64000,
      maxOutputTokens: 40000,
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: async (url) => {
        calls.push(url.toString());
        return {
          ok: true,
          status: 200,
          async json() {
            return {
              schemaVersion: "run-budget-profile@1",
              profileId: "phase-budget-v1-build",
              phase: "build",
              rolloutMode: "enforced",
              tokenBudgetMode: "split_enforced",
              operationBudgetMode: "enforced",
              enforcedLimits: tokenLimits,
              phaseTargetLimits: tokenLimits,
              operationLimits: {
                maxGrossInputTokens: 450000,
                maxUncachedInputTokens: 270000,
                maxOutputTokens: 80000,
                maxTurns: 30,
                maxToolCalls: 100,
              },
              profileHash: "a".repeat(64),
            };
          },
        };
      },
    });

    expect((await client.getRunBudgetProfile("run/1")).phase).toBe("build");
    expect(calls).toEqual(["http://runtime.local/runs/run%2F1/budget-profile"]);
  });

  it("continues and cancels runs with project principal authorization", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return {
        ok: true,
        status: 200,
        async json() {
          return url.toString().endsWith("/cancel")
            ? { runId: "run/1", status: "cancelled" }
            : { runId: "run/1", status: "running" };
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      publicPrincipalToken: "project.jwt.token",
    });

    await client.continueRun("run/1", { userMessage: "Use the darker direction" });
    await client.cancelRun("run/1");

    expect(calls).toEqual([
      {
        url: "http://runtime.local/runs/run%2F1/continue",
        init: {
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization: "Bearer project.jwt.token",
          },
          body: JSON.stringify({ userMessage: "Use the darker direction" }),
        },
      },
      {
        url: "http://runtime.local/runs/run%2F1/cancel",
        init: {
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization: "Bearer project.jwt.token",
          },
          body: "{}",
        },
      },
    ]);
  });

  it("forwards the project principal to protected JSON and SSE proxy requests", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      publicPrincipalToken: "project.jwt.token",
      fetch: async (url, init) => {
        calls.push({ url: url.toString(), init });
        return {
          ok: true,
          status: 200,
          async json() {
            return { projectId: "project-1", items: [] };
          },
        };
      },
    });

    await client.getConversation("project-1");
    await client.getProjectHistory("project-1");

    expect(calls[0].init?.headers).toEqual({
      authorization: "Bearer project.jwt.token",
    });
    expect(calls[1]).toEqual({
      url: "http://runtime.local/projects/project-1/history",
      init: {
        headers: { authorization: "Bearer project.jwt.token" },
      },
    });
    expect(client.runEventsProxyHeaders("run-1/3")).toEqual({
      authorization: "Bearer project.jwt.token",
      "last-event-id": "run-1/3",
    });
  });

  it("surfaces runtime error responses", async () => {
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: async () => ({
        ok: false,
        status: 404,
        async json() {
          return { error: "run not found: run-missing" };
        },
      }),
    });

    await expect(client.health()).rejects.toMatchObject({
      name: "RuntimeApiError",
      status: 404,
      message: "run not found: run-missing",
    } satisfies Partial<RuntimeApiError>);
  });

  it("calls DesignProfile endpoints with shared validation", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      const profile = designProfile();
      return {
        ok: true,
        status: 200,
        async json() {
          if (url.toString().includes("/versions")) {
            return { designProfileId: "design-profile-1", versions: [profile] };
          }
          if (url.toString().includes("/diff")) {
            return {
              designProfileId: "design-profile-1",
              fromVersion: 1,
              toVersion: 2,
              changes: [
                {
                  path: "visual.direction",
                  before: "clean enterprise calm",
                  after: "editorial enterprise calm",
                },
              ],
            };
          }
          if (url.toString().includes("includeArchived=true")) {
            return { designProfiles: [profile] };
          }
          if (url.toString().includes("/projects/")) {
            return { projectId: "project-1", designProfile: profile };
          }
          return { designProfile: profile };
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local/",
      fetch: fetchImpl,
    });
    const createPayload = {
      projectId: "project-1",
      name: "Harness Calm Ops",
      profile: {
        ...designProfile(),
        id: undefined,
        name: undefined,
        version: undefined,
        createdAt: undefined,
        updatedAt: undefined,
      },
    } as Parameters<typeof client.createDesignProfile>[0];

    expect((await client.createDesignProfile(createPayload)).designProfile.id).toBe(
      "design-profile-1",
    );
    expect((await client.getDesignProfile("design-profile-1")).designProfile.id).toBe(
      "design-profile-1",
    );
    expect((await client.getDesignProfileVersions("design-profile-1")).versions).toHaveLength(1);
    expect(
      (
        await client.diffDesignProfileVersions("design-profile-1", {
          fromVersion: 1,
          toVersion: 2,
        })
      ).changes[0]?.path,
    ).toBe("visual.direction");
    expect(
      (
        await client.listDesignProfiles({
          projectId: "project-1",
          includeArchived: true,
        })
      ).designProfiles,
    ).toHaveLength(1);
    expect(
      (
        await client.updateDesignProfile("design-profile-1", {
          name: "Harness Calm Ops v2",
          profile: createPayload.profile,
        })
      ).designProfile.id,
    ).toBe("design-profile-1");
    expect((await client.archiveDesignProfile("design-profile-1")).designProfile.id).toBe(
      "design-profile-1",
    );
    expect(
      (
        await client.bindProjectDesignProfile("project-1", {
          designProfileId: "design-profile-1",
        })
      ).designProfile?.id,
    ).toBe("design-profile-1");
    expect((await client.getProjectDesignProfile("project-1")).designProfile?.id).toBe(
      "design-profile-1",
    );

    expect(calls.map((call) => call.url)).toEqual([
      "http://runtime.local/design-profiles",
      "http://runtime.local/design-profiles/design-profile-1",
      "http://runtime.local/design-profiles/design-profile-1/versions",
      "http://runtime.local/design-profiles/design-profile-1/diff?fromVersion=1&toVersion=2",
      "http://runtime.local/design-profiles?projectId=project-1&includeArchived=true",
      "http://runtime.local/design-profiles/design-profile-1",
      "http://runtime.local/design-profiles/design-profile-1/archive",
      "http://runtime.local/projects/project-1/design-profile",
      "http://runtime.local/projects/project-1/design-profile",
    ]);
    expect(calls[5].init?.method).toBe("PUT");
    expect(calls[6].init?.method).toBe("POST");
  });

  it("uses service authorization for immutable design source endpoints", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const content = new TextEncoder().encode("# Design\n");
    const artifact = {
      id: "design-source-1",
      scope: { projectId: "project-1" },
      fileName: "DESIGN.md",
      mediaType: "text/markdown",
      contentEncoding: "identity",
      sizeBytes: content.byteLength,
      sha256: "a".repeat(64),
      createdAt: timestamp,
    };
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return {
        ok: true,
        status: 200,
        async json() {
          return { artifact };
        },
        async arrayBuffer() {
          return content.buffer.slice(
            content.byteOffset,
            content.byteOffset + content.byteLength,
          ) as ArrayBuffer;
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      internalAdminToken: "runtime-secret",
    });

    expect(
      (
        await client.createDesignSourceArtifact({
          scope: { projectId: "project-1" },
          fileName: "DESIGN.md",
          mediaType: "text/markdown",
          contentBase64: "IyBEZXNpZ24K",
          clientSha256: "a".repeat(64),
        })
      ).artifact.id,
    ).toBe("design-source-1");
    expect((await client.getDesignSourceArtifact("design-source-1")).artifact.id).toBe(
      "design-source-1",
    );
    expect(new TextDecoder().decode(await client.getDesignSourceArtifactContent("design-source-1")))
      .toBe("# Design\n");

    for (const call of calls) {
      expect(call.init?.headers).toMatchObject({
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": "runtime-secret",
      });
    }
    expect(calls[0]?.init?.method).toBe("POST");
  });

  it("imports, reviews, and activates design profile drafts through service endpoints", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const report = {
      id: "conversion-report-1",
      designProfileId: "design-profile-draft-1",
      profileVersion: 1,
      converterVersion: "design-profile-import@1",
      deterministicParserVersion: "markdown-css-parser@1",
      sourceArtifactId: "design-source-1",
      sourceHash: "a".repeat(64),
      extractedSections: ["Design"],
      extractedTokenCount: 1,
      extractedComponentCount: 0,
      requiredSignatureRuleCount: 0,
      unmappedItems: [],
      warnings: ["review required"],
      createdAt: timestamp,
    };
    const draft = {
      id: "design-profile-draft-1",
      schemaVersion: "design-profile@2",
      version: 1,
      name: "Imported Design",
      status: "draft",
      scope: { projectId: "project-1" },
      source: {
        kind: "imported",
        sourceArtifactIds: ["design-source-1"],
        primarySourceArtifactId: "design-source-1",
        sourceHash: "a".repeat(64),
        converterVersion: "design-profile-import@1",
        importedAt: timestamp,
        integrity: "verified",
      },
      candidate: { tokens: { color: { primary: "#663af3" } } },
      conversionReportId: "conversion-report-1",
      validationIssues: [
        {
          path: "product",
          code: "required",
          message: "product is required before activation",
          blocking: true,
        },
      ],
      createdAt: timestamp,
      updatedAt: timestamp,
    } as const;
    const fidelityReport = {
      designProfileId: "design-profile-draft-1",
      version: 3,
      schemaVersion: "design-profile@2",
      surface: "website",
      template: "next-app",
      styleContractVersion: "runtime-style-contract@p3",
      effectiveProfileHash: "b".repeat(64),
      sourceIntegrity: "verified",
      sourceHashMatches: true,
      requiredSignatureRuleIds: ["primary-color"],
      capsuleIncludedRuleIds: ["primary-color"],
      capsuleMissingRuleIds: [],
      unsupportedExtendedTokens: [],
      warnings: [],
    };
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return {
        ok: true,
        status: 200,
        async json() {
          if (url.toString().endsWith("/import")) {
            return { designProfileDraft: draft, conversionReport: report, requiresReview: true };
          }
          if (url.toString().includes("fidelity-report")) return fidelityReport;
          if (url.toString().includes("conversion-report")) return report;
          return { designProfile: designProfile() };
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      internalAdminToken: "runtime-secret",
    });

    expect(
      (
        await client.importDesignProfile({
          name: "Imported Design",
          scope: { projectId: "project-1" },
          sourceArtifactId: "design-source-1",
        })
      ).designProfileDraft.status,
    ).toBe("draft");
    expect(
      (await client.getDesignProfileConversionReport("design-profile-draft-1", 1)).id,
    ).toBe("conversion-report-1");
    expect(
      (
        await client.activateDesignProfile("design-profile-draft-1", {
          expectedVersion: 2,
        })
      ).designProfile.status,
    ).toBe("active");
    expect(
      (
        await client.getDesignProfileFidelityReport("design-profile-draft-1", 3, {
          surface: "website",
          template: "next-app",
        })
      ).capsuleMissingRuleIds,
    ).toEqual([]);

    expect(calls.map((call) => call.url)).toEqual([
      "http://runtime.local/design-profiles/import",
      "http://runtime.local/design-profiles/design-profile-draft-1/versions/1/conversion-report",
      "http://runtime.local/design-profiles/design-profile-draft-1/activate",
      "http://runtime.local/design-profiles/design-profile-draft-1/versions/3/fidelity-report?surface=website&template=next-app",
    ]);
    for (const call of calls.slice(0, 3)) {
      expect(call.init?.headers).toMatchObject({
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": "runtime-secret",
      });
    }
  });

  it("builds SSE URLs and proxy headers for browser-to-BFF reconnects", () => {
    expect(runEventsUrl("http://bff.local/api", "run/with space")).toBe(
      "http://bff.local/api/runs/run%2Fwith%20space/events",
    );
    expect(runEventsUrl("http://bff.local/api", "run-1", "run-1/3")).toBe(
      "http://bff.local/api/runs/run-1/events?lastEventId=run-1%2F3",
    );
    expect(runEventsProxyHeaders("run-1/3")).toEqual({
      "last-event-id": "run-1/3",
    });
    expect(runEventsProxyHeaders()).toEqual({});
    expect(runEventsProxyHeaders(undefined, "principal.jwt.token")).toEqual({
      authorization: "Bearer principal.jwt.token",
    });
  });

  it("uses protected DCP diagnostics and Profile Sync control-plane routes", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const hash = "a".repeat(64);
    const packageSummary = {
      version: "design-context@1",
      contentHash: hash,
      artifactManifestHash: "b".repeat(64),
      compilerVersion: "design-context-compiler@1",
      briefHash: "c".repeat(64),
      expectedAppRoot: "project",
      declaredEnforcementMode: "enforced",
      effectiveCompatibilityMode: "enforced",
      enforcementPolicy: {
        source: "persistent",
        enabled: true,
        policyRevision: 3,
        policyUpdatedBy: "operator-1",
      },
      verificationPolicyId: "website-verification@1",
      warnings: [],
      surface: "website",
      template: "next-app",
      designProfileId: "profile-1",
      designProfileVersion: 2,
      effectiveProfileHash: "d".repeat(64),
    } as const;
    const operation = {
      operationId: "profile-sync-1",
      status: "planned",
      expiresAt: timestamp,
      planHash: "e".repeat(64),
      sourceContentHash: hash,
      targetDesignProfileId: "profile-1",
      targetDesignProfileVersion: 2,
      targetEffectiveProfileHash: "d".repeat(64),
      styleContractIdentity: {
        hash: "f".repeat(64), version: "runtime-style-contract@p3",
        template: "next-app", appRoot: "project", tokenMappings: { "color.primary": "--primary" },
      },
      snapshots: { baseHash: hash, currentHash: "b".repeat(64), targetHash: "c".repeat(64) },
      items: [], conflictDecisions: {}, childRunId: null,
    } as const;
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      const target = url.toString();
      if (target.endsWith("design-context-manifest")) {
        return { ok: true, status: 200, async json() { return { runId: "run-1", package: packageSummary, artifacts: [] }; } };
      }
      if (target.endsWith("design-context-diagnostics")) {
        return {
          ok: true, status: 200,
          async json() {
            return {
              runId: "run-1", package: packageSummary, requiredReads: [], readFiles: [],
              missingRequiredReads: [], gate: "ready", materialization: { hash: "m", ready: true },
              styleContract: { verified: null },
              verification: { policyId: "website-verification@1", registryVersion: "runtime-verifier-registry@1", capabilities: {} },
              fidelity: {
                status: "failed", checkedAt: "2026-07-15T00:00:00Z", outputVersionId: "version-1",
                requiredFailedRuleIds: ["craft:responsive-layout:no-horizontal-overflow:375"],
                assertions: [{
                  ruleId: "craft:responsive-layout:no-horizontal-overflow:375", recipeId: "responsive-layout",
                  priority: "required", kind: "viewport", route: "/", viewport: 375,
                  selector: "html", property: "scrollWidth", actualSummary: "420px",
                  expectedSummary: "375px", comparator: "less-than-or-equal", passed: false,
                  reason: "Page width exceeds the required mobile viewport.",
                }],
                repairContext: { targets: ["project/src/styles/global.css"], instructions: ["Repair imported source."] },
              },
            };
          },
        };
      }
      return { ok: true, status: 200, async json() { return operation; } };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local", fetch: fetchImpl, publicPrincipalToken: "principal-token",
    });

    await client.getRunDesignContextManifest("run/1");
    const diagnostics = await client.getRunDesignContextDiagnostics("run/1");
    expect(diagnostics.fidelity?.assertions[0]).toMatchObject({
      ruleId: "craft:responsive-layout:no-horizontal-overflow:375",
      viewport: 375,
      actualSummary: "420px",
      expectedSummary: "375px",
    });
    expect(diagnostics.package.enforcementPolicy).toEqual({
      source: "persistent",
      enabled: true,
      policyRevision: 3,
      policyUpdatedBy: "operator-1",
    });
    await client.planDesignProfileSync("run/1", {
      targetDesignProfileId: "profile-1", targetDesignProfileVersion: 2,
      targetEffectiveProfileHash: "d".repeat(64), expectedSourceContentHash: hash,
      idempotencyKey: "plan-key",
    });
    await client.confirmDesignProfileSync("run/1", "profile-sync/1", {
      planHash: "e".repeat(64), conflictDecisions: {}, idempotencyKey: "confirm-key",
    });

    expect(calls.map((call) => call.url)).toEqual([
      "http://runtime.local/runs/run%2F1/design-context-manifest",
      "http://runtime.local/runs/run%2F1/design-context-diagnostics",
      "http://runtime.local/runs/run%2F1/design-profile-sync-plan",
      "http://runtime.local/runs/run%2F1/design-profile-sync-operations/profile-sync%2F1/confirm",
    ]);
    for (const call of calls) {
      expect(call.init?.headers).toMatchObject({ authorization: "Bearer principal-token" });
    }
  });

  it("parses run event messages with the shared AgentEvent schema", () => {
    const message = parseRunEventMessage(
      JSON.stringify({
        type: "agent.message",
        runId: "run-1",
        text: "Working",
        timestamp,
      }),
      "run-1/2",
    );

    expect(message).toEqual({
      id: "run-1/2",
      event: {
        type: "agent.message",
        runId: "run-1",
        text: "Working",
        timestamp,
      },
    });
  });

  it("uses ProjectAsset routes", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const asset = {
      schemaVersion: "project-asset@1",
      assetId: "asset-1",
      projectId: "project-1",
      sourceArtifactId: "visual-1",
      source: "upload",
      targetPath: "public/assets/abc-hero.png",
      contentHash: "c".repeat(64),
      license: "user-owned",
      provenance: { origin: "upload" },
      width: 1200,
      height: 800,
      altText: "Hero",
      createdByRunId: "run-1",
      createdAt: timestamp,
    } as const;
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      const target = url.toString();
      if (target.endsWith("/assets")) {
        return { ok: true, status: 200, async json() { return [asset]; } };
      }
      if (target.endsWith("/assets/asset-1")) {
        return { ok: true, status: 200, async json() { return asset; } };
      }
      return { ok: false, status: 404, async json() { return {}; } };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      publicPrincipalToken: "principal-token",
    });

    await client.listProjectAssets("project-1");
    await client.getProjectAsset("project-1", "asset-1");

    expect(calls.map((call) => call.url)).toEqual([
      "http://runtime.local/projects/project-1/assets",
      "http://runtime.local/projects/project-1/assets/asset-1",
    ]);
  });

  it("subscribes with EventSource and closes on terminal run.completed", () => {
    MockEventSource.instances = [];
    const received: unknown[] = [];
    const subscription = subscribeRunEvents("http://bff.local/api", {
      runId: "run-1",
      lastEventId: "run-1/2",
      withCredentials: true,
      EventSource: MockEventSource,
      onEvent: (message) => received.push(message),
    });
    const source = MockEventSource.instances[0];

    expect(source.url).toBe("http://bff.local/api/runs/run-1/events?lastEventId=run-1%2F2");
    expect(source.init).toEqual({ withCredentials: true });

    source.emit(
      {
        type: "run.completed",
        runId: "run-1",
        status: "completed",
        summary: "done",
        timestamp,
      },
      "run-1/3",
    );

    expect(received).toHaveLength(1);
    expect(source.closed).toBe(true);
    subscription.close();
    expect(source.closed).toBe(true);
  });
});
