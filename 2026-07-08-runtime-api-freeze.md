---
date: 2026-07-08
status: freeze-candidate
type: api-contract
topic: phase-a-runtime-http-sse-freeze
based_on_commit: d60e2f9
---

# Phase A Runtime API Freeze Candidate

## Summary

This document records the Phase A Runtime HTTP/SSE contract implemented by
`services/runtime/src/http_api.rs` and the shared TypeScript schemas in
`packages/shared/src`. It is the freeze candidate for Phase B BFF and product UI
work.

The Phase A freeze gate and real `deepseek-v4-pro` website generation regression
passed on 2026-07-08. See `2026-07-08-phase-a-acceptance-report.md` for the
test evidence.

API review note: the runtime implementation currently emits a few SSE variants
and `tool.failed.metadata` fields that are not yet represented in
`packages/shared/src/events.ts`. Phase B should not treat the shared event schema
as final until the "Phase B Freeze Blockers" section below is resolved.

## Phase B Freeze Blockers

These items must be resolved before this candidate becomes the final Phase B API
freeze:

| Priority | Area | Required action |
|---|---|---|
| P1 | SSE event schema | Add `tool.recovery_suggested`, `chunk.received`, `chunk.committed`, and `metric.recorded` to `packages/shared/src/events.ts`, or filter those events out of the public `/runs/{runId}/events` stream. |
| P1 | Typed tool failure metadata | Add optional `metadata` to the shared `tool.failed` event schema so Phase B can read `errorKind`, `guidance`, and retry diagnostics. |
| P2 | Internal template build endpoint | Gate `/internal/template-build` behind internal authorization or remove it from non-test runtime routers before exposing the runtime outside trusted local/dev environments. |
| P2 | Server-side request validation | Enforce the same non-empty string constraints in Rust request handlers that `packages/shared` currently enforces with Zod. |

## Public Runtime API

These routes are stable for Phase B consumption.

| Method | Path | Request | Response | Notes |
|---|---|---|---|---|
| `GET` | `/health` | none | `HealthResponse` | Returns `{ "status": "ready" }` when config loads |
| `POST` | `/runs` | `StartRunRequest` | `StartRunResponse` | Starts brief/build/review/repair/edit/export runs |
| `POST` | `/runs/{runId}/continue` | `ContinueRunRequest` | `ContinueRunResponse` | Adds user input or resumes a paused run |
| `POST` | `/runs/{runId}/cancel` | none | `CancelRunResponse` | Terminal cancellation; completed tool results remain persisted |
| `GET` | `/runs/{runId}/events` | `Last-Event-ID` optional | SSE `AgentEvent` JSON payloads | Replays after the supplied event id without duplicates |
| `POST` | `/permissions/{permissionId}/decision` | `ResolvePermissionRequest` | `ResolvePermissionResponse` | Resolves platform permission asks |
| `GET` | `/projects/{projectId}/conversation` | `includeDebug` optional query | `ConversationListResponse` | Defaults to user-visible conversation items only |
| `GET` | `/preview/{projectId}/current` | none | `PreviewCurrentResponse` | Returns the promoted current preview |
| `GET` | `/preview/{projectId}/{versionId}` | none | `PreviewVersionResponse` | Returns a candidate, promoted, or failed version for the project |

All public error responses use:

```ts
type ErrorResponse = {
  error: string;
};
```

## Shared Contract Source Of Truth

Phase B should import runtime contract types from `packages/shared/src`:

- `api-types.ts`
  - `ContentSource`
  - `StartRunRequest`
  - `StartRunResponse`
  - `ContinueRunRequest`
  - `ContinueRunResponse`
  - `CancelRunResponse`
  - `ResolvePermissionRequest`
  - `ResolvePermissionResponse`
  - `PreviewCurrentResponse`
  - `PreviewVersionResponse`
  - `ConversationListResponse`
  - `HealthResponse`
  - `ErrorResponse`
- `events.ts`
  - `AgentEvent`
- `schemas.ts`
  - `AgentPhase`
  - `AgentRunStatus`
  - `ConversationItem`
  - `ProjectVersion`
  - `SandboxBinding`
  - `Brief`
  - related enum schemas

Phase B should validate runtime payloads with these Zod schemas at the BFF edge
before passing data into UI state. The schemas must be updated to match the full
runtime SSE surface before final freeze; see the blockers above and the SSE
contract below.

## Request And Response Shapes

### Start Run

```ts
type StartRunRequest = {
  projectId: string;
  phase: "brief" | "build" | "repair" | "review" | "edit" | "export";
  agentProfile: string;
  inputContext?: {
    contentSources?: ContentSource[];
    briefId?: string;
    baseVersionId?: string;
    sandboxBindingId?: string;
    parentRunId?: string;
    findingIds?: string[];
  };
};

type StartRunResponse = {
  runId: string;
  status: "queued";
};
```

Build/review/repair/edit runs require a valid sandbox binding unless the runtime
auto-provisions one from a confirmed brief. Repair runs may target parent review
findings through `parentRunId` and `findingIds`.

### Continue Run

```ts
type ContinueRunRequest = {
  userMessage: string;
};

type ContinueRunResponse = {
  runId: string;
  status: "running" | "needs_user_input" | "completed";
};
```

Continuing a running run queues an interrupt. `Block` tools finish first;
`Cancel` tools receive synthetic interrupted tool results.

### Cancel Run

```ts
type CancelRunResponse = {
  runId: string;
  status: "cancelled";
};
```

### Resolve Permission

```ts
type ResolvePermissionRequest = {
  decision: "allow" | "ask" | "deny";
  updatedInput?: unknown;
};

type ResolvePermissionResponse = {
  runId: string;
  status: "running" | "needs_user_input" | "blocked";
};
```

