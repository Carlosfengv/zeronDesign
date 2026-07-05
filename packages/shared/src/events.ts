import { z } from "zod";
import { AgentRunStatusSchema } from "./schemas.js";

const OptionalStringFromRustOption = z.string().min(1).nullish();

const BaseEventSchema = z.object({
  runId: z.string().min(1),
  timestamp: z.string().datetime(),
});

export const AgentEventSchema = z.discriminatedUnion("type", [
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
    type: z.literal("tool.failed"),
    tool: z.string().min(1),
    error: z.string().min(1),
    toolUseId: z.string().min(1),
    recoverable: z.boolean(),
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

export type AgentEvent = z.infer<typeof AgentEventSchema>;
