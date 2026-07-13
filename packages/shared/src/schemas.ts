import { z } from "zod";

const OptionalStringFromRustOption = z.string().min(1).nullish();
const OptionalTimestampFromRustOption = z.string().datetime().nullish();

export const AgentPhaseSchema = z.enum([
  "brief",
  "build",
  "repair",
  "review",
  "edit",
  "export",
]);

export const AgentRunStatusSchema = z.enum([
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

export const AgentRunSchema = z.object({
  id: z.string().min(1),
  projectId: z.string().min(1),
  sessionId: z.string().min(1),
  parentRunId: OptionalStringFromRustOption,
  triggeredByEventId: OptionalStringFromRustOption,
  phase: AgentPhaseSchema,
  agentProfile: z.string().min(1),
  status: AgentRunStatusSchema,
  model: z.string().min(1),
  sandboxId: OptionalStringFromRustOption,
  briefVersion: OptionalStringFromRustOption,
  designVersion: OptionalStringFromRustOption,
  designProfileId: OptionalStringFromRustOption,
  designProfileVersion: z.number().int().positive().nullish(),
  designProfileHash: OptionalStringFromRustOption,
  designProfileSurface: OptionalStringFromRustOption,
  designProfileTemplate: OptionalStringFromRustOption,
  designProfileSurfaceOverrideHash: OptionalStringFromRustOption,
  designProfileTemplateOverrideHash: OptionalStringFromRustOption,
  designProfileEffectiveHash: OptionalStringFromRustOption,
  designProfileUnsupportedExtendedTokens: z.array(z.string().min(1)).optional().default([]),
  designProfileBlockingCapabilityRuleIds: z.array(z.string().min(1)).optional().default([]),
  designFidelityMode: z.enum(["profile_only", "source_fallback"]).nullish(),
  designSourceArtifactId: OptionalStringFromRustOption,
  designSourceHash: OptionalStringFromRustOption,
  designSourceSizeBytes: z.number().int().positive().nullish(),
  designSourceBudgetBytes: z.number().int().positive().nullish(),
  designSourceBytesRead: z.number().int().nonnegative().optional().default(0),
  designSourceSections: z.array(z.object({
    id: z.string().min(1),
    heading: z.string(),
    startByte: z.number().int().nonnegative(),
    endByte: z.number().int().nonnegative(),
    sha256: z.string().regex(/^[a-fA-F0-9]{64}$/),
    requiredByRuleIds: z.array(z.string().min(1)),
  })).optional().default([]),
  designSourceRequiredSectionIds: z.array(z.string().min(1)).optional().default([]),
  designSourceReadSectionHashes: z.array(z.string().min(1)).optional().default([]),
  designContextReadFiles: z.array(z.string().min(1)).optional().default([]),
  baseVersionId: OptionalStringFromRustOption,
  outputVersionId: OptionalStringFromRustOption,
  findingIds: z.array(z.string().min(1)).nullish(),
  inputMessageIds: z.array(z.string().min(1)),
  checkpointId: OptionalStringFromRustOption,
  profileSnapshot: z.object({
    allowedTools: z.array(z.string()),
    deniedTools: z.array(z.string()),
    permissionMode: z.enum(["normal", "read_only", "scoped_repair"]),
    transcriptMode: z.enum(["main", "sidechain"]),
    sourceCheckpointId: OptionalStringFromRustOption,
    mcpServerNames: z.array(z.string()),
  }),
  startedAt: z.string().datetime(),
  updatedAt: z.string().datetime(),
  completedAt: OptionalTimestampFromRustOption,
});

export const ConversationItemKindSchema = z.enum([
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
]);

export const ConversationItemSchema = z.object({
  id: z.string().min(1),
  projectId: z.string().min(1),
  runId: OptionalStringFromRustOption,
  versionId: OptionalStringFromRustOption,
  checkpointId: OptionalStringFromRustOption,
  kind: ConversationItemKindSchema,
  role: z.enum(["user", "assistant", "system"]).nullish(),
  text: z.string(),
  metadata: z.unknown().nullish(),
  visibility: z
    .enum(["user", "debug"])
    .optional()
    .transform((visibility) => visibility ?? "user"),
  createdAt: z.string().datetime(),
});

export const ReviewFindingSchema = z.object({
  id: z.string().min(1),
  projectId: z.string().min(1),
  runId: z.string().min(1),
  versionId: z.string().min(1),
  severity: z.enum(["info", "warning", "blocking"]),
  category: z.enum(["build", "runtime", "visual", "content", "safety"]),
  summary: z.string().min(1),
  evidence: z
    .object({
      screenshotId: OptionalStringFromRustOption,
      filePath: OptionalStringFromRustOption,
      logExcerpt: OptionalStringFromRustOption,
    })
    .nullish(),
  repairable: z.boolean(),
  status: z.enum(["open", "repairing", "fixed", "accepted", "needs_user_input"]),
});

export const ProjectVersionStatusSchema = z.enum(["candidate", "promoted", "failed"]);
export const BriefStatusSchema = z.enum(["draft", "confirmed", "superseded"]);

export const ProjectVersionSchema = z.object({
  id: z.string().min(1),
  projectId: z.string().min(1),
  sourceSnapshotUri: OptionalStringFromRustOption,
  previewUrl: z.string().url(),
  screenshotUri: OptionalStringFromRustOption,
  screenshotId: OptionalStringFromRustOption,
  status: ProjectVersionStatusSchema,
  createdByRunId: z.string().min(1),
  createdAt: z.string().datetime(),
  promotedAt: OptionalTimestampFromRustOption,
});

export const SandboxBindingStatusSchema = z.enum([
  "claiming",
  "starting",
  "ready",
  "busy",
  "idle",
  "paused",
  "failed",
  "deleted",
]);

export const SandboxBindingSchema = z.object({
  id: z.string().min(1),
  projectId: z.string().min(1),
  sandboxName: z.string().min(1),
  sandboxClaimName: z.string().min(1),
  workspacePvcName: z.string().min(1),
  channelServiceName: z.string().min(1).optional(),
  warmPoolName: z.string().min(1),
  namespace: z.string().min(1),
  status: SandboxBindingStatusSchema,
  channelProtocol: z.enum(["grpc", "websocket"]),
  lastSeenAt: z.string().datetime(),
});

export const BriefPageSchema = z.object({
  title: z.string().min(1),
  purpose: z.string().min(1),
  keyContent: z.array(z.string().min(1)),
});

export const BriefSectionSchema = z.object({
  title: z.string().min(1),
  level: z.number().int().positive(),
  content: z.string(),
});

export const BriefSchema = z.object({
  projectType: z.enum(["website", "docs"]),
  audience: z.string().min(1),
  contentHierarchy: z.array(z.string().min(1)),
  pageStructure: z.union([
    z.array(BriefPageSchema),
    z.array(BriefSectionSchema),
  ]),
  visualDirection: z.string().min(1),
  recommendedTemplate: z.enum([
    "astro-website",
    "fumadocs-docs",
    "nextjs-website",
    "docusaurus-docs",
  ]),
  assumptions: z.array(z.string()),
  missingInformation: z.array(z.string()),
});

export const DesignProfileStatusSchema = z.enum(["draft", "active", "archived"]);
export const DesignProfileSchemaVersionSchema = z.enum([
  "design-profile@1",
  "design-profile@2",
]);

export const DesignProfileScopeSchema = z
  .object({
    projectId: z.string().min(1).optional(),
    workspaceId: z.string().min(1).optional(),
    organizationId: z.string().min(1).optional(),
  })
  .refine(
    (scope) => Boolean(scope.projectId || scope.workspaceId || scope.organizationId),
    "DesignProfile scope requires projectId, workspaceId, or organizationId",
  );

export const DesignSourceScopeSchema = z
  .object({
    projectId: z.string().min(1).optional(),
    workspaceId: z.string().min(1).optional(),
    organizationId: z.string().min(1).optional(),
  })
  .strict()
  .refine(
    (scope) =>
      [scope.projectId, scope.workspaceId, scope.organizationId].filter(Boolean)
        .length === 1,
    "Design source scope requires exactly one of projectId, workspaceId, or organizationId",
  );

export const DesignSourceFileNameSchema = z
  .string()
  .min(1)
  .max(255)
  .refine(
    (value) =>
      value !== "." &&
      value !== ".." &&
      !/[\\/\u0000-\u001f\u007f]/.test(value),
    "fileName must be a plain file name without path separators",
  );

export const DesignSourceArtifactSchema = z.object({
  id: z.string().min(1),
  scope: DesignSourceScopeSchema,
  fileName: DesignSourceFileNameSchema,
  mediaType: z.enum(["text/markdown", "text/plain"]),
  contentEncoding: z.literal("identity"),
  sizeBytes: z.number().int().positive().max(256 * 1024),
  sha256: z.string().regex(/^[a-fA-F0-9]{64}$/),
  createdAt: z.string().datetime(),
});

const StringListSchema = z.array(z.string().min(1));
const FidelityRouteSchema = z.string().regex(/^\/(?!\/)/, "must be a root-relative route");

export const ValueComparatorSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("exact") }),
  z.object({ kind: z.literal("contains") }),
  z.object({ kind: z.literal("color-equivalent") }),
  z.object({
    kind: z.literal("numeric-tolerance"),
    tolerance: z.number().nonnegative(),
    unit: z.string().min(1).optional(),
  }),
  z.object({
    kind: z.literal("numeric-ratio"),
    ratio: z.number(),
    tolerance: z.number().nonnegative(),
  }),
  z.object({ kind: z.literal("forbidden-anywhere") }),
]);

const DesignSignatureVerificationSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("token"),
    token: z.string().min(1),
    expected: z.string(),
    comparator: ValueComparatorSchema,
  }),
  z.object({
    kind: z.literal("computed-style"),
    route: FidelityRouteSchema,
    selector: z.string().min(1),
    property: z.string().min(1),
    referenceProperty: z.string().min(1).optional(),
    expected: z.string(),
    comparator: ValueComparatorSchema,
    minMatches: z.number().int().positive().optional(),
    excludeWithin: z.string().min(1).optional(),
    matchPolicy: z.enum(["all", "any"]).optional(),
  }),
  z.object({
    kind: z.literal("dom"),
    route: FidelityRouteSchema,
    selector: z.string().min(1),
    minMatches: z.number().int().positive(),
  }),
  z.object({
    kind: z.literal("source-pattern"),
    paths: StringListSchema.min(1),
    pattern: z.string().min(1),
  }),
  z.object({
    kind: z.literal("visual-review"),
    rubric: z.string().min(1).max(500),
  }),
]);

export const DesignSignatureRuleSchema = z
  .object({
    id: z.string().min(1),
    category: z.enum([
      "color",
      "typography",
      "spacing",
      "component",
      "composition",
      "imagery",
      "content",
    ]),
    statement: z.string().min(1).max(240),
    priority: z.enum(["required", "preferred"]),
    sourceSectionIds: StringListSchema.optional(),
    appliesTo: z.union([
      z.literal("all"),
      z
        .array(z.enum(["website", "docs"]))
        .min(1)
        .refine((values) => new Set(values).size === values.length, "appliesTo must be unique"),
    ]),
    verification: DesignSignatureVerificationSchema,
  })
  .superRefine((rule, context) => {
    if (
      rule.verification.kind === "computed-style" &&
      rule.verification.comparator.kind === "numeric-ratio" &&
      !rule.verification.referenceProperty
    ) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: ["verification", "referenceProperty"],
        message: "referenceProperty is required for numeric-ratio",
      });
    }
  });

