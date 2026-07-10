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

const designProfile = () => ({
  id: "design-profile-1",
  name: "Harness Calm Ops",
  status: "active",
  version: 1,
  scope: { projectId: "project-1" },
  source: { kind: "manual" },
  product: {
    name: "AnyDesign Runtime",
    category: "agent harness",
    audience: ["internal builders"],
    primaryUseCases: ["generate websites"],
    productQualities: ["reliable"],
  },
  brand: {
    voice: {
      tone: ["clear"],
      sentenceStyle: "technical",
      vocabulary: { prefer: ["runtime"], avoid: ["magic"] },
      writingRules: ["Use concrete status text."],
    },
    messaging: {
      headlineStyle: "specific",
      bodyStyle: "concise",
      ctaStyle: "verb first",
      proofStyle: "evidence based",
      forbiddenClaims: ["guaranteed"],
    },
  },
  visual: {
    direction: "quiet operational interface",
    principles: ["scan friendly"],
    moodKeywords: ["calm"],
    avoidKeywords: ["flashy"],
    composition: {},
    imagery: {},
    motion: {},
  },
  tokens: { color: {}, typography: {}, radius: {}, shadow: {}, spacing: {} },
  runtimeTokenMapping: {
    "color.background": "#ffffff",
    "color.surface": "#f8fafc",
    "color.surfaceStrong": "#e2e8f0",
    "color.text": "#0f172a",
    "color.muted": "#475569",
    "color.primary": "#2563eb",
    "color.primaryContrast": "#ffffff",
    "color.border": "#cbd5e1",
    "radius.card": "8px",
    "radius.control": "6px",
    "font.sans": "Inter, sans-serif",
    "shadow.soft": "0 1px 2px rgba(15, 23, 42, 0.12)",
  },
  components: {
    primitives: {
      button: { intent: "clear action", usage: ["primary actions"], avoid: ["overuse"] },
      input: { intent: "precise entry", usage: ["forms"], avoid: ["placeholder-only labels"] },
      card: { intent: "group repeated items", usage: ["lists"], avoid: ["nested cards"] },
      badge: { intent: "show status", usage: ["statuses"], avoid: ["decorative noise"] },
    },
  },
  content: {},
  accessibility: {},
  technical: {
    allowedTemplates: ["astro-website", "fumadocs-docs"],
    preferredTemplates: { website: "astro-website", docs: "fumadocs-docs" },
    cssStrategy: "runtime-style-contract",
    dependencyPolicy: {},
    filePolicy: {
      designProfilePath: "/workspace/inputs/design-profile.json",
      designMarkdownPath: "/workspace/inputs/design.md",
      styleContractPath: "/workspace/state/style-contract.json",
    },
  },
  governance: { conflictBehavior: "ask" },
  createdAt: timestamp,
  updatedAt: timestamp,
});

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

  it("calls DesignProfile endpoints with shared validation", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      const profile = designProfile();
      return {
        ok: true,
        status: 200,
        async json() {
          if (url.toString().includes("/versions")) {
            return { designProfileId: "design-profile-1", versions: [profile] };
          }
          if (url.toString().includes("/diff")) {
            return {
              designProfileId: "design-profile-1",
              fromVersion: 1,
              toVersion: 2,
              changes: [
                {
                  path: "visual.direction",
                  before: "clean enterprise calm",
                  after: "editorial enterprise calm",
                },
              ],
            };
          }
          if (url.toString().includes("includeArchived=true")) {
            return { designProfiles: [profile] };
          }
          if (url.toString().includes("/projects/")) {
            return { projectId: "project-1", designProfile: profile };
          }
          return { designProfile: profile };
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local/",
      fetch: fetchImpl,
    });
    const createPayload = {
      projectId: "project-1",
      name: "Harness Calm Ops",
      profile: {
        ...designProfile(),
        id: undefined,
        name: undefined,
        version: undefined,
        createdAt: undefined,
        updatedAt: undefined,
      },
    } as Parameters<typeof client.createDesignProfile>[0];

    expect((await client.createDesignProfile(createPayload)).designProfile.id).toBe(
      "design-profile-1",
    );
    expect((await client.getDesignProfile("design-profile-1")).designProfile.id).toBe(
      "design-profile-1",
    );
    expect((await client.getDesignProfileVersions("design-profile-1")).versions).toHaveLength(1);
    expect(
      (
        await client.diffDesignProfileVersions("design-profile-1", {
          fromVersion: 1,
          toVersion: 2,
        })
      ).changes[0]?.path,
    ).toBe("visual.direction");
    expect(
      (
        await client.listDesignProfiles({
          projectId: "project-1",
          workspaceId: "workspace-1",
          includeArchived: true,
        })
      ).designProfiles,
    ).toHaveLength(1);
    expect(
      (
        await client.updateDesignProfile("design-profile-1", {
          name: "Harness Calm Ops v2",
          profile: createPayload.profile,
        })
      ).designProfile.id,
    ).toBe("design-profile-1");
    expect((await client.archiveDesignProfile("design-profile-1")).designProfile.id).toBe(
      "design-profile-1",
    );
    expect(
      (
        await client.bindProjectDesignProfile("project-1", {
          designProfileId: "design-profile-1",
        })
      ).designProfile?.id,
    ).toBe("design-profile-1");
    expect((await client.getProjectDesignProfile("project-1")).designProfile?.id).toBe(
      "design-profile-1",
    );

    expect(calls.map((call) => call.url)).toEqual([
      "http://runtime.local/design-profiles",
      "http://runtime.local/design-profiles/design-profile-1",
      "http://runtime.local/design-profiles/design-profile-1/versions",
      "http://runtime.local/design-profiles/design-profile-1/diff?fromVersion=1&toVersion=2",
      "http://runtime.local/design-profiles?projectId=project-1&workspaceId=workspace-1&includeArchived=true",
      "http://runtime.local/design-profiles/design-profile-1",
      "http://runtime.local/design-profiles/design-profile-1/archive",
      "http://runtime.local/projects/project-1/design-profile",
      "http://runtime.local/projects/project-1/design-profile",
    ]);
    expect(calls[5].init?.method).toBe("PUT");
    expect(calls[6].init?.method).toBe("POST");
  });

  it("uses service authorization for immutable design source endpoints", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const content = new TextEncoder().encode("# Design\n");
    const artifact = {
      id: "design-source-1",
      scope: { projectId: "project-1" },
      fileName: "DESIGN.md",
      mediaType: "text/markdown",
      contentEncoding: "identity",
      sizeBytes: content.byteLength,
      sha256: "a".repeat(64),
      createdAt: timestamp,
    };
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return {
        ok: true,
        status: 200,
        async json() {
          return { artifact };
        },
        async arrayBuffer() {
          return content.buffer.slice(
            content.byteOffset,
            content.byteOffset + content.byteLength,
          ) as ArrayBuffer;
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      internalAdminToken: "runtime-secret",
    });

    expect(
      (
        await client.createDesignSourceArtifact({
          scope: { projectId: "project-1" },
          fileName: "DESIGN.md",
          mediaType: "text/markdown",
          contentBase64: "IyBEZXNpZ24K",
          clientSha256: "a".repeat(64),
        })
      ).artifact.id,
    ).toBe("design-source-1");
    expect((await client.getDesignSourceArtifact("design-source-1")).artifact.id).toBe(
      "design-source-1",
    );
    expect(new TextDecoder().decode(await client.getDesignSourceArtifactContent("design-source-1")))
      .toBe("# Design\n");

    for (const call of calls) {
      expect(call.init?.headers).toMatchObject({
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": "runtime-secret",
      });
    }
    expect(calls[0]?.init?.method).toBe("POST");
  });

  it("imports, reviews, and activates design profile drafts through service endpoints", async () => {
    const calls: Array<{ url: string; init?: Parameters<RuntimeFetch>[1] }> = [];
    const report = {
      id: "conversion-report-1",
      designProfileId: "design-profile-draft-1",
      profileVersion: 1,
      converterVersion: "design-profile-import@1",
      deterministicParserVersion: "markdown-css-parser@1",
      sourceArtifactId: "design-source-1",
      sourceHash: "a".repeat(64),
      extractedSections: ["Design"],
      extractedTokenCount: 1,
      extractedComponentCount: 0,
      requiredSignatureRuleCount: 0,
      unmappedItems: [],
      warnings: ["review required"],
      createdAt: timestamp,
    };
    const draft = {
      id: "design-profile-draft-1",
      schemaVersion: "design-profile@2",
      version: 1,
      name: "Imported Design",
      status: "draft",
      scope: { projectId: "project-1" },
      source: {
        kind: "imported",
        sourceArtifactIds: ["design-source-1"],
        primarySourceArtifactId: "design-source-1",
        sourceHash: "a".repeat(64),
        converterVersion: "design-profile-import@1",
        importedAt: timestamp,
        integrity: "verified",
      },
      candidate: { tokens: { color: { primary: "#663af3" } } },
      conversionReportId: "conversion-report-1",
      validationIssues: [
        {
          path: "product",
          code: "required",
          message: "product is required before activation",
          blocking: true,
        },
      ],
      createdAt: timestamp,
      updatedAt: timestamp,
    } as const;
    const fidelityReport = {
      designProfileId: "design-profile-draft-1",
      version: 3,
      schemaVersion: "design-profile@2",
      surface: "website",
      template: "astro-website",
      styleContractVersion: "runtime-style-contract@p3",
      effectiveProfileHash: "b".repeat(64),
      sourceIntegrity: "verified",
      sourceHashMatches: true,
      requiredSignatureRuleIds: ["primary-color"],
      capsuleIncludedRuleIds: ["primary-color"],
      capsuleMissingRuleIds: [],
      unsupportedExtendedTokens: [],
      warnings: [],
    };
    const fetchImpl: RuntimeFetch = async (url, init) => {
      calls.push({ url: url.toString(), init });
      return {
        ok: true,
        status: 200,
        async json() {
          if (url.toString().endsWith("/import")) {
            return { designProfileDraft: draft, conversionReport: report, requiresReview: true };
          }
          if (url.toString().includes("fidelity-report")) return fidelityReport;
          if (url.toString().includes("conversion-report")) return report;
          return { designProfile: designProfile() };
        },
      };
    };
    const client = createRuntimeClient({
      baseUrl: "http://runtime.local",
      fetch: fetchImpl,
      internalAdminToken: "runtime-secret",
    });

    expect(
      (
        await client.importDesignProfile({
          name: "Imported Design",
          scope: { projectId: "project-1" },
          sourceArtifactId: "design-source-1",
        })
      ).designProfileDraft.status,
    ).toBe("draft");
    expect(
      (await client.getDesignProfileConversionReport("design-profile-draft-1", 1)).id,
    ).toBe("conversion-report-1");
    expect(
      (
        await client.activateDesignProfile("design-profile-draft-1", {
          expectedVersion: 2,
        })
      ).designProfile.status,
    ).toBe("active");
    expect(
      (
        await client.getDesignProfileFidelityReport("design-profile-draft-1", 3, {
          surface: "website",
          template: "astro-website",
        })
      ).capsuleMissingRuleIds,
    ).toEqual([]);

    expect(calls.map((call) => call.url)).toEqual([
      "http://runtime.local/design-profiles/import",
      "http://runtime.local/design-profiles/design-profile-draft-1/versions/1/conversion-report",
      "http://runtime.local/design-profiles/design-profile-draft-1/activate",
      "http://runtime.local/design-profiles/design-profile-draft-1/versions/3/fidelity-report?surface=website&template=astro-website",
    ]);
    for (const call of calls.slice(0, 3)) {
      expect(call.init?.headers).toMatchObject({
        "x-anydesign-internal": "true",
        "x-runtime-admin-token": "runtime-secret",
      });
    }
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
