---
date: 2026-07-04
status: active
type: feat
origin: ./2026-07-04-internal-ai-website-docs-generator-requirements.md
architecture:
  - ./2026-07-04-internal-ai-website-docs-generator-architecture.md
  - ./2026-07-04-agent-harness-design.md
  - ./2026-07-04-rust-agent-harness-delivery-review.md
---

# Internal AI Website / Docs Generator MVP Implementation Plan

## 1. Problem Frame

We need an internal product that turns prompt, Markdown, and attachments into high-quality Website or Docs outputs for designers. The core workflow is Brief-first: content sources are normalized into a user-confirmable Brief before any code generation starts. After confirmation, each project receives an isolated agent sandbox and enters a detail workspace with left-side LLM chat / events and right-side preview.

**Revised delivery strategy:** This plan is organized into two sequential phases:

- **Phase A — Runtime + Sandbox First:** Build, test, and stabilize the Rust agent runtime as a standalone service exposing HTTP + SSE APIs, including real K8s agent-sandbox E2E verification. No Next.js product shell and no frontend in this phase. Phase A is complete when the runtime, sandbox adapter, Astro preview promotion, permission integration gate, mock BFF contract tests, and E2E acceptance tests pass and the runtime API contract is frozen.
- **Phase A.5 — Docs Template Loop:** Add the Fumadocs Docs template after the Astro runtime loop is accepted. This keeps Docs support close to Phase A without making the first runtime freeze depend on two template ecosystems.
- **Phase B — Product Integration:** Build the Next.js control plane and frontend workspace on top of the stable runtime API. Phase B begins only after Phase A acceptance.

The end-to-end path remains:

```text
Prompt / Markdown
  -> Brief Agent
  -> Designer confirms Brief
  -> Astro sandbox generation
  -> Candidate preview
  -> Promoted preview
  -> Chat edit
```

Docs generation via Fumadocs is the Phase A.5 template loop, after the Astro website loop is accepted.

## 2. Scope Lock

### Phase A — Runtime Scope

Phase A delivers a standalone Rust service with no product UI dependencies.

- Rust agent runtime service (`services/runtime/`).
- Shared TypeScript/Rust schemas for AgentRun, AgentEvent, ConversationItem, Brief.
- Agent loop, tool registry, tool executor.
- Control-plane tools: `content.*`, `brief.*`, `run.*`, `user.ask`.
- Sandbox tools: `fs.*`, `shell.run`, `package.install`, `preview.*`, `diagnostics.*`, `browser.*`.
- Permission engine: deny-by-default, path policy, command policy (exec array), network policy, audit.
- Agent profiles: `brief`, `build`, `repair`, `visual-review`, `edit`.
- Review/Repair child run graph with bounded repair loop.
- Checkpoint persistence.
- HTTP + SSE API: StartRun, ContinueRun, CancelRun, StreamRunEvents, ResolvePermission; `PromotePreview` is test-only/admin-only behind feature flag and not part of the product API.
- Sandbox adapter for Kubernetes agent-sandbox v1beta1 with `wait_ready`.
- `astro-website` SandboxTemplate.

Phase A reserves the `fumadocs-docs` template key in shared schemas, but does not require the Fumadocs sandbox template or E2E loop to pass before the runtime API freeze.

### Phase B — Product Scope

Phase B builds on the frozen runtime API.

- Next.js product control plane shell.
- Product data models: Project, ContentSource, Brief, AgentSession, ProjectVersion, SandboxBinding, AuditRecord.
- BFF routes proxying runtime HTTP/SSE API.
- Designer-facing Brief review and confirmation.
- Detail workspace: left chat + right preview panel.
- Preview Router: `/preview/{projectId}/current` and `/preview/{projectId}/{versionId}`.
- Export entry point.

### Explicitly Out of Scope

- Figma MCP as input source.
- Figma-to-code or high-fidelity Figma reproduction.
- Full visual editor.
- Public SaaS onboarding, billing, tenant management, or public workspace sharing.
- Full admin governance console.
- Next.js and Docusaurus production-quality templates.
- Multi-user simultaneous editing.
- External publishing flow.

Note: Real Figma MCP input is out of scope, but the Rust runtime must still reserve the MCP/deferred-tool contract in Phase A (`mcp_info`, `input_json_schema`, MCP stub, token budgeting). This prevents a later Figma integration from forcing a ToolRegistry rewrite.

## 3. Key Technical Decisions

### D1. Runtime Is Backend, Not Frontend

The LLM loop, tool execution, permission enforcement, sandbox channel, checkpointing, and preview promotion live in the backend runtime. The frontend renders state, streams events, sends user messages, and handles business confirmations.

Rationale: agent runs are long-lived, permissioned, auditable, and tied to sandbox execution. Next.js route handlers are not the right boundary for tool execution or sandbox control.

### D2. Rust Runtime Builds First, Product Integrates Second

Phase A delivers a standalone Rust runtime service with HTTP + SSE API. The product control plane (Next.js, DB schema, BFF routes) starts only after the runtime API contract is frozen and acceptance tests pass.

Rationale: the runtime is the highest-risk component. Building it first means API contracts are derived from real behavior, not upfront guesses. Frontend and DB schema changes caused by runtime surprises are eliminated.

### D3. Runtime API Is HTTP + SSE

The runtime exposes:
- HTTP endpoints for StartRun, ContinueRun, CancelRun, ResolvePermission.
- SSE stream for AgentEvent per run (StreamRunEvents).

Rationale: HTTP + SSE is debuggable, curl-testable, and directly consumable by Next.js BFF without a gRPC-Web layer. Protocol can be reassessed after MVP if performance requires it.