export const RuntimeTokenMappingSchema = z.object({
  "color.background": z.string().min(1),
  "color.surface": z.string().min(1),
  "color.surfaceStrong": z.string().min(1),
  "color.text": z.string().min(1),
  "color.muted": z.string().min(1),
  "color.primary": z.string().min(1),
  "color.primaryContrast": z.string().min(1),
  "color.border": z.string().min(1),
  "radius.card": z.string().min(1),
  "radius.control": z.string().min(1),
  "font.sans": z.string().min(1),
  "shadow.soft": z.string().min(1),
});

const ComponentGuidelineSchema = z
  .object({
    role: z.string().min(1).optional(),
    intent: z.string().min(1).optional(),
    usage: StringListSchema,
    avoid: StringListSchema,
  })
  .passthrough()
  .superRefine((guideline, context) => {
    if (!guideline.role && !guideline.intent) {
      context.addIssue({
        code: "custom",
        path: ["role"],
        message: "component guideline requires role or legacy intent",
      });
    }
    if (guideline.role && guideline.intent && guideline.role !== guideline.intent) {
      context.addIssue({
        code: "custom",
        path: ["intent"],
        message: "component guideline role and intent must match",
      });
    }
  })
  .transform(({ intent, role, ...guideline }) => ({
    ...guideline,
    role: role ?? intent!,
  }));

