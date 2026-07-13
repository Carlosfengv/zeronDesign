---
date: 2026-07-08
status: frozen
type: api-contract
topic: phase-a-runtime-http-sse-freeze
based_on_commit: d60e2f9
---

# Phase A Runtime API Freeze

## Summary

This document records the Phase A Runtime HTTP/SSE contract implemented by
`services/runtime/src/http_api.rs` and the shared TypeScript schemas in
`packages/shared/src`. It is the frozen contract for Phase B BFF and product UI
work.

The Phase A freeze gate and real `deepseek-v4-pro` website generation regression
passed on 2026-07-08. See `2026-07-08-phase-a-acceptance-report.md` for the
test evidence.

The Phase B freeze blockers from the API review have been resolved in code and
covered by the verification suite listed below.

## Executable Route Inventory

The complete Runtime HTTP route inventory is now machine-readable at
`services/runtime/contracts/http-routes.json`. It covers the public, internal-service, and isolated
capture routers and freezes, for every route:

- exact Axum path and HTTP methods;
- exposed surface;
- authorization mode;
- request body-limit policy;
- feature-flag dependency;
- response family, including JSON, SSE, HTML, artifact, and proxied responses.

`services/runtime/tests/http_api/contract_manifest.rs` compares that manifest with every `.route()`
declaration in `src/http_api.rs`. Adding, removing, moving, or changing a route method without an
intentional manifest update fails the HTTP integration test target.

Run the executable freeze gate with:

```bash
cd services/runtime
cargo test --test http_api contract_manifest
```

Changes to this frozen surface are additive by default. Route removal, method removal,
authorization weakening, body-limit expansion, or moving an internal/capture route onto the public
surface requires a separately reviewed contract-version decision; updating the JSON file alone is
not sufficient approval.

### Additive Route Groups After The Original Phase A Freeze

The original Phase A public table below remains stable. The executable inventory additionally
records these already-implemented groups:

| Route group | Paths | Contract status |
|---|---|---|
| Runtime identity | `/`, `/version` | additive |
| Design source | `/design-source-artifacts`, `/design-source-artifacts/{artifact_id}`, `/design-source-artifacts/{artifact_id}/content` | additive, internal-service authorization |
| Design Profile | `/design-profiles...`, `/projects/{project_id}/design-profile` | additive; source/import/activation/conversion routes are explicitly annotated |
| Structured Brief | `/briefs/{brief_id}`, `/briefs/{brief_id}/confirm` | additive, project-scoped read/write authorization |
| Editable runtime state | `/projects/{project_id}/runtime-state` | additive |
| Candidate preview proxy | `/previews/{lease_id}...` | additive, conditional public-principal authorization |
| Immutable artifacts | `/artifacts/{project_id}/current...`, `/artifacts/{project_id}/versions/{version_id}...`, `/_next/{*artifact_path}` | additive |
| Internal control plane | `/internal/template-build`, `/internal/previews/promote`, `/internal/projects/{project_id}/...` | additive, internal-service authorization; build/promote are feature-gated |
| Isolated capture listener | `/preview-captures/{lease_id}...` | additive, not part of the public router |

## Phase B Freeze Blockers Resolved

These items were resolved before declaring the Phase B API contract frozen:

| Priority | Area | Resolution |
|---|---|---|
| P1 | SSE event schema | `packages/shared/src/events.ts` now includes `tool.recovery_suggested`, `chunk.received`, `chunk.committed`, and `metric.recorded`; mock BFF contract tests parse them. |
| P1 | Typed tool failure metadata | The shared `tool.failed` schema now accepts optional `metadata` for `errorKind`, `guidance`, and retry diagnostics. |
| P2 | Internal template build endpoint | `/internal/template-build` is gated behind `ENABLE_INTERNAL_TEMPLATE_BUILD_API` and internal service authorization. It is disabled by default. |
| P2 | Server-side request validation | Rust request handlers now reject empty contract identifiers and user messages with schema-compatible `{ error }` responses. |

## Public Runtime API

These routes are stable for Phase B consumption.

