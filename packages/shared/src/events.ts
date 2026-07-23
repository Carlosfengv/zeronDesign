import { z } from "zod";
import { AgentRunStatusSchema } from "./schemas.js";

const OptionalStringFromRustOption = z.string().min(1).nullish();

const BaseEventSchema = z.object({
  runId: z.string().min(1),
  timestamp: z.string().datetime(),
});

export const ObservationReceiptSchema = z
  .object({
    schemaVersion: z.literal("observation-receipt@1"),
    runId: z.string().min(1),
    normalizedPath: z.string().min(1),
    contentSha256: z.string().regex(/^[0-9a-f]{64}$/),
    contextWindowEpoch: z.number().int().nonnegative(),
    view: z.enum(["full", "partial", "injected"]),
    lastOutcome: z.enum(["content_returned", "unchanged"]),
    firstReadTurn: z.number().int().nonnegative(),
    lastReadTurn: z.number().int().nonnegative(),
    readCount: z.number().int().positive(),
    purpose: z.enum(["context", "source", "diagnostic", "verification", "runtime_internal"]),
    deliveredBytes: z.number().int().nonnegative(),
    estimatedTokens: z.number().int().nonnegative(),
    duplicateDelivery: z.boolean(),
  })
  .strict();

const AgentEventBaseSchema = z.discriminatedUnion("type", [
  BaseEventSchema.extend({
    type: z.literal("run.started"),
    label: z.string().min(1),
  }),
  BaseEventSchema.extend({
    type: z.literal("agent.message"),
    text: z.string(),
  }),
  BaseEventSchema.extend({
    type: z.literal("tool.started"),
    tool: z.string().min(1),
    summary: z.string(),
    toolUseId: z.string().min(1),
  }),
  BaseEventSchema.extend({
    type: z.literal("tool.completed"),
    tool: z.string().min(1),
    summary: z.string(),
    toolUseId: z.string().min(1),
    metadata: z.unknown().nullish(),
  }),
  BaseEventSchema.extend({
    type: z.literal("tool.output"),
    tool: z.string().min(1),
    toolUseId: z.string().min(1),
    stream: z.enum(["stdout", "stderr"]),
    text: z.string(),
  }),
  BaseEventSchema.extend({
    type: z.literal("tool.failed"),
    tool: z.string().min(1),
    error: z.string().min(1),
    toolUseId: z.string().min(1),
    recoverable: z.boolean(),
    metadata: z.unknown().nullish(),
  }),
  BaseEventSchema.extend({
    type: z.literal("tool.recovery_suggested"),
    tool: z.string().min(1),
    errorKind: z.string().min(1),
    fingerprint: z.string().min(1),
    attempt: z.number().int().nonnegative(),
    guidance: z.string().min(1),
    metadata: z.unknown().nullish(),
  }),
  BaseEventSchema.extend({
    type: z.literal("chunk.received"),
    path: z.string().min(1),
    sessionId: z.string().min(1),
    index: z.number().int().nonnegative(),
    total: z.number().int().nonnegative(),
    bytes: z.number().int().nonnegative(),
    chars: z.number().int().nonnegative(),
  }),
  BaseEventSchema.extend({
    type: z.literal("chunk.committed"),
    path: z.string().min(1),
    sessionId: z.string().min(1),
    total: z.number().int().nonnegative(),
    bytes: z.number().int().nonnegative(),
    chars: z.number().int().nonnegative(),
    sha256: z.string().min(1),
  }),
  BaseEventSchema.extend({
    type: z.literal("metric.recorded"),
    name: z.string().min(1),
    value: z.number().int().nonnegative(),
    metadata: z.unknown().nullish(),
  }),
  BaseEventSchema.extend({
    type: z.literal("model.turn_started"),
    turn: z.number().int().nonnegative(),
  }),
  BaseEventSchema.extend({
    type: z.literal("model.execution"),
    turn: z.number().int().nonnegative(),
    snapshot: z.unknown(),
  }),
  BaseEventSchema.extend({
    type: z.literal("model.usage"),
    turn: z.number().int().nonnegative(),
    inputTokens: z.number().int().nonnegative(),
    outputTokens: z.number().int().nonnegative(),
    cachedInputTokens: z.number().int().nonnegative(),
    estimated: z.boolean(),
  }),
  BaseEventSchema.extend({
    type: z.literal("prompt.composition"),
    turn: z.number().int().nonnegative(),
    estimatedInputTokens: z.number().int().nonnegative(),
    systemTokens: z.number().int().nonnegative(),
    messageTokens: z.number().int().nonnegative(),
    toolDefinitionTokens: z.number().int().nonnegative(),
    generationContextTokens: z.number().int().nonnegative(),
    staticPrefixHash: z.string().regex(/^[0-9a-f]{64}$/),
    toolSetHashVersion: z.literal("tool-definition-set@1").optional(),
    toolSetHash: z.string().regex(/^[0-9a-f]{64}$/),
  }),
  BaseEventSchema.extend({
    type: z.literal("token.budget_decision"),
    turn: z.number().int().nonnegative(),
    mode: z.enum(["legacy", "split_shadow", "split_enforced"]),
    budgetKind: z.enum([
      "legacy_gross_input",
      "gross_input",
      "uncached_input",
      "turn_prompt",
      "output",
      "turn",
      "tool_call",
    ]),
    used: z.number().int().nonnegative(),
    limit: z.number().int().positive(),
    exhausted: z.boolean(),
    enforced: z.boolean(),
    grossInputTokens: z.number().int().nonnegative(),
    cachedInputTokens: z.number().int().nonnegative(),
    uncachedInputTokens: z.number().int().nonnegative(),
  }),
  BaseEventSchema.extend({
    type: z.literal("token.usage_contract_violation"),
    turn: z.number().int().nonnegative(),
    inputTokens: z.number().int().nonnegative(),
    cachedInputTokens: z.number().int().nonnegative(),
    normalizedCachedInputTokens: z.number().int().nonnegative(),
  }),
  BaseEventSchema.extend({
    type: z.literal("workflow.lifecycle_started"),
    driverId: z.string().min(1),
    action: z.string().min(1),
    sequence: z.number().int().positive(),
    attempt: z.number().int().positive(),
    idempotencyKey: z.string().regex(/^[0-9a-f]{64}$/),
  }),
  BaseEventSchema.extend({
    type: z.literal("workflow.lifecycle_completed"),
    driverId: z.string().min(1),
    action: z.string().min(1),
    sequence: z.number().int().positive(),
    attempt: z.number().int().positive(),
    idempotencyKey: z.string().regex(/^[0-9a-f]{64}$/),
    outcome: z.enum(["completed", "fallback_selected"]),
    progressEvidence: z.object({
      schemaVersion: z.literal("workflow-lifecycle-progress@1"),
      isError: z.boolean(),
      content: z.record(z.string(), z.unknown()),
      metadata: z.record(z.string(), z.unknown()).nullish(),
    }).strict(),
  }),
  BaseEventSchema.extend({
    type: z.literal("workflow.lifecycle_failed"),
    driverId: z.string().min(1),
    action: z.string().min(1),
    sequence: z.number().int().positive(),
    attempt: z.number().int().positive(),
    idempotencyKey: z.string().regex(/^[0-9a-f]{64}$/),
    errorKind: z.string().min(1),
    recoverable: z.boolean(),
    diagnosticRef: z.string().min(1).nullish(),
    sourceSnapshotUri: z.string().min(1).nullish(),
    sourceHash: z.string().regex(/^[0-9a-f]{64}$/).nullish(),
  }),
  BaseEventSchema.extend({
    type: z.literal("run.continuation_created"),
    operationId: z.string().min(1),
    predecessorRunId: z.string().min(1),
    continuationSnapshotId: z.string().min(1),
    attempt: z.number().int().positive(),
    automatic: z.boolean(),
  }),
  BaseEventSchema.extend({
    type: z.literal("observation.receipt"),
    receipt: ObservationReceiptSchema,
  }),
  BaseEventSchema.extend({
    type: z.literal("generation_context.compiled"),
    contextContentHash: z.string().regex(/^[0-9a-f]{64}$/),
    runContextBindingHash: z.string().regex(/^[0-9a-f]{64}$/),
    serializedBytes: z.number().int().nonnegative(),
    sections: z.array(z.string().min(1)),
  }),
  BaseEventSchema.extend({
    type: z.literal("generation_context.fallback"),
    reason: z.string().min(1),
    designSourceKind: z.enum(["design_profile", "template_default"]),
  }),
  BaseEventSchema.extend({
    type: z.literal("content_plan.approval_rejected"),
    planId: z.string().min(1).nullable(),
    revision: z.number().int().positive().nullable(),
    contentHash: z.string().regex(/^[0-9a-f]{64}$/).nullable(),
    state: z.enum(["verified", "missing", "invalidated", "identity_mismatch"]),
    reason: z.string().min(1),
  }),
  BaseEventSchema.extend({
    type: z.literal("permission.requested"),
    permissionId: z.string().min(1),
    tool: z.string().min(1),
    reason: z.string().min(1),
  }),
  BaseEventSchema.extend({
    type: z.literal("permission.denied"),
    tool: z.string().min(1),
    reason: z.string().min(1),
  }),
  BaseEventSchema.extend({
    type: z.literal("state.changed"),
    state: z.string().min(1),
  }),
  BaseEventSchema.extend({
    type: z.literal("preview.rebuilding"),
    previousVersionId: OptionalStringFromRustOption,
  }),
  BaseEventSchema.extend({
    type: z.literal("preview.candidate"),
    url: z.string().url(),
    versionId: z.string().min(1),
    screenshotId: OptionalStringFromRustOption,
  }),
  BaseEventSchema.extend({
    type: z.literal("preview.updated"),
    url: z.string().url(),
    versionId: z.string().min(1),
    screenshotId: OptionalStringFromRustOption,
  }),
  BaseEventSchema.extend({
    type: z.literal("review.finding"),
    findingId: z.string().min(1),
    severity: z.enum(["info", "warning", "blocking"]),
    summary: z.string().min(1),
  }),
  BaseEventSchema.extend({
    type: z.literal("run.completed"),
    status: AgentRunStatusSchema,
    summary: z.string(),
  }),
]);

