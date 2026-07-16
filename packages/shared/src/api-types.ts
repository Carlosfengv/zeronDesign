import { z } from "zod";
import {
  AgentPhaseSchema,
  AgentRunStatusSchema,
  BriefSchema,
  BriefStatusSchema,
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

// The DCP payload stays Runtime-owned.  The Web surface only receives the
// identity and verification facts it needs to explain a Run and to start an
// explicit Profile Sync operation.
export const DesignContextPackageSummarySchema = z.object({
  version: z.string().min(1).nullish(),
  contentHash: z.string().regex(/^[a-fA-F0-9]{64}$/).nullish(),
  artifactManifestHash: z.string().regex(/^[a-fA-F0-9]{64}$/).nullish(),
  compilerVersion: z.string().min(1).nullish(),
  briefHash: z.string().regex(/^[a-fA-F0-9]{64}$/).nullish(),
  expectedAppRoot: z.string().min(1).nullish(),
  declaredEnforcementMode: z.string().min(1).nullish(),
  effectiveCompatibilityMode: z.string().min(1).nullish(),
  enforcementPolicy: z.object({
    source: z.enum(["persistent", "config"]),
    enabled: z.boolean(),
    policyRevision: z.number().int().positive().nullable(),
    policyUpdatedBy: z.string().min(1).nullable(),
  }).nullish(),
  verificationPolicyId: z.string().min(1),
  warnings: z.array(z.string()),
  surface: z.enum(["website", "docs"]),
  template: z.string().min(1),
  designProfileId: z.string().min(1),
  designProfileVersion: z.number().int().positive(),
  effectiveProfileHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
});

export const DesignContextArtifactSchema = z.object({
  path: z.string().min(1),
  kind: z.string().min(1),
  bytes: z.number().int().nonnegative(),
  sha256: z.string().regex(/^[a-fA-F0-9]{64}$/),
  requiredBeforeMutation: z.boolean(),
});

export const DesignContextManifestResponseSchema = z.object({
  runId: z.string().min(1),
  package: DesignContextPackageSummarySchema,
  artifacts: z.array(DesignContextArtifactSchema),
});

export const DesignContextFidelityAssertionSummarySchema = z.object({
  ruleId: z.string().min(1).max(200),
  recipeId: z.string().min(1).max(200).nullable(),
  priority: z.string().min(1).max(40),
  kind: z.string().min(1).max(80),
  route: z.string().min(1).max(200),
  viewport: z.number().int().nonnegative().nullable(),
  selector: z.string().min(1).max(240).nullable(),
  property: z.string().min(1).max(120).nullable(),
  actualSummary: z.string().max(160).nullable(),
  expectedSummary: z.string().max(160).nullable(),
  comparator: z.string().min(1).max(80).nullable(),
  passed: z.boolean(),
  reason: z.string().min(1).max(320).nullable(),
});

export const DesignContextFidelitySummarySchema = z.object({
  status: z.enum(["passed", "failed"]),
  checkedAt: z.string().min(1).max(80).nullable(),
  outputVersionId: z.string().min(1).max(200).nullable(),
  requiredFailedRuleIds: z.array(z.string().min(1).max(200)).max(64),
  assertions: z.array(DesignContextFidelityAssertionSummarySchema).max(64),
  repairContext: z.object({
    targets: z.array(z.string().min(1).max(240)).max(2),
    instructions: z.array(z.string().min(1).max(240)).max(4),
  }),
});

export const DesignContextDiagnosticsResponseSchema = z.object({
  runId: z.string().min(1),
  package: DesignContextPackageSummarySchema,
  requiredReads: z.array(z.object({ path: z.string().min(1), reason: z.string().min(1) })),
  readFiles: z.array(z.string().min(1)),
  missingRequiredReads: z.array(z.string().min(1)),
  gate: z.enum(["ready", "read_required"]),
  materialization: z.object({ hash: z.string().min(1).nullish(), ready: z.boolean() }),
  // A newly created child Run owns a frozen DCP before its Agent loop has
  // materialized and verified the restored style contract.
  styleContract: z.object({ verified: z.boolean().nullable() }),
  verification: z.object({
    policyId: z.string().min(1).nullish(),
    registryVersion: z.string().min(1).nullish(),
    capabilitySnapshotHash: z.string().regex(/^[a-fA-F0-9]{64}$/).nullish(),
    capabilities: z.record(z.string().min(1), z.object({ available: z.boolean() })).default({}),
  }),
  fidelity: DesignContextFidelitySummarySchema.nullable(),
});

export const ProfileTokenSyncStateSchema = z.enum([
  "already_target", "apply_target", "conflict", "not_managed",
]);
export const ProfileTokenSyncResolutionSchema = z.enum(["keep_current", "apply_target"]);
export const ProfileTokenSyncItemSchema = z.object({
  token: z.string().min(1),
  base: z.string().nullable(),
  current: z.string().nullable(),
  target: z.string().nullable(),
  state: ProfileTokenSyncStateSchema,
  resolution: ProfileTokenSyncResolutionSchema.nullable(),
});
export const ProfileTokenSyncOperationStatusSchema = z.enum([
  "planned", "confirmed", "applying", "applied", "rejected", "recovery_required",
]);
export const PlanDesignProfileSyncRequestSchema = z.object({
  targetDesignProfileId: z.string().min(1),
  targetDesignProfileVersion: z.number().int().positive(),
  targetEffectiveProfileHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  expectedSourceContentHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  idempotencyKey: z.string().min(1).max(200),
});
export const ConfirmDesignProfileSyncRequestSchema = z.object({
  planHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  conflictDecisions: z.record(z.string().min(1), ProfileTokenSyncResolutionSchema).default({}),
  idempotencyKey: z.string().min(1).max(200),
});
export const ProfileTokenSyncOperationResponseSchema = z.object({
  operationId: z.string().min(1),
  status: ProfileTokenSyncOperationStatusSchema,
  expiresAt: z.string().datetime(),
  planHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  sourceContentHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  targetDesignProfileId: z.string().min(1),
  targetDesignProfileVersion: z.number().int().positive(),
  targetEffectiveProfileHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  styleContractIdentity: z.object({
    hash: z.string().regex(/^[a-fA-F0-9]{64}$/),
    version: z.string().min(1),
    template: z.string().min(1),
    appRoot: z.string().min(1),
    tokenMappings: z.record(z.string().min(1), z.string().min(1)),
  }),
  snapshots: z.object({
    baseHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
    currentHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
    targetHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  }),
  items: z.array(ProfileTokenSyncItemSchema),
  conflictDecisions: z.record(z.string().min(1), ProfileTokenSyncResolutionSchema),
  childRunId: z.string().min(1).nullable(),
});

export const BriefResponseSchema = z.object({
  briefId: z.string().min(1),
  projectId: z.string().min(1),
  runId: z.string().min(1),
  status: BriefStatusSchema,
  runStatus: AgentRunStatusSchema,
  brief: BriefSchema,
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

export const CreateReleaseRequestSchema = z.object({
  runtimeProfileId: z.literal("static-web-v1").default("static-web-v1"),
});

export const WorkReleaseStatusSchema = z.enum([
  "packaging", "packaged", "validated", "failed", "garbage_collectable", "garbage_collected",
]);

export const ReleasePackagingStatusSchema = z.enum([
  "prepared", "building", "pushed", "scanning", "signing", "validated", "failed",
  "reconcile_required",
]);

export const PackagingScanEvidenceSchema = z.object({
  policyVersion: z.string().min(1),
  passed: z.boolean(),
  criticalVulnerabilities: z.number().int().nonnegative(),
  highVulnerabilities: z.number().int().nonnegative(),
  secretFindings: z.number().int().nonnegative(),
  reportDigest: z.string().regex(/^[a-fA-F0-9]{64}$/),
});

export const WorkReleaseSchema = z.object({
  id: z.string().min(1),
  projectId: z.string().min(1),
  versionId: z.string().min(1),
  runId: z.string().min(1),
  templateId: z.string().min(1),
  templateVersion: z.string().min(1),
  artifactManifestHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  runtimeManifestHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  sourceSnapshotUri: z.string().min(1),
  runtimeProfileId: z.literal("static-web-v1"),
  runtimeImageRef: z.string().min(1).nullish(),
  runtimeImageDigest: z.string().regex(/^sha256:[a-fA-F0-9]{64}$/).nullish(),
  status: WorkReleaseStatusSchema,
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime(),
});

export const ReleasePackagingRecordSchema = z.object({
  id: z.string().min(1),
  idempotencyKey: z.string().min(1),
  projectId: z.string().min(1),
  releaseId: z.string().min(1),
  artifactManifestHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  runtimeManifestHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  baseImageDigest: z.string().regex(/^sha256:[a-fA-F0-9]{64}$/),
  packagerVersion: z.string().min(1),
  registryRepository: z.string().min(1),
  builtImageDigest: z.string().regex(/^sha256:[a-fA-F0-9]{64}$/).nullish(),
  pushedImageDigest: z.string().regex(/^sha256:[a-fA-F0-9]{64}$/).nullish(),
  sbomDigest: z.string().regex(/^[a-fA-F0-9]{64}$/).nullish(),
  provenanceDigest: z.string().regex(/^[a-fA-F0-9]{64}$/).nullish(),
  signatureIdentity: z.string().min(1).nullish(),
  signatureDigest: z.string().regex(/^[a-fA-F0-9]{64}$/).nullish(),
  scanPolicyVersion: z.string().min(1),
  scanEvidence: PackagingScanEvidenceSchema.nullish(),
  status: ReleasePackagingStatusSchema,
  attempts: z.number().int().nonnegative(),
  lastError: z.string().nullish(),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime(),
});

export const ReleasePackagingResponseSchema = z.object({
  release: WorkReleaseSchema,
  packaging: ReleasePackagingRecordSchema,
});

export const PublishWorkRequestSchema = z.object({
  releaseId: z.string().min(1),
  expectedCurrentReleaseId: z.string().min(1).optional(),
  expectedGeneration: z.number().int().nonnegative(),
  runtimeProfileId: z.literal("static-web-v1").default("static-web-v1"),
});

export const UnpublishWorkRequestSchema = z.object({
  expectedCurrentReleaseId: z.string().min(1),
  expectedGeneration: z.number().int().nonnegative(),
  runtimeProfileId: z.literal("static-web-v1").default("static-web-v1"),
});

export const PublishOperationKindSchema = z.enum(["publish", "update", "rollback", "unpublish"]);
export const PublishOperationStatusSchema = z.enum([
  "requested", "packaging", "release_validated", "desired_state_committed", "reconciling",
  "workload_ready", "traffic_switched", "external_probe_passed", "completed", "failed",
  "cancelled", "reconcile_required",
]);
export const PublishCheckpointSchema = z.enum([
  "requested", "packaging", "release_validated", "desired_state_committed", "reconciling",
  "workload_ready", "traffic_switched", "external_probe_passed", "completed",
]);
export const PublishOperationSchema = z.object({
  schemaVersion: z.literal("publish-operation@1"),
  id: z.string().min(1),
  idempotencyKeyHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  requestHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
  projectId: z.string().min(1),
  releaseId: z.string().min(1).nullish(),
  expectedCurrentReleaseId: z.string().min(1).nullish(),
  desiredGeneration: z.number().int().nonnegative(),
  kind: PublishOperationKindSchema,
  status: PublishOperationStatusSchema,
  checkpoint: PublishCheckpointSchema,
  lastError: z.string().nullish(),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime(),
});
export const PublicationOperationResponseSchema = z.object({ operation: PublishOperationSchema });

export const PublicationDesiredStateSchema = z.enum(["unpublished", "published"]);
export const WorkRuntimeStatusSchema = z.enum([
  "unpublished", "publishing", "published", "updating", "unpublishing", "publish_failed",
  "update_failed", "reconcile_required",
]);
export const WorkRuntimeStateSchema = z.object({
  schemaVersion: z.literal("work-runtime-state@1"),
  projectId: z.string().min(1),
  desiredPublication: PublicationDesiredStateSchema,
  desiredReleaseId: z.string().min(1).nullish(),
  currentReleaseId: z.string().min(1).nullish(),
  previousReleaseId: z.string().min(1).nullish(),
  lastSuccessfulReleaseId: z.string().min(1).nullish(),
  desiredGeneration: z.number().int().nonnegative(),
  hostSlug: z.string().min(1),
  runtimeProfileId: z.string().min(1),
  currentDeploymentName: z.string().min(1).nullish(),
  previousDeploymentName: z.string().min(1).nullish(),
  serviceName: z.string().min(1),
  ingressName: z.string().min(1),
  deploymentUid: z.string().min(1).nullish(),
  deploymentResourceVersion: z.string().min(1).nullish(),
  serviceUid: z.string().min(1).nullish(),
  serviceResourceVersion: z.string().min(1).nullish(),
  ingressUid: z.string().min(1).nullish(),
  ingressResourceVersion: z.string().min(1).nullish(),
  observedGeneration: z.number().int().nonnegative(),
  status: WorkRuntimeStatusSchema,
  lastError: z.string().nullish(),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime(),
});
export const DeploymentStateResponseSchema = z.object({
  runtime: WorkRuntimeStateSchema,
  publicUrl: z.string().url().nullish(),
});
export const WorkReleaseListResponseSchema = z.object({
  projectId: z.string().min(1),
  releases: z.array(WorkReleaseSchema),
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
  status: z.enum(["ready", "not_ready"]),
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
  errorCode: z.string().min(1).optional(),
});

export type ContentSource = z.infer<typeof ContentSourceSchema>;
export type StartRunRequest = z.infer<typeof StartRunRequestSchema>;
export type StartRunResponse = z.infer<typeof StartRunResponseSchema>;
export type ContinueRunRequest = z.infer<typeof ContinueRunRequestSchema>;
export type ContinueRunResponse = z.infer<typeof ContinueRunResponseSchema>;
export type CancelRunResponse = z.infer<typeof CancelRunResponseSchema>;
export type DesignContextManifestResponse = z.infer<typeof DesignContextManifestResponseSchema>;
export type DesignContextDiagnosticsResponse = z.infer<typeof DesignContextDiagnosticsResponseSchema>;
export type PlanDesignProfileSyncRequest = z.input<typeof PlanDesignProfileSyncRequestSchema>;
export type ConfirmDesignProfileSyncRequest = z.input<typeof ConfirmDesignProfileSyncRequestSchema>;
export type ProfileTokenSyncOperationResponse = z.infer<
  typeof ProfileTokenSyncOperationResponseSchema
>;
export type BriefResponse = z.infer<typeof BriefResponseSchema>;
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
export type CreateReleaseRequest = z.input<typeof CreateReleaseRequestSchema>;
export type WorkRelease = z.infer<typeof WorkReleaseSchema>;
export type ReleasePackagingRecord = z.infer<typeof ReleasePackagingRecordSchema>;
export type ReleasePackagingResponse = z.infer<typeof ReleasePackagingResponseSchema>;
export type PublishWorkRequest = z.input<typeof PublishWorkRequestSchema>;
export type UnpublishWorkRequest = z.input<typeof UnpublishWorkRequestSchema>;
export type PublishOperation = z.infer<typeof PublishOperationSchema>;
export type PublicationOperationResponse = z.infer<typeof PublicationOperationResponseSchema>;
export type WorkRuntimeState = z.infer<typeof WorkRuntimeStateSchema>;
export type DeploymentStateResponse = z.infer<typeof DeploymentStateResponseSchema>;
export type WorkReleaseListResponse = z.infer<typeof WorkReleaseListResponseSchema>;
export type PromotePreviewRequest = z.infer<typeof PromotePreviewRequestSchema>;
export type PromotePreviewResponse = z.infer<typeof PromotePreviewResponseSchema>;
export type HealthResponse = z.infer<typeof HealthResponseSchema>;
export type UpsertProjectAccessRequest = z.infer<typeof UpsertProjectAccessRequestSchema>;
export type ProjectAccessResponse = z.infer<typeof ProjectAccessResponseSchema>;
export type ErrorResponse = z.infer<typeof ErrorResponseSchema>;