| Method | Path | Request | Response | Notes |
|---|---|---|---|---|
| `GET` | `/health` | none | `HealthResponse` | Returns `200 { "status": "ready" }` only after recovery; returns `503 { "status": "not_ready" }` during shutdown or after a fatal supervised task |
| `POST` | `/runs` | `StartRunRequest` | `StartRunResponse` | Starts brief/build/review/repair/edit/export runs |
| `POST` | `/runs/{runId}/continue` | `ContinueRunRequest` | `ContinueRunResponse` | Adds user input or resumes a paused run |
| `POST` | `/runs/{runId}/cancel` | none | `CancelRunResponse` | Terminal cancellation; completed tool results remain persisted |
| `GET` | `/runs/{runId}/events` | `Last-Event-ID` optional | SSE `AgentEvent` JSON payloads | Replays after the supplied event id without duplicates |
| `GET` | `/briefs/{briefId}` | none | `BriefResponse` | Returns the structured Brief, owner project/run and current statuses |
| `POST` | `/briefs/{briefId}/confirm` | none | `BriefResponse` | Idempotently confirms a draft through the existing Brief run lifecycle |
| `POST` | `/permissions/{permissionId}/decision` | `ResolvePermissionRequest` | `ResolvePermissionResponse` | Resolves platform permission asks |
| `GET` | `/projects/{projectId}/conversation` | `includeDebug` optional query | `ConversationListResponse` | Defaults to user-visible conversation items only |
| `GET` | `/preview/{projectId}/current` | none | `PreviewCurrentResponse` | Returns the promoted current preview |
| `GET` | `/preview/{projectId}/{versionId}` | none | `PreviewVersionResponse` | Returns a candidate, promoted, or failed version for the project |
| `GET` | `/artifacts/{projectId}/current/{*artifactPath}` | none | Artifact bytes | Reads the current promoted artifact with `preview.read` authorization |
| `GET` | `/artifacts/{projectId}/versions/{versionId}/{*artifactPath}` | none | Immutable artifact bytes | Reads a project-owned fixed version; bytes and manifest identity are verified |
| `POST` | `/projects/{projectId}/versions/{versionId}/releases` | `CreateReleaseRequest` | `ReleasePackagingResponse` | Prepares idempotent packaging for a promoted immutable version |
| `GET` | `/release-packagings/{packagingId}` | none | `ReleasePackagingResponse` | Reads current packaging and release state |
| `POST` | `/projects/{projectId}/publish` | `PublishWorkRequest` + CAS headers | `PublicationOperationResponse` | First publish or atomic update to a validated Release |
| `POST` | `/projects/{projectId}/rollback` | `PublishWorkRequest` + `If-Match` | `PublicationOperationResponse` | Rolls the stable host back to a validated historical Release |
| `POST` | `/projects/{projectId}/unpublish` | `UnpublishWorkRequest` + `If-Match` | `PublicationOperationResponse` | Removes public serving while retaining host identity and history |
| `GET` | `/projects/{projectId}/deployment-state` | none | `DeploymentStateResponse` | Reads desired/current Release, generation, status and stable public URL |
| `GET` | `/projects/{projectId}/releases` | none | `WorkReleaseListResponse` | Lists immutable Release history for one project |
| `GET` | `/operations/{operationId}` | none | `PublicationOperationResponse` | Reads recoverable publication progress |

All public error responses use:

```ts
type ErrorResponse = {
  error: string;
};
```

Create Release requires `Idempotency-Key`. Runtime derives the actual Release
identity from the immutable version, artifact/runtime manifest hashes and
packaging trust inputs, so retries with the same content converge on the same
Release and Packaging records.

The supervised packaging controller is enabled only when the complete
production configuration is present: `RELEASE_BASE_IMAGE_DIGEST`,
`RELEASE_PACKAGER_VERSION`, `RELEASE_REGISTRY_REPOSITORY`,
`RELEASE_SCAN_POLICY_VERSION`, `ANYDESIGN_RELEASE_PACKAGER_HELPER`, and
`RELEASE_PACKAGING_HELPER_SHA256`. Partial configuration fails startup.

## Shared Contract Source Of Truth

