import { z } from "zod";
import {
  ActivateDesignProfileRequest,
  ActivateDesignProfileResponseSchema,
  CancelRunResponseSchema,
  BindProjectDesignProfileRequest,
  ContinueRunRequest,
  ContinueRunResponseSchema,
  ConversationListResponseSchema,
  CreateDesignSourceArtifactRequest,
  CreateDesignProfileRequest,
  DesignSourceArtifactResponseSchema,
  DesignProfileDiffResponseSchema,
  DesignProfileConversionReportSchema,
  DesignProfileFidelityReportSchema,
  DesignProfileResponseSchema,
  DesignProfileVersionsResponseSchema,
  ErrorResponseSchema,
  HealthResponseSchema,
  ImportDesignProfileRequest,
  ImportDesignProfileResponseSchema,
  ListDesignProfilesResponseSchema,
  PreviewCurrentResponseSchema,
  PreviewVersionResponseSchema,
  ProjectAccessResponseSchema,
  ProjectDesignProfileResponseSchema,
  ProjectRuntimeStateResponseSchema,
  ResolvePermissionRequest,
  ResolvePermissionResponseSchema,
  StartRunRequest,
  StartRunResponseSchema,
  UpsertProjectAccessRequest,
  UpdateDesignProfileRequest,
} from "./api-types.js";
import { AgentEventSchema, type AgentEvent } from "./events.js";

export type RuntimeFetch = (
  input: string | URL,
  init?: {
    method?: string;
    headers?: Record<string, string>;
    body?: string;
  },
) => Promise<RuntimeResponse>;

export type RuntimeResponse = {
  ok: boolean;
  status: number;
  statusText?: string;
  json(): Promise<unknown>;
  arrayBuffer?(): Promise<ArrayBuffer>;
};

export type RuntimeClientOptions = {
  baseUrl: string;
  fetch?: RuntimeFetch;
  internalAdminToken?: string;
};

export class RuntimeApiError extends Error {
  readonly status: number;
  readonly payload: unknown;

  constructor(status: number, message: string, payload: unknown) {
    super(message);
    this.name = "RuntimeApiError";
    this.status = status;
    this.payload = payload;
  }
}

export type RunEventEnvelope = {
  id: string;
  event: AgentEvent;
};

