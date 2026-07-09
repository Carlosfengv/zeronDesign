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

export const StartRunRequestSchema = z
  .object({
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
  })
  .superRefine((request, ctx) => {
    if (
      request.phase === "build" &&
      !request.inputContext.parentRunId &&
      !request.inputContext.briefId
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["inputContext", "briefId"],
        message: "build runs require briefId",
      });
    }
    if (request.phase !== "edit") {
      return;
    }
    if (!request.inputContext.baseVersionId) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["inputContext", "baseVersionId"],
        message: "edit runs require baseVersionId",
      });
    }
    if (!request.inputContext.sandboxBindingId) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["inputContext", "sandboxBindingId"],
        message: "edit runs require sandboxBindingId",
      });
    }
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

export const RuntimeStyleContractSchema = z
  .object({
    tokenFile: z.string().min(1),
    globalCssFile: z.string().min(1),
    componentRoot: z.string().min(1),
    tailwind: z
      .object({
        version: z.string().min(1),
        entryImport: z.string().min(1),
        themeSource: z.string().min(1),
      })
      .passthrough(),
    tokens: z
      .record(z.string().min(1), z.string().min(1))
      .refine((tokens) => Object.keys(tokens).length > 0, "style contract tokens required"),
  })
  .passthrough();

export const RuntimeLatestBuildSchema = z
  .object({
    status: z.string().min(1),
    sourceSnapshotUri: z.string().min(1),
  })
  .passthrough();

export const RuntimeDependencyStateSchema = z
  .object({
    needsRestore: z.boolean(),
  })
  .passthrough();

export const RuntimePreviewStateSchema = z
  .object({
    status: z.string().min(1),
  })
  .passthrough();

export const ProjectRuntimeStateResponseSchema = z
  .object({
    projectId: z.string().min(1),
    currentVersionId: z.string().min(1),
    sandboxBindingId: z.string().min(1),
    sourceSnapshotUri: z.string().min(1),
    appRoot: z.string().min(1),
    templateKey: z.string().min(1),
    styleContractPath: z.string().min(1).nullable().optional(),
    styleContract: RuntimeStyleContractSchema.nullable().optional(),
    latestBuild: RuntimeLatestBuildSchema.nullable().optional(),
    dependencyState: RuntimeDependencyStateSchema.nullable().optional(),
    preview: RuntimePreviewStateSchema.nullable().optional(),
  })
  .superRefine((state, ctx) => {
    if (
      state.latestBuild?.sourceSnapshotUri &&
      state.latestBuild.sourceSnapshotUri !== state.sourceSnapshotUri
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["latestBuild", "sourceSnapshotUri"],
        message: "latestBuild sourceSnapshotUri must match sourceSnapshotUri",
      });
    }
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
export type RuntimeStyleContract = z.infer<typeof RuntimeStyleContractSchema>;
export type RuntimeLatestBuild = z.infer<typeof RuntimeLatestBuildSchema>;
export type RuntimeDependencyState = z.infer<typeof RuntimeDependencyStateSchema>;
export type RuntimePreviewState = z.infer<typeof RuntimePreviewStateSchema>;
export type ProjectRuntimeStateResponse = z.infer<typeof ProjectRuntimeStateResponseSchema>;
export type PromotePreviewRequest = z.infer<typeof PromotePreviewRequestSchema>;
export type PromotePreviewResponse = z.infer<typeof PromotePreviewResponseSchema>;
export type HealthResponse = z.infer<typeof HealthResponseSchema>;
export type ErrorResponse = z.infer<typeof ErrorResponseSchema>;