Phase B should import runtime contract types from `packages/shared/src`:

- `api-types.ts`
  - `ContentSource`
  - `StartRunRequest`
  - `StartRunResponse`
  - `ContinueRunRequest`
  - `ContinueRunResponse`
  - `CancelRunResponse`
  - `BriefResponse`
  - `ResolvePermissionRequest`
  - `ResolvePermissionResponse`
  - `PreviewCurrentResponse`
  - `PreviewVersionResponse`
  - `ConversationListResponse`
  - `CreateReleaseRequest`
  - `ReleasePackagingResponse`
  - `PublishWorkRequest`
  - `UnpublishWorkRequest`
  - `PublicationOperationResponse`
  - `DeploymentStateResponse`
  - `WorkReleaseListResponse`
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
  status: "queued" | "needs_user_input";
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

### Structured Brief

```ts
type BriefResponse = {
  briefId: string;
  projectId: string;
  runId: string;
  status: "draft" | "confirmed" | "superseded";
  runStatus: AgentRunStatus;
  brief: Brief;
};
```

`POST /briefs/{briefId}/confirm` is idempotent after confirmation. For a draft,
it succeeds only while its owning Brief run is in `needs_user_input` and the run
still references that Brief. Confirmation reuses the same lifecycle as an
explicit confirmation message sent to `/runs/{runId}/continue`; it persists the
confirmed Brief checkpoint, completes the run, emits `run.completed`, and adds
the user-visible completion conversation item.

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
user-input state, and `deny` blocks it. Runtime persists `updatedInput` with the
permission decision. For `allow`, it is bound to the pending tool identity,
revalidated through the normal tool schema and deny policy, and consumed once
when the matching tool is retried. Audit records store only an input digest.

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

### Project-scoped public authorization

When `PUBLIC_PRINCIPAL_AUTH_MODE=required`, public Runtime calls require a
short-lived signed principal whose `projectId` and owner identity match the
persisted project access record. The operation scopes are:

| Scope | Routes |
|---|---|
| `project.read` | run events, structured Brief reads, project conversation, editable runtime state |
| `project.write` | start/continue/cancel run, structured Brief confirmation, resolve permission |
| `preview.read` | candidate/promoted preview metadata, current and fixed-version artifact bytes |
| `publication.read` | deployment state, release and operation reads |
| `publication.write` | Create Release, publish, unpublish and rollback |

The BFF must forward the same bearer token to SSE and artifact proxy requests;
a Referer or an opaque resource ID identifies a resource but never authorizes
access to it.

## DesignProfile Fidelity Addendum (2026-07-10)

The following additive routes extend the frozen Runtime contract. They do not
change existing run, preview, conversation, or SSE route semantics.

| Method | Path | Authorization | Purpose |
|---|---|---|---|
| `POST` | `/design-source-artifacts` | trusted BFF/service | Create an immutable UTF-8 source artifact, maximum 256 KiB |
| `GET` | `/design-source-artifacts/{artifactId}` | trusted BFF/service | Read source metadata without source body |
| `GET` | `/design-source-artifacts/{artifactId}/content` | trusted BFF/service | Read hash-verified source bytes |
| `POST` | `/design-profiles/import` | trusted BFF/service | Deterministically import a source artifact into a V2 draft and conversion report |
| `GET` | `/design-profiles/{id}/conversion-report` | trusted BFF/service | Read the latest draft conversion report |
| `GET` | `/design-profiles/{id}/versions/{version}/conversion-report` | trusted BFF/service | Read a versioned conversion report |
| `POST` | `/design-profiles/{id}/activate` | trusted BFF/service | Validate and append a strict active revision |
| `GET` | `/design-profiles/{id}/versions/{version}/fidelity-report?surface=...&template=...` | normal Profile visibility | Read a side-effect-free, versioned capability report |

Trusted source routes require both headers:

```http
x-anydesign-internal: true
x-runtime-admin-token: <RUNTIME_INTERNAL_ADMIN_TOKEN>
```

`DesignSourceArtifact` bytes are immutable. Runtime calculates the canonical
SHA-256 digest, validates an optional `clientSha256`, requires exactly one
scope key, and revalidates size and digest before every content response.
Profile/source scope must match exactly in this contract revision.