export type EventSourceLike = {
  onmessage: ((event: { data: string; lastEventId?: string }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  close(): void;
};

export type EventSourceConstructor = new (
  url: string,
  init?: { withCredentials?: boolean },
) => EventSourceLike;

export type SubscribeRunEventsOptions = {
  runId: string;
  lastEventId?: string;
  withCredentials?: boolean;
  closeOnCompleted?: boolean;
  EventSource?: EventSourceConstructor;
  onEvent: (message: RunEventEnvelope) => void;
  onError?: (error: unknown) => void;
};

export type RunEventSubscription = {
  close(): void;
};

export type RuntimeClient = {
  health(): Promise<z.output<typeof HealthResponseSchema>>;
  startRun(request: StartRunRequest): Promise<z.output<typeof StartRunResponseSchema>>;
  continueRun(
    runId: string,
    request: ContinueRunRequest,
  ): Promise<z.output<typeof ContinueRunResponseSchema>>;
  cancelRun(runId: string): Promise<z.output<typeof CancelRunResponseSchema>>;
  createDesignSourceArtifact(
    request: CreateDesignSourceArtifactRequest,
  ): Promise<z.output<typeof DesignSourceArtifactResponseSchema>>;
  getDesignSourceArtifact(
    artifactId: string,
  ): Promise<z.output<typeof DesignSourceArtifactResponseSchema>>;
  getDesignSourceArtifactContent(artifactId: string): Promise<Uint8Array>;
  importDesignProfile(
    request: ImportDesignProfileRequest,
  ): Promise<z.output<typeof ImportDesignProfileResponseSchema>>;
  activateDesignProfile(
    designProfileId: string,
    request: ActivateDesignProfileRequest,
  ): Promise<z.output<typeof ActivateDesignProfileResponseSchema>>;
  getDesignProfileConversionReport(
    designProfileId: string,
    version?: number,
  ): Promise<z.output<typeof DesignProfileConversionReportSchema>>;
  getDesignProfileFidelityReport(
    designProfileId: string,
    version: number,
    query: { surface: "website" | "docs"; template: string },
  ): Promise<z.output<typeof DesignProfileFidelityReportSchema>>;
  createDesignProfile(
    request: CreateDesignProfileRequest,
  ): Promise<z.output<typeof DesignProfileResponseSchema>>;
  getDesignProfile(
    designProfileId: string,
  ): Promise<z.output<typeof DesignProfileResponseSchema>>;
  getDesignProfileVersions(
    designProfileId: string,
  ): Promise<z.output<typeof DesignProfileVersionsResponseSchema>>;
  diffDesignProfileVersions(
    designProfileId: string,
    query: { fromVersion: number; toVersion: number },
  ): Promise<z.output<typeof DesignProfileDiffResponseSchema>>;
  listDesignProfiles(query?: {
    projectId?: string;
    workspaceId?: string;
    organizationId?: string;
    includeArchived?: boolean;
  }): Promise<z.output<typeof ListDesignProfilesResponseSchema>>;
  updateDesignProfile(
    designProfileId: string,
    request: UpdateDesignProfileRequest,
  ): Promise<z.output<typeof DesignProfileResponseSchema>>;
  archiveDesignProfile(
    designProfileId: string,
  ): Promise<z.output<typeof DesignProfileResponseSchema>>;
  bindProjectDesignProfile(
    projectId: string,
    request: BindProjectDesignProfileRequest,
  ): Promise<z.output<typeof ProjectDesignProfileResponseSchema>>;
  getProjectDesignProfile(
    projectId: string,
  ): Promise<z.output<typeof ProjectDesignProfileResponseSchema>>;
  resolvePermission(
    permissionId: string,
    request: ResolvePermissionRequest,
  ): Promise<z.output<typeof ResolvePermissionResponseSchema>>;
  getConversation(projectId: string): Promise<z.output<typeof ConversationListResponseSchema>>;
  getProjectRuntimeState(
    projectId: string,
  ): Promise<z.output<typeof ProjectRuntimeStateResponseSchema>>;
  getPreviewCurrent(projectId: string): Promise<z.output<typeof PreviewCurrentResponseSchema>>;
  getPreviewVersion(
    projectId: string,
    versionId: string,
  ): Promise<z.output<typeof PreviewVersionResponseSchema>>;
  upsertProjectAccess(
    projectId: string,
    request: UpsertProjectAccessRequest,
  ): Promise<z.output<typeof ProjectAccessResponseSchema>>;
  runtimePreviewPath(leaseId: string, previewPath?: string): string;
  previewProxyHeaders(principalToken: string, previewPrefix: string): Record<string, string>;
  runEventsUrl(runId: string, options?: { lastEventId?: string }): string;
  runtimeRunEventsPath(runId: string): string;
  runEventsProxyHeaders(lastEventId?: string): Record<string, string>;
  subscribeRunEvents(options: SubscribeRunEventsOptions): RunEventSubscription;
};

export function createRuntimeClient(options: RuntimeClientOptions): RuntimeClient {
  const baseUrl = normalizeBaseUrl(options.baseUrl);
  const fetchImpl = options.fetch ?? globalFetch();
  const internalHeaders: Record<string, string> = options.internalAdminToken
    ? {
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": options.internalAdminToken,
      }
    : {};

  async function get<TSchema extends z.ZodTypeAny>(
    path: string,
    schema: TSchema,
  ): Promise<z.infer<TSchema>> {
    return requestJson(fetchImpl, baseUrl, path, schema);
  }

  async function post<TSchema extends z.ZodTypeAny>(
    path: string,
    body: unknown,
    schema: TSchema,
  ): Promise<z.infer<TSchema>> {
    return requestJson(fetchImpl, baseUrl, path, schema, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  }

  return {
    health: () => get("/health", HealthResponseSchema),
    startRun: (request) => post("/runs", request, StartRunResponseSchema),
    continueRun: (runId, request) =>
      post(`/runs/${encodePathSegment(runId)}/continue`, request, ContinueRunResponseSchema),
    cancelRun: (runId) =>
      post(`/runs/${encodePathSegment(runId)}/cancel`, {}, CancelRunResponseSchema),
    createDesignSourceArtifact: (request) =>
      requestJson(
        fetchImpl,
        baseUrl,
        "/design-source-artifacts",
        DesignSourceArtifactResponseSchema,
        {
          method: "POST",
          headers: { "content-type": "application/json", ...internalHeaders },
          body: JSON.stringify(request),
        },
      ),
    getDesignSourceArtifact: (artifactId) =>
      requestJson(
        fetchImpl,
        baseUrl,
        `/design-source-artifacts/${encodePathSegment(artifactId)}`,
        DesignSourceArtifactResponseSchema,
        { headers: internalHeaders },
      ),
    getDesignSourceArtifactContent: (artifactId) =>
      requestBytes(
        fetchImpl,
        baseUrl,
        `/design-source-artifacts/${encodePathSegment(artifactId)}/content`,
        { headers: internalHeaders },
      ),
    importDesignProfile: (request) =>
      requestJson(
        fetchImpl,
        baseUrl,
        "/design-profiles/import",
        ImportDesignProfileResponseSchema,
        {
          method: "POST",
          headers: { "content-type": "application/json", ...internalHeaders },
          body: JSON.stringify(request),
        },
      ),
    activateDesignProfile: (designProfileId, request) =>
      requestJson(
        fetchImpl,
        baseUrl,
        `/design-profiles/${encodePathSegment(designProfileId)}/activate`,
        ActivateDesignProfileResponseSchema,
        {
          method: "POST",
          headers: { "content-type": "application/json", ...internalHeaders },
          body: JSON.stringify(request),
        },
      ),
    getDesignProfileConversionReport: (designProfileId, version) =>
      requestJson(
        fetchImpl,
        baseUrl,
        version === undefined
          ? `/design-profiles/${encodePathSegment(designProfileId)}/conversion-report`
          : `/design-profiles/${encodePathSegment(designProfileId)}/versions/${version}/conversion-report`,
        DesignProfileConversionReportSchema,
        { headers: internalHeaders },
      ),
    getDesignProfileFidelityReport: (designProfileId, version, query) =>
      get(
        designProfileFidelityReportPath(designProfileId, version, query),
        DesignProfileFidelityReportSchema,
      ),
    createDesignProfile: (request) =>
      post("/design-profiles", request, DesignProfileResponseSchema),
    getDesignProfile: (designProfileId) =>
      get(
        `/design-profiles/${encodePathSegment(designProfileId)}`,
        DesignProfileResponseSchema,
      ),
    getDesignProfileVersions: (designProfileId) =>
      get(
        `/design-profiles/${encodePathSegment(designProfileId)}/versions`,
        DesignProfileVersionsResponseSchema,
      ),
    diffDesignProfileVersions: (designProfileId, query) =>
      get(designProfileDiffPath(designProfileId, query), DesignProfileDiffResponseSchema),
    listDesignProfiles: (query) =>
      get(designProfilesPath(query), ListDesignProfilesResponseSchema),
    updateDesignProfile: (designProfileId, request) =>
      requestJson(
        fetchImpl,
        baseUrl,
        `/design-profiles/${encodePathSegment(designProfileId)}`,
        DesignProfileResponseSchema,
        {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(request),
        },
      ),
    archiveDesignProfile: (designProfileId) =>
      post(
        `/design-profiles/${encodePathSegment(designProfileId)}/archive`,
        {},
        DesignProfileResponseSchema,
      ),
    bindProjectDesignProfile: (projectId, request) =>
      post(
        `/projects/${encodePathSegment(projectId)}/design-profile`,
        request,
        ProjectDesignProfileResponseSchema,
      ),
    getProjectDesignProfile: (projectId) =>
      get(
        `/projects/${encodePathSegment(projectId)}/design-profile`,
        ProjectDesignProfileResponseSchema,
      ),
    resolvePermission: (permissionId, request) =>
      post(
        `/permissions/${encodePathSegment(permissionId)}/decision`,
        request,
        ResolvePermissionResponseSchema,
      ),
    getConversation: (projectId) =>
      get(`/projects/${encodePathSegment(projectId)}/conversation`, ConversationListResponseSchema),
    getProjectRuntimeState: (projectId) =>
      get(
        `/projects/${encodePathSegment(projectId)}/runtime-state`,
        ProjectRuntimeStateResponseSchema,
      ),
    getPreviewCurrent: (projectId) =>
      get(`/preview/${encodePathSegment(projectId)}/current`, PreviewCurrentResponseSchema),
    getPreviewVersion: (projectId, versionId) =>
      get(
        `/preview/${encodePathSegment(projectId)}/${encodePathSegment(versionId)}`,
        PreviewVersionResponseSchema,
      ),
    upsertProjectAccess: (projectId, request) =>
      requestJson(
        fetchImpl,
        baseUrl,
        `/internal/projects/${encodePathSegment(projectId)}/access`,
        ProjectAccessResponseSchema,
        {
          method: "PUT",
          headers: { "content-type": "application/json", ...internalHeaders },
          body: JSON.stringify(request),
        },
      ),
    runtimePreviewPath,
    previewProxyHeaders,
    runEventsUrl: (runId, eventOptions) =>
      runEventsUrl(baseUrl, runId, eventOptions?.lastEventId),
    runtimeRunEventsPath,
    runEventsProxyHeaders,
    subscribeRunEvents: (subscribeOptions) =>
      subscribeRunEvents(baseUrl, subscribeOptions),
  };
}

export function runtimeRunEventsPath(runId: string): string {
  return `/runs/${encodePathSegment(runId)}/events`;
}

export function runEventsUrl(
  baseUrl: string,
  runId: string,
  lastEventId?: string,
): string {
  const url = new URL(
    `${normalizeBaseUrl(baseUrl)}${runtimeRunEventsPath(runId)}`,
  );
  if (lastEventId) {
    url.searchParams.set("lastEventId", lastEventId);
  }
  return url.toString();
}

export function runEventsProxyHeaders(lastEventId?: string): Record<string, string> {
  return lastEventId ? { "last-event-id": lastEventId } : {};
}

export function runtimePreviewPath(leaseId: string, previewPath = ""): string {
  const suffix = previewPath
    .split("/")
    .filter(Boolean)
    .map(encodePathSegment)
    .join("/");
  return `/previews/${encodePathSegment(leaseId)}/${suffix}`;
}

export function previewProxyHeaders(
  principalToken: string,
  previewPrefix: string,
): Record<string, string> {
  if (!principalToken.trim()) {
    throw new Error("preview principal token is required");
  }
  if (!previewPrefix.startsWith("/projects/")) {
    throw new Error("BFF preview prefix is required");
  }
  return {
    authorization: `Bearer ${principalToken}`,
    "x-anydesign-preview-prefix": previewPrefix,
  };
}

export function parseRunEventMessage(data: string, id = ""): RunEventEnvelope {
  return {
    id,
    event: AgentEventSchema.parse(JSON.parse(data)),
  };
}

export function subscribeRunEvents(
  baseUrl: string,
  options: SubscribeRunEventsOptions,
): RunEventSubscription {
  const EventSourceCtor = options.EventSource ?? globalEventSource();
  const source = new EventSourceCtor(
    runEventsUrl(baseUrl, options.runId, options.lastEventId),
    { withCredentials: options.withCredentials },
  );
  let closed = false;
  const close = () => {
    if (closed) {
      return;
    }
    closed = true;
    source.close();
  };
  source.onmessage = (message) => {
    try {
      const parsed = parseRunEventMessage(message.data, message.lastEventId ?? "");
      options.onEvent(parsed);
      if (options.closeOnCompleted !== false && parsed.event.type === "run.completed") {
        close();
      }
    } catch (error) {
      options.onError?.(error);
    }
  };
  source.onerror = (error) => {
    options.onError?.(error);
  };
  return { close };
}

async function requestJson<TSchema extends z.ZodTypeAny>(
  fetchImpl: RuntimeFetch,
  baseUrl: string,
  path: string,
  schema: TSchema,
  init?: Parameters<RuntimeFetch>[1],
): Promise<z.infer<TSchema>> {
  const response = await fetchImpl(new URL(path, `${baseUrl}/`).toString(), init);
  const payload = await response.json();
  if (!response.ok) {
    const parsedError = ErrorResponseSchema.safeParse(payload);
    throw new RuntimeApiError(
      response.status,
      parsedError.success
        ? parsedError.data.error
        : response.statusText || `Runtime request failed with ${response.status}`,
      payload,
    );
  }
  return schema.parse(payload);
}

async function requestBytes(
  fetchImpl: RuntimeFetch,
  baseUrl: string,
  path: string,
  init?: Parameters<RuntimeFetch>[1],
): Promise<Uint8Array> {
  const response = await fetchImpl(new URL(path, `${baseUrl}/`).toString(), init);
  if (!response.ok) {
    const payload = await response.json();
    const parsedError = ErrorResponseSchema.safeParse(payload);
    throw new RuntimeApiError(
      response.status,
      parsedError.success
        ? parsedError.data.error
        : response.statusText || `Runtime request failed with ${response.status}`,
      payload,
    );
  }
  if (!response.arrayBuffer) {
    throw new Error("Runtime fetch implementation does not support binary responses");
  }
  return new Uint8Array(await response.arrayBuffer());
}

function normalizeBaseUrl(baseUrl: string): string {
  return baseUrl.replace(/\/+$/, "");
}

function encodePathSegment(value: string): string {
  return encodeURIComponent(value);
}

function designProfilesPath(query?: {
  projectId?: string;
  workspaceId?: string;
  organizationId?: string;
  includeArchived?: boolean;
}): string {
  const params = new URLSearchParams();
  if (query?.projectId) {
    params.set("projectId", query.projectId);
  }
  if (query?.workspaceId) {
    params.set("workspaceId", query.workspaceId);
  }
  if (query?.organizationId) {
    params.set("organizationId", query.organizationId);
  }
  if (query?.includeArchived) {
    params.set("includeArchived", "true");
  }
  const suffix = params.toString();
  return suffix ? `/design-profiles?${suffix}` : "/design-profiles";
}

function designProfileDiffPath(
  designProfileId: string,
  query: { fromVersion: number; toVersion: number },
): string {
  const params = new URLSearchParams();
  params.set("fromVersion", String(query.fromVersion));
  params.set("toVersion", String(query.toVersion));
  return `/design-profiles/${encodePathSegment(designProfileId)}/diff?${params.toString()}`;
}

function designProfileFidelityReportPath(
  designProfileId: string,
  version: number,
  query: { surface: "website" | "docs"; template: string },
): string {
  const params = new URLSearchParams();
  params.set("surface", query.surface);
  params.set("template", query.template);
  return `/design-profiles/${encodePathSegment(designProfileId)}/versions/${version}/fidelity-report?${params.toString()}`;
}

function globalFetch(): RuntimeFetch {
  if (typeof fetch !== "function") {
    throw new Error("Runtime client requires fetch to be provided");
  }
  return fetch as RuntimeFetch;
}

function globalEventSource(): EventSourceConstructor {
  if (typeof EventSource === "undefined") {
    throw new Error("Runtime event subscription requires EventSource to be provided");
  }
  return EventSource as EventSourceConstructor;
}
