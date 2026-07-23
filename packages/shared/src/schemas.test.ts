import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import path from "node:path";
import {
  AgentRunSchema,
  AgentRunStatusSchema,
  BriefStatusSchema,
  BriefSchema,
  ConversationItemSchema,
  DesignProfileSchema,
  DesignSourceArtifactSchema,
  DraftPreviewSessionSchema,
  DraftSnapshotSchema,
  EditBaseSchema,
  EditImpactPlanSchema,
  ElementObservationSchema,
  HistoryItemSchema,
  ModelVisionCapabilitySchema,
  ProjectVersionSchema,
  ProjectAssetSchema,
  ReviewFindingSchema,
  RunVisualBindingSchema,
  SandboxBindingSchema,
  SandboxBindingStatusSchema,
  PublishSourceSchema,
  ToolResultContentBlockSchema,
  VisualArtifactSchema,
  VisualFindingSchema,
  VisualReviewStateSchema,
} from "./schemas.js";
import { AgentEventSchema, DraftPreviewEventSchema } from "./events.js";
import {
  CancelRunResponseSchema,
  BindProjectDesignProfileRequestSchema,
  ConversationListResponseSchema,
  ContinueRunResponseSchema,
  CreateDesignSourceArtifactRequestSchema,
  CreateDesignProfileRequestSchema,
  ImportDesignProfileResponseSchema,
  DesignSourceArtifactResponseSchema,
  DesignProfileDiffResponseSchema,
  DesignProfileResponseSchema,
  DesignProfileVersionsResponseSchema,
  ErrorResponseSchema,
  HealthResponseSchema,
  ListDesignProfilesResponseSchema,
  PromotePreviewRequestSchema,
  PromotePreviewResponseSchema,
  PublishWorkflowSchema,
  PreviewCurrentResponseSchema,
  ProjectRuntimeStateResponseSchema,
  ResolvePermissionResponseSchema,
  ProjectDesignProfileResponseSchema,
  StartRunRequestSchema,
  StartRunResponseSchema,
  UpdateDesignProfileRequestSchema,
} from "./api-types.js";

const timestamp = "2026-07-04T00:00:00.000Z";