### D4. App DB Owns Product State; K8s Owns Sandbox State

Use application tables for product-facing entities and bind them to Kubernetes resources through `SandboxBinding`.

Rationale: MVP product iteration should stay fast. Business CRDs can be added later if GitOps or platform reconciliation becomes necessary.

### D5. Pin Agent Sandbox API Version Before Phase A Sandbox Work

Use `agents.x-k8s.io/v1beta1` and `extensions.agents.x-k8s.io/v1beta1` manifests. `SandboxClaim` must reference `SandboxWarmPool` through `spec.warmpoolRef`. Confirm the sandbox channel protocol (gRPC/WebSocket/other) before implementing the sandbox adapter.

Rationale: agent-sandbox v0.5.0 graduated APIs to `v1beta1` and changed `SandboxClaim` behavior. The channel protocol decision cannot be deferred past RA4.

### D6. Permission UX Separates Business Confirmation from Platform Policy

Designers confirm product-level decisions: Brief, project type, template, major direction changes, export. Platform policy handles dependency install, network, shell, secrets, and sandbox lifetime. Shell commands use exec array, not string interpolation.

### D7. Preview Promotion Is Atomic and Enforced at Tool Layer

```text
preview.rebuilding -> preview.candidate -> preview.updated
```

`run.complete` on a build/edit run is rejected by the tool if `output_version_id` is not yet `promoted`. This enforces ordering at the implementation layer, not only in prompts.

## 4. Proposed Repository Layout

```text
services/runtime/                 # Phase A: Rust agent runtime (standalone)
packages/shared/                  # Phase A: Shared Rust/TS schemas and API types
infra/agent-sandbox/              # Phase A: K8s manifests and sandbox templates
apps/web/                         # Phase B: Next.js product control plane
docs/product/2026-07-04-anydesign-mvp/
```

| Path | Phase | Responsibility |
|---|---|---|
| `services/runtime` | A | Query session, agent loop, tools, permissions, sandbox channel, MCP/deferred-tool contract, HTTP+SSE API |
| `packages/shared` | A | AgentRun, AgentEvent, ConversationItem, Brief schemas; runtime API client types |
| `infra/agent-sandbox` | A | SandboxTemplate, SandboxWarmPool, NetworkPolicy, RBAC |
| `apps/web` | B | UI, BFF routes, event stream client, project workspace |

**Phase A does not create `apps/web`.** The runtime is tested via HTTP clients and integration tests only.

## 5. Data Model

### Product Tables

```text
users
projects
content_sources
briefs
agent_sessions
agent_runs
agent_events
conversation_items
project_versions
sandbox_bindings
artifacts
audit_records
```

### Core Fields

`projects`

- `id`
- `name`
- `kind`: `website | docs`
- `template_key`
- `status`
- `current_brief_id`
- `current_version_id`
- `created_by`
- `created_at`
- `updated_at`

`content_sources`

- `id`
- `project_id`
- `kind`: `prompt | markdown | attachment_text | design_md`
- `storage_uri`
- `text_excerpt`
- `sha256`
- `created_at`

`briefs`

- `id`
- `project_id`
- `status`: `draft | confirmed | superseded`
- `version`
- `content_json`
- `content_markdown`
- `created_by_run_id`
- `confirmed_at`

`agent_sessions`

- `id`
- `project_id`
- `sandbox_binding_id`
- `status`: `active | paused | archived`
- `current_version_id`

`agent_runs`

- `id`
- `session_id`
- `project_id`
- `parent_run_id`
- `phase`: `brief | build | review | repair | edit | export`
- `agent_profile`
- `status`: `queued | running | needs_user_input | completed | partial | failed | blocked | cancelled`
- `base_version_id`
- `output_version_id`
- `checkpoint_id`
- `started_at`
- `completed_at`

`agent_events`

- `id`
- `run_id`
- `project_id`
- `type`
- `payload_json`
- `created_at`

`conversation_items`

- `id`
- `project_id`
- `run_id`
- `version_id`
- `kind`
- `role`
- `text`
- `metadata_json`
- `created_at`

`project_versions`

- `id`
- `project_id`
- `source_snapshot_uri`
- `preview_url`
- `screenshot_uri`
- `status`: `candidate | promoted | failed`
- `created_by_run_id`
- `created_at`

`sandbox_bindings`

- `id`
- `project_id`
- `sandbox_name`
- `sandbox_claim_name`
- `workspace_pvc_name`
- `warm_pool_name`
- `namespace`
- `status`: `claiming | ready | busy | idle | paused | failed | deleted`
- `last_seen_at`

`sandbox_name + workspace_pvc_name` defines the project workspace scope. Agent tools may modify only the content mounted from this PVC into that sandbox's `/workspace`; this PVC is the durable source tree, state, outputs, and dependency cache for one work.

`audit_records`

- `id`
- `project_id`
- `run_id`
- `actor_type`: `user | agent | system`
- `action`
- `target`
- `decision`
- `metadata_json`
- `created_at`

## 6. API Contract

### Phase A — Runtime HTTP + SSE API

这是 Phase A 的 runtime API。非 internal endpoint 是 Phase B BFF 的上游，接口冻结后不应变更；test-only endpoint 不进入 Product BFF client，生产默认不启用。

```text
POST   /runs                              # StartRun(project_id, phase, agent_profile, input_context)
POST   /runs/{runId}/continue             # ContinueRun(run_id, user_message)
POST   /runs/{runId}/cancel              # CancelRun(run_id)
GET    /runs/{runId}/events              # StreamRunEvents(run_id) — SSE stream of AgentEvent
POST   /permissions/{permissionId}/decision  # ResolvePermission(permission_id, allow|ask|deny)
POST   /internal/previews/promote        # PromotePreview test-only/admin-only behind feature flag; production path uses in-process orchestrator call
GET    /health                           # Health check
```