export const AgentEventSchema = AgentEventBaseSchema.superRefine((event, ctx) => {
  if (event.type !== "tool.failed" || !event.recoverable) {
    return;
  }
  const metadata = event.metadata;
  if (
    !metadata ||
    typeof metadata !== "object" ||
    !("errorKind" in metadata) ||
    typeof metadata.errorKind !== "string" ||
    metadata.errorKind.trim() === ""
  ) {
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["metadata", "errorKind"],
      message: "recoverable tool.failed events require metadata.errorKind",
    });
  }
});

export type AgentEvent = z.infer<typeof AgentEventSchema>;

const DraftPreviewEventBaseSchema = z.object({
  projectId: z.string().min(1),
  sessionId: z.string().min(1),
  sessionEpoch: z.number().int().positive(),
  workspaceRevision: z.number().int().nonnegative(),
  timestamp: z.string().datetime(),
});

export const DraftPreviewEventSchema = z.discriminatedUnion("type", [
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("preview.dev_starting"),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("preview.dev_ready"),
    proxyUrl: z.string().url(),
    readyRevision: z.number().int().nonnegative(),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("preview.dev_updating"),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("preview.dev_compile_error"),
    error: z.string().min(1),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("preview.dev_restarting"),
    restartCount: z.number().int().positive(),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("preview.dev_failed"),
    error: z.string().min(1),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("preview.dev_stopped"),
    reason: z.string().min(1),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("source.revision_committed"),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("source.revision_durable"),
    snapshotId: z.string().min(1),
    sourceHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  }).strict(),
  DraftPreviewEventBaseSchema.extend({
    type: z.literal("source.snapshot_created"),
    snapshotId: z.string().min(1),
    sourceHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  }).strict(),
]);

export type DraftPreviewEvent = z.infer<typeof DraftPreviewEventSchema>;