const runtimeStyleContract = () => ({
  tokenFile: "project/src/styles/tokens.css",
  globalCssFile: "project/src/styles/global.css",
  componentRoot: "project/src/components/ui",
  tailwind: {
    version: "4",
    entryImport: '@import "tailwindcss"',
    themeSource: "css-variables",
  },
  tokens: { "color.primary": "--runtime-primary" },
});

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
    primaryUseCases: ["generate websites", "edit docs"],
    productQualities: ["reliable", "inspectable"],
  },
  brand: {
    voice: {
      tone: ["clear", "precise"],
      sentenceStyle: "technical",
      vocabulary: { prefer: ["runtime", "evidence"], avoid: ["magic"] },
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
  tokens: {
    color: {},
    typography: {},
    radius: {},
    shadow: {},
    spacing: {},
  },
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

describe("shared schemas", () => {
  it("accepts every AgentRun status including partial", () => {
    expect(AgentRunStatusSchema.options).toEqual([
      "queued",
      "running",
      "validating",
      "needs_user_input",
      "completed",
      "partial",
      "blocked",
      "failed",
      "cancelled",
    ]);
    for (const status of AgentRunStatusSchema.options) {
      expect(() =>
        AgentRunSchema.parse({
          id: `run-${status}`,
          projectId: "project-1",
          sessionId: "session-1",
          phase: "brief",
          agentProfile: "brief",
          budgetProfile: {
            schemaVersion: "run-budget-profile@1",
            profileId: "phase-budget-v1-brief",
            phase: "brief",
            rolloutMode: "shadow",
            tokenBudgetMode: "legacy",
            operationBudgetMode: "shadow",
            enforcedLimits: {
              maxTurns: 20,
              maxToolCalls: 60,
              maxInputTokens: 200_000,
              maxGrossInputTokens: 300_000,
              maxUncachedInputTokens: 180_000,
              maxPromptTokensPerTurn: 64_000,
              maxOutputTokens: 40_000,
            },
            phaseTargetLimits: {
              maxTurns: 6,
              maxToolCalls: 60,
              maxInputTokens: 80_000,
              maxGrossInputTokens: 80_000,
              maxUncachedInputTokens: 40_000,
              maxPromptTokensPerTurn: 24_000,
              maxOutputTokens: 40_000,
            },
            operationLimits: {
              maxGrossInputTokens: 450_000,
              maxUncachedInputTokens: 270_000,
              maxOutputTokens: 80_000,
              maxTurns: 30,
              maxToolCalls: 100,
            },
            profileHash: "a".repeat(64),
          },
          status,
          model: "internal-balanced",
          parentRunId: null,
          triggeredByEventId: null,
          sandboxId: null,
          briefVersion: null,
          designVersion: null,
          baseVersionId: null,
          outputVersionId: null,
          findingIds: null,
          inputMessageIds: ["message-1"],
          checkpointId: null,
          profileSnapshot: {
            allowedTools: [],
            deniedTools: [],
            permissionMode: "normal",
            transcriptMode: "main",
            sourceCheckpointId: null,
            mcpServerNames: [],
          },
          startedAt: timestamp,
          updatedAt: timestamp,
          completedAt:
            status === "queued" || status === "running" || status === "needs_user_input"
              ? undefined
              : timestamp,
        }),
      ).not.toThrow();
    }
  });

  it("accepts every SandboxBinding status with the workspace PVC contract", () => {
    for (const status of SandboxBindingStatusSchema.options) {
      const binding = SandboxBindingSchema.parse({
        id: `sandbox-binding-${status}`,
        projectId: "project-1",
        sandboxName: "project-project-1-sandbox-1",
        sandboxClaimName: "project-project-1-sandbox-1",
        workspacePvcName: "workspace-project-project-1-sandbox-1",
        channelServiceName: status === "claiming" ? undefined : "workspace-channel-7f9b",
        warmPoolName: "anydesign-next-app-pool",
        namespace: "anydesign-sandboxes",
        status,
        channelProtocol: "websocket",
        lastSeenAt: timestamp,
      });

      expect(binding.workspacePvcName).toBe("workspace-project-project-1-sandbox-1");
      expect(binding.status).toBe(status);
    }
  });

  it("rejects sandbox bindings without a workspace PVC", () => {
    expect(() =>
      SandboxBindingSchema.parse({
        id: "sandbox-binding-1",
        projectId: "project-1",
        sandboxName: "project-project-1-sandbox-1",
        sandboxClaimName: "project-project-1-sandbox-1",
        warmPoolName: "anydesign-next-app-pool",
        namespace: "anydesign-sandboxes",
        status: "ready",
        channelProtocol: "websocket",
        lastSeenAt: timestamp,
      }),
    ).toThrow();
  });

  it("validates DesignProfile schemas and API contracts", () => {
    const profile = DesignProfileSchema.parse(designProfile());
    expect(profile.runtimeTokenMapping["color.primary"]).toBe("#2563eb");
    expect(profile.schemaVersion).toBe("design-profile@1");
    expect(() =>
      DesignProfileSchema.parse({
        ...designProfile(),
        runtimeTokenMapping: { "color.background": "#fff" },
      }),
    ).toThrow();
    expect(() =>
      DesignProfileSchema.parse({
        ...designProfile(),
        governance: {},
      }),
    ).toThrow();
    expect(() =>
      DesignProfileSchema.parse({
        ...designProfile(),
        schemaVersion: "design-profile@2",
        source: {
          kind: "imported",
          primarySourceArtifactId: "design-source-1",
          sourceHash: "a".repeat(64),
          converterVersion: "design-profile-import@1",
          integrity: "verified",
        },
        signatureRules: [],
      }),
    ).toThrow("required signature rule");

    const importedV2 = DesignProfileSchema.parse({
      ...designProfile(),
      schemaVersion: "design-profile@2",
      source: {
        kind: "imported",
        primarySourceArtifactId: "design-source-1",
        sourceHash: "a".repeat(64),
        converterVersion: "design-profile-import@1",
        integrity: "verified",
      },
      signatureRules: [{
        id: "primary-color",
        category: "color",
        statement: "Primary actions use the specified violet.",
        priority: "required",
        appliesTo: ["website"],
        verification: {
          kind: "token",
          token: "color.primary",
          expected: "#663af3",
          comparator: { kind: "color-equivalent" },
        },
      }],
    });
    expect(importedV2.schemaVersion).toBe("design-profile@2");

    expect(
      ImportDesignProfileResponseSchema.parse({
        designProfileDraft: {
          id: "design-profile-draft-1",
          schemaVersion: "design-profile@2",
          version: 1,
          name: "Imported",
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
          candidate: {},
          conversionReportId: "report-1",
          validationIssues: [],
          createdAt: timestamp,
          updatedAt: timestamp,
        },
        conversionReport: {
          id: "report-1",
          designProfileId: "design-profile-draft-1",
          profileVersion: 1,
          converterVersion: "design-profile-import@1",
          deterministicParserVersion: "markdown-css-parser@1",
          sourceArtifactId: "design-source-1",
          sourceHash: "a".repeat(64),
          extractedSections: [],
          extractedTokenCount: 0,
          extractedComponentCount: 0,
          requiredSignatureRuleCount: 0,
          unmappedItems: [],
          warnings: [],
          createdAt: timestamp,
        },
        requiresReview: true,
      }).designProfileDraft.status,
    ).toBe("draft");

    const createRequest = CreateDesignProfileRequestSchema.parse({
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
    });
    expect(createRequest.projectId).toBe("project-1");
    expect(
      DesignProfileResponseSchema.parse({
        designProfile: designProfile(),
      }).designProfile.id,
    ).toBe("design-profile-1");
    expect(
      ListDesignProfilesResponseSchema.parse({
        designProfiles: [designProfile()],
      }).designProfiles,
    ).toHaveLength(1);
    expect(
      DesignProfileVersionsResponseSchema.parse({
        designProfileId: "design-profile-1",
        versions: [designProfile()],
      }).versions,
    ).toHaveLength(1);
    expect(
      DesignProfileDiffResponseSchema.parse({
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
      }).changes[0]?.path,
    ).toBe("visual.direction");
    expect(
      UpdateDesignProfileRequestSchema.parse({
        name: "Harness Calm Ops v2",
        profile: {
          ...designProfile(),
          id: undefined,
          name: undefined,
          version: undefined,
          createdAt: undefined,
          updatedAt: undefined,
        },
      }).name,
    ).toBe("Harness Calm Ops v2");
    expect(
      BindProjectDesignProfileRequestSchema.parse({
        designProfileId: "design-profile-1",
      }).designProfileId,
    ).toBe("design-profile-1");
    expect(
      ProjectDesignProfileResponseSchema.parse({
        projectId: "project-1",
        designProfile: designProfile(),
      }).designProfile?.id,
    ).toBe("design-profile-1");
  });

  it("validates the AuthKit and ElevenLabs V2 fidelity fixtures", () => {
    for (const fileName of ["authkit-v2.json", "elevenlabs-v2.json"]) {
      const fixturePath = path.resolve(
        process.cwd(),
        "../../services/runtime/fixtures/design-profiles",
        fileName,
      );
      const profile = DesignProfileSchema.parse(
        JSON.parse(readFileSync(fixturePath, "utf8")),
      );
      expect(profile.schemaVersion).toBe("design-profile@2");
      expect(profile.signatureRules.filter((rule) => rule.priority === "required").length).toBeGreaterThanOrEqual(8);
    }
  });

  it("rejects external fidelity routes and incomplete numeric-ratio assertions", () => {
    const fixturePath = path.resolve(
      process.cwd(),
      "../../services/runtime/fixtures/design-profiles/authkit-v2.json",
    );
    const profile = JSON.parse(readFileSync(fixturePath, "utf8"));
    profile.signatureRules[0].verification = {
      kind: "computed-style",
      route: "https://example.com",
      selector: "[data-eyebrow]",
      property: "letter-spacing",
      expected: "0.10",
      comparator: { kind: "numeric-ratio", ratio: 0.1, tolerance: 0.01 },
    };

    expect(DesignProfileSchema.safeParse(profile).success).toBe(false);
    profile.signatureRules[0].verification.route = "/";
    expect(DesignProfileSchema.safeParse(profile).success).toBe(false);
    profile.signatureRules[0].verification.referenceProperty = "font-size";
    expect(DesignProfileSchema.safeParse(profile).success).toBe(true);
  });

  it("validates immutable design source artifact contracts", () => {
    const sha256 = "a".repeat(64);
    const artifact = DesignSourceArtifactSchema.parse({
      id: "design-source-1",
      scope: { projectId: "project-1" },
      fileName: "DESIGN.md",
      mediaType: "text/markdown",
      contentEncoding: "identity",
      sizeBytes: 18,
      sha256,
      createdAt: timestamp,
    });
    expect(artifact.fileName).toBe("DESIGN.md");
    expect(
      CreateDesignSourceArtifactRequestSchema.parse({
        scope: { projectId: "project-1" },
        fileName: "DESIGN.md",
        mediaType: "text/markdown",
        contentBase64: "IyBEZXNpZ24K",
        clientSha256: sha256,
      }).contentBase64,
    ).toBe("IyBEZXNpZ24K");
    expect(
      DesignSourceArtifactResponseSchema.parse({ artifact }).artifact.id,
    ).toBe("design-source-1");
    expect(() =>
      DesignSourceArtifactSchema.parse({
        ...artifact,
        fileName: "../../DESIGN.md",
      }),
    ).toThrow();
  });

  it("round-trips the shared visual runtime contract fixture", () => {
    const fixturePath = path.resolve(
      process.cwd(),
      "../../services/runtime/contracts/visual-runtime-contract-v1.fixture.json",
    );
    const fixture = JSON.parse(readFileSync(fixturePath, "utf8"));

    expect(DraftSnapshotSchema.parse(fixture.draftSnapshot).schemaVersion).toBe(
      "draft-snapshot@1",
    );
    expect(
      fixture.publishSources.map((source: unknown) =>
        PublishSourceSchema.parse(source),
      ),
    ).toHaveLength(2);
    expect(
      fixture.editBases.map((base: unknown) => EditBaseSchema.parse(base)),
    ).toHaveLength(2);
    expect(VisualArtifactSchema.parse(fixture.visualArtifact).schemaVersion).toBe(
      "visual-artifact@1",
    );
    expect(
      fixture.runVisualBindings.map((binding: unknown) =>
        RunVisualBindingSchema.parse(binding),
      ),
    ).toHaveLength(3);
    expect(
      fixture.toolResultContentBlocks.map((block: unknown) =>
        ToolResultContentBlockSchema.parse(block),
      ),
    ).toHaveLength(3);
    expect(
      fixture.modelVisionCapabilities.map((capability: unknown) =>
        ModelVisionCapabilitySchema.parse(capability),
      ),
    ).toHaveLength(2);
    expect(
      fixture.visualReviewStates.map((state: unknown) =>
        VisualReviewStateSchema.parse(state),
      ),
    ).toHaveLength(2);
    expect(
      DraftPreviewSessionSchema.parse(fixture.draftPreviewSession).schemaVersion,
    ).toBe("draft-preview-session@1");
    expect(
      fixture.draftPreviewEvents.map((event: unknown) =>
        DraftPreviewEventSchema.parse(event),
      ),
    ).toHaveLength(2);
    expect(
      ElementObservationSchema.parse(fixture.elementObservation).schemaVersion,
    ).toBe("element-observation@1");
    expect(EditImpactPlanSchema.parse(fixture.editImpactPlan).schemaVersion).toBe(
      "edit-impact-plan@1",
    );
    expect(
      fixture.historyItems.map((item: unknown) => HistoryItemSchema.parse(item)),
    ).toHaveLength(2);
    expect(VisualFindingSchema.parse(fixture.visualFinding).schemaVersion).toBe(
      "visual-finding@1",
    );
    expect(ProjectAssetSchema.parse(fixture.projectAsset).schemaVersion).toBe(
      "project-asset@1",
    );
    expect(PublishWorkflowSchema.parse(fixture.publishWorkflow).schemaVersion).toBe(
      "publish-workflow@1",
    );
  });

  it("rejects an unbounded enabled vision capability", () => {
    expect(
      ModelVisionCapabilitySchema.safeParse({
        visionInput: true,
        supportedImageMediaTypes: [],
        maxImageBytes: 0,
        maxImageCount: 0,
      }).success,
    ).toBe(false);
    expect(
      ModelVisionCapabilitySchema.safeParse({
        visionInput: false,
      }).success,
    ).toBe(true);
  });

  it("validates local ProjectAssets and Draft Edit starts", () => {
    const editBase = {
      kind: "draft" as const,
      snapshotId: "snapshot-1",
      sessionId: "session-1",
      expectedSessionEpoch: 1,
      expectedWorkspaceRevision: 0,
      writerLeaseId: "lease-1",
    };
    expect(
      StartRunRequestSchema.safeParse({
        projectId: "project-1",
        phase: "edit",
        agentProfile: "edit",
        inputContext: {
          editBase,
          editImpactPlanHash: "a".repeat(64),
          sandboxBindingId: "binding-1",
        },
      }).success,
    ).toBe(true);
    expect(
      ProjectAssetSchema.parse({
        schemaVersion: "project-asset@1",
        assetId: "asset-1",
        projectId: "project-1",
        sourceArtifactId: "visual-1",
        source: "upload",
        targetPath: "public/assets/abc-hero.png",
        contentHash: "b".repeat(64),
        license: "user-owned",
        provenance: { origin: "upload" },
        width: 1200,
        height: 800,
        altText: "Hero image",
        createdByRunId: "run-1",
        createdAt: timestamp,
      }).source,
    ).toBe("upload");
  });

  it("validates Brief JSON required by the runtime contract", () => {
    const brief = BriefSchema.parse({
      projectType: "website",
      audience: "enterprise designers",
      contentHierarchy: ["hero", "features"],
      pageStructure: [
        {
          title: "Home",
          purpose: "Explain the product",
          keyContent: ["hero", "proof"],
        },
      ],
      visualDirection: "quiet technical confidence",
      recommendedTemplate: "next-app",
      assumptions: [],
      missingInformation: [],
    });

    expect(brief.recommendedTemplate).toBe("next-app");
  });

  it("accepts Brief lifecycle statuses used by the runtime gate", () => {
    expect(BriefStatusSchema.options).toEqual(["draft", "confirmed", "superseded"]);
  });

  it("accepts all Phase A AgentEvent variants", () => {
    const events = [
      { type: "run.started", runId: "run-1", label: "Brief Agent", timestamp },
      { type: "agent.message", runId: "run-1", text: "Working", timestamp },
      {
        type: "tool.started",
        runId: "run-1",
        tool: "content.read_source",
        summary: "Reading",
        toolUseId: "tool-1",
        timestamp,
      },
      {
        type: "tool.completed",
        runId: "run-1",
        tool: "content.read_source",
        summary: "Done",
        toolUseId: "tool-1",
        metadata: null,
        timestamp,
      },
      {
        type: "tool.output",
        runId: "run-1",
        tool: "package.install",
        toolUseId: "tool-install",
        stream: "stdout",
        text: "added 42 packages",
        timestamp,
      },
      {
        type: "tool.failed",
        runId: "run-1",
        tool: "content.read_source",
        error: "Missing source",
        toolUseId: "tool-1",
        recoverable: true,
        metadata: {
          errorKind: "content.source_missing",
        },
        timestamp,
      },
      {
        type: "workflow.lifecycle_started",
        runId: "run-1",
        driverId: "workflow-driver-1",
        action: "project.build",
        sequence: 1,
        attempt: 1,
        idempotencyKey: "a".repeat(64),
        timestamp,
      },
      {
        type: "workflow.lifecycle_completed",
        runId: "run-1",
        driverId: "workflow-driver-1",
        action: "project.build",
        sequence: 1,
        attempt: 1,
        idempotencyKey: "a".repeat(64),
        outcome: "completed",
        progressEvidence: {
          schemaVersion: "workflow-lifecycle-progress@1",
          isError: false,
          content: { status: "success" },
          metadata: null,
        },
        timestamp,
      },
      {
        type: "workflow.lifecycle_failed",
        runId: "run-1",
        driverId: "workflow-driver-1",
        action: "preview.dev_status",
        sequence: 2,
        attempt: 1,
        idempotencyKey: "b".repeat(64),
        errorKind: "preview.compile_failed",
        recoverable: true,
        diagnosticRef: "state/logs/dev.log",
        sourceSnapshotUri: null,
        sourceHash: null,
        timestamp,
      },
      {
        type: "run.continuation_created",
        runId: "run-2",
        operationId: "operation-1",
        predecessorRunId: "run-1",
        continuationSnapshotId: "continuation-1",
        attempt: 2,
        automatic: true,
        timestamp,
      },
      {
        type: "permission.requested",
        runId: "run-1",
        permissionId: "permission-1",
        tool: "package.install",
        reason: "Needs platform approval",
        timestamp,
      },
      {
        type: "permission.denied",
        runId: "run-1",
        tool: "shell.run",
        reason: "Denied",
        timestamp,
      },
      { type: "state.changed", runId: "run-1", state: "needs_user_input", timestamp },
      { type: "preview.rebuilding", runId: "run-1", previousVersionId: null, timestamp },
      {
        type: "preview.candidate",
        runId: "run-1",
        url: "http://preview.local/candidate",
        versionId: "version-1",
        screenshotId: null,
        timestamp,
      },
      {
        type: "preview.updated",
        runId: "run-1",
        url: "http://preview.local/current",
        versionId: "version-1",
        screenshotId: null,
        timestamp,
      },
      {
        type: "review.finding",
        runId: "run-1",
        findingId: "finding-1",
        severity: "blocking",
        summary: "Blank page",
        timestamp,
      },
      {
        type: "run.completed",
        runId: "run-1",
        status: "completed",
        summary: "Ready",
        timestamp,
      },
    ];

    for (const event of events) {
      expect(() => AgentEventSchema.parse(event)).not.toThrow();
    }
    expect(() =>
      AgentEventSchema.parse({
        type: "tool.failed",
        runId: "run-1",
        tool: "fs.patch",
        error: "oldStr not found",
        toolUseId: "tool-1",
        recoverable: true,
        timestamp,
      }),
    ).toThrow();
    expect(() =>
      AgentEventSchema.parse({
        type: "tool.failed",
        runId: "run-1",
        tool: "shell.run",
        error: "process exited 1",
        toolUseId: "tool-1",
        recoverable: false,
        timestamp,
      }),
    ).not.toThrow();
  });

  it("keeps historical prompt composition events readable without making the hash version implicit", () => {
    const historical = AgentEventSchema.parse({
      type: "prompt.composition",
      runId: "run-legacy",
      turn: 1,
      estimatedInputTokens: 100,
      systemTokens: 10,
      messageTokens: 20,
      toolDefinitionTokens: 30,
      generationContextTokens: 5,
      staticPrefixHash: "a".repeat(64),
      toolSetHash: "b".repeat(64),
      timestamp,
    });

    expect(historical.type).toBe("prompt.composition");
    if (historical.type === "prompt.composition") {
      expect(historical.toolSetHashVersion).toBeUndefined();
    }
  });

  it("validates durable conversation and finding records", () => {
    const runtimeConversationKinds = [
      "user_message",
      "assistant_message",
      "tool_summary",
      "tool_failed",
      "tool_completed",
      "progress",
      "approval_request",
      "permission_requested",
      "permission_resolved",
      "permission_denied",
      "preview_update",
      "review_finding",
      "run_completed",
      "error_summary",
    ] as const;

    for (const kind of runtimeConversationKinds) {
      expect(() =>
        ConversationItemSchema.parse({
          id: `conversation-${kind}`,
          projectId: "project-1",
          runId: "run-1",
          versionId: null,
          checkpointId: null,
          kind,
          role: "assistant",
          text: `${kind} text`,
          metadata: { kind },
          createdAt: timestamp,
        }),
      ).not.toThrow();
    }

    expect(() =>
      ConversationItemSchema.parse({
        id: "conversation-1",
        projectId: "project-1",
        runId: null,
        versionId: null,
        checkpointId: null,
        kind: "preview_update",
        role: null,
        text: "Preview ready",
        metadata: null,
        createdAt: timestamp,
      }),
    ).not.toThrow();

    expect(() =>
      ReviewFindingSchema.parse({
        id: "finding-1",
        projectId: "project-1",
        runId: "run-1",
        versionId: "version-1",
        severity: "blocking",
        category: "visual",
        summary: "Blank page",
        evidence: null,
        repairable: true,
        status: "open",
      }),
    ).not.toThrow();

    expect(() =>
      ProjectVersionSchema.parse({
        id: "version-1",
        projectId: "project-1",
        sourceSnapshotUri: null,
        previewUrl: "http://preview.local/current",
        screenshotUri: null,
        screenshotId: "shot-1",
        status: "promoted",
        createdByRunId: "run-1",
        createdAt: timestamp,
        promotedAt: timestamp,
      }),
    ).not.toThrow();

    expect(() =>
      SandboxBindingSchema.parse({
        id: "sandbox-binding-1",
        projectId: "project-1",
        sandboxName: "project-project-1-sandbox-1",
        sandboxClaimName: "project-project-1-sandbox-1",
        workspacePvcName: "workspace-project-project-1-sandbox-1",
        channelServiceName: "workspace-channel-7f9b",
        warmPoolName: "anydesign-next-app-pool",
        namespace: "anydesign-sandboxes",
        status: "ready",
        channelProtocol: "websocket",
        lastSeenAt: timestamp,
      }),
    ).not.toThrow();
  });

  it("validates runtime API request and response contracts", () => {
    expect(
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "brief",
        agentProfile: "brief",
      }).inputContext,
    ).toEqual({});
    expect(
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "build",
        agentProfile: "build",
        inputContext: {
          briefId: "brief-1",
          designProfileId: "design-profile-1",
          modelResourceId: "deepseek-v4-pro",
          visualBindings: [{
            artifactId: "visual-reference-1",
            role: "reference",
            route: "/",
            viewport: { width: 1440, height: 900, deviceScaleFactor: 1 },
            target: {
              kind: "static-snapshot",
              snapshotId: "draft-snapshot-1",
              sourceHash: "a".repeat(64),
            },
            order: 0,
          }],
        },
      }).inputContext,
    ).toMatchObject({
      briefId: "brief-1",
      designProfileId: "design-profile-1",
      modelResourceId: "deepseek-v4-pro",
      visualBindings: [{ role: "reference", artifactId: "visual-reference-1" }],
    });
    expect(() =>
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "build",
        agentProfile: "build",
        inputContext: {
          briefId: "brief-1",
          visualBindings: [{
            artifactId: "visual-candidate-1",
            role: "candidate",
            route: "/",
            viewport: { width: 1440, height: 900, deviceScaleFactor: 1 },
            target: {
              kind: "static-snapshot",
              snapshotId: "draft-snapshot-1",
              sourceHash: "a".repeat(64),
            },
            order: 0,
          }],
        },
      }),
    ).toThrow();
    expect(() =>
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "build",
        agentProfile: "build",
      }),
    ).toThrow();
    expect(
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "build",
        agentProfile: "build",
        inputContext: {
          parentRunId: "run-parent-1",
        },
      }).inputContext.parentRunId,
    ).toBe("run-parent-1");
    expect(
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "repair",
        agentProfile: "repair",
        inputContext: {
          predecessorRunId: "run-replan-1",
          editBase: {
            kind: "draft",
            snapshotId: "draft-snapshot-1",
            sessionId: "draft-session-1",
            expectedSessionEpoch: 1,
            expectedWorkspaceRevision: 2,
            writerLeaseId: "writer-lease-1",
          },
          editImpactPlanHash: "a".repeat(64),
          sandboxBindingId: "sandbox-binding-1",
        },
      }).inputContext.predecessorRunId,
    ).toBe("run-replan-1");
    expect(() =>
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "repair",
        agentProfile: "repair",
        inputContext: {
          parentRunId: "run-parent-1",
          predecessorRunId: "run-replan-1",
        },
      }),
    ).toThrow();
    expect(
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "edit",
        agentProfile: "edit",
        inputContext: {
          baseVersionId: "version-1",
          sandboxBindingId: "sandbox-binding-1",
        },
      }).inputContext.baseVersionId,
    ).toBe("version-1");
    expect(() =>
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "edit",
        agentProfile: "edit",
        inputContext: {
          sandboxBindingId: "sandbox-binding-1",
        },
      }),
    ).toThrow();
    expect(() =>
      StartRunRequestSchema.parse({
        projectId: "project-1",
        phase: "edit",
        agentProfile: "edit",
        inputContext: {
          baseVersionId: "version-1",
        },
      }),
    ).toThrow();

    expect(StartRunResponseSchema.parse({ runId: "run-1", status: "queued" }).status).toBe(
      "queued",
    );
    expect(
      StartRunResponseSchema.parse({ runId: "run-1", status: "needs_user_input" }).status,
    ).toBe("needs_user_input");
    expect(ContinueRunResponseSchema.parse({ runId: "run-1", status: "running" }).status).toBe(
      "running",
    );
    expect(
      ContinueRunResponseSchema.parse({ runId: "run-1", status: "needs_user_input" }).status,
    ).toBe("needs_user_input");
    expect(CancelRunResponseSchema.parse({ runId: "run-1", status: "cancelled" }).status).toBe(
      "cancelled",
    );
    expect(
      ResolvePermissionResponseSchema.parse({ runId: "run-1", status: "needs_user_input" })
        .status,
    ).toBe("needs_user_input");
    expect(
      ResolvePermissionResponseSchema.parse({ runId: "run-1", status: "blocked" }).status,
    ).toBe("blocked");
    expect(
      ProjectRuntimeStateResponseSchema.parse({
        projectId: "project-1",
        currentVersionId: "version-1",
        sandboxBindingId: "sandbox-binding-1",
        sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        appRoot: "project",
        templateKey: "next-app",
        styleContractPath: "/workspace/state/style-contract.json",
        styleContract: runtimeStyleContract(),
        latestBuild: {
          status: "success",
          sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        },
        dependencyState: { needsRestore: false },
        preview: { status: "running", url: "http://127.0.0.1:4321" },
      }).sandboxBindingId,
    ).toBe("sandbox-binding-1");
    expect(() =>
      ProjectRuntimeStateResponseSchema.parse({
        projectId: "project-1",
        currentVersionId: "version-1",
        sandboxBindingId: "sandbox-binding-1",
        sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        appRoot: "project",
        templateKey: "next-app",
        styleContractPath: "/workspace/state/style-contract.json",
        styleContract: runtimeStyleContract(),
        latestBuild: {
          status: "success",
          sourceSnapshotUri: "runtime://snapshots/project-1/stale-version",
        },
        dependencyState: { needsRestore: false },
        preview: { status: "running", url: "http://127.0.0.1:4321" },
      }),
    ).toThrow();
    expect(() =>
      ProjectRuntimeStateResponseSchema.parse({
        projectId: "project-1",
        currentVersionId: "version-1",
        sandboxBindingId: "sandbox-binding-1",
        sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        appRoot: "project",
        templateKey: "next-app",
        styleContractPath: "/workspace/state/style-contract.json",
        styleContract: {
          tokens: { "color.primary": "--runtime-primary" },
        },
        latestBuild: {
          status: "success",
          sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        },
        dependencyState: { needsRestore: false },
        preview: { status: "running" },
      }),
    ).toThrow();
    expect(() =>
      ProjectRuntimeStateResponseSchema.parse({
        projectId: "project-1",
        currentVersionId: "version-1",
        sandboxBindingId: "sandbox-binding-1",
        sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        appRoot: "project",
        templateKey: "next-app",
        styleContractPath: "/workspace/state/style-contract.json",
        styleContract: {
          ...runtimeStyleContract(),
          tokens: {},
        },
        latestBuild: {
          status: "success",
          sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        },
        dependencyState: { needsRestore: false },
        preview: { status: "running" },
      }),
    ).toThrow();
    expect(() =>
      ProjectRuntimeStateResponseSchema.parse({
        projectId: "project-1",
        currentVersionId: "version-1",
        sandboxBindingId: "sandbox-binding-1",
        sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        appRoot: "project",
        templateKey: "next-app",
        styleContractPath: "/workspace/state/style-contract.json",
        styleContract: {
          ...runtimeStyleContract(),
          tailwind: undefined,
        },
        latestBuild: {
          status: "success",
          sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        },
        dependencyState: { needsRestore: false },
        preview: { status: "running" },
      }),
    ).toThrow();
    expect(() =>
      ProjectRuntimeStateResponseSchema.parse({
        projectId: "project-1",
        currentVersionId: "version-1",
        sandboxBindingId: "sandbox-binding-1",
        sourceSnapshotUri: "runtime://snapshots/project-1/version-1",
        appRoot: "project",
        templateKey: "next-app",
        latestBuild: {},
        dependencyState: { needsRestore: false },
        preview: { status: "running" },
      }),
    ).toThrow();
    expect(
      PreviewCurrentResponseSchema.parse({
        projectId: "project-1",
        versionId: "version-1",
        previewUrl: "http://preview.local/current",
        status: "promoted",
      }).status,
    ).toBe("promoted");
    expect(
      ConversationListResponseSchema.parse({
        projectId: "project-1",
        items: [
          {
            id: "conversation-1",
            projectId: "project-1",
            runId: "run-1",
            kind: "assistant_message",
            role: "assistant",
            text: "Brief is ready.",
            visibility: "user",
            createdAt: timestamp,
          },
        ],
      }).items[0].text,
    ).toBe("Brief is ready.");
    expect(
      PromotePreviewRequestSchema.parse({
        projectId: "project-1",
        runId: "run-1",
        candidateVersionId: "version-1",
      }).gateReport,
    ).toEqual({});
    expect(
      PromotePreviewResponseSchema.parse({
        projectId: "project-1",
        versionId: "version-1",
        previewUrl: "http://preview.local/current",
        status: "promoted",
      }).status,
    ).toBe("promoted");
    expect(HealthResponseSchema.parse({ status: "ready" }).status).toBe("ready");
    expect(HealthResponseSchema.parse({ status: "not_ready" }).status).toBe("not_ready");
    expect(ErrorResponseSchema.parse({ error: "run not found: run-missing" }).error).toContain(
      "run not found",
    );
  });
});