所有接口返回结构使用 `packages/shared` 中定义的类型。

### Phase B — Product API (BFF)

Phase B 的 Next.js BFF 在 Product API 层对接 Runtime HTTP API 和 App DB。

```text
POST   /api/projects
GET    /api/projects/{projectId}
POST   /api/projects/{projectId}/content-sources
GET    /api/projects/{projectId}/content-sources
POST   /api/projects/{projectId}/brief-runs
GET    /api/projects/{projectId}/briefs/current
POST   /api/projects/{projectId}/briefs/{briefId}/confirm
POST   /api/projects/{projectId}/generation-runs
POST   /api/projects/{projectId}/messages
GET    /api/projects/{projectId}/conversation
GET    /api/projects/{projectId}/events
GET    /api/projects/{projectId}/preview
POST   /api/runs/{runId}/cancel
POST   /api/permissions/{permissionId}/decision
```

## 7. Implementation Units

---

## Phase A — Runtime Implementation Units

### RA1. Runtime Skeleton and Shared Contracts

**Goal:** Establish the runtime service skeleton and shared schema layer. No frontend files.

**Files**

- `packages/shared/package.json`
- `packages/shared/src/schemas.ts`  — AgentRun, AgentEvent, ConversationItem, Brief (canonical definitions)
- `packages/shared/src/events.ts`
- `packages/shared/src/api-types.ts` — Runtime HTTP API request/response types
- `services/runtime/Cargo.toml`
- `services/runtime/src/main.rs`
- `services/runtime/src/config.rs`
- `services/runtime/src/http_api.rs`
- `services/runtime/src/tools/registry.rs`
- `services/runtime/src/tools/schema.rs`

**Approach**

- Define shared TypeScript schemas for AgentRun, AgentEvent, ConversationItem, Brief. These are the canonical definitions; all other documents reference them.
- Brief JSON schema fields: `projectType`, `audience`, `contentHierarchy`, `pageStructure`, `visualDirection`, `recommendedTemplate`, `assumptions`, `missingInformation`.
- Define the runtime Tool contract fields that affect API shape: `input_schema`, `input_json_schema`, `output_schema`, `is_enabled`, `interrupt_behavior`, `tool_loading`, `mcp_info`.
- Define Rust config for model gateway, DB, object storage, and K8s namespace.
- Stand up HTTP server with `/health` endpoint.

**Test Scenarios**

- `packages/shared/src/schemas.test.ts`: validates AgentRun (all status values including `partial`), AgentEvent (all event types), Brief JSON against schema.
- `services/runtime/tests/tool_registry.rs`: output schema exists, disabled tools are not sent to model, deferred/MCP metadata is stored.
- `services/runtime/tests/health.rs`: health endpoint returns ready when config loads.

**Dependencies**

- None.

### RA2. Agent Loop and Brief Agent

**Goal:** Implement the core agent loop and the Brief Agent profile. No sandbox, no frontend.

**Files**

- `services/runtime/src/agent_loop.rs`
- `services/runtime/src/query_session.rs`
- `services/runtime/src/model_gateway.rs`
- `services/runtime/src/profiles/brief.rs`
- `services/runtime/src/tools/mod.rs`
- `services/runtime/src/tools/content.rs`
- `services/runtime/src/tools/brief.rs`
- `services/runtime/src/tools/run.rs`
- `services/runtime/src/conversation.rs`

**Approach**

- Implement agent loop: model stream → tool call → permission check → tool execute → event emit → checkpoint → iterate.
- Implement QuerySession around the loop: system prompt assembly, stable tool-set snapshot per turn, fallback model config, max turns/task budget, structured output enforcement.
- Enforce the tool_use/tool_result invariant: every emitted tool_use must receive exactly one tool_result, including model error, fallback, abort, unknown tool, and executor discard paths.
- Empty-turn guard: 3 consecutive turns with no tool calls → `partial` status, not infinite spin.
- Brief Agent profile: reads content sources, writes draft Brief, validates output against Brief JSON schema, handles revision, calls `run.complete`.
- `run.complete` is the only valid agent-declared `completed` signal; no implicit completion from logs or process exit. Runtime guards, cancellation, terminal errors, or recovery failure may still produce `partial | blocked | failed | cancelled`.
- Allow-all placeholder policy for control-plane tools (`content.*`, `brief.*`, `run.*`). Mark with `// TODO: replace in RA3a`.
- ConversationItem persistence: user input, assistant replies, approval requests, error summaries must be persisted. High-frequency tool events are collapsed to `tool_summary`.
- SSE event stream exposed on `GET /runs/{runId}/events`.

**Test Scenarios**

- `services/runtime/tests/brief_agent.rs`: prompt + Markdown → Brief with all required fields (projectType, audience, contentHierarchy, pageStructure, visualDirection, recommendedTemplate, assumptions, missingInformation).
- `services/runtime/tests/brief_agent.rs`: empty input → run pauses with status `needs_user_input` and emits `state.changed` plus an actionable message.
- `services/runtime/tests/brief_agent.rs`: unreadable content source → run ends with status `blocked`.
- `services/runtime/tests/run_completion.rs`: run cannot reach `completed` without `run.complete`; runtime guards may still end it as `partial | blocked | failed | cancelled`.
- `services/runtime/tests/agent_loop.rs`: 3 consecutive empty turns → run transitions to `partial`.
- `services/runtime/tests/agent_loop.rs`: SSE stream delivers all emitted events in order.
- `services/runtime/tests/agent_loop.rs`: model error after tool_use emits missing tool_result before final error.
- `services/runtime/tests/agent_loop.rs`: fallback discards old executor and prevents stale tool_result leakage.
- `services/runtime/tests/brief_agent.rs`: Brief JSON fails schema validation → recoverable tool/model feedback, not accepted as completed output.

