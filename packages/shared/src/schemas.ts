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
  visibility: z.enum(["user", "debug"]).default("user"),
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
