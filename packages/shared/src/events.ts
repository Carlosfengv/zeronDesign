import { z } from "zod";
import { AgentRunStatusSchema } from "./schemas.js";

const OptionalStringFromRustOption = z.string().min(1).nullish();

const BaseEventSchema = z.object({
  runId: z.string().min(1),
  timestamp: z.string().datetime(),
});

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