**Dependencies**

- RA1.

### RA3a. Permission Engine Core

**Goal:** Implement the permission policy core with unit and mock integration tests. RA3a replaces RA2 allow-all for control-plane tools and proves deny-by-default behavior without depending on a live sandbox channel.

**Files**

- `services/runtime/src/permissions/mod.rs`
- `services/runtime/src/permissions/policy.rs`
- `services/runtime/src/permissions/path_policy.rs`
- `services/runtime/src/permissions/command_policy.rs`
- `services/runtime/src/permissions/network_policy.rs`
- `services/runtime/src/audit.rs`

**Approach**

- Policy resolution order: org deny → project deny → agent profile deny → run scoped deny → tool permission → run scoped allow/ask → agent profile allow/ask → platform default.
- `deny` always beats `allow` and `ask`.
- Implement hook resolution: PreToolUse allow cannot bypass deny/ask rules; hook `updated_input` is used for permission and execution; headless PermissionRequest hooks can allow/deny before auto-deny.
- Shell: `shell.run` accepts `{ argv: string[] }`. Check `argv[0]` and scan full argv for deny patterns. `sh -c` / `bash -c` always denied.
- Path: all `fs.*` tools apply `realpath` before boundary check to prevent symlink escapes.
- After RA3a ships, replace `// TODO` placeholder in RA2 with real permission engine calls for control-plane tools.
- Persist audit records for every allow, ask, deny decision.

**Test Scenarios**

- deny beats allow.
- `/workspace` reads allowed; external path denied.
- `.env`, kubeconfig, private key patterns denied.
- `["pnpm", "build"]` argv allowed; `["kubectl", "get", "pods"]` denied; `["npx", "foo"]` becomes platform ask.
- `["sh", "-c", "pnpm build"]` denied.
- `fs.read` on a symlink pointing outside `/workspace` denied after realpath.
- PreToolUse hook allow + matching deny rule still denies.
- Headless Ask with no PermissionRequest hook decision auto-denies with AsyncAgent reason.

**Done means**

- Policy resolver, command policy, path policy, hook resolution, and audit writer pass unit tests.
- Control-plane tools no longer use the RA2 allow-all placeholder.
- Sandbox tool enforcement is not considered complete until RA4b runs the same engine on real `fs.*`, `shell.run`, `package.install`, `preview.*`, `browser.*`, and `diagnostics.*` calls.

**Dependencies**

- RA2.

### RA4. Sandbox Adapter and Workspace Tools

**Goal:** Implement sandbox claim/channel and all sandbox-scoped tools. No frontend.

**Pre-conditions for RA4 to start (all must be confirmed):**
- agent-sandbox release version locked; CRD manifests obtained.
- Development Kubernetes cluster available.
- Internal package registry/proxy available.
- Preview routing strategy confirmed (pod IP / internal DNS).
- Object storage bucket/prefix allocated.
- Sandbox channel protocol confirmed; documented in `infra/agent-sandbox/base/controller-version.md`.

**Files**

- `services/runtime/src/sandbox/mod.rs`
- `services/runtime/src/sandbox/kubernetes.rs`
- `services/runtime/src/sandbox/channel.rs`
- `services/runtime/src/tools/fs.rs`
- `services/runtime/src/tools/shell.rs`
- `services/runtime/src/tools/package.rs`
- `services/runtime/src/tools/preview.rs`
- `services/runtime/src/tools/diagnostics.rs`
- `services/runtime/src/tool_executor.rs`
- `infra/agent-sandbox/base/controller-version.md`
- `infra/agent-sandbox/templates/astro-website.yaml`
- `infra/agent-sandbox/warmpools/astro-website.yaml`
- `infra/agent-sandbox/network/default-deny.yaml`
- `infra/agent-sandbox/rbac/runtime-service-account.yaml`

**Approach**

- `SandboxClaim` uses `spec.warmpoolRef` (v1beta1). After claim, `wait_ready` watches until phase == Ready or 120s timeout.
- `open_channel` only after `wait_ready` succeeds.
- SandboxTemplate startup script pre-creates all `/workspace` subdirectories and writes empty `tasks.json` / `preview.json`.
- Tool trait: `name`, `input_schema`, `is_read_only`, `is_concurrency_safe`, `is_destructive`, `check_permission`, `call`.
- Read-only tools can run in parallel; write/shell tools serialize.
- `shell.run` accepts `{ argv: string[] }`, never a shell string.

**Test Scenarios**

- `sandbox_claim.rs`: claim creates correct K8s object with `warmpoolRef`.
- `sandbox_claim.rs`: `wait_ready` times out at 120s → `sandbox_unavailable`.
- `sandbox_binding.rs`: ready SandboxClaim updates SandboxBinding to ready.
- `sandbox_template.rs`: startup pre-creates all workspace dirs and writes empty state files.
- `tool_executor.rs`: concurrent-safe reads run in parallel; write/shell tools serialize.
- `infra/agent-sandbox/templates/astro-website.test.yaml`: validates against pinned CRD schema.

**Dependencies**

- RA3a.

### RA4b. Permission Integration Security Gate

**Goal:** Prove the RA3a permission engine on the real sandbox tool path before Build Agent work starts.

**Files**