`schemaVersion` describes the Profile payload contract and is independent from
the append-only numeric revision `version`. Historical Profile JSON without
`schemaVersion` is normalized at read time to `design-profile@1`; persisted
historical bytes are not rewritten.

Imported source creates `DesignProfileDraft` only. Drafts cannot bind to a
project or enter run context. Draft updates require `expectedVersion`; a stale
revision returns `409`. Activation also requires `expectedVersion`, returns
`409` with blocking validation issues when incomplete, and creates a new strict
active revision on success.

V2 run snapshots add the resolved surface/template, base and effective Profile
hashes, source artifact/hash, fidelity mode, source budget, indexed source
sections, and read hashes. `profile_only` denies raw source reads.
`source_fallback` enforces the 32 KiB full-read threshold, 16 KiB per-section
limit, and 48 KiB per-run budget before allowing sandbox mutation.

Built-in `astro-website` and `fumadocs-docs` templates now declare
`runtime-style-contract@p3`; p2 remains parse-compatible. Post-publish fidelity
assertions are written to `state/design-profile-fidelity.json` and recorded as
`design_profile_fidelity_checked` conversation evidence before `run.complete`.

p3 distinguishes semantic action roles: `color.primary` remains the Profile
primary color, generic controls consume `color.action` and
`color.actionContrast`, and auth-submit controls consume `color.authSubmit`.
This prevents a brand accent that is intentionally scoped to one component
from leaking into every `.runtime-button`.

Required `computed-style` assertions are collected from the promoted preview
with an isolated headless Chrome/Chromium CDP session. Browser launch,
navigation, selector/property evaluation, timeout, or collector-exit failures
are explicit assertion failures. Evidence is never reused from a previous
publish. Comparators support equivalent browser color forms, forbidden scope,
all/any matching, and relative numeric ratios such as
`letter-spacing / font-size = 0.10`.

Every successful build records a deterministic `sourceFingerprint`. The
post-publish fidelity report binds that fingerprint and exposes a
`repairContext` containing the Style Contract path, token file, global CSS
file, component root, and repair constraints. If required rules failed and the
next publish has the same fingerprint, Runtime returns the recoverable error
`design_profile.no_source_change_after_fidelity_failure`; an unchanged rebuild
does not consume the repair opportunity.

Under the `local-e2e` policy, `preview.publish` owns the preview lifecycle and
ignores model-provided `url`, `port`, `command`, and `mode` values. It allocates
a fresh managed loopback port for each publish so an old `localhost:4321`
server cannot satisfy browser checks for a newer source snapshot. Production
sandbox endpoint selection remains Runtime-controlled by its deployment
adapter.

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
| `POST` | `/internal/template-build` | Disabled by default; requires explicit enablement and internal service authorization |
| `POST` | `/internal/previews/promote` | Disabled by default and requires internal service authorization |

Phase B must not call the internal promotion endpoint as a normal product
operation. Promotion remains enforced by runtime tools and gates. Phase B should
also avoid `/internal/template-build`; normal product generation should go
through `POST /runs` and runtime-managed build/edit flows.

## Validation Boundary

The shared Zod schemas and Rust request handlers both enforce non-empty string
constraints for public contract identifiers and user messages. Rust handlers
return schema-compatible `{ error }` responses for invalid empty values before
starting or mutating a run.

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

The frozen contract is covered by:

- `services/runtime/tests/http_api.rs`
- `services/runtime/tests/mock_bff_contract.rs`
- `packages/shared/src/mock-bff-contract-types.test.ts`
- `packages/shared/src/schemas.test.ts`
- `infra/phase-a/verify.sh`
- `apps/web` typecheck and production build (when the Phase B application exists)
- Real `deepseek-v4-pro` website generation E2E:
  `real_deepseek_design_md_website_generation_e2e`

The freeze gate and the real `deepseek-v4-pro` website generation E2E passed on
2026-07-08 after the blocker fixes. Phase B may now consume this Runtime
HTTP/SSE contract as frozen.