Permission decisions are audited. `allow` resumes the run, `ask` keeps it in a
user-input state, and `deny` blocks it.

### Preview

```ts
type PreviewCurrentResponse = {
  projectId: string;
  versionId: string;
  previewUrl: string;
  status: "promoted";
};

type PreviewVersionResponse = {
  projectId: string;
  versionId: string;
  previewUrl: string;
  status: "candidate" | "promoted" | "failed";
};
```

`/preview/{projectId}/current` only returns promoted versions. Candidate
versions are visible through `/preview/{projectId}/{versionId}`.

### Conversation

```ts
type ConversationListResponse = {
  projectId: string;
  items: ConversationItem[];
};
```

By default, debug conversation items are filtered out. Phase B may opt in with
`includeDebug=true` for internal diagnostics views only.

## SSE Contract

`GET /runs/{runId}/events` emits SSE records whose `data` field is one
serialized `AgentEvent` JSON object. Event ids use:

```text
{runId}/{sequence}
```

Clients should reconnect with `Last-Event-ID` to resume after the last received
sequence.

Runtime event variants:

| Event type | Required payload fields |
|---|---|
| `run.started` | `runId`, `timestamp`, `label` |
| `agent.message` | `runId`, `timestamp`, `text` |
| `tool.started` | `runId`, `timestamp`, `tool`, `summary`, `toolUseId` |
| `tool.completed` | `runId`, `timestamp`, `tool`, `summary`, `toolUseId`, optional `metadata` |
| `tool.output` | `runId`, `timestamp`, `tool`, `toolUseId`, `stream`, `text` |
| `tool.failed` | `runId`, `timestamp`, `tool`, `error`, `toolUseId`, `recoverable`, optional `metadata` |
| `tool.recovery_suggested` | `runId`, `timestamp`, `tool`, `errorKind`, `fingerprint`, `attempt`, `guidance`, optional `metadata` |
| `chunk.received` | `runId`, `timestamp`, `path`, `sessionId`, `index`, `total`, `bytes`, `chars` |
| `chunk.committed` | `runId`, `timestamp`, `path`, `sessionId`, `total`, `bytes`, `chars`, `sha256` |
| `metric.recorded` | `runId`, `timestamp`, `name`, `value`, optional `metadata` |
| `permission.requested` | `runId`, `timestamp`, `permissionId`, `tool`, `reason` |
| `permission.denied` | `runId`, `timestamp`, `tool`, `reason` |
| `state.changed` | `runId`, `timestamp`, `state` |
| `preview.rebuilding` | `runId`, `timestamp`, optional `previousVersionId` |
| `preview.candidate` | `runId`, `timestamp`, `url`, `versionId`, optional `screenshotId` |
| `preview.updated` | `runId`, `timestamp`, `url`, `versionId`, optional `screenshotId` |
| `review.finding` | `runId`, `timestamp`, `findingId`, `severity`, `summary` |
| `run.completed` | `runId`, `timestamp`, `status`, `summary` |

Successful build/edit flows must emit `preview.updated` before the terminal
`run.completed` event.

Phase B UI may choose to hide debug-like telemetry such as `metric.recorded`,
but the BFF must either parse and tolerate it or the runtime must filter it from
the public stream. Failing closed on unknown event variants will break chunked
write and tool recovery flows.

## Internal-Only APIs

These routes are not Phase B public product contracts:

| Method | Path | Status |
|---|---|---|
| `POST` | `/internal/template-build` | Local/test/admin helper currently registered without auth; must be gated before non-local exposure |
| `POST` | `/internal/previews/promote` | Disabled by default and requires internal service authorization |
| `GET` | `/artifacts/{projectId}/current` | Runtime artifact serving surface; product preview routing should use preview APIs |
| `GET` | `/artifacts/{projectId}/{*path}` | Runtime artifact serving surface |

Phase B must not call the internal promotion endpoint as a normal product
operation. Promotion remains enforced by runtime tools and gates. Phase B should
also avoid `/internal/template-build`; normal product generation should go
through `POST /runs` and runtime-managed build/edit flows.

## Validation Boundary

The shared Zod schemas currently enforce non-empty string constraints for
identifiers and user messages. Rust request structs accept deserialized strings
and rely mostly on later store and workflow validation. Before final API freeze,
runtime handlers should reject empty `projectId`, `agentProfile`, `userMessage`,
`briefId`, `sandboxBindingId`, `parentRunId`, `findingIds`, and permission ids
with schema-compatible error responses.

Until that server-side validation is added, Phase B BFF validation is required
and direct runtime callers can exercise a looser request surface than the shared
contract describes.

## Compatibility Rules

Allowed after final freeze:

- Add optional fields to request/response/event payloads.
- Add new event variants only after updating `AgentEventSchema` and mock BFF
  contract tests.
- Add new public endpoints without changing existing route semantics.
- Add internal-only routes behind explicit config gates.

Breaking after final freeze:

- Removing or renaming public routes.
- Removing required fields from shared schemas.
- Changing enum string values.
- Changing event ordering guarantees for preview promotion and run completion.
- Making Phase B depend on internal-only routes.
- Returning non-schema-compatible payloads from public runtime endpoints.

Any breaking change requires a new contract revision document and corresponding
updates to `packages/shared` tests plus runtime mock BFF contract tests.

## Verification

The candidate contract is covered by:

- `services/runtime/tests/http_api.rs`
- `services/runtime/tests/mock_bff_contract.rs`
- `packages/shared/src/mock-bff-contract-types.test.ts`
- `packages/shared/src/schemas.test.ts`
- `infra/phase-a/verify.sh`

The freeze gate passed on 2026-07-08. Final Phase B freeze additionally requires
the blocker items above to be either fixed in code/schema or explicitly moved
behind a documented internal/debug boundary.