export const DesignProfileBaseSchema = z.object({
  id: z.string().min(1),
  schemaVersion: DesignProfileSchemaVersionSchema.default("design-profile@1"),
  name: z.string().min(1),
  status: DesignProfileStatusSchema,
  version: z.number().int().positive(),
  scope: DesignProfileScopeSchema,
  source: z
    .object({
      kind: z.enum(["manual", "brief", "imported", "generated"]),
      sourceIds: StringListSchema.optional(),
      sourceArtifactIds: StringListSchema.optional(),
      primarySourceArtifactId: z.string().min(1).optional(),
      sourceHash: z.string().regex(/^[a-fA-F0-9]{64}$/).optional(),
      converterVersion: z.string().min(1).optional(),
      importedAt: z.string().datetime().optional(),
      integrity: z.enum(["verified", "unverified", "missing"]).optional(),
      notes: z.string().optional(),
    })
    .passthrough(),
  product: z
    .object({
      name: z.string().min(1),
      category: z.string().min(1),
      audience: StringListSchema,
      primaryUseCases: StringListSchema,
      productQualities: StringListSchema,
    })
    .passthrough(),
  brand: z
    .object({
      voice: z
        .object({
          tone: StringListSchema,
          sentenceStyle: z.enum(["concise", "balanced", "editorial", "technical"]),
          vocabulary: z.object({
            prefer: StringListSchema,
            avoid: StringListSchema,
          }),
          writingRules: StringListSchema,
        })
        .passthrough(),
      messaging: z
        .object({
          headlineStyle: z.string().min(1),
          bodyStyle: z.string().min(1),
          ctaStyle: z.string().min(1),
          proofStyle: z.string().min(1),
          forbiddenClaims: StringListSchema,
        })
        .passthrough(),
    })
    .passthrough(),
  visual: z
    .object({
      direction: z.string().min(1),
      principles: StringListSchema,
      moodKeywords: StringListSchema,
      avoidKeywords: StringListSchema,
      composition: z.object({}).passthrough(),
      imagery: z.object({}).passthrough(),
      motion: z.object({}).passthrough(),
    })
    .passthrough(),
  tokens: z
    .object({
      color: z.object({}).passthrough(),
      typography: z.object({}).passthrough(),
      radius: z.object({}).passthrough(),
      shadow: z.object({}).passthrough(),
      spacing: z.object({}).passthrough(),
    })
    .passthrough(),
  runtimeTokenMapping: RuntimeTokenMappingSchema,
  extendedTokenMapping: z.record(z.string(), z.string()).optional().default({}),
  components: z
    .object({
      primitives: z
        .object({
          button: ComponentGuidelineSchema,
          input: ComponentGuidelineSchema,
          card: ComponentGuidelineSchema,
          badge: ComponentGuidelineSchema,
        })
        .passthrough(),
    })
    .passthrough(),
  content: z.object({}).passthrough(),
  accessibility: z.object({}).passthrough(),
  technical: z
    .object({
      allowedTemplates: z
        .array(z.enum(["astro-website", "fumadocs-docs", "nextjs-website", "docusaurus-docs"]))
        .min(1),
      preferredTemplates: z
        .object({
          website: z.enum(["astro-website", "nextjs-website"]),
          docs: z.enum(["fumadocs-docs", "docusaurus-docs"]),
        })
        .passthrough(),
      cssStrategy: z.enum(["runtime-style-contract", "tailwind-css-variables"]),
      dependencyPolicy: z.object({}).passthrough(),
      filePolicy: z
        .object({
          designProfilePath: z.literal("/workspace/inputs/design-profile.json"),
          designMarkdownPath: z.literal("/workspace/inputs/design.md"),
          styleContractPath: z.literal("/workspace/state/style-contract.json"),
        })
        .passthrough(),
    })
    .passthrough(),
  governance: z
    .object({
      conflictBehavior: z.enum(["prefer-user", "ask", "block"]),
    })
    .passthrough(),
  signatureRules: z.array(DesignSignatureRuleSchema).max(64).optional().default([]),
  overrides: z.object({}).passthrough().optional().default({}),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime(),
});

