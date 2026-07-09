import { describe, expect, it } from "vitest";
import {
  createRuntimeClient,
  parseRunEventMessage,
  runEventsProxyHeaders,
  runEventsUrl,
  RuntimeApiError,
  subscribeRunEvents,
  type EventSourceLike,
  type RuntimeFetch,
} from "./runtime-client.js";

const timestamp = "2026-07-09T00:00:00.000Z";

class MockEventSource implements EventSourceLike {
  static instances: MockEventSource[] = [];
  onmessage: ((event: { data: string; lastEventId?: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  closed = false;

  constructor(
    readonly url: string,
    readonly init?: { withCredentials?: boolean },
  ) {
    MockEventSource.instances.push(this);
  }

  emit(data: unknown, lastEventId = "run-1/1") {
    this.onmessage?.({ data: JSON.stringify(data), lastEventId });
  }

  close() {
    this.closed = true;
  }
}

describe("runtime client", () => {
  it("calls runtime JSON endpoints with shared response validation", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return {
        ok: true,
        status: 200,
        async json() {
          return { runId: "run-1", status: "queued" };
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local/",
      fetch: fetchImpl,
    });

    const response = await client.startRun({
      projectId: "project-1",
      phase: "brief",
      agentProfile: "brief",
      inputContext: {},
    });

    expect(response).toEqual({ runId: "run-1", status: "queued" });
    expect(calls).toEqual([
      {
        url: "http://runtime.local/runs",
        init: {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            projectId: "project-1",
            phase: "brief",
            agentProfile: "brief",
            inputContext: {},
          }),
        },
      },
    ]);
  });

  it("surfaces runtime error responses", async () => {
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: async () => ({
        ok: false,
        status: 404,
        async json() {
          return { error: "run not found: run-missing" };
        },
      }),
    });

    await expect(client.health()).rejects.toMatchObject({
      name: "RuntimeApiError",
      status: 404,
      message: "run not found: run-missing",
    } satisfies Partial<RuntimeApiError>);
  });

  it("builds SSE URLs and proxy headers for browser-to-BFF reconnects", () => {
    expect(runEventsUrl("http://bff.local/api", "run/with space")).toBe(
      "http://bff.local/api/runs/run%2Fwith%20space/events",
    );
    expect(runEventsUrl("http://bff.local/api", "run-1", "run-1/3")).toBe(
      "http://bff.local/api/runs/run-1/events?lastEventId=run-1%2F3",
    );
    expect(runEventsProxyHeaders("run-1/3")).toEqual({
      "last-event-id": "run-1/3",
    });
    expect(runEventsProxyHeaders()).toEqual({});
  });

  it("parses run event messages with the shared AgentEvent schema", () => {
    const message = parseRunEventMessage(
      JSON.stringify({
        type: "agent.message",
        runId: "run-1",
        text: "Working",
        timestamp,
      }),
      "run-1/2",
    );

    expect(message).toEqual({
      id: "run-1/2",
      event: {
        type: "agent.message",
        runId: "run-1",
        text: "Working",
        timestamp,
      },
    });
  });

  it("subscribes with EventSource and closes on terminal run.completed", () => {
    MockEventSource.instances = [];
    const received: unknown[] = [];
    const subscription = subscribeRunEvents("http://bff.local/api", {
      runId: "run-1",
      lastEventId: "run-1/2",
      withCredentials: true,
      EventSource: MockEventSource,
      onEvent: (message) => received.push(message),
    });
    const source = MockEventSource.instances[0];

    expect(source.url).toBe("http://bff.local/api/runs/run-1/events?lastEventId=run-1%2F2");
    expect(source.init).toEqual({ withCredentials: true });

    source.emit(
      {
        type: "run.completed",
        runId: "run-1",
        status: "completed",
        summary: "done",
        timestamp,
      },
      "run-1/3",
    );

    expect(received).toHaveLength(1);
    expect(source.closed).toBe(true);
    subscription.close();
    expect(source.closed).toBe(true);
  });
});
