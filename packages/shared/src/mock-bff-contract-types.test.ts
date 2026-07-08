import { describe, expect, it } from "vitest";
import type {
  CancelRunResponse,
  ConversationListResponse,
  ContinueRunRequest,
  ContinueRunResponse,
  ContentSource,
  ErrorResponse,
  PreviewCurrentResponse,
  PreviewVersionResponse,
  PromotePreviewRequest,
  PromotePreviewResponse,
  ResolvePermissionRequest,
  ResolvePermissionResponse,
  StartRunRequest,
  StartRunResponse,
} from "./api-types.js";
import {
  CancelRunResponseSchema,
  ConversationListResponseSchema,
  ContinueRunRequestSchema,
  ContinueRunResponseSchema,
  ContentSourceSchema,
  ErrorResponseSchema,
  PreviewCurrentResponseSchema,
  PreviewVersionResponseSchema,
  PromotePreviewRequestSchema,
  PromotePreviewResponseSchema,
  ResolvePermissionRequestSchema,
  ResolvePermissionResponseSchema,
  StartRunRequestSchema,
  StartRunResponseSchema,
} from "./api-types.js";
import type { AgentEvent } from "./events.js";
import { AgentEventSchema } from "./events.js";
import type { SandboxBinding } from "./schemas.js";
import { SandboxBindingSchema } from "./schemas.js";