- `services/runtime/tests/tool_permissions_integration.rs`
- `services/runtime/tests/sandbox_security.rs`
- `services/runtime/src/tools/fs.rs`
- `services/runtime/src/tools/shell.rs`
- `services/runtime/src/tools/package.rs`
- `services/runtime/src/tools/preview.rs`
- `services/runtime/src/tools/browser.rs`
- `services/runtime/src/tools/diagnostics.rs`

**Approach**

- Wire every sandbox tool through the shared permission engine before execution.
- Verify tool-specific input validation runs before permission checks.
- Verify every allow/ask/deny decision writes an audit record with projectId, runId, tool, input summary, decision, and reason.
- Verify `Ask` in headless runs goes through PermissionRequest hooks and auto-denies if no hook resolves it.
- Treat RA4b as a release gate: RA5 cannot start until the real sandbox tool path passes security tests.

**Test Scenarios**

- `fs.read("/etc/passwd")` denied and file contents never reach ToolResult, run log, SSE, or ConversationItem.
- `/workspace` symlink escape denied after realpath on `fs.read`, `fs.patch`, and `fs.delete`.
- `shell.run(["sh", "-c", "pnpm build"])` denied; `shell.run(["pnpm", "build"])` allowed.
- `shell.run(["pnpm", "install"])` denied with guidance to use `package.install`.
- `package.install` using internal registry allowed or platform-approved; public registry URL becomes platform ask.
- Network egress to public internet denied at policy and NetworkPolicy layers.
- Every sandbox tool call has one audit record.

**Dependencies**

- RA4.

### RA5. Build Agent and Preview Promotion

**Goal:** Generate a runnable Astro website and implement the preview promotion gate.

**Files**

- `services/runtime/src/profiles/build.rs`
- `services/runtime/src/profiles/docs_build.rs`
- `services/runtime/src/preview.rs`
- `services/runtime/src/versions.rs`
- `services/runtime/src/tools/browser.rs`

**Approach**

- Build Agent: reads `brief.md`, generates `/workspace/project`, installs deps through `package.install`, builds, starts preview, screenshots, emits `preview.candidate`. Runtime orchestrator runs promotion gate, performs internal promote, emits `preview.updated`, then the agent can call `run.complete`.
- `run.complete` for build/edit phase rejects if `output_version_id` is not yet `promoted` (tool-layer enforcement, not prompt).
- Preview promotion is atomic and runtime-controlled: `preview.candidate` → gate check → internal promote → `preview.updated` → update `current_version_id`.
- `PromotePreview` production path is an in-process runtime orchestrator call. The HTTP route is disabled by default, enabled only for integration tests or admin break-glass operations, and must require internal network, service auth, and audit.

**Test Scenarios**

- `astro_build_agent.rs`: confirmed Brief → project files → `preview.candidate` emitted.
- `preview_promotion.rs`: candidate does not update current version; promoted does.
- `preview_promotion.rs`: `run.complete` on unpromoted candidate → error returned to agent.
- `preview_promotion.rs`: HTTP promote route is disabled unless test/admin feature flag and internal auth are present.
- `tool_permissions_integration.rs`: denied shell command blocked even when model requests it.

**Dependencies**

- RA4b.

### RA6. Review / Repair / Edit Agents and Checkpoint

**Goal:** Automated quality loop, conversational edit, and checkpoint persistence.

**Files**

- `services/runtime/src/profiles/review.rs`
- `services/runtime/src/profiles/repair.rs`
- `services/runtime/src/profiles/edit.rs`
- `services/runtime/src/review_findings.rs`
- `services/runtime/src/checkpoint.rs`

**Approach**

- Review Agent is read-only; uses `browser.*`, `diagnostics.*`, `preview.status`. Emits `review.finding` events.
- Blocking findings prevent promotion. Non-blocking findings are surfaced as info.
- Repair Agent spawns as child run of Build/Edit run with `parentRunId`. Bounded repair loop: same error max 3 attempts; doom-loop detection on identical argv+path. On exhaustion → `partial` or `blocked`.
- Child runs freeze `allowedTools`, `deniedTools`, permission mode, transcript mode, source checkpoint, and inherited/agent-scoped MCP server list at creation time.
- Review child runs use sidechain transcript and read-only tool set; Repair child runs do not inherit parent session allow rules except org/project-level policy.
- Child run cleanup releases agent-scoped MCP stubs/servers, background shell tasks, temporary hooks, read-file cache, and sandbox locks on completion, abort, or failure.
- Error deduplication key: error type/code (not raw string with line numbers).
- Edit Agent: reads `context.md`, `brief.md`, makes focused changes, rebuilds, and emits `preview.candidate`; runtime promotes after gate checks. If request conflicts with confirmed Brief, pauses the run with `needs_user_input`.
- Checkpoint on: Brief confirmed, first generation success, each edit success, before export, after repair. Includes Brief version, source snapshot, conversation range, build result, last preview URL.
- Runtime restart resumes run from latest checkpoint or marks it `failed` (recoverable) with checkpoint context preserved.

**Test Scenarios**

- `review_repair.rs`: blocking finding prevents promotion.
- `review_repair.rs`: repair run has correct `parentRunId` and `findingIds`.
- `review_repair.rs`: review child run has read-only tool set and sidechain transcript.
- `review_repair.rs`: repair child run does not inherit parent session allow rules.
- `review_repair.rs`: loop stops at configured max; run marked `partial` or `blocked`.
- `edit_agent.rs`: edit modifies existing project, not new project.
- `edit_agent.rs`: Brief conflict → run status `needs_user_input`, then resumes through `ContinueRun`.
- `checkpoint.rs`: checkpoint contains Brief version, snapshot URI, conversation range.

