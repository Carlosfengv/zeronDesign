import { describe, expect, it } from "vitest";
import {
  AgentRunSchema,
  AgentRunStatusSchema,
  BriefStatusSchema,
  BriefSchema,
  ConversationItemSchema,
  ProjectVersionSchema,
  ReviewFindingSchema,
  SandboxBindingSchema,
  SandboxBindingStatusSchema,
} from "./schemas.js";
import { AgentEventSchema } from "./events.js";
import {
  CancelRunResponseSchema,
  ConversationListResponseSchema,
  ContinueRunResponseSchema,
  ErrorResponseSchema,
  HealthResponseSchema,
  PromotePreviewRequestSchema,
  PromotePreviewResponseSchema,
  PreviewCurrentResponseSchema,
  ResolvePermissionResponseSchema,
  StartRunRequestSchema,
  StartRunResponseSchema,
} from "./api-types.js";

const timestamp = "2026-07-04T00:00:00.000Z";

describe("shared schemas", () => {
  it("accepts every AgentRun status including partial", () => {
    for (const status of AgentRunStatusSchema.options) {
      expect(() =>
        AgentRunSchema.parse({
          id: `run-${status}`,
          projectId: "project-1",
          sessionId: "session-1",
          phase: "brief",
          agentProfile: "brief",
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
            mcpServerNames: ["figma"],
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
        warmPoolName: "anydesign-astro-website-pool",
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
        warmPoolName: "anydesign-astro-website-pool",
        namespace: "anydesign-sandboxes",
        status: "ready",
        channelProtocol: "websocket",
        lastSeenAt: timestamp,
      }),
    ).toThrow();
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
      recommendedTemplate: "astro-website",
      assumptions: [],
      missingInformation: [],
    });

    expect(brief.recommendedTemplate).toBe("astro-website");
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
        type: "tool.failed",
        runId: "run-1",
        tool: "content.read_source",
        error: "Missing source",
        toolUseId: "tool-1",
        recoverable: true,
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
        warmPoolName: "anydesign-astro-website-pool",
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

    expect(StartRunResponseSchema.parse({ runId: "run-1", status: "queued" }).status).toBe(
      "queued",
    );
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
    expect(ErrorResponseSchema.parse({ error: "run not found: run-missing" }).error).toContain(
      "run not found",
    );
  });
});