describe("mock BFF shared runtime contract types", () => {
  it("builds Phase B runtime API payloads from shared request and response types", () => {
    const contentSource = {
      id: "source-1",
      kind: "prompt",
      text: "Build a product site",
      readable: true,
    } satisfies ContentSource;
    const contentSourceWithRuntimeDefault = {
      id: "source-2",
      kind: "markdown",
      text: "# Product notes",
    };
    const sandboxBinding = {
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
      lastSeenAt: "2026-07-04T00:00:00.000Z",
    } satisfies SandboxBinding;
    const startRun = {
      projectId: "project-1",
      phase: "brief",
      agentProfile: "brief",
      inputContext: {
        contentSources: [contentSource],
        briefId: "brief-1",
        baseVersionId: "version-1",
      },
    } satisfies StartRunRequest;
    const startBuildRun = {
      projectId: "project-1",
      phase: "build",
      agentProfile: "build",
      inputContext: {
        briefId: "brief-1",
        sandboxBindingId: "sandbox-binding-1",
      },
    } satisfies StartRunRequest;
    const startRepairRun = {
      projectId: "project-1",
      phase: "repair",
      agentProfile: "repair",
      inputContext: {
        parentRunId: "run-review-1",
        findingIds: ["finding-1"],
      },
    } satisfies StartRunRequest;
    const startRunResponse = { runId: "run-1", status: "queued" } satisfies StartRunResponse;

    const continueRun = {
      userMessage: "Make the hero sharper",
    } satisfies ContinueRunRequest;
    const continueRunResponse = {
      runId: "run-1",
      status: "running",
    } satisfies ContinueRunResponse;
    const continueRunNeedsInputResponse = {
      runId: "run-1",
      status: "needs_user_input",
    } satisfies ContinueRunResponse;
    const continueRunConfirmedResponse = {
      runId: "run-1",
      status: "completed",
    } satisfies ContinueRunResponse;
    const conversationResponse = {
      projectId: "project-1",
      items: [
        {
          id: "conversation-1",
          projectId: "project-1",
          runId: "run-1",
          versionId: null,
          checkpointId: null,
          kind: "assistant_message",
          role: "assistant",
          text: "Brief is ready for confirmation.",
          metadata: { briefId: "brief-1" },
          visibility: "user",
          createdAt: "2026-07-04T00:00:00.000Z",
        },
      ],
    } satisfies ConversationListResponse;

    const permissionDecision = {
      decision: "allow",
      updatedInput: { registry: "internal" },
    } satisfies ResolvePermissionRequest;
    const permissionResponse = {
      runId: "run-1",
      status: "running",
    } satisfies ResolvePermissionResponse;
    const permissionAskDecision = {
      decision: "ask",
      updatedInput: { question: "Which registry should I use?" },
    } satisfies ResolvePermissionRequest;
    const permissionAskResponse = {
      runId: "run-1",
      status: "needs_user_input",
    } satisfies ResolvePermissionResponse;

    const cancelResponse = { runId: "run-1", status: "cancelled" } satisfies CancelRunResponse;
    const previewCurrent = {
      projectId: "project-1",
      versionId: "version-1",
      previewUrl: "http://preview.local/preview/project-1/current",
      status: "promoted",
    } satisfies PreviewCurrentResponse;
    const previewVersion = {
      projectId: "project-1",
      versionId: "version-1",
      previewUrl: "http://preview.local/preview/project-1/version-1",
      status: "candidate",
    } satisfies PreviewVersionResponse;
    const promotePreview = {
      projectId: "project-1",
      runId: "run-1",
      candidateVersionId: "version-1",
      gateReport: {
        previewAccessible: true,
        screenshotAvailable: true,
      },
    } satisfies PromotePreviewRequest;
    const promotePreviewResponse = previewCurrent satisfies PromotePreviewResponse;
    const errorResponse = {
      error: "run not found: run-missing",
    } satisfies ErrorResponse;

    expect(ContentSourceSchema.parse(contentSource).readable).toBe(true);
    expect(ContentSourceSchema.parse(contentSourceWithRuntimeDefault).readable).toBe(true);
    expect(SandboxBindingSchema.parse(sandboxBinding).workspacePvcName).toBe(
      "workspace-project-project-1-sandbox-1",
    );
    expect(StartRunRequestSchema.parse(startRun).phase).toBe("brief");
    expect(StartRunRequestSchema.parse(startBuildRun).inputContext.sandboxBindingId).toBe(
      "sandbox-binding-1",
    );
    expect(StartRunRequestSchema.parse(startRepairRun).inputContext.findingIds).toEqual([
      "finding-1",
    ]);
    expect(StartRunResponseSchema.parse(startRunResponse).status).toBe("queued");
    expect(ContinueRunRequestSchema.parse(continueRun).userMessage).toContain("hero");
    expect(ContinueRunResponseSchema.parse(continueRunResponse).status).toBe("running");
    expect(ContinueRunResponseSchema.parse(continueRunNeedsInputResponse).status).toBe(
      "needs_user_input",
    );
    expect(ContinueRunResponseSchema.parse(continueRunConfirmedResponse).status).toBe(
      "completed",
    );
    expect(ConversationListResponseSchema.parse(conversationResponse).items[0].kind).toBe(
      "assistant_message",
    );
    expect(ResolvePermissionRequestSchema.parse(permissionDecision).decision).toBe("allow");
    expect(ResolvePermissionResponseSchema.parse(permissionResponse).runId).toBe("run-1");
    expect(ResolvePermissionRequestSchema.parse(permissionAskDecision).decision).toBe("ask");
    expect(ResolvePermissionResponseSchema.parse(permissionAskResponse).status).toBe(
      "needs_user_input",
    );
    expect(CancelRunResponseSchema.parse(cancelResponse).status).toBe("cancelled");
    expect(PreviewCurrentResponseSchema.parse(previewCurrent).previewUrl).toContain(
      "/preview/project-1/current",
    );
    expect(PreviewVersionResponseSchema.parse(previewVersion).status).toBe("candidate");
    expect(PromotePreviewRequestSchema.parse(promotePreview).gateReport.previewAccessible).toBe(
      true,
    );
    expect(PromotePreviewResponseSchema.parse(promotePreviewResponse).status).toBe("promoted");
    expect(ErrorResponseSchema.parse(errorResponse).error).toContain("run not found");
  });

  it("parses runtime SSE events from the shared AgentEvent union", () => {
    const events = [
      {
        type: "run.started",
        runId: "run-1",
        label: "Brief Agent",
        timestamp: "2026-07-04T00:00:00.000Z",
      },
      {
        type: "preview.updated",
        runId: "run-1",
        url: "http://preview.local/preview/project-1/current",
        versionId: "version-1",
        screenshotId: "shot-1",
        timestamp: "2026-07-04T00:00:01.000Z",
      },
      {
        type: "tool.output",
        runId: "run-1",
        tool: "package.install",
        toolUseId: "tool-install",
        stream: "stdout",
        text: "added 42 packages",
        timestamp: "2026-07-04T00:00:01.500Z",
      },
      {
        type: "run.completed",
        runId: "run-1",
        status: "completed",
        summary: "Ready",
        timestamp: "2026-07-04T00:00:02.000Z",
      },
    ] satisfies AgentEvent[];

    expect(events.map((event) => AgentEventSchema.parse(event).type)).toEqual([
      "run.started",
      "preview.updated",
      "tool.output",
      "run.completed",
    ]);
  });
});
