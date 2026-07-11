import { z } from "zod";
import {
  AgentPhaseSchema,
  ConversationItemSchema,
  DesignProfileBaseSchema,
  DesignProfileDraftSchema,
  DesignProfileRecordSchema,
  DesignProfileScopeSchema,
  DesignProfileSchema,
  DesignSourceArtifactSchema,
  DesignSourceFileNameSchema,
  DesignSourceScopeSchema,
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
        designProfileId: z.string().min(1).optional(),
        designFidelityMode: z.enum(["profile_only", "source_fallback"]).optional(),
        workspaceId: z.string().min(1).optional(),
        organizationId: z.string().min(1).optional(),
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
  status: z.enum(["queued", "needs_user_input"]),
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

export const CreateDesignSourceArtifactRequestSchema = z.object({
  scope: DesignSourceScopeSchema,
  fileName: DesignSourceFileNameSchema,
  mediaType: z.enum(["text/markdown", "text/plain"]),
  contentBase64: z.string().min(1).max(Math.ceil((256 * 1024) / 3) * 4),
  clientSha256: z.string().regex(/^[a-fA-F0-9]{64}$/).optional(),
});

export const DesignSourceArtifactResponseSchema = z.object({
  artifact: DesignSourceArtifactSchema,
});

export const DesignProfileConversionReportSchema = z.object({
  id: z.string().min(1),
  designProfileId: z.string().min(1),
  profileVersion: z.number().int().positive(),
  converterVersion: z.string().min(1),
  deterministicParserVersion: z.string().min(1),
  sourceArtifactId: z.string().min(1),
  sourceHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  extractedSections: z.array(z.string().min(1)),
  extractedTokenCount: z.number().int().nonnegative(),
  extractedComponentCount: z.number().int().nonnegative(),
  requiredSignatureRuleCount: z.number().int().nonnegative(),
  unmappedItems: z.array(z.object({
    sourceSection: z.string(),
    startByte: z.number().int().nonnegative(),
    endByte: z.number().int().nonnegative(),
    excerpt: z.string().max(500),
    excerptHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
    reason: z.enum(["unsupported-field", "ambiguous", "duplicate", "invalid-value"]),
  })),
  warnings: z.array(z.string()),
  createdAt: z.string().datetime(),
});

export const ImportDesignProfileRequestSchema = z.object({
  name: z.string().min(1),
  scope: DesignSourceScopeSchema,
  sourceArtifactId: z.string().min(1),
});

export const ImportDesignProfileResponseSchema = z.object({
  designProfileDraft: DesignProfileDraftSchema,
  conversionReport: DesignProfileConversionReportSchema,
  requiresReview: z.literal(true),
});

export const DesignProfileCreatePayloadSchema = DesignProfileBaseSchema.omit({
  id: true,
  name: true,
  version: true,
  createdAt: true,
  updatedAt: true,
}).extend({
  name: z.string().min(1).optional(),
});

export const CreateDesignProfileRequestSchema = z.object({
  projectId: z.string().min(1).optional(),
  name: z.string().min(1),
  profile: DesignProfileCreatePayloadSchema,
});

export const DesignProfileResponseSchema = z.object({
  designProfile: DesignProfileRecordSchema,
  profile: DesignProfileRecordSchema.optional(),
});

export const ListDesignProfilesResponseSchema = z.object({
  designProfiles: z.array(DesignProfileRecordSchema),
});

export const DesignProfileVersionsResponseSchema = z.object({
  designProfileId: z.string().min(1),
  versions: z.array(DesignProfileRecordSchema),
});

export const DesignProfileDiffChangeSchema = z.object({
  path: z.string().min(1),
  before: z.unknown().optional(),
  after: z.unknown().optional(),
});

export const DesignProfileDiffResponseSchema = z.object({
  designProfileId: z.string().min(1),
  fromVersion: z.number().int().positive(),
  toVersion: z.number().int().positive(),
  changes: z.array(DesignProfileDiffChangeSchema),
});

export const DesignProfileFidelityReportSchema = z.object({
  designProfileId: z.string().min(1),
  version: z.number().int().positive(),
  schemaVersion: z.enum(["design-profile@1", "design-profile@2"]),
  surface: z.enum(["website", "docs"]),
  template: z.string().min(1),
  styleContractVersion: z.enum([
    "runtime-style-contract@p2",
    "runtime-style-contract@p3",
  ]),
  effectiveProfileHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  sourceIntegrity: z.enum(["verified", "unverified", "missing"]),
  sourceHashMatches: z.boolean().nullable(),
  requiredSignatureRuleIds: z.array(z.string().min(1)),
  capsuleIncludedRuleIds: z.array(z.string().min(1)),
  capsuleMissingRuleIds: z.array(z.string().min(1)),
  unsupportedExtendedTokens: z.array(z.string().min(1)),
  warnings: z.array(z.string()),
});

export const UpdateDesignProfileRequestSchema = z.object({
  expectedVersion: z.number().int().positive().optional(),
  name: z.string().min(1),
  profile: z.union([DesignProfileCreatePayloadSchema, z.object({}).passthrough()]),
});

export const ActivateDesignProfileRequestSchema = z.object({
  expectedVersion: z.number().int().positive(),
});

export const ActivateDesignProfileResponseSchema = z.object({
  designProfile: DesignProfileSchema,
  profile: DesignProfileSchema.optional(),
});

export const BindProjectDesignProfileRequestSchema = z.object({
  designProfileId: z.string().min(1),
});

export const ProjectDesignProfileResponseSchema = z.object({
  projectId: z.string().min(1),
  designProfile: DesignProfileSchema.nullable(),
  profile: DesignProfileSchema.nullish(),
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
    version: z
      .enum(["runtime-style-contract@p2", "runtime-style-contract@p3"])
      .default("runtime-style-contract@p2"),
    template: z.string().min(1).optional(),
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
    editableTokens: z.array(z.string().min(1)).optional(),
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

export const UpsertProjectAccessRequestSchema = z.object({
  ownerPrincipalId: z.string().min(1),
  workspaceId: z.string().min(1).optional(),
  organizationId: z.string().min(1).optional(),
});

export const ProjectAccessRecordSchema = z.object({
  projectId: z.string().min(1),
  ownerPrincipalId: z.string().min(1),
  workspaceId: z.string().min(1).nullable().optional(),
  organizationId: z.string().min(1).nullable().optional(),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime(),
});

export const ProjectAccessResponseSchema = z.object({
  projectAccess: ProjectAccessRecordSchema,
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
export type CreateDesignSourceArtifactRequest = z.infer<
  typeof CreateDesignSourceArtifactRequestSchema
>;
export type DesignSourceArtifactResponse = z.infer<typeof DesignSourceArtifactResponseSchema>;
export type DesignProfileConversionReport = z.infer<
  typeof DesignProfileConversionReportSchema
>;
export type ImportDesignProfileRequest = z.infer<typeof ImportDesignProfileRequestSchema>;
export type ImportDesignProfileResponse = z.infer<typeof ImportDesignProfileResponseSchema>;
export type CreateDesignProfileRequest = z.input<typeof CreateDesignProfileRequestSchema>;
export type DesignProfileCreatePayload = z.infer<typeof DesignProfileCreatePayloadSchema>;
export type DesignProfileResponse = z.infer<typeof DesignProfileResponseSchema>;
export type ListDesignProfilesResponse = z.infer<typeof ListDesignProfilesResponseSchema>;
export type DesignProfileVersionsResponse = z.infer<typeof DesignProfileVersionsResponseSchema>;
export type DesignProfileDiffChange = z.infer<typeof DesignProfileDiffChangeSchema>;
export type DesignProfileDiffResponse = z.infer<typeof DesignProfileDiffResponseSchema>;
export type DesignProfileFidelityReport = z.infer<
  typeof DesignProfileFidelityReportSchema
>;
export type UpdateDesignProfileRequest = z.input<typeof UpdateDesignProfileRequestSchema>;
export type ActivateDesignProfileRequest = z.infer<typeof ActivateDesignProfileRequestSchema>;
export type ActivateDesignProfileResponse = z.infer<typeof ActivateDesignProfileResponseSchema>;
export type BindProjectDesignProfileRequest = z.infer<
  typeof BindProjectDesignProfileRequestSchema
>;
export type ProjectDesignProfileResponse = z.infer<typeof ProjectDesignProfileResponseSchema>;
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
export type UpsertProjectAccessRequest = z.infer<typeof UpsertProjectAccessRequestSchema>;
export type ProjectAccessResponse = z.infer<typeof ProjectAccessResponseSchema>;
export type ErrorResponse = z.infer<typeof ErrorResponseSchema>;