export const DesignProfileSchema = DesignProfileBaseSchema.superRefine((profile, context) => {
  const requiredRules = profile.signatureRules.filter(
    (rule) => rule.priority === "required",
  );
  if (requiredRules.length > 24) {
    context.addIssue({
      code: "custom",
      path: ["signatureRules"],
      message: "signatureRules must contain at most 24 required rules",
    });
  }
  const ids = profile.signatureRules.map((rule) => rule.id);
  if (new Set(ids).size !== ids.length) {
    context.addIssue({
      code: "custom",
      path: ["signatureRules"],
      message: "signatureRules ids must be unique",
    });
  }
  if (profile.schemaVersion === "design-profile@2" && profile.source.kind === "imported") {
    for (const key of ["primarySourceArtifactId", "sourceHash", "converterVersion"] as const) {
      if (!profile.source[key]) {
        context.addIssue({
          code: "custom",
          path: ["source", key],
          message: `imported V2 profile requires ${key}`,
        });
      }
    }
    if (profile.source.integrity !== "verified") {
      context.addIssue({
        code: "custom",
        path: ["source", "integrity"],
        message: "imported V2 profile integrity must be verified",
      });
    }
    if (requiredRules.length === 0) {
      context.addIssue({
        code: "custom",
        path: ["signatureRules"],
        message: "imported V2 profile requires a required signature rule",
      });
    }
  }
});

export const DesignProfileValidationIssueSchema = z.object({
  path: z.string(),
  code: z.string().min(1),
  message: z.string().min(1),
  blocking: z.boolean(),
});

export const DesignProfileDraftSchema = z.object({
  id: z.string().min(1),
  schemaVersion: z.literal("design-profile@2"),
  version: z.number().int().positive(),
  name: z.string().min(1),
  status: z.literal("draft"),
  scope: DesignProfileScopeSchema,
  source: z.object({
    kind: z.literal("imported"),
    sourceArtifactIds: StringListSchema.min(1),
    primarySourceArtifactId: z.string().min(1),
    sourceHash: z.string().regex(/^[a-fA-F0-9]{64}$/),
    converterVersion: z.string().min(1),
    importedAt: z.string().datetime(),
    integrity: z.enum(["verified", "unverified", "missing"]),
  }).passthrough(),
  candidate: z.object({}).passthrough(),
  conversionReportId: z.string().min(1),
  validationIssues: z.array(DesignProfileValidationIssueSchema),
  createdAt: z.string().datetime(),
  updatedAt: z.string().datetime(),
});

export const DesignProfileRecordSchema = z.union([
  DesignProfileDraftSchema,
  DesignProfileSchema,
]);

export type AgentPhase = z.infer<typeof AgentPhaseSchema>;
export type AgentRunStatus = z.infer<typeof AgentRunStatusSchema>;
export type AgentRun = z.infer<typeof AgentRunSchema>;
export type ConversationItem = z.infer<typeof ConversationItemSchema>;
export type ReviewFinding = z.infer<typeof ReviewFindingSchema>;
export type ProjectVersionStatus = z.infer<typeof ProjectVersionStatusSchema>;
export type BriefStatus = z.infer<typeof BriefStatusSchema>;
export type ProjectVersion = z.infer<typeof ProjectVersionSchema>;
export type SandboxBindingStatus = z.infer<typeof SandboxBindingStatusSchema>;
export type SandboxBinding = z.infer<typeof SandboxBindingSchema>;
export type Brief = z.infer<typeof BriefSchema>;
export type BriefPage = z.infer<typeof BriefPageSchema>;
export type BriefSection = z.infer<typeof BriefSectionSchema>;
export type DesignProfileStatus = z.infer<typeof DesignProfileStatusSchema>;
export type DesignProfileSchemaVersion = z.infer<typeof DesignProfileSchemaVersionSchema>;
export type DesignProfileScope = z.infer<typeof DesignProfileScopeSchema>;
export type DesignSourceArtifact = z.infer<typeof DesignSourceArtifactSchema>;
export type RuntimeTokenMapping = z.infer<typeof RuntimeTokenMappingSchema>;
export type DesignSignatureRule = z.infer<typeof DesignSignatureRuleSchema>;
export type DesignProfile = z.infer<typeof DesignProfileSchema>;
export type DesignProfileDraft = z.infer<typeof DesignProfileDraftSchema>;
export type DesignProfileRecord = z.infer<typeof DesignProfileRecordSchema>;