**Dependencies**

- RA5.

### Phase A Contract Freeze Gate

**Goal:** Validate the runtime API exactly the way Phase B will consume it before declaring the contract frozen.

**Files**

- `services/runtime/tests/mock_bff_contract.rs`
- `packages/shared/src/api-types.ts`

**Approach**

- Use a mock BFF test client, not `apps/web`, to exercise the public runtime API.
- Cover the Phase B call shape for project creation handoff, brief run start, event streaming, reconnect, generation run start, edit continuation, cancel, permission decision, and preview-current lookup.
- Assert the mock BFF imports request/response/event types from `packages/shared` rather than hand-defining shapes.
- Freeze the runtime API only after these tests pass.

**Test Scenarios**

- `mock_bff_contract.rs`: start brief run, stream events, reconnect with `Last-Event-ID`, and replay without duplicates.
- `mock_bff_contract.rs`: start build run, observe `preview.updated`, and resolve `/preview/{projectId}/current` through the runtime contract.
- `mock_bff_contract.rs`: send edit via `ContinueRun` and receive a new promoted version without changing preview URL shape.
- `mock_bff_contract.rs`: resolve permission request and verify the same run resumes.
- `mock_bff_contract.rs`: cancel a run and verify terminal status plus event replay.

**Dependencies**

- RA6.

---

### Phase A Acceptance

Phase A is complete when all of the following are true:

- Prompt / Markdown → Brief Agent produces structured Brief and waits for confirmation.
- Confirmed Brief → Build Agent generates runnable Astro website in sandbox.
- Candidate preview promoted atomically; `run.complete` rejected before promotion.
- Edit via `ContinueRun` modifies existing project, rebuilds, promotes new version.
- Permission engine blocks external paths, secrets, public internet, denied shell commands.
- Every tool call has an audit record.
- Review/Repair child run graph works; bounded loop stops correctly.
- Runtime restarts resume from checkpoint or mark run `failed` with checkpoint context.
- Mock BFF contract tests verify Phase B call patterns against runtime API before freeze.
- All Phase A HTTP + SSE endpoints respond correctly in integration tests.
- No `apps/web` code exists.

---

## Phase A.5 — Docs Template Loop

### RA7. Fumadocs Docs Template

**Goal:** Add the Markdown-first Docs generation loop after Phase A runtime acceptance.

**Files**

- `services/runtime/src/profiles/docs_build.rs`
- `infra/agent-sandbox/templates/fumadocs-docs.yaml`
- `infra/agent-sandbox/warmpools/fumadocs-docs.yaml`

**Approach**

- Reuse Brief Agent, workspace tooling, permission engine, promotion gate, review/repair loop, and checkpoint mechanics from Phase A.
- Add docs-specific Brief criteria: navigation, sections, sidebar, content page coverage, and Markdown source mapping.
- Validate home page, at least one content page, and sidebar/nav links before promotion.

**Test Scenarios**

- `fumadocs_build_agent.rs`: Markdown → docs sections and sidebar.
- `fumadocs_build_agent.rs`: preview includes homepage, content page, and navigation.
- `fumadocs_build_agent.rs`: missing required docs structure produces a blocking finding, not a promoted preview.

**Dependencies**

- Phase A accepted.

---

## Phase B — Product Integration Units

Phase B contains only product control plane, BFF, and frontend workspace work. Runtime, sandbox, build agent, Astro preview promotion, and review/repair are Phase A responsibilities. Fumadocs generation is Phase A.5.

### RB1. Product Data Model and BFF

**Goal:** App DB schema, BFF API routes, runtime API client. Phase B starts here.

**Pre-condition:** Phase A acceptance complete; runtime HTTP + SSE API contract frozen.

**Files**

- `apps/web/lib/db/schema.ts`
- `apps/web/lib/db/client.ts`
- `apps/web/lib/runtime-client.ts`  — typed HTTP client wrapping Phase A runtime API
- `apps/web/app/api/projects/route.ts`
- `apps/web/app/api/projects/[projectId]/route.ts`
- `apps/web/app/api/projects/[projectId]/content-sources/route.ts`
- `apps/web/app/api/projects/[projectId]/brief-runs/route.ts`
- `apps/web/app/api/projects/[projectId]/events/route.ts`
- `apps/web/app/api/projects/[projectId]/conversation/route.ts`

**Approach**

- App DB stores: Project, ContentSource, Brief, AgentSession, ProjectVersion, SandboxBinding, AuditRecord.
- ConversationItem and AgentEvent are stored in runtime; BFF proxies SSE stream from runtime to frontend.
- `runtime-client.ts` uses types from `packages/shared`; no hand-typed API shapes.

**Test Scenarios**

- Project creation stores correct fields; content source stores kind, hash, excerpt.
- Event stream endpoint proxies runtime SSE and replays persisted events on reconnect.

**Dependencies**

- Phase A accepted.

### RB2. Preview Version Contract and Runtime Client Integration

**Goal:** Align left chat events and right preview versions.

**Files**

- `apps/web/app/api/projects/[projectId]/preview/route.ts`
- `apps/web/components/workspace/chat-panel.tsx`
- `apps/web/components/workspace/preview-panel.tsx`
- `apps/web/components/workspace/event-timeline.tsx`
- `apps/web/components/workspace/version-badge.tsx`

**Approach**

- Consume Phase A `ProjectVersion` and `preview.updated` semantics through `runtime-client.ts`; do not reimplement promotion in BFF.
- Right preview panel keeps old promoted preview during rebuild.
- Candidate preview is visible only as status/debug metadata.
- Treat `preview.updated` as the only frontend switch signal.
- **Preview URL contract:** The Preview Router exposes `/preview/{projectId}/current` (stable, follows current promoted version) and `/preview/{projectId}/{versionId}` (pinned). The `current` URL is what designers share and bookmark. Do not use `runId` in preview URLs.

