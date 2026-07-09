import { z } from "zod";
import {
  CancelRunResponseSchema,
  ContinueRunRequest,
  ContinueRunResponseSchema,
  ConversationListResponseSchema,
  ErrorResponseSchema,
  HealthResponseSchema,
  PreviewCurrentResponseSchema,
  PreviewVersionResponseSchema,
  ProjectRuntimeStateResponseSchema,
  ResolvePermissionRequest,
  ResolvePermissionResponseSchema,
  StartRunRequest,
  StartRunResponseSchema,
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
};

export type RuntimeClientOptions = {
  baseUrl: string;
  fetch?: RuntimeFetch;
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
  runEventsUrl(runId: string, options?: { lastEventId?: string }): string;
  runtimeRunEventsPath(runId: string): string;
  runEventsProxyHeaders(lastEventId?: string): Record<string, string>;
  subscribeRunEvents(options: SubscribeRunEventsOptions): RunEventSubscription;
};

export function createRuntimeClient(options: RuntimeClientOptions): RuntimeClient {
  const baseUrl = normalizeBaseUrl(options.baseUrl);
  const fetchImpl = options.fetch ?? globalFetch();

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

function normalizeBaseUrl(baseUrl: string): string {
  return baseUrl.replace(/\/+$/, "");
}

function encodePathSegment(value: string): string {
  return encodeURIComponent(value);
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
