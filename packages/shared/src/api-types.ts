import { z } from "zod";
import {
  AgentPhaseSchema,
  ConversationItemSchema,
  ProjectVersionStatusSchema,
} from "./schemas.js";

export const ContentSourceSchema = z.object({
  id: z.string().min(1),
  kind: z.string().min(1),
  text: z.string(),
  readable: z.boolean().default(true),
});

export const StartRunRequestSchema = z.object({
  projectId: z.string().min(1),
  phase: AgentPhaseSchema,
  agentProfile: z.string().min(1),
  inputContext: z
    .object({
      contentSources: z.array(ContentSourceSchema).optional(),
      briefId: z.string().min(1).optional(),
      baseVersionId: z.string().min(1).optional(),
      sandboxBindingId: z.string().min(1).optional(),
      parentRunId: z.string().min(1).optional(),
      findingIds: z.array(z.string().min(1)).optional(),
    })
    .default({}),
});

export const StartRunResponseSchema = z.object({
  runId: z.string().min(1),
  status: z.literal("queued"),
});

export const ContinueRunRequestSchema = z.object({
  userMessage: z.string().min(1),
});

export const ContinueRunResponseSchema = z.object({
  runId: z.string().min(1),
  status: z.enum(["running", "needs_user_input", "completed"]),
});

export const CancelRunResponseSchema = z.object({
  runId: z.string().min(1),
  status: z.literal("cancelled"),
});

export const ResolvePermissionRequestSchema = z.object({
  decision: z.enum(["allow", "ask", "deny"]),
  updatedInput: z.unknown().optional(),
});

export const ResolvePermissionResponseSchema = z.object({
  runId: z.string().min(1),
  status: z.enum(["running", "needs_user_input", "blocked"]),
});

export const PreviewCurrentResponseSchema = z.object({
  projectId: z.string().min(1),
  versionId: z.string().min(1),
  previewUrl: z.string().url(),
  status: z.literal("promoted"),
});

export const PreviewVersionResponseSchema = z.object({
  projectId: z.string().min(1),
  versionId: z.string().min(1),
  previewUrl: z.string().url(),
  status: ProjectVersionStatusSchema,
});

export const ConversationListResponseSchema = z.object({
  projectId: z.string().min(1),
  items: z.array(ConversationItemSchema),
});

export const PromotePreviewRequestSchema = z.object({
  projectId: z.string().min(1),
  runId: z.string().min(1),
  candidateVersionId: z.string().min(1),
  gateReport: z
    .object({
      buildLogHasTerminalError: z.boolean().optional(),
      previewAccessible: z.boolean().optional(),
      screenshotBlank: z.boolean().optional(),
      screenshotAvailable: z.boolean().optional(),
      blockingFindings: z.number().int().nonnegative().optional(),
    })
    .default({}),
});

export const PromotePreviewResponseSchema = PreviewCurrentResponseSchema;

export const HealthResponseSchema = z.object({
  status: z.literal("ready"),
});

export const ErrorResponseSchema = z.object({
  error: z.string().min(1),
});

export type ContentSource = z.infer<typeof ContentSourceSchema>;
export type StartRunRequest = z.infer<typeof StartRunRequestSchema>;
export type StartRunResponse = z.infer<typeof StartRunResponseSchema>;
export type ContinueRunRequest = z.infer<typeof ContinueRunRequestSchema>;
export type ContinueRunResponse = z.infer<typeof ContinueRunResponseSchema>;
export type CancelRunResponse = z.infer<typeof CancelRunResponseSchema>;
export type ResolvePermissionRequest = z.infer<typeof ResolvePermissionRequestSchema>;
export type ResolvePermissionResponse = z.infer<typeof ResolvePermissionResponseSchema>;
export type PreviewCurrentResponse = z.infer<typeof PreviewCurrentResponseSchema>;
export type PreviewVersionResponse = z.infer<typeof PreviewVersionResponseSchema>;
export type ConversationListResponse = z.infer<typeof ConversationListResponseSchema>;
export type PromotePreviewRequest = z.infer<typeof PromotePreviewRequestSchema>;
export type PromotePreviewResponse = z.infer<typeof PromotePreviewResponseSchema>;
export type HealthResponse = z.infer<typeof HealthResponseSchema>;
export type ErrorResponse = z.infer<typeof ErrorResponseSchema>;