**Test Scenarios**

- Preview route returns the current promoted preview URL from runtime-client.
- Preview panel ignores `preview.candidate` for default display and switches only on `preview.updated`.
- Version badge shows rebuilding state while keeping the previous promoted preview visible.

**Dependencies**

- RB1.

### RB3. Brief Confirm and Generation Start UI

**Goal:** Designer-facing flows for Brief review, confirmation, and generation trigger.

**Files**

- `apps/web/app/projects/[projectId]/brief/page.tsx`
- `apps/web/app/api/projects/[projectId]/briefs/[briefId]/confirm/route.ts`
- `apps/web/app/api/projects/[projectId]/generation-runs/route.ts`
- `apps/web/app/projects/[projectId]/generate/route.ts`

**Approach**

- Frontend renders Brief from `packages/shared` schema types (not free-form text parsing).
- Confirm Brief calls BFF → runtime; triggers sandbox claim and Build Agent.
- Frontend transitions to workspace page on generation start.

**Test Scenarios**

- Brief page renders all schema fields correctly.
- Confirm button calls confirm route and receives run ID.
- Generation run start transitions to workspace URL.

**Dependencies**

- RB1, RB2.

### RB4. Detail Workspace: Chat + Preview Panel

**Goal:** Left chat / right preview dual-panel workspace.

**Files**

- `apps/web/app/projects/[projectId]/workspace/page.tsx`
- `apps/web/components/workspace/chat-panel.tsx`
- `apps/web/components/workspace/preview-panel.tsx`
- `apps/web/components/workspace/chat-composer.tsx`
- `apps/web/components/workspace/conversation-list.tsx`
- `apps/web/components/workspace/event-timeline.tsx`
- `apps/web/components/workspace/version-badge.tsx`
- `apps/web/app/api/projects/[projectId]/messages/route.ts`
- `apps/web/app/api/projects/[projectId]/preview/route.ts`

**Approach**

- Left panel: renders ConversationItems from BFF, subscribes to SSE event stream for live updates.
- Right panel: shows `/preview/{projectId}/current`; during rebuild shows previous version + rebuilding indicator; switches only on `preview.updated` event.
- Chat composer: sends user message → BFF → `ContinueRun`; appends user ConversationItem immediately.
- Preview Router: `/preview/{projectId}/current` and `/preview/{projectId}/{versionId}`. No runId in URLs.

**Test Scenarios**

- Preview panel keeps old version during rebuild; switches on `preview.updated`.
- Chat composer appends user message and transitions workspace to rebuilding state.
- Workspace renders left chat + right preview on load.

**Dependencies**

- RB2, RB3.

### RB5. Review Finding UI and Export

**Goal:** Surface review findings in left panel; export entry point.

**Files**

- `apps/web/components/workspace/review-finding.tsx`
- `apps/web/components/project/template-selector.tsx`
- `apps/web/app/api/projects/[projectId]/export/route.ts`

**Approach**

- Review findings from SSE `review.finding` events rendered in left panel with severity badge.
- Blocking findings shown with action prompt; info/warning shown as collapsible.
- Export entry: triggers Export Agent run via BFF → runtime.
- Template selector UI: Website defaults to Astro, Docs defaults to Fumadocs.

**Test Scenarios**

- Review finding with `blocking` severity shown prominently; `info` collapsible.
- Template selector defaults match configured priority.

**Dependencies**

- RB4.

### RB6. Fumadocs Docs Template UI Check

**Goal:** Verify Docs entry points after the Phase A.5 Fumadocs runtime loop is accepted.

Fumadocs template is implemented in Phase A.5 (RA7). Phase B only verifies the template selector and Docs entry points use the accepted runtime template key.

**Dependencies**

- RA7 (runtime), RB5 (UI).

## 8. Sequencing

### Phase A

```text
RA1  Runtime Skeleton + Shared Contracts
  -> RA2  Agent Loop + Brief Agent
    -> RA3a Permission Engine Core
      -> RA4  Sandbox Adapter + Workspace Tools
        -> RA4b Permission Integration Security Gate
          -> RA5  Build Agent + Preview Promotion
            -> RA6  Review / Repair / Edit + Checkpoint
              -> Mock BFF Contract Tests
              -> Phase A Acceptance
              -> RA7  Fumadocs Docs Template (Phase A.5)
```

### Phase B

Starts only after Phase A acceptance. Phase A runtime API contract must be frozen before RB1.

```text
RB1  Product Data Model + BFF
  -> RB2  Preview Version Contract + Runtime Client Integration
    -> RB3  Brief Confirm + Generation Start UI
      -> RB4  Detail Workspace: Chat + Preview Panel
        -> RB5  Review Finding UI + Export
          -> RB6  Fumadocs Docs Template UI Check
            -> Phase B Acceptance (MVP)
```

**RA4 前置条件（开工前必须全部确认）：**
- agent-sandbox release 版本锁定，CRD 清单已获取。
- 开发 Kubernetes 集群可用。
- 内部 package registry/proxy 可用。
- Preview routing 策略确认（pod IP / 内部 DNS）。
- 对象存储 bucket/prefix 已分配。
- Sandbox channel 协议确认，记录在 `infra/agent-sandbox/base/controller-version.md`。
## 9. External Dependencies

### Required Before RA1-RA2

- Internal model gateway endpoint confirmed, or mock contract defined with same interface as real API.
- Product DB choice for local development confirmed.
- Object storage or local filesystem substitute agreed for dev (with migration path to real storage for RA4).

### Required Before RA4

All items in the RA4 Pre-conditions list (see RA4 section above).
- Preview URL contract agreed: `/preview/{projectId}/current` and `/preview/{projectId}/{versionId}`; no `runId` in URLs.

### Required Before RB1

- Phase A acceptance complete.
- Runtime HTTP + SSE API contract frozen (no breaking changes after this point).
- Mock BFF contract tests pass against the frozen candidate API.
- Astro base template package lock confirmed against internal registry.
- Preview URL auth model confirmed.
- Browser/screenshot capability location confirmed: inside the same sandbox or separate checker sandbox.
- WarmPool cold-start UX agreed: what the frontend shows during `sandbox.claiming` before `sandbox.ready`.

## 10. Agent Sandbox Version Decision

Pin to a release that exposes:

- `agents.x-k8s.io/v1beta1` `Sandbox`
- `extensions.agents.x-k8s.io/v1beta1` `SandboxTemplate`
- `extensions.agents.x-k8s.io/v1beta1` `SandboxWarmPool`
- `extensions.agents.x-k8s.io/v1beta1` `SandboxClaim` with `spec.warmpoolRef`

Confirm the selected release version and sandbox channel protocol before RA4 starts. Document in `infra/agent-sandbox/base/controller-version.md`. If the selected release differs from the above, update RA4 before implementation starts.

## 11. Runtime Completion Rules

Every agent run must be either in one recoverable paused state or one explicit terminal state.

- Recoverable paused state: `needs_user_input`
- Terminal states: `completed`, `partial`, `blocked`, `failed`, `cancelled`

- A successful build/edit run must emit `preview.updated` before `run.completed`. The `run.complete` tool enforces this at the tool layer: if `output_version_id` is not yet `promoted`, the call is rejected and an error is returned to the agent.
- `needs_user_input` is not terminal. It does not set `completed_at`; the same run resumes through `ContinueRun` or `ResolvePermission`.
- A `partial` or `blocked` run may complete without preview promotion but must retain the previous promoted preview.
- Tool failures are classified as recoverable or terminal at call time.
- Runtime restart resumes from the latest checkpoint, or marks the run `failed` (recoverable) with checkpoint context preserved.
- 3 consecutive empty turns with no tool calls → run transitions to `partial` automatically.

## 12. Permission Policy Baseline

- Reads allowed only for current project content sources and `/workspace`.
- Writes allowed only where agent profile permits.
- Shell denied unless command policy allows; uses exec array, not string; `sh -c`/`bash -c` always denied; dependency installation must use `package.install`, not `shell.run`.
- Path checks apply realpath/canonicalize before boundary check for existing paths; create operations canonicalize the nearest existing parent before validating the final target.
- Public internet denied; internal model gateway, package registry, object storage, preview router allowlisted per environment.
- Secrets and secret-like paths denied.

Designer-facing confirmations: Brief, project type / template, major direction change, export.
Platform-facing confirmations: unknown dependency install, network exception, long sandbox lease.
Admin-only: public network access, external publish, cross-project asset read, high-sensitivity credential.

## 13. MVP Acceptance

### Phase A Acceptance

- Prompt / Markdown → Brief Agent produces structured Brief and waits for confirmation.
- Confirmed Brief → Build Agent generates runnable Astro website in isolated sandbox.
- `run.complete` rejected when preview not yet promoted.
- Candidate promoted atomically; `preview.updated` emitted before run completes.
- Edit via `ContinueRun` modifies existing project, rebuilds, promotes new version.
- Permission engine blocks external paths, secrets, public internet, denied shell commands.
- `sh -c "pnpm build"` denied; symlink outside `/workspace` denied after realpath.
- Every emitted tool_use has exactly one tool_result across success, error, fallback, interrupt, and cancel paths.
- Tool registry supports output schema, disabled tools, deferred metadata, and MCP stub without connecting real Figma MCP.
- Headless permission hooks resolve Ask before auto-deny; hook allow does not bypass deny/ask rules.
- Every tool call has an audit record.
- Review/Repair child run graph: sidechain transcript, read-only review tools, scoped repair permissions, blocking finding prevents promotion, bounded loop stops correctly.
- Runtime restart resumes from checkpoint or marks run `failed` with checkpoint context.
- Mock BFF contract tests verify Phase B call patterns against runtime API: create/run/stream/reconnect/continue/cancel/resolve-permission/preview-current.
- All Phase A HTTP + SSE endpoints respond correctly in integration tests.
- No `apps/web` code exists.

### Phase B Acceptance (Full MVP)

- Designer can create project from prompt / Markdown in the web UI.
- Brief confirmed in UI → generation starts → workspace opens.
- Workspace shows left chat + right preview; preview switches only on `preview.updated`.
- Designer can send chat edit and receive updated promoted preview.
- Review findings shown in left panel with correct severity.
- `/preview/{projectId}/current` stable across runs; no runId in URLs.
- All Phase A security acceptance criteria hold end-to-end through the product UI.

## 14. Open Execution Questions

Require confirmation before RA4:

- Which exact agent-sandbox release will be pinned?
- Which Kubernetes cluster will host dev sandboxes?
- Which sandbox channel protocol (gRPC / WebSocket / other)?
- Which internal model gateway should runtime call?
- Which object storage bucket/prefix stores attachments, screenshots, exports, run logs?
- Which internal package registry/proxy should sandbox use?
- Will screenshot/browser checks run inside the same sandbox or a separate checker sandbox?
- Preview routing strategy: pod IP / internal DNS, or explicit Service creation?
